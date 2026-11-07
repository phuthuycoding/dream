import XCTest
@testable import Dream

/// The pure seams the ping watchdog and the profiles-file quarantine
/// were extracted onto:
///
/// - `WSClient.isStale(lastReceiveAt:at:window:)` — the half-open kill
///   rule the ~15s keepalive applies. nil means the watchdog was never
///   armed (no receive has completed), which is not stale by contract;
///   past the window means the socket is dead.
/// - `ProfileStore.quarantineURL(for:at:)` — the `corrupt-<ts>` sibling
///   the dead file is renamed to. The move itself is pinned by a real
///   round-trip: garbage bytes on disk → empty store → save → the
///   quarantined sibling still holds the original bytes.
/// - `TerminalController.resumeAction(for:reconnectSuppressed:)` — the
///   foreground-resume decision `resumeIfNeeded` acts on (see
///   `ResumeSeamTests` below).
final class WatchdogSeamTests: XCTestCase {

    // MARK: - WSClient.isStale

    func testANeverArmedWatchdogIsNotStale() {
        // lastReceiveAt is nil until the first completed receive — the
        // watchdog must not kill a socket whose upgrade never proved
        // live, no matter how long it has been connecting.
        XCTAssertFalse(
            WSClient.isStale(
                lastReceiveAt: nil,
                at: Date(timeIntervalSince1970: 1_700_000_000),
                window: 31
            ),
            "nil lastReceiveAt means never armed — never stale"
        )
    }

    func testSilenceWithinTheWindowIsNotStale() {
        let lastReceiveAt = Date(timeIntervalSince1970: 1_700_000_000)

        XCTAssertFalse(
            WSClient.isStale(
                lastReceiveAt: lastReceiveAt,
                at: lastReceiveAt.addingTimeInterval(30),
                window: 31
            ),
            "30s of silence is inside the 31s liveness window"
        )
    }

    func testSilenceExactlyAtTheWindowIsNotStale() {
        // The kill is strictly `>` — a socket whose last proof of life
        // is exactly one window old still gets the next ping.
        let lastReceiveAt = Date(timeIntervalSince1970: 1_700_000_000)

        XCTAssertFalse(
            WSClient.isStale(
                lastReceiveAt: lastReceiveAt,
                at: lastReceiveAt.addingTimeInterval(31),
                window: 31
            ),
            "the window boundary itself is not stale — the rule is strictly greater"
        )
    }

    func testSilencePastTheWindowIsStale() {
        let lastReceiveAt = Date(timeIntervalSince1970: 1_700_000_000)

        XCTAssertTrue(
            WSClient.isStale(
                lastReceiveAt: lastReceiveAt,
                at: lastReceiveAt.addingTimeInterval(32),
                window: 31
            ),
            "silence longer than the window means a half-open socket"
        )
        XCTAssertTrue(
            WSClient.isStale(
                lastReceiveAt: lastReceiveAt,
                at: lastReceiveAt.addingTimeInterval(600),
                window: 31
            ),
            "a long-dead socket is stale too"
        )
    }

    // MARK: - ProfileStore corrupt-file round-trip

    func testACorruptFileIsMovedAsideAndSurvivesTheNextSaveByteForByte() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("dream-quarantine-\(UUID().uuidString)")
        let fileURL = directory.appendingPathComponent("profiles.json")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let garbage = Data("not json".utf8)
        try garbage.write(to: fileURL)
        defer { try? FileManager.default.removeItem(at: directory) }

        let store = ProfileStore(fileURL: fileURL)
        XCTAssertTrue(
            store.profiles.isEmpty,
            "an undecodable file must fall back to an empty store, not crash"
        )

        // The quarantine ran at init; a real save mutation must leave the
        // quarantined copy untouched while writing a fresh profiles.json.
        let profile = DaemonProfile(
            id: UUID(),
            name: "mac-studio",
            baseURL: URL(string: "http://192.168.1.5:4020")!
        )
        try store.save(profile, token: "tok-\(UUID().uuidString)")
        defer { try? store.clearToken(for: profile.id) }

