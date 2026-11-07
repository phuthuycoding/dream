import ObjectiveC
import XCTest
import UserNotifications
@testable import Dream

/// The `AttentionCenter` edge detector: priming, foreign-profile drops,
/// re-priming on a profile switch, foreground suppression, and
/// retraction on a true→false edge. `UNUserNotificationCenter.current()`
/// is a singleton the detector calls directly — there is no injected
/// seam, and `add` fails outright without notification permission — so
/// `NotificationCenterSpy` replaces the three request-facing methods the
/// detector calls with a recorder. The detector's decisions are then
/// observable synchronously, without SpringBoard granting anything; no
/// assertion here depends on a banner actually being delivered.
@MainActor
final class AttentionCenterEdgeTests: XCTestCase {
    private var center: UNUserNotificationCenter!
    private var previousDelegate: UNUserNotificationCenterDelegate?
    private var spy: NotificationCenterSpy!
    private let profileA = UUID()
    private let profileB = UUID()

    override func setUp() async throws {
        try await super.setUp()
        let center = UNUserNotificationCenter.current()
        self.center = center
        previousDelegate = center.delegate
        spy = NotificationCenterSpy()
        spy.install()
    }

    override func tearDown() async throws {
        spy.uninstall()
        spy = nil
        center.delegate = previousDelegate
        center = nil
        try await super.tearDown()
    }

    private func makeCenter(active profileID: UUID) -> AttentionCenter {
        let attention = AttentionCenter()
        attention.activeProfileID = profileID
        return attention
    }

    private func session(_ id: String, attention: Bool = true) -> DreamSession {
        DreamSession(
            id: id,
            name: "job-\(id)",
            cmd: "make \(id)",
            cwd: "/work",
            status: .live,
            createdAt: Date(),
            logPath: "/tmp/\(id).cast",
            attention: attention
        )
    }

    /// Identifiers the detector asked the center to queue — a re-flag
    /// under the same identifier replaces rather than stacks.
    private var postedIDs: Set<String> {
        Set(spy.requests.keys)
    }

    // MARK: - Delegate wiring

    func testInitInstallsItselfAsNotificationCenterDelegate() {
        let attention = AttentionCenter()

        XCTAssertTrue(
            center.delegate === attention,
            "banner taps route through the center's delegate — the center must hold it"
        )
        XCTAssertNil(attention.requestedSessionID)
    }

    // MARK: - Priming

    func testFirstObservationSeedsTheBaselineWithoutNotifying() {
        let attention = makeCenter(active: profileA)

        attention.observe([session("s1")], for: profileA)
        // Same flag again: if the prime had not recorded it, this diff
        // would treat it as a fresh edge and post a banner.
        attention.observe([session("s1")], for: profileA)

        XCTAssertFalse(
            postedIDs.contains("s1"),
            "a flag already set when observation starts is not a fresh edge"
        )
    }

    func testFlagRisingAfterPrimingPostsABanner() {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)

        attention.observe([session("s1")], for: profileA)

