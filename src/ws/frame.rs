//! Binary frame codec for the WS attach protocol.
//!
//! Wire layout: byte 0 is an ASCII opcode, the remaining bytes are the
//! payload verbatim. Prior art: ttyd/gotty one-byte opcodes.
//!
//! client → server: `i`nput(raw stdin bytes), `r`esize(cols, rows — two
//! u16 big-endian fields). server → client: `o`utput(raw pty bytes),
//! `s`napshot(DEC-2026-wrapped dump), `e`nd(notice text), `a`ttention.

use std::fmt;

/// One decoded protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Client input: raw bytes for the session's stdin.
    Input(Vec<u8>),
    /// Client resize: new terminal geometry.
    Resize { cols: u16, rows: u16 },
    /// Server output: scrollback or live pty bytes.
    Output(Vec<u8>),
    /// Server snapshot: DEC-2026-wrapped screen dump (see `vt::snapshot`).
    Snapshot(Vec<u8>),
    /// Server end: the session is over; the payload says why and offers
    /// the respawn route.
    End(Vec<u8>),
    /// Server attention flag (bell/idle); empty payload.
    Attention,
}

/// Why a frame could not be decoded. Rejection is a clean `Err`, never a
/// panic — a bad peer frame is dropped, not fatal to the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Zero bytes: there is no opcode to read.
    Empty,
    /// Byte 0 is not one of the six protocol opcodes.
    UnknownOpcode(u8),
    /// `r` must carry exactly four payload bytes (cols, rows as u16 BE).
    BadResizePayload(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Empty => write!(f, "empty frame has no opcode"),
            DecodeError::UnknownOpcode(op) => write!(f, "unknown opcode {op:#04x}"),
            DecodeError::BadResizePayload(len) => {
                write!(f, "resize payload must be 4 bytes, got {len}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

impl Frame {
    /// Serialize as `opcode || payload`.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Frame::Input(bytes) => with_opcode(b'i', bytes),
            Frame::Resize { cols, rows } => {
                let mut out = Vec::with_capacity(5);
                out.push(b'r');
                out.extend_from_slice(&cols.to_be_bytes());
                out.extend_from_slice(&rows.to_be_bytes());
                out
            }
            Frame::Output(bytes) => with_opcode(b'o', bytes),
            Frame::Snapshot(bytes) => with_opcode(b's', bytes),
            Frame::End(bytes) => with_opcode(b'e', bytes),
            Frame::Attention => vec![b'a'],
        }
    }

    /// Parse one wire frame. Unknown opcodes and malformed payloads are
    /// rejected; the caller decides whether to drop or disconnect.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (&opcode, payload) = bytes.split_first().ok_or(DecodeError::Empty)?;
        match opcode {
            b'i' => Ok(Frame::Input(payload.to_vec())),
            b'r' => {
                let arr: [u8; 4] = payload
                    .try_into()
                    .map_err(|_| DecodeError::BadResizePayload(payload.len()))?;
                Ok(Frame::Resize {
                    cols: u16::from_be_bytes([arr[0], arr[1]]),
                    rows: u16::from_be_bytes([arr[2], arr[3]]),
                })
            }
            b'o' => Ok(Frame::Output(payload.to_vec())),
            b's' => Ok(Frame::Snapshot(payload.to_vec())),
            b'e' => Ok(Frame::End(payload.to_vec())),
            b'a' => Ok(Frame::Attention),
            other => Err(DecodeError::UnknownOpcode(other)),
        }
    }
}

fn with_opcode(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}
