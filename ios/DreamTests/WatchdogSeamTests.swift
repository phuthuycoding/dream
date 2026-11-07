import XCTest
@testable import Dream

/// The pure seams the ping watchdog and the profiles-file quarantine
/// were extracted onto:
///
/// - `WSClient.isStale(lastReceiveAt:at:window:)` — the half-open kill
///   rule the ~15s keepalive applies. nil means the watchdog was never
///   armed (no receive has completed), which is not stale by contract;
///   past the window means the socket is dead.
/// - `ProfileStore.shouldQuarantine(decodeError:)` — an existing file
///   that won't decode is always moved aside before the store starts
///   empty.
/// - `ProfileStore.quarantineURL(for:at:)` — the `corrupt-<ts>` sibling
///   the dead file is renamed to.
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

    // MARK: - ProfileStore.shouldQuarantine

    func testADecodeErrorIsQuarantined() {
        let corrupt = Data("not json".utf8)
        let decodeError: Error
        do {
            _ = try JSONDecoder().decode([String: String].self, from: corrupt)
            return XCTFail("decoding garbage must throw")
        } catch {
            decodeError = error
        }

        XCTAssertTrue(
            ProfileStore.shouldQuarantine(decodeError: decodeError),
            "an undecodable profiles file must be moved aside, never overwritten"
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
