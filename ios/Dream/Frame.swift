import Foundation

/// One frame of the WS attach protocol — byte 0 is an ASCII opcode, the
/// remaining bytes are the payload verbatim (mirrors `src/ws/frame.rs`).
///
/// Client → server: `i`nput (raw stdin bytes), `r`esize (cols, rows as
/// two u16 big-endian fields). Server → client: `o`utput (pty bytes),
/// `s`napshot (DEC-2026-wrapped dump), `e`nd (notice text), `a`ttention.
enum Frame: Equatable {
    /// Client input: raw bytes for the session's stdin.
    case input(Data)
    /// Client resize: new terminal geometry.
    case resize(cols: UInt16, rows: UInt16)
    /// Server output: scrollback or live pty bytes.
    case output(Data)
    /// Server snapshot: DEC-2026-wrapped screen dump.
    case snapshot(Data)
    /// Server end: the session is over; the payload says why and names
    /// the respawn route.
    case end(Data)
    /// Server attention flag (bell/idle); empty payload.
    case attention

    /// Serialize as `opcode || payload`.
    func encode() -> Data {
        switch self {
        case .input(let bytes):
            return Frame.withOpcode(0x69, bytes) // 'i'
        case .resize(let cols, let rows):
            var out = Data(capacity: 5)
            out.append(0x72) // 'r'
            out.append(UInt8(cols >> 8))
            out.append(UInt8(cols & 0xFF))
            out.append(UInt8(rows >> 8))
            out.append(UInt8(rows & 0xFF))
            return out
        case .output(let bytes):
            return Frame.withOpcode(0x6F, bytes) // 'o'
        case .snapshot(let bytes):
            return Frame.withOpcode(0x73, bytes) // 's'
        case .end(let bytes):
            return Frame.withOpcode(0x65, bytes) // 'e'
        case .attention:
            return Data([0x61]) // 'a'
        }
    }

    /// Parse one wire frame. A bad frame — empty, unknown opcode, or a
    /// resize that is not exactly four payload bytes — decodes to nil so
    /// the caller drops it; a bad peer frame is never fatal.
    static func decode(_ data: Data) -> Frame? {
        guard let opcode = data.first else { return nil }
        let payload = data.dropFirst()
        switch opcode {
        case 0x69: // 'i'
            return .input(Data(payload))
        case 0x72: // 'r'
            guard payload.count == 4 else { return nil }
            var iterator = payload.makeIterator()
            // payload.count == 4 guarantees the four bytes exist.
            let c0 = iterator.next()!, c1 = iterator.next()!
            let r0 = iterator.next()!, r1 = iterator.next()!
            return .resize(
                cols: UInt16(c0) << 8 | UInt16(c1),
                rows: UInt16(r0) << 8 | UInt16(r1)
            )
        case 0x6F: // 'o'
            return .output(Data(payload))
        case 0x73: // 's'
            return .snapshot(Data(payload))
        case 0x65: // 'e'
            return .end(Data(payload))
        case 0x61: // 'a'
            return .attention
        default:
            return nil
        }
    }

    private static func withOpcode(_ opcode: UInt8, _ payload: Data) -> Data {
        var out = Data(capacity: 1 + payload.count)
        out.append(opcode)
        out.append(payload)
        return out
    }
}
