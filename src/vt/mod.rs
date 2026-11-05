//! Headless VT emulator + attach snapshots.
//!
//! Wiring stub only — the developer seat provides `emulator.rs` (`VtEmu`)
//! and `snapshot.rs` (snapshot byte-stream build/verify). This file is the
//! module index plus the test attachment point, matching the sibling
//! `state`/`pty` layout.
//!
//! `pub mod vt;` still has to be added to `src/lib.rs` by whoever owns it;
//! that path is outside the test-writer allowlist, so this module is
//! dormant until then.

pub mod emulator;
pub mod snapshot;

#[cfg(test)]
mod tests;
