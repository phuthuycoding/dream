import XCTest
@testable import Dream

/// The bounded-backoff reattach decision behind the terminal overlay: a
/// 500ms floor doubling to an 8s ceiling, an automatic retry only for
/// unclassified transport drops, and a hard cap past which the overlay
/// offers manual reconnect instead of retrying a dead route forever.
final class ReconnectPolicyTests: XCTestCase {
    // MARK: - Delay schedule

    func testFirstReconnectAttemptWaitsAtTheFloor() {
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: 0), 500)
    }

    func testReconnectDelayDoublesWithEachAttempt() {
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: 1), 1_000)
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: 2), 2_000)
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: 3), 4_000)
    }

    func testReconnectDelayNeverExceedsTheCeiling() {
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: 4), 8_000)
        XCTAssertEqual(
            ReconnectPolicy.nextDelayMs(attempt: ReconnectPolicy.maxAttempts - 1),
            8_000,
            "the last retry under the cap still waits no more than the ceiling"
        )
        XCTAssertEqual(
            ReconnectPolicy.nextDelayMs(attempt: ReconnectPolicy.maxAttempts + 100),
            8_000,
            "a pathological attempt counter must never overflow past the ceiling"
        )
    }

    func testNegativeAttemptCountsFromTheFloor() {
        XCTAssertEqual(ReconnectPolicy.nextDelayMs(attempt: -5), 500)
    }

    // MARK: - Retry decision

    func testTransportCloseRetriesForEveryAttemptBelowTheCap() {
        for attempt in 0..<ReconnectPolicy.maxAttempts {
            XCTAssertTrue(
                ReconnectPolicy.shouldReconnect(kind: .transport, attempt: attempt),
                "an unclassified transport drop on attempt \(attempt) must schedule another attach"
            )
        }
    }

    func testTransportCloseStopsRetryingAtTheAttemptCap() {
        XCTAssertFalse(
            ReconnectPolicy.shouldReconnect(kind: .transport, attempt: ReconnectPolicy.maxAttempts),
            "at the cap the overlay takes over with a manual reconnect affordance"
        )
        XCTAssertFalse(
            ReconnectPolicy.shouldReconnect(kind: .transport, attempt: ReconnectPolicy.maxAttempts + 1)
        )
    }

    func testClassifiedCloseNeverRetries() {
        for attempt in [0, 3, ReconnectPolicy.maxAttempts - 1] {
            XCTAssertFalse(
                ReconnectPolicy.shouldReconnect(kind: .classified, attempt: attempt),
                "a classified close — a 401/404 or a deliberate teardown — must not retry on attempt \(attempt)"
            )
        }
    }
}
