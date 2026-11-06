import XCTest
@testable import Dream

/// `APIClient` against a stubbed `URLSession`: every call carries
/// `Authorization: Bearer`, session payloads decode, and the daemon's
/// failure modes map onto the typed error — 401 re-pair, 404 not-found,
/// transport failure unreachable.
final class APIClientTests: XCTestCase {
    private let baseURL = URL(string: "http://192.168.1.5:4020")!
    private let token = "test-bearer-token"
    private var stubbedSession: URLSession!

    override func setUp() {
        super.setUp()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        stubbedSession = URLSession(configuration: configuration)
    }

    override func tearDown() {
        StubURLProtocol.requestHandler = nil
        stubbedSession = nil
        super.tearDown()
    }

    private func makeClient() -> APIClient {
        APIClient(
            baseURL: baseURL,
            tokenProvider: { [token] in token },
            urlSession: stubbedSession
        )
    }

    /// Builds a stub handler that answers `status` + `body` and runs
    /// `inspect` on the incoming request. `inspect` executes inside the
    /// URLProtocol callback — synchronously within the await — so
    /// request assertions there are recorded against this test.
    private func respond(
        status: Int,
        body: String = "",
        inspect: @escaping (URLRequest) -> Void = { _ in }
    ) -> (URLRequest) throws -> (HTTPURLResponse, Data) {
        { request in
            inspect(request)
            let response = HTTPURLResponse(
                url: request.url!,
                statusCode: status,
                httpVersion: nil,
                headerFields: ["Content-Type": "application/json"]
            )!
            return (response, Data(body.utf8))
        }
    }

    // MARK: - Session decode over the wire

    func testListSessionsSendsBearerTokenAndDecodesResponse() async throws {
        StubURLProtocol.requestHandler = respond(
            status: 200,
            body: """
            [
              {
                "id": "b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f",
                "name": "dev shell",
                "cmd": "zsh",
                "cwd": "/Users/dev/project",
                "setup_script": null,
                "status": "Live",
                "created_at": "2026-11-06T12:00:00.123456Z",
                "log_path": "/Users/dev/.local/share/dream/sessions/b7f3d8c2/replay.cast",
                "resurrected": false,
                "attention": true
              }
            ]
            """
        ) { [token] request in
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(request.url?.path, "/api/sessions")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer \(token)",
                "the client must authenticate every call with the bearer token"
            )
        }

        let sessions = try await makeClient().listSessions()

        XCTAssertEqual(sessions.count, 1)
        XCTAssertTrue(sessions[0].attention)
        XCTAssertEqual(sessions[0].id, "b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f")
    }

    func testGetSessionRequestsSessionByIDAndDecodesIt() async throws {
        StubURLProtocol.requestHandler = respond(
            status: 200,
            body: """
            {
              "id": "session-42",
              "name": "logs",
              "cmd": "tail -f app.log",
              "cwd": "/var/log",
              "setup_script": null,
              "status": "Resurrected",
              "created_at": "2026-11-01T08:30:00Z",
              "log_path": "/var/lib/dream/sessions/session-42/replay.cast",
              "resurrected": true
            }
            """
        ) { request in
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(request.url?.path, "/api/sessions/session-42")
        }

        let session = try await makeClient().getSession(id: "session-42")

        XCTAssertEqual(session.id, "session-42")
        XCTAssertEqual(session.status, .resurrected)
        // An old-daemon payload carries no `attention` key: decode must
        // still succeed and read as no attention pending.
        XCTAssertFalse(session.attention)
    }

    // MARK: - Error mapping

    func testUnauthorizedResponseMapsToRePairError() async {
        StubURLProtocol.requestHandler = respond(status: 401, body: "unauthorized")

        await assertAPIError(.unauthorized) { client in
            _ = try await client.listSessions()
        }
    }

    func testNotFoundResponseMapsToNotFoundError() async {
        StubURLProtocol.requestHandler = respond(status: 404, body: "no session with id ghost")

        await assertAPIError(.notFound) { client in
            _ = try await client.getSession(id: "ghost")
        }
    }

    func testUnreachableDaemonMapsToUnreachableError() async {
        StubURLProtocol.requestHandler = { _ in
            throw URLError(.cannotConnectToHost)
        }

        await assertAPIError(.unreachable) { client in
            _ = try await client.listSessions()
        }
    }

    /// Matches a single `APIError` case by name only, so payloads the
    /// implementation hangs off a case don't break the contract.
    private enum ExpectedError {
        case unauthorized, notFound, unreachable
    }

    private func assertAPIError(
        _ expected: ExpectedError,
        file: StaticString = #filePath,
        line: UInt = #line,
        whenCallRuns body: (APIClient) async throws -> Void
    ) async {
        do {
            try await body(makeClient())
            XCTFail("expected APIError.\(expected)", file: file, line: line)
        } catch let error as APIError {
            let matched: Bool
            switch (expected, error) {
            case (.unauthorized, .unauthorized),
                 (.notFound, .notFound),
                 (.unreachable, .unreachable):
                matched = true
            default:
                matched = false
            }
            XCTAssertTrue(
                matched,
                "expected APIError.\(expected), got \(error)",
                file: file,
                line: line
            )
        } catch {
            XCTFail(
                "expected APIError.\(expected), got \(type(of: error)): \(error)",
                file: file,
                line: line
            )
        }
    }
}
