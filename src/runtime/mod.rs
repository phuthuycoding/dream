//! Session lifecycle layer: a `RuntimeManager` owning one live runtime per
//! session (the TASK-007 `ws::runtime::SessionRuntime` shape — pty + vt +
//! replay log + broadcast), dead-mark + end broadcast on pty exit,
//! daemon-restart resurrection, attention tracking, and a periodic
//! retention enforcer.

pub mod manager;

pub use manager::{RuntimeManager, DEFAULT_IDLE_ATTENTION};

#[cfg(test)]
mod tests;