        guard let request = spy.requests["s1"] else {
            return XCTFail("a false→true edge must post a banner named by the session id")
        }
        XCTAssertEqual(request.content.userInfo["sessionID"] as? String, "s1")
        XCTAssertEqual(
            request.content.userInfo["profileID"] as? String, profileA.uuidString,
            "a tap landing after a profile switch must route to the owning daemon"
        )
    }

    // MARK: - Profile scoping

    func testSnapshotForAnotherProfileIsDropped() {
        let attention = makeCenter(active: profileA)
        attention.observe([session("s1")], for: profileA) // primes {s1}

        // Belongs to a different daemon — must neither notify nor
        // re-seed the baseline.
        attention.observe([session("s2")], for: profileB)
        attention.observe([session("s1")], for: profileA)

        XCTAssertFalse(
            postedIDs.contains("s2"),
            "a foreign-profile snapshot must not fire its edges here"
        )
        XCTAssertFalse(
            postedIDs.contains("s1"),
            "a dropped snapshot must not rewrite the baseline — s1 was never a new edge"
        )
    }

    func testProfileSwitchRePrimesTheBaseline() {
        let attention = makeCenter(active: profileA)
        attention.observe([session("s1")], for: profileA) // primes {s1}
        attention.activeProfileID = profileB

        // The new daemon's pre-existing flag is a baseline, not an edge:
        // diffed against {s1} it would notify s2 as rising.
        attention.observe([session("s2")], for: profileB)

        XCTAssertFalse(
            postedIDs.contains("s2"),
            "the first observation after a profile switch only re-primes"
        )

        // And the detector is live on the new profile afterwards.
        attention.observe([session("s2"), session("s3")], for: profileB)
        XCTAssertTrue(
            postedIDs.contains("s3"),
            "a real rising edge on the new profile must still notify"
        )
        XCTAssertFalse(
            postedIDs.contains("s2"),
            "the primed flag itself never notified"
        )
    }

    // MARK: - Foreground suppression

    func testForegroundedSessionEdgeIsSuppressedButStillSeeded() {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)

        attention.foregroundedSessionID = "s1"
        attention.observe([session("s1")], for: profileA)

        attention.foregroundedSessionID = nil
        // If suppression had skipped the baseline bookkeeping, this
        // second observation would re-fire s1 as a rising edge.
        attention.observe([session("s1")], for: profileA)

        XCTAssertFalse(
            postedIDs.contains("s1"),
            "the foregrounded terminal's edge is suppressed — the attach already acked its flag"
        )
    }

    // MARK: - Retraction

    func testClearedFlagRetractsItsBanner() {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)
        attention.observe([session("s1")], for: profileA)
        XCTAssertNotNil(
            spy.requests["s1"],
            "fixture: the rising edge must have posted"
        )

        attention.observe([session("s1", attention: false)], for: profileA)

        XCTAssertFalse(
            postedIDs.contains("s1"),
            "a true→false edge must retract the queued banner"
        )
        XCTAssertTrue(
            spy.removedPending.contains("s1") && spy.removedDelivered.contains("s1"),
            "retraction must clear the session id from both pending and delivered"
        )
    }
}

/// Records the `UNUserNotificationCenter` calls `AttentionCenter` can
/// make — a post, and pending/delivered retraction — by replacing the
/// method implementations on the singleton for one test's duration.
/// The center has no init seam and rejects queued requests without
/// notification permission, so swizzling is the only way to observe the
/// detector's decisions deterministically. `install`/`uninstall` must
/// bracket exactly one test at a time.
private final class NotificationCenterSpy {
    /// Requests the center accepted, keyed by identifier — the pending
    /// list, where a re-add under the same identifier replaces.
    private(set) var requests: [String: UNNotificationRequest] = [:]
    /// Identifiers handed to pending-request removal, in call order.
    private(set) var removedPending: [String] = []
    /// Identifiers handed to delivered-notification removal, in call order.
    private(set) var removedDelivered: [String] = []

    /// Original implementations to hand back on `uninstall`.
    private var originals: [(method: Method, implementation: IMP)] = []
    /// The trampoline blocks — must outlive the swizzled IMPs.
    private var blocks: [Any] = []

    func install() {
        let post: @convention(block) (
            UNUserNotificationCenter, UNNotificationRequest,
            (@convention(block) (NSError?) -> Void)?
        ) -> Void = { _, request, completion in
            self.requests[request.identifier] = request
            completion?(nil)
        }
        replace("addNotificationRequest:withCompletionHandler:", block: post)

        let retractPending: @convention(block) (UNUserNotificationCenter, NSArray) -> Void = { _, ids in
            let ids = ids.compactMap { $0 as? String }
            self.removedPending.append(contentsOf: ids)
            for id in ids {
                self.requests.removeValue(forKey: id)
            }
        }
        replace("removePendingNotificationRequestsWithIdentifiers:", block: retractPending)

        let retractDelivered: @convention(block) (UNUserNotificationCenter, NSArray) -> Void = { _, ids in
            self.removedDelivered.append(contentsOf: ids.compactMap { $0 as? String })
        }
        replace("removeDeliveredNotificationsWithIdentifiers:", block: retractDelivered)
    }

    func uninstall() {
        for (method, implementation) in originals.reversed() {
            method_setImplementation(method, implementation)
        }
        originals.removeAll()
        blocks.removeAll()
    }

    private func replace(_ selectorName: String, block: Any) {
        let selector = NSSelectorFromString(selectorName)
        guard let method = class_getInstanceMethod(UNUserNotificationCenter.self, selector) else {
            XCTFail("UNUserNotificationCenter does not respond to \(selectorName) — the spy cannot observe it")
            return
        }
        blocks.append(block)
        let previous = method_setImplementation(method, imp_implementationWithBlock(block))
        originals.append((method, previous))
    }
}
