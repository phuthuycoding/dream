import XCTest
@testable import Dream

/// `WSClient.attachURL` — the attach-side seam that can be pinned
/// without a live socket: `ws`/`wss` scheme mapping, path-prefix
/// preservation for a daemon behind a reverse proxy, and the rule that
/// no credential ever enters the URL (the token rides the upgrade's
/// `Authorization` header, built inside `connect()` — that header has
/// no injectable seam: a `URLSessionWebSocketTask` does not route
/// through `URLProtocol`, so its construction stays untested here).
/// The frame state machine (`handle`/`flushPending`) is private, so
/// scrollback buffering is likewise out of reach without a socket.
final class WSClientAttachURLTests: XCTestCase {

    func testPlainHTTPBaseMapsToTheWSScheme() throws {
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "http://192.168.1.5:4020")!,
            sessionID: "s1"
        ))

        XCTAssertEqual(url.scheme, "ws")
        XCTAssertEqual(url.host, "192.168.1.5")
        XCTAssertEqual(url.port, 4020)
        XCTAssertEqual(url.path, "/api/sessions/s1/ws")
    }

    func testHTTPSBaseMapsToTheWSSScheme() throws {
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "https://dream.example.com")!,
            sessionID: "s1"
        ))

        XCTAssertEqual(url.scheme, "wss")
        XCTAssertEqual(url.path, "/api/sessions/s1/ws")
    }

    func testPathPrefixedBaseKeepsItsPrefix() throws {
        // A daemon reverse-proxied under a path must keep it — every
        // attach misses the route otherwise.
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "https://dream.example.com/proxy/dream/")!,
            sessionID: "s1"
        ))

        XCTAssertEqual(url.scheme, "wss")
        XCTAssertEqual(url.path, "/proxy/dream/api/sessions/s1/ws")
    }

    func testAttachURLCarriesNoCredential() throws {
        // The token travels on the upgrade's Authorization header —
        // never in the URL, where proxies and access logs would record
        // it. The built URL must stay credential-free.
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "http://10.0.0.2:4020")!,
            sessionID: "s1"
        ))

        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertNil(components.queryItems)
        XCTAssertNil(components.user)
        XCTAssertNil(components.password)
    }

    func testBaseURLQueryAndFragmentAreDropped() throws {
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "http://10.0.0.2:4020/dream?x=1#frag")!,
            sessionID: "s1"
        ))

        XCTAssertNil(url.query)
        XCTAssertNil(url.fragment)
        XCTAssertEqual(url.path, "/dream/api/sessions/s1/ws")
    }

    func testSessionIDLandsInThePathNotTheQuery() throws {
        let url = try XCTUnwrap(WSClient.attachURL(
            baseURL: URL(string: "http://10.0.0.2:4020")!,
            sessionID: "b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f"
        ))

        XCTAssertEqual(
            url.path,
            "/api/sessions/b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f/ws"
        )
    }
}