        let siblings = try FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        )
        let quarantined = siblings.filter {
            $0.lastPathComponent.hasPrefix("profiles.json.corrupt-")
        }
        XCTAssertEqual(
            quarantined.count, 1,
            "exactly one corrupt-<ts> sibling must exist after the save"
        )
        XCTAssertEqual(
            try Data(contentsOf: try XCTUnwrap(quarantined.first)),
            garbage,
            "the quarantined file must hold the original garbage bytes — nothing overwritten"
        )
        XCTAssertTrue(
            ProfileStore(fileURL: fileURL).profiles.contains { $0.id == profile.id },
            "the fresh profiles.json must decode — a second store round-trips the saved profile"
        )
    }

    func testPersistIsRefusedWhenTheQuarantineMoveItselfFails() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("dream-quarantine-blocked-\(UUID().uuidString)")
        let fileURL = directory.appendingPathComponent("profiles.json")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let garbage = Data("not json".utf8)
        try garbage.write(to: fileURL)
        // A read-only directory makes the rename fail — the file can be
        // read (decode attempt runs) but never moved aside.
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o555], ofItemAtPath: directory.path
        )
        defer {
            try? FileManager.default.setAttributes(
                [.posixPermissions: 0o755], ofItemAtPath: directory.path
            )
            try? FileManager.default.removeItem(at: directory)
        }

        let store = ProfileStore(fileURL: fileURL)
        XCTAssertTrue(
            store.profiles.isEmpty,
            "a failed quarantine still falls back to an empty store"
        )
        XCTAssertEqual(
            try Data(contentsOf: fileURL), garbage,
            "a failed quarantine must leave the unreadable file in place"
        )

        let profile = DaemonProfile(
            id: UUID(),
            name: "vps",
            baseURL: URL(string: "http://192.168.1.6:4020")!
        )
        XCTAssertThrowsError(try store.save(profile, token: "tok-\(UUID().uuidString)")) { error in
            let nsError = error as NSError
            XCTAssertEqual(nsError.domain, "ProfileStore")
            XCTAssertEqual(nsError.code, 1)
            XCTAssertTrue(
                nsError.localizedDescription.contains("refusing to overwrite"),
                "persist must refuse rather than overwrite the only copy — got \(nsError.localizedDescription)"
            )
        }
        try? store.clearToken(for: profile.id)
        XCTAssertEqual(
            try Data(contentsOf: fileURL), garbage,
            "the refused persist must not have overwritten the unreadable file"
        )
    }

    func testAMissingFileNeverReachesTheQuarantineDecision() {
        // `load()` returns early on a read failure — an absent file is
        // first launch, not corruption, so nothing is moved aside.
        let fileURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("dream-watchdog-seam-\(UUID().uuidString)")
            .appendingPathComponent("profiles.json")

        let store = ProfileStore(fileURL: fileURL)

        XCTAssertTrue(
            store.profiles.isEmpty,
            "a missing file must start the store empty"
        )
        let siblings = (try? FileManager.default.contentsOfDirectory(
            at: fileURL.deletingLastPathComponent(),
            includingPropertiesForKeys: nil
        )) ?? []
        XCTAssertFalse(
            siblings.contains { $0.lastPathComponent.contains("corrupt-") },
            "a first-launch read failure must not quarantine anything"
        )
    }

    // MARK: - ProfileStore.quarantineURL

    func testQuarantineURLKeepsTheFileAndAddsACorruptTimestampExtension() {
        let fileURL = URL(fileURLWithPath: "/tmp/Dream/profiles.json")
        let at = Date(timeIntervalSince1970: 1_700_000_000)

        XCTAssertEqual(
            ProfileStore.quarantineURL(for: fileURL, at: at),
            URL(fileURLWithPath: "/tmp/Dream/profiles.json.corrupt-1700000000"),
            "the quarantined copy is a sibling carrying corrupt-<unix-timestamp>"
        )
    }

    func testQuarantineURLsFromDifferentMomentsNeverCollide() {
        // A second corruption must not overwrite the first quarantine —
        // the timestamp is what keeps them apart.
        let fileURL = URL(fileURLWithPath: "/tmp/Dream/profiles.json")

        XCTAssertNotEqual(
            ProfileStore.quarantineURL(
                for: fileURL,
                at: Date(timeIntervalSince1970: 1_700_000_000)
            ),
            ProfileStore.quarantineURL(
                for: fileURL,
                at: Date(timeIntervalSince1970: 1_700_000_001)
            )
        )
    }
}

/// `TerminalController.resumeAction(for:reconnectSuppressed:)` — the
/// pure decision `resumeIfNeeded` dispatches on when the scene returns
/// to the foreground. `.skip` must leave a classified failure's overlay
/// and any in-flight/ended attach untouched; `.reconnecting` runs the
/// already-scheduled attempt early; `.disconnected` starts a fresh
/// bounded series.
@MainActor
final class ResumeSeamTests: XCTestCase {

    private var allPhases: [TerminalController.Phase] {
        [
            .connecting,
            .live,
            .ended("session exited"),
            .reconnecting("Reconnecting in 4s… (attempt 3/8)"),
            .disconnected("The daemon closed the connection."),
        ]
    }

    func testASuppressedReconnectSkipsEveryPhase() {
        // A classified 401/404 owns its overlay — reattaching on resume
        // would loop the same rejection, whatever the last phase was.
        for phase in allPhases {
            XCTAssertEqual(
                TerminalController.resumeAction(for: phase, reconnectSuppressed: true),
                .skip,
                "reconnectSuppressed must win over \(phase)"
            )
        }
    }

    func testConnectingLiveAndEndedPhasesAreLeftUntouched() {
        for phase in [
            TerminalController.Phase.connecting,
            .live,
            .ended("session exited"),
        ] {
            XCTAssertEqual(
                TerminalController.resumeAction(for: phase, reconnectSuppressed: false),
                .skip,
                "an in-flight or ended attach must not be superseded by a resume — \(phase)"
            )
        }
    }

    func testAReconnectingPhaseFastPathsTheScheduledAttempt() {
        // The delay was chosen for a network that may already be back —
        // resume runs the scheduled attempt early and keeps the counter.
        XCTAssertEqual(
            TerminalController.resumeAction(
                for: .reconnecting("Reconnecting in 4s… (attempt 3/8)"),
                reconnectSuppressed: false
            ),
            .fastPathRetry,
            "a scheduled reconnect must run immediately without resetting the attempt counter"
        )
    }

    func testADisconnectedPhaseStartsAFreshBoundedSeries() {
        // Retries are already exhausted — resume begins a new bounded
        // series (the attempt counter resets before attach()).
        XCTAssertEqual(
            TerminalController.resumeAction(
                for: .disconnected("reconnect gave up"),
                reconnectSuppressed: false
            ),
            .reattachNow,
            "an exhausted reconnect must start a fresh bounded series"
        )
    }
}
