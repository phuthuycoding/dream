import XCTest
@testable import Dream

/// `DreamSession` decoding against the daemon's wire format: snake_case
/// keys (`created_at`, `log_path`, `setup_script`), `"Live"/"Dead"/
/// "Resurrected"` status values, RFC 3339 timestamps, and an `attention`
/// flag that older daemons may omit entirely.
final class DreamSessionDecodingTests: XCTestCase {
    /// The JSON a `GET /api/sessions/<id>` response carries, exactly as
    /// the daemon serializes it — `attention` included by newer daemons.
    private func sessionJSON(attention: Bool) -> String {
        """
        {
          "id": "b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f",
          "name": "dev shell",
          "cmd": "zsh",
          "cwd": "/Users/dev/project",
          "setup_script": "source .envrc",
          "status": "Live",
          "created_at": "2026-11-06T12:00:00.123456Z",
          "log_path": "/Users/dev/.local/share/dream/sessions/b7f3d8c2/replay.cast",
          "resurrected": false,
          "attention": \(attention)
        }
        """
    }

    func testDecodingSessionWithAttentionFlag() throws {
        let session = try JSONDecoder().decode(
            DreamSession.self,
            from: Data(sessionJSON(attention: true).utf8)
        )

        XCTAssertTrue(session.attention)
        XCTAssertEqual(session.id, "b7f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f")
        XCTAssertEqual(session.name, "dev shell")
        XCTAssertEqual(session.cmd, "zsh")
        XCTAssertEqual(session.cwd, "/Users/dev/project")
        XCTAssertEqual(session.status, .live)
        XCTAssertEqual(
            session.logPath,
            "/Users/dev/.local/share/dream/sessions/b7f3d8c2/replay.cast"
        )
        XCTAssertFalse(session.resurrected)
        let expected = try XCTUnwrap(
            ISO8601DateFormatter.fractional.parse("2026-11-06T12:00:00.123456Z")
        )
        XCTAssertEqual(session.createdAt.timeIntervalSince1970,
                       expected.timeIntervalSince1970,
                       accuracy: 1.0)
    }

    /// Old daemons serialize no `attention` key at all — decoding must
    /// succeed and report `false` rather than fail on the missing key.
    func testDecodingSessionWithoutAttentionDefaultsToFalse() throws {
        let json = """
        {
          "id": "c8f3d8c2-1a2b-4c3d-9e8f-0a1b2c3d4e5f",
          "name": "old daemon session",
          "cmd": "bash",
          "cwd": "/srv",
          "setup_script": null,
          "status": "Dead",
          "created_at": "2026-01-02T03:04:05Z",
          "log_path": "/var/lib/dream/sessions/c8f3d8c2/replay.cast",
          "resurrected": true
        }
        """

        let session = try JSONDecoder().decode(DreamSession.self, from: Data(json.utf8))

        XCTAssertFalse(session.attention)
        XCTAssertEqual(session.status, .dead)
        XCTAssertTrue(session.resurrected)
        let expected = try XCTUnwrap(
            ISO8601DateFormatter().date(from: "2026-01-02T03:04:05Z")
        )
        XCTAssertEqual(session.createdAt.timeIntervalSince1970,
                       expected.timeIntervalSince1970,
                       accuracy: 1.0)
    }

    func testDecodingEveryWireStatusValue() throws {
        XCTAssertEqual(try sessionWithStatus("Live").status, .live)
        XCTAssertEqual(try sessionWithStatus("Dead").status, .dead)
        XCTAssertEqual(try sessionWithStatus("Resurrected").status, .resurrected)
    }

    private func sessionWithStatus(_ wireStatus: String) throws -> DreamSession {
        let json = sessionJSON(attention: false)
            .replacingOccurrences(of: "\"status\": \"Live\"", with: "\"status\": \"\(wireStatus)\"")
        return try JSONDecoder().decode(DreamSession.self, from: Data(json.utf8))
    }
}

private extension ISO8601DateFormatter {
    static let fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    func parse(_ string: String) -> Date? {
        date(from: string)
    }
}
