//! Attach snapshot byte-stream: build and verify.
//!
//! Layout: `\x1bc` (RIS) + `\x1b[8;<rows>;<cols>t` (geometry field) +
//! `\x1b[?2026h` (synchronized-update open) + alt-screen re-entry when
//! active + screen dump (cells and attributes) + input-mode restores +
//! `\x1b[?2026l` (sync close) + cursor state + attribute restore.
//!
//! The DEC-2026 wrap lets the attaching client apply the repaint
//! atomically instead of flickering through the dumped frames.
//!
//! Attach contract: the snapshot contains no implicit reflow — the dump
//! wraps at the emulator's columns. The client MUST resize to the
//! session's cols/rows (FitAddon) before replaying. The geometry is
//! encoded as `\x1b[8;<rows>;<cols>t` immediately after the RIS; the
//! attach path can parse that field to enforce the contract, and clients
//! that honor window ops will resize themselves on replay.

use vt100::Screen;

const RIS: &[u8] = b"\x1bc";
const SYNC_OPEN: &[u8] = b"\x1b[?2026h";
const SYNC_CLOSE: &[u8] = b"\x1b[?2026l";
const ALT_ENTER: &[u8] = b"\x1b[?1049h";

/// Serialize the visible screen as an attach snapshot.
pub(crate) fn build(screen: &Screen) -> Vec<u8> {
    let (rows, cols) = screen.size();

    let mut out = Vec::new();
    out.extend_from_slice(RIS);
    // CSI 8 ; rows ; cols t — geometry field (see module doc) and a
    // resize request for clients that honor it; vt100 replays ignore it
    // (resize is a callback, not parser state).
    out.extend_from_slice(format!("\x1b[8;{rows};{cols}t").as_bytes());
    out.extend_from_slice(SYNC_OPEN);
    // contents_formatted() dumps the *active* screen; the attach must
    // re-enter the alt screen first so replay lands on the right buffer.
    if screen.alternate_screen() {
        out.extend_from_slice(ALT_ENTER);
    }
    out.extend_from_slice(&screen.contents_formatted());
    // RIS reset the client's input modes; re-emit them inside the sync
    // block so keys, paste and mouse keep their negotiated encodings.
    // vt100 exposes no DECAWM/DECOM state, so those cannot be restored.
    out.extend_from_slice(&screen.input_mode_formatted());
    out.extend_from_slice(SYNC_CLOSE);
    // cursor_state_formatted() re-parks a pending-wrap cursor by
    // redrawing the last cell; a bare CUP would drop the wrap flag and
    // the next printed char would not auto-wrap.
    out.extend_from_slice(&screen.cursor_state_formatted());
    // That restore may redraw a cell and leave its attributes active.
    out.extend_from_slice(&screen.attributes_formatted());
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
