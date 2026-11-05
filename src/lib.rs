//! webterm — single-binary web terminal daemon.
//! Module declarations live here so each task's code + tests compile
//! as soon as its files land; main.rs stays a thin CLI entry.

pub mod cli;
pub mod model;
pub mod config;
pub mod pty;
pub mod state;
pub mod replay;
pub mod vt;
pub mod auth;
pub mod http;
