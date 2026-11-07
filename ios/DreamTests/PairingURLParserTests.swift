import XCTest
@testable import Dream

/// Pairing URL parsing: the `dream url` output line a user pastes must be
/// reduced to a validated base URL plus bearer token, and anything
/// malformed must raise the parser's typed error rather than produce a
/// profile that cannot connect.
final class PairingURLParserTests: XCTestCase {
    func testParsingPairingURLExtractsHostPortAndToken() throws {
        let result = try PairingURLParser.parse("http://192.168.1.5:4020/?token=abc")

        XCTAssertEqual(result.baseURL.scheme, "http")
        XCTAssertEqual(result.baseURL.host, "192.168.1.5")
        XCTAssertEqual(result.baseURL.port, 4020)
        XCTAssertEqual(result.token, "abc")
    }

    func testParsingURLFromDaemonOutputLineExtractsItFromSurroundingText() throws {
        let line = "dream ready — open: http://192.168.1.5:4020/?token=abc\n"
        let result = try PairingURLParser.parse(line)

        XCTAssertEqual(result.baseURL.host, "192.168.1.5")
        XCTAssertEqual(result.baseURL.port, 4020)
        XCTAssertEqual(result.token, "abc")
    }

    func testParsingHTTPSPairingURLPreservesScheme() throws {
        let result = try PairingURLParser.parse("https://daemon.example.com:4020/?token=secret")

        XCTAssertEqual(result.baseURL.scheme, "https")
        XCTAssertEqual(result.baseURL.host, "daemon.example.com")
        XCTAssertEqual(result.baseURL.port, 4020)
        XCTAssertEqual(result.token, "secret")
    }

    func testParsingURLWithoutSchemeIsRejected() {
        assertMalformed("192.168.1.5:4020/?token=abc")
    }

    func testParsingURLWithoutTokenIsRejected() {
        assertMalformed("http://192.168.1.5:4020/")
        assertMalformed("http://192.168.1.5:4020/?other=abc")
        assertMalformed("http://192.168.1.5:4020/?token=")
    }

    func testParsingURLWithBadPortIsRejected() {
        assertMalformed("http://192.168.1.5:abc/?token=abc")
        assertMalformed("http://192.168.1.5:99999/?token=abc")
    }

    func testParsingPlainTextWithoutAnyURLIsRejected() {
        assertMalformed("dream ready — nothing to open")
        assertMalformed("")
    }

    private func assertMalformed(
        _ input: String,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(
            try PairingURLParser.parse(input),
            "malformed input must be rejected: \(input)",
            file: file,
            line: line
        ) { error in
            XCTAssertTrue(
                error is PairingURLError,
                "expected the parser's typed error, got \(type(of: error)): \(error)",
                file: file,
                line: line
            )
        }
    }
}
