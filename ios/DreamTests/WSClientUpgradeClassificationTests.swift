import XCTest
@testable import Dream

/// The failed-upgrade classification in `WSClient`: a handshake answered
/// with an HTTP status routes to the dedicated callbacks before the
/// generic close — 401 → re-pair, 404 → session-gone — any other status
/// is a bare transport failure, and a late completion on an
/// already-finished client classifies nothing.
final class WSClientUpgradeClassificationTests: XCTestCase {
    private var stubSession: URLSession!

    override func setUp() {
        super.setUp()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        stubSession = URLSession(configuration: configuration)
    }

    override func tearDown() {
        StubURLProtocol.requestHandler = nil
        stubSession.invalidateAndCancel()
        stubSession = nil
        super.tearDown()
    }

    /// A completed task whose `response` carries the given HTTP status —
    /// the shape a failed upgrade leaves on `task.response` when
    /// `didCompleteWithError` runs. A data task stands in for the
    /// upgrade task: the classifier only reads the response.
    private func taskResponding(status: Int) -> URLSessionTask {
        let url = URL(string: "http://stubbed.test/api/sessions/s1/ws")!
        StubURLProtocol.requestHandler = { request in
            let response = HTTPURLResponse(
                url: request.url!,
                statusCode: status,
                httpVersion: "HTTP/1.1",
                headerFields: nil
            )!
            return (response, Data())
        }
        let finished = expectation(description: "stub task completed")
        let task = stubSession.dataTask(with: url) { _, _, _ in
            finished.fulfill()
        }
        task.resume()
        wait(for: [finished], timeout: 5)
        return task
    }

    private func makeClientRecordingEvents() -> (WSClient, Recorder) {
        let recorder = Recorder()
        let client = WSClient(url: URL(string: "ws://stubbed.test/ws")!)
        client.onUnauthorized = { recorder.events.append("unauthorized") }
        client.onSessionGone = { recorder.events.append("sessionGone") }
        client.onClose = { error in
            recorder.events.append("close")
            recorder.closeError = error
        }
        return (client, recorder)
    }

    /// Callback order is observable state here — the contract says the
    /// classification fires before `onClose`.
    private final class Recorder {
        var events: [String] = []
        var closeError: Error?
    }

    func testUpgradeRejectedWithUnauthorizedClassifiesBeforeClose() {
        let task = taskResponding(status: 401)
        XCTAssertEqual(
            (task.response as? HTTPURLResponse)?.statusCode, 401,
            "fixture check: the task must carry the stubbed status"
        )
        let (client, recorder) = makeClientRecordingEvents()

        client.urlSession(
            stubSession,
            task: task,
            didCompleteWithError: URLError(.badServerResponse)
        )

        XCTAssertEqual(
            recorder.events, ["unauthorized", "close"],
            "a 401 upgrade answer must route to re-pair ahead of the generic close"
        )
        XCTAssertNotNil(recorder.closeError, "the underlying failure still reaches onClose")
    }

    func testUpgradeAnsweredNotFoundClassifiesSessionGoneBeforeClose() {
        let task = taskResponding(status: 404)
        let (client, recorder) = makeClientRecordingEvents()

        client.urlSession(
            stubSession,
            task: task,
            didCompleteWithError: URLError(.badServerResponse)
        )

        XCTAssertEqual(
            recorder.events, ["sessionGone", "close"],
            "a 404 upgrade answer names a dead session ahead of the generic close"
        )
    }

    func testUpgradeAnsweredOtherStatusClosesWithoutClassification() {
        let task = taskResponding(status: 500)
        let (client, recorder) = makeClientRecordingEvents()

        client.urlSession(
            stubSession,
            task: task,
            didCompleteWithError: URLError(.badServerResponse)
        )

        XCTAssertEqual(
            recorder.events, ["close"],
            "a status without a dedicated route reports only the generic close"
        )
        XCTAssertNotNil(recorder.closeError)
    }

    func testLateCompletionOnAFinishedClientClassifiesNothing() {
        let task = taskResponding(status: 401)
        let (client, recorder) = makeClientRecordingEvents()
        client.disconnect()

        XCTAssertEqual(recorder.events, ["close"], "detach reports its own close first")

        client.urlSession(
            stubSession,
            task: task,
            didCompleteWithError: URLError(.badServerResponse)
        )

        XCTAssertEqual(
            recorder.events, ["close"],
            "a superseded socket's late callback must not classify — its 401 "
                + "would report a credential it never presented"
        )
        XCTAssertNil(recorder.closeError, "the only close was the clean detach")
    }
}
