//! Headless VT emulator: per-session shadow terminal fed by teed pty output.
//!
//! Backend: `vt100::Parser`. `avt` was tried first (its `Terminal::dump()`
//! natively emits a screen dump) but rejected: the dump encodes indexed
//! colors in colon sub-parameter form (`38:5:196`, not the required
//! `38;5;196`), and its primary-screen reflow anchors the bottom on a
//! row-shrink, dropping top rows that must survive. vt100 re-emits SGR in
//! semicolon form and keeps top rows on shrink.

use vt100::Parser;

use super::snapshot;

/// Shadow terminal tracking one pty session's screen state.
pub struct VtEmu {
    parser: Parser,
}

impl VtEmu {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            // vt100 takes (rows, cols); no scrollback — the snapshot is a
            // pure function of the visible screen.
            parser: Parser::new(rows.max(1), cols.max(1), 0),
        }
    }

    /// Consume raw pty output. vt100's parser is incremental: partial
    /// UTF-8 and split escape sequences carry over between calls.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Track a grid resize. Shrinking rows keeps the top of the screen;
    /// rows below the new bottom leave the visible grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    /// Attach snapshot: reset + DEC-2026-wrapped screen dump + cursor state.
    /// Deterministic — a pure function of the current screen state.
    pub fn snapshot(&self) -> Vec<u8> {
        snapshot::build(&self.parser.screen())
    }
}
