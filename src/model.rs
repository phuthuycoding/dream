//! Core domain model shared across the state, pty and http layers.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle status of a terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Live,
    Dead,
    Resurrected,
}

/// A persisted terminal session.
///
/// Serialized verbatim to `<data_dir>/sessions/<id>/session.json`; these field
/// names are the on-disk schema and must stay stable across releases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// `uuid::Uuid::new_v4()` in string form; also the per-session dir name.
    pub id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: PathBuf,
    pub setup_script: Option<String>,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    /// `<data_dir>/sessions/<id>/replay.cast`.
    pub log_path: PathBuf,
    /// Set when the session was resurrected after a daemon restart.
    #[serde(default)]
    pub resurrected: bool,
}
