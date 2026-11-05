//! Attach snapshot byte-stream: build and verify.
//!
//! Layout: `\x1bc` (RIS) + `\x1b[?2026h` (synchronized-update open) +
//! alt-screen re-entry when active + screen dump (cells and attributes)
//! + `\x1b[?2026l` (sync close) + CUP for the cursor position.
//!
//! The DEC-2026 wrap lets the attaching client apply the repaint
//! atomically instead of flickering through the dumped frames.

use vt100::Screen;

const RIS: &[u8] = b"\x1bc";
const SYNC_OPEN: &[u8] = b"\x1b[?2026h";
const SYNC_CLOSE: &[u8] = b"\x1b[?2026l";
const ALT_ENTER: &[u8] = b"\x1b[?1049h";

/// Serialize the visible screen as an attach snapshot.
pub(crate) fn build(screen: &Screen) -> Vec<u8> {
    let (row, col) = screen.cursor_position();

    let mut out = Vec::new();
    out.extend_from_slice(RIS);
    out.extend_from_slice(SYNC_OPEN);
    // contents_formatted() dumps the *active* screen; the attach must
    // re-enter the alt screen first so replay lands on the right buffer.
    if screen.alternate_screen() {
        out.extend_from_slice(ALT_ENTER);
    }
    out.extend_from_slice(&screen.contents_formatted());
    out.extend_from_slice(SYNC_CLOSE);
    // cursor_position() is 0-based; CUP is 1-based.
    out.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Structural check: a complete snapshot begins with a terminal reset and
/// contains a balanced DEC-2026 synchronized-update pair in order. A
/// stream torn before the close is rejected — replaying it would wedge
/// the client terminal in synchronized-update mode.
pub fn verify(bytes: &[u8]) -> bool {
    if !(bytes.starts_with(RIS) || bytes.starts_with(b"\x1b[!p")) {
        return false;
    }
    match (find(bytes, SYNC_OPEN), find(bytes, SYNC_CLOSE)) {
        (Some(open), Some(close)) => open < close,
        _ => false,
    }
}
