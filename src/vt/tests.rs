//! Behaviour tests for the headless VT emulator and its attach snapshot.
//!
//! Assumed public contract, taken from the task brief (deviation means a
//! compile error — conform the impl or ask for a test fix):
//!
//! ```text
//! emulator.rs:
//!   pub struct VtEmu;
//!   VtEmu::new(cols: u16, rows: u16) -> VtEmu   // constructor shape assumed
//!   .feed(&mut self, bytes: &[u8])             // consume raw pty output
//!   .resize(&mut self, cols: u16, rows: u16)   // track grid size
//!   .snapshot(&self) -> Vec<u8>                // reset + DEC-2026-wrapped
//!                                              //   screen dump + cursor state
//! snapshot.rs:
//!   pub fn verify(bytes: &[u8]) -> bool        // signature assumed
//! ```
//!
//! A snapshot is screen state encoded as a byte stream. Replayed into a
//! fresh emulator it must reproduce text, attributes and cursor exactly —
//! so `snapshot() == snapshot()` is the observable form of "identical
//! screen" that this interface affords. `snapshot()` is therefore required
//! to be deterministic (same state -> same bytes); that premise has its own
//! test below.

use super::emulator::VtEmu;
use super::snapshot;

/// Recorded-TUI-style byte stream: a normal-screen line, alt-screen enter,
/// a full-screen redraw with a reverse-video header, a 256-color line with
/// wide glyphs, then the cursor parked on an otherwise-empty cell.
/// The stream ends *inside* the alt screen — the mid-TUI attach case.
fn tui_stream() -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(b"shell$ ready\r\n"); // normal-screen content
    s.extend_from_slice(b"\x1b[?1049h"); // enter alt screen
    s.extend_from_slice(b"\x1b[2J\x1b[H"); // TUI clears + homes
    s.extend_from_slice(b"\x1b[1;1H\x1b[7mTUI-PANEL\x1b[0m"); // reverse-video header
    s.extend_from_slice(b"\x1b[3;1H\x1b[38;5;196mERR "); // 256-color + wide glyphs
    s.extend_from_slice("\u{7ea2}\u{8272}".as_bytes()); // 
    s.extend_from_slice(b"\x1b[0m");
    s.extend_from_slice(b"\x1b[5;2H\x1b[1;34mstatus row\x1b[0m");
    s.extend_from_slice(b"\x1b[12;55H"); // cursor rests at row 12 col 55
    s
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn fed_tui() -> VtEmu {
    let mut emu = VtEmu::new(80, 24);
    emu.feed(&tui_stream());
    emu
}

#[test]
fn snapshot_starts_with_a_reset_and_wraps_the_dump_in_synchronized_updates() {
    let snap = fed_tui().snapshot();

    assert!(
        snap.starts_with(b"\x1bc") || snap.starts_with(b"\x1b[!p"),
        "snapshot must begin with a terminal reset (RIS or DECSTR); got {:?}",
        &snap[..snap.len().min(8)]
    );

    let open = find(&snap, b"\x1b[?2026h");
    let close = find(&snap, b"\x1b[?2026l");
    assert!(open.is_some(), "snapshot missing DEC-2026 sync-update open");
    assert!(close.is_some(), "snapshot missing DEC-2026 sync-update close");
    assert!(
        open < close,
        "sync-update open must precede close in the snapshot"
    );
}

#[test]
fn snapshot_carries_rendered_text_colors_and_wide_glyphs() {
    let snap = fed_tui().snapshot();

    for needle in [
        &b"TUI-PANEL"[..],
        &b"status row"[..],
        "\u{7ea2}\u{8272}".as_bytes(), // wide glyphs must survive the dump
    ] {
        assert!(
            find(&snap, needle).is_some(),
            "snapshot dump lost rendered text {needle:?}"
        );
    }

    // The dump must re-emit the cell attributes, not bare text; a faithful
    // 256-color dump re-encodes the palette index we fed.
    assert!(
        find(&snap, b"38;5;196").is_some(),
        "256-color attribute not preserved in snapshot dump"
    );
}

#[test]
fn snapshot_encodes_the_current_cursor_position() {
    // The stream leaves the cursor at row 12 col 55 on an empty cell, so a
    // cursor-positioning sequence for that spot can only come from the
    // snapshot's cursor state, not from the dumped cell contents.
    let snap = fed_tui().snapshot();
    assert!(
        find(&snap, b"\x1b[12;55H").is_some() || find(&snap, b"\x1b[12;55f").is_some(),
        "snapshot does not restore the cursor to 12;55"
    );
}

