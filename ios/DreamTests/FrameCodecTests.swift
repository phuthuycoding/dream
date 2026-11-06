import XCTest
@testable import Dream

/// WebSocket binary frame codec: byte 0 is an ASCII opcode, the
/// remaining bytes are the payload verbatim. Client→server frames are
/// input (`i` + raw stdin bytes) and resize (`r` + cols, rows as two
/// u16 big-endian fields); server→client frames are output (`o` + pty
/// bytes), snapshot (`s` + screen dump), end (`e` + UTF-8 notice), and
/// attention (`a`, empty payload). Bytes that are not a well-formed
/// frame decode to nil — a bad frame is dropped, never fatal.
final class FrameCodecTests: XCTestCase {
    func testEncodingInputPrependsInputOpcodeToRawBytes() {
        let encoded = Frame.input(Data("hi".utf8)).encode()

        XCTAssertEqual(encoded, Data([0x69, 0x68, 0x69]))
    }

    func testEncodingResizeWritesGeometryAsTwoBigEndianU16() {
        let encoded = Frame.resize(cols: 80, rows: 24).encode()

        XCTAssertEqual(encoded, Data([0x72, 0x00, 0x50, 0x00, 0x18]))
    }

    func testDecodingOutputFrameYieldsPtyBytes() {
        let payload = Data("total 0\r\n".utf8)
        let frame = Frame.decode(Data([0x6F]) + payload)

        XCTAssertEqual(frame, .output(payload))
    }

    func testDecodingSnapshotFrameYieldsOpaquePayload() {
        let payload = Data([0x1B, 0x63, 0x1B, 0x5B, 0x38, 0x3B, 0x32])
        let frame = Frame.decode(Data([0x73]) + payload)

        XCTAssertEqual(frame, .snapshot(payload))
    }

    func testDecodingEndFrameYieldsNoticeText() {
        let payload = Data("session exited".utf8)
        let frame = Frame.decode(Data([0x65]) + payload)

        XCTAssertEqual(frame, .end(payload))
    }

    func testDecodingAttentionFrameHasNoPayload() {
        let frame = Frame.decode(Data([0x61]))

        XCTAssertEqual(frame, .attention)
    }

    func testDecodingEmptyBytesIsDropped() {
        XCTAssertNil(Frame.decode(Data()))
    }

    func testDecodingUnknownOpcodeIsDropped() {
        XCTAssertNil(Frame.decode(Data([0x7A]) + Data("hi".utf8)))
        XCTAssertNil(Frame.decode(Data([0x00])))
    }

    func testDecodingResizeWithWrongPayloadLengthIsDropped() {
        for payloadLength in [0, 1, 3, 5] {
            let bytes = Data([0x72] + Array(repeating: UInt8(0), count: payloadLength))
            XCTAssertNil(
                Frame.decode(bytes),
                "resize with \(payloadLength) payload bytes must be dropped"
            )
        }
    }
}
