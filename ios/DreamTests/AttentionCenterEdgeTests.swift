import XCTest
import UserNotifications
@testable import Dream

/// The `AttentionCenter` edge detector against the real
/// `UNUserNotificationCenter`: priming, foreign-profile drops,
/// re-priming on a profile switch, foreground suppression, and
/// retraction on a true→false edge. There is no injected seam —
/// `UNUserNotificationCenter.current()` is a singleton the class calls
/// directly — so the pending/delivered request lists are the only
/// observable state. An environment where the center cannot queue
/// requests skips honestly rather than asserting nothing.
@MainActor
final class AttentionCenterEdgeTests: XCTestCase {
    private var center: UNUserNotificationCenter!
    private var previousDelegate: UNUserNotificationCenterDelegate?
    private let profileA = UUID()
    private let profileB = UUID()

    override func setUp() async throws {
        try await super.setUp()
        let center = UNUserNotificationCenter.current()
        self.center = center
        previousDelegate = center.delegate
        center.removeAllPendingNotificationRequests()
        center.removeAllDeliveredNotifications()

        // Probe the seam: the detector's only observable output is a
        // queued request — if this environment cannot queue one, the
        // tests below would assert on nothing.
        _ = try? await center.requestAuthorization(options: [.alert, .sound])
        let probe = UNMutableNotificationContent()
        probe.title = "probe"
        do {
            try await center.add(
                UNNotificationRequest(identifier: "dream-edge-probe", content: probe, trigger: nil)
            )
        } catch {
            throw XCTSkip("notification center rejected a probe request: \(error.localizedDescription)")
        }
        let queued = await notificationIdentifiers()
        center.removePendingNotificationRequests(withIdentifiers: ["dream-edge-probe"])
        center.removeDeliveredNotifications(withIdentifiers: ["dream-edge-probe"])
        guard queued.contains("dream-edge-probe") else {
            throw XCTSkip("notification center does not queue requests in this environment")
        }
    }

    override func tearDown() async throws {
        center.removeAllPendingNotificationRequests()
        center.removeAllDeliveredNotifications()
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

    /// Pending plus delivered request identifiers — a nil-trigger
    /// banner can land in either list depending on delivery timing.
    private func notificationIdentifiers() async -> Set<String> {
        let pending = await center.pendingNotificationRequests().map(\.identifier)
        let delivered = await center.deliveredNotifications().map { $0.request.identifier }
        return Set(pending + delivered)
    }

    /// `add(_:)` completes asynchronously — give any (erroneously)
    /// posted banner time to land before asserting none did.
    private func settledIdentifiers() async -> Set<String> {
        try? await Task.sleep(for: .milliseconds(250))
        return await notificationIdentifiers()
    }

    private func waitFor(_ id: String) async -> Bool {
        for _ in 0..<40 {
            if await notificationIdentifiers().contains(id) { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    private func waitUntilAbsent(_ id: String) async -> Bool {
        for _ in 0..<40 {
            if await !notificationIdentifiers().contains(id) { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    // MARK: - Priming

    func testFirstObservationSeedsTheBaselineWithoutNotifying() async {
        let attention = makeCenter(active: profileA)

        attention.observe([session("s1")], for: profileA)
        // Same flag again: if the prime had not recorded it, this diff
        // would treat it as a fresh edge and post a banner.
        attention.observe([session("s1")], for: profileA)

        let ids = await settledIdentifiers()
        XCTAssertFalse(
            ids.contains("s1"),
            "a flag already set when observation starts is not a fresh edge"
        )
    }

    func testFlagRisingAfterPrimingPostsABanner() async {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)

        attention.observe([session("s1")], for: profileA)

        let posted = await waitFor("s1")
        XCTAssertTrue(
            posted,
            "a false→true edge must post a banner named by the session id"
        )
    }

    // MARK: - Profile scoping

    func testSnapshotForAnotherProfileIsDropped() async {
        let attention = makeCenter(active: profileA)
        attention.observe([session("s1")], for: profileA) // primes {s1}

        // Belongs to a different daemon — must neither notify nor
        // re-seed the baseline.
        attention.observe([session("s2")], for: profileB)
        attention.observe([session("s1")], for: profileA)

        let ids = await settledIdentifiers()
        XCTAssertFalse(
            ids.contains("s2"),
            "a foreign-profile snapshot must not fire its edges here"
        )
        XCTAssertFalse(
            ids.contains("s1"),
            "a dropped snapshot must not rewrite the baseline — s1 was never a new edge"
        )
    }

    func testProfileSwitchRePrimesTheBaseline() async {
        let attention = makeCenter(active: profileA)
        attention.observe([session("s1")], for: profileA) // primes {s1}
        attention.activeProfileID = profileB

        // The new daemon's pre-existing flag is a baseline, not an edge:
        // diffed against {s1} it would notify s2 as rising.
        attention.observe([session("s2")], for: profileB)

        var ids = await settledIdentifiers()
        XCTAssertFalse(
            ids.contains("s2"),
            "the first observation after a profile switch only re-primes"
        )

        // And the detector is live on the new profile afterwards.
        attention.observe([session("s2"), session("s3")], for: profileB)
        let notified = await waitFor("s3")
        XCTAssertTrue(notified, "a real rising edge on the new profile must still notify")
        ids = await notificationIdentifiers()
        XCTAssertFalse(ids.contains("s2"), "the primed flag itself never notified")
    }

    // MARK: - Foreground suppression

    func testForegroundedSessionEdgeIsSuppressedButStillSeeded() async {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)

        attention.foregroundedSessionID = "s1"
        attention.observe([session("s1")], for: profileA)

        attention.foregroundedSessionID = nil
        // If suppression had skipped the baseline bookkeeping, this
        // second observation would re-fire s1 as a rising edge.
        attention.observe([session("s1")], for: profileA)

        let ids = await settledIdentifiers()
        XCTAssertFalse(
            ids.contains("s1"),
            "the foregrounded terminal's edge is suppressed — the attach already acked its flag"
        )
    }

    // MARK: - Retraction

    func testClearedFlagRetractsItsBanner() async {
        let attention = makeCenter(active: profileA)
        attention.observe([], for: profileA)
        attention.observe([session("s1")], for: profileA)
        let posted = await waitFor("s1")
        XCTAssertTrue(posted, "fixture: the rising edge must have posted")

        attention.observe([session("s1", attention: false)], for: profileA)

        let retracted = await waitUntilAbsent("s1")
        XCTAssertTrue(
            retracted,
            "a true→false edge must retract the pending and delivered banner"
        )
    }
}