#[test]
fn snapshots_are_deterministic_for_identical_input() {
    let a = fed_tui().snapshot();
    let b = fed_tui().snapshot();
    assert_eq!(a, b, "identical state must snapshot to identical bytes");
}

#[test]
fn snapshot_replayed_into_a_fresh_emulator_reproduces_the_identical_screen() {
    let live = fed_tui();
    let snap = live.snapshot();

    let mut attached = VtEmu::new(80, 24);
    attached.feed(&snap);

    assert_eq!(
        attached.snapshot(),
        snap,
        "replayed snapshot did not reconstruct the mid-TUI screen \
         (alt-screen contents, attributes and cursor)"
    );
}

#[test]
fn leaving_the_alt_screen_restores_the_normal_screen_in_the_snapshot() {
    let mut emu = fed_tui();
    emu.feed(b"\x1b[?1049l"); // TUI exits: alt buffer hides, normal screen returns

    let snap = emu.snapshot();
    assert!(
        find(&snap, b"shell$ ready").is_some(),
        "normal-screen line missing after alt-screen exit"
    );
    assert!(
        find(&snap, b"TUI-PANEL").is_none(),
        "alt-screen content leaked into the snapshot after exit"
    );

    let mut attached = VtEmu::new(80, 24);
    attached.feed(&snap);
    assert_eq!(
        attached.snapshot(),
        snap,
        "post-exit snapshot did not reconstruct the normal screen"
    );
}

#[test]
fn shrinking_the_grid_drops_rows_below_the_new_bottom_from_the_screen_dump() {
    let mut emu = VtEmu::new(80, 24);
    emu.feed(b"TOP_LINE");
    emu.feed(b"\x1b[24;1HBOTTOM_LINE");
    assert!(
        find(&emu.snapshot(), b"BOTTOM_LINE").is_some(),
        "precondition: row-24 marker is on screen at 24 rows"
    );

    emu.resize(80, 10);
    let snap = emu.snapshot();

    assert!(
        find(&snap, b"TOP_LINE").is_some(),
        "row-1 content must survive a row shrink"
    );
    assert!(
        find(&snap, b"BOTTOM_LINE").is_none(),
        "row-24 content is still in the dumped screen after shrinking to 10 rows"
    );
}

#[test]
fn growing_the_grid_opens_new_cells_and_the_snapshot_describes_them() {
    let mut emu = VtEmu::new(80, 24);
    emu.feed(b"TOP_LINE");
    emu.resize(120, 40);

    // Row 40 is only addressable if the resize was tracked. Col 100 keeps
    // the 10-char marker clear of the 120-col edge so no autowrap/scroll
    // disturbs the existing rows.
    emu.feed(b"\x1b[40;100HNEW_CORNER");
    let snap = emu.snapshot();

    assert!(
        find(&snap, b"NEW_CORNER").is_some(),
        "marker written in the grown region missing from the dump"
    );
    assert!(
        find(&snap, b"TOP_LINE").is_some(),
        "existing content lost when the grid grew"
    );

    // The marker lands on row 40 only if the grid really grew (a tracked
    // resize); otherwise the CUP clamps and it sits on row 24. The dump
    // must reach row 40 somehow: absolute addressing (`CSI 40 ;`/`H`/`f`),
    // a cursor-down of 39, or 39+ emitted line breaks.
    let describes_row_40 = find(&snap, b"\x1b[40;").is_some()
        || find(&snap, b"\x1b[40H").is_some()
        || find(&snap, b"\x1b[40f").is_some()
        || find(&snap, b"\x1b[39B").is_some()
        || snap.iter().filter(|&&b| b == b'\n').count() >= 39;
    assert!(
        describes_row_40,
        "no evidence row 40 exists in the snapshot — resize not tracked"
    );
}

#[test]
fn verify_accepts_a_generated_snapshot() {
    let snap = fed_tui().snapshot();
    assert!(
        snapshot::verify(&snap),
        "a generated snapshot must verify"
    );
}

#[test]
fn verify_rejects_streams_that_are_not_complete_snapshots() {
    assert!(!snapshot::verify(b"plain terminal output, no framing"));

    // A snapshot torn before the DEC-2026 close must not verify: attaching
    // it would wedge the client terminal in synchronized-update mode.
    let snap = fed_tui().snapshot();
    let close = find(&snap, b"\x1b[?2026l").expect("generated snapshot has a sync close");
    assert!(
        !snapshot::verify(&snap[..close]),
        "snapshot truncated before the sync-update close verified anyway"
    );
}
