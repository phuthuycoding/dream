//! WebSocket attach layer: binary frame codec + session attach handler.
//!
//! Wiring stub only — the developer seat provides `frame.rs` (opcode byte +
//! payload codec; client `i`/`r`, server `o`/`s`/`e`/`a`) and `attach.rs`
//! (authed upgrade → retained log-tail scrollback → DEC-2026 snapshot → live
//! broadcast, input→PTY, resize→PTY+vt). This file is the module index plus
//! the test attachment point, matching the sibling `state`/`vt`/`http`
//! layout.
//!
//! `pub mod ws;` still has to be added to `src/lib.rs` by whoever owns it;
//! that path is outside the test-writer allowlist, so this module is
//! dormant until then.

pub mod attach;
pub mod frame;
pub mod runtime;

#[cfg(test)]
mod tests;
