import XCTest
@testable import Dream

/// `TerminalController.strippingLeadingRIS` and `snapshotGeometry` —
/// the pure helpers that shape how an `s` payload is fed to the
/// emulator: the leading RIS is re-fed by `applySnapshot` ahead of the
/// scrollback replay, and the geometry marker drives the resize.
@MainActor
final class SnapshotPayloadHelpersTests: XCTestCase {

    // MARK: - strippingLeadingRIS

    func testLeadingRISIsStrippedFromThePayload() {
        let payload = Data([0x1B, 0x63] + "\u{1B}[8;24;80tdump".utf8)

        XCTAssertEqual(
            TerminalController.strippingLeadingRIS(payload),
            Data("\u{1B}[8;24;80tdump".utf8),
            "a second RIS after the replay would wipe the scrollback just restored"
        )
    }

    func testPayloadWithoutLeadingRISPassesThroughUnchanged() {
        // `\x1b[!p` is a DEC soft reset — not RIS — and must survive.
        let payload = Data("\u{1B}[!ptext".utf8)

        XCTAssertEqual(TerminalController.strippingLeadingRIS(payload), payload)
    }

    func testRISAwayFromTheHeadIsLeftAlone() {
        let payload = Data("pre".utf8) + Data([0x1B, 0x63]) + Data("post".utf8)

        XCTAssertEqual(TerminalController.strippingLeadingRIS(payload), payload)
    }

    // MARK: - snapshotGeometry

    func testGeometryMarkerParsesRowsThenCols() throws {
        // Wire order is `\x1b[8;<rows>;<cols>t`, like xterm's window op.
        let payload = Data([0x1B, 0x63] + "\u{1B}[8;24;80t".utf8) + Data("dump".utf8)

        let geometry = try XCTUnwrap(TerminalController.snapshotGeometry(payload))

        XCTAssertEqual(geometry.rows, 24)
        XCTAssertEqual(geometry.cols, 80)
    }

    func testPayloadWithoutGeometryMarkerReturnsNil() {
        XCTAssertNil(TerminalController.snapshotGeometry(Data("plain dump".utf8)))
    }

    func testGeometryWithAZeroDimensionIsRejected() {
        XCTAssertNil(TerminalController.snapshotGeometry(Data("\u{1B}[8;0;80t".utf8)))
        XCTAssertNil(TerminalController.snapshotGeometry(Data("\u{1B}[8;24;0t".utf8)))
    }

    func testMalformedGeometryReturnsNil() {
        // Missing the terminating 't'.
        XCTAssertNil(TerminalController.snapshotGeometry(Data("\u{1B}[8;24;80".utf8)))
        // Missing the cols field.
        XCTAssertNil(TerminalController.snapshotGeometry(Data("\u{1B}[8;24t".utf8)))
    }

    func testGeometryMarkerBeyondTheHeadIsIgnored() {
        // The scanner only looks at the first 64 bytes — a marker
        // deeper in the dump is payload, not geometry.
        var payload = Data(repeating: 0x20, count: 64)
        payload.append(contentsOf: "\u{1B}[8;24;80t".utf8)

        XCTAssertNil(TerminalController.snapshotGeometry(payload))
    }
}
