//! Filesystem-backed session registry.
//!
//! Layout: `<data_dir>/sessions/<session-id>/{session.json, replay.log}`.
//! `session.json` writes are atomic (tmp file + rename). On load, a state dir
//! without `session.json` is skipped silently and a corrupt one is quarantined
//! (renamed aside) — startup load never fails on a single bad session.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tracing::warn;
use uuid::Uuid;

use crate::model::{Session, Status};

const SESSIONS_DIR: &str = "sessions";
const SESSION_FILE: &str = "session.json";
const LOG_FILE: &str = "replay.log";

/// In-memory view of every persisted session under a data dir.
pub struct Registry {
    data_dir: PathBuf,
    sessions: Vec<Session>,
}

impl Registry {
    /// Load all sessions under `<data_dir>/sessions/`.
    ///
    /// A missing sessions dir yields an empty registry. Per-dir failures
    /// (unreadable or corrupt `session.json`) are warned and skipped; the file
    /// is moved to `session.json.corrupt-<uuid>` so a later load cannot trip
    /// over it again.
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let mut reg = Registry {
            data_dir: data_dir.to_path_buf(),
            sessions: Vec::new(),
        };
        let root = reg.sessions_root();
        if !root.is_dir() {
            return Ok(reg);
        }
        let entries = fs::read_dir(&root)
            .with_context(|| format!("cannot read sessions dir {}", root.display()))?;
        for entry in entries {
            let dir = match entry {
                Ok(entry) => entry.path(),
                Err(err) => {
                    warn!("skipping unreadable entry in {}: {err}", root.display());
                    continue;
                }
            };
            if dir.is_dir() {
                reg.load_session_dir(&dir);
            }
        }
        Ok(reg)
    }

    /// Create and persist a new session in `<data_dir>/sessions/<id>/`.
    pub fn create(
        &mut self,
        name: &str,
        cmd: &str,
        cwd: &Path,
        setup_script: Option<String>,
    ) -> anyhow::Result<Session> {
        let id = Uuid::new_v4().to_string();
        let dir = self.session_dir(&id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create session dir {}", dir.display()))?;
        let session = Session {
            id,
            name: name.to_string(),
            cmd: cmd.to_string(),
            cwd: cwd.to_path_buf(),
            setup_script,
            status: Status::Live,
            created_at: chrono::Utc::now(),
            log_path: dir.join(LOG_FILE),
            resurrected: false,
        };
        save_session(&dir, &session)?;
        self.sessions.push(session.clone());
        Ok(session)
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn list(&self) -> &[Session] {
        &self.sessions
    }

    /// Apply a partial metadata patch; `None` leaves the field unchanged.
    /// `cmd` is intentionally not updatable (it is fixed at spawn time).
    pub fn update(
        &mut self,
        id: &str,
        name: Option<String>,
        cwd: Option<PathBuf>,
        setup_script: Option<String>,
    ) -> anyhow::Result<()> {
        let idx = self.index_of(id)?;
        {
            let session = &mut self.sessions[idx];
            if let Some(name) = name {
                session.name = name;
            }
            if let Some(cwd) = cwd {
                session.cwd = cwd;
            }
            if let Some(script) = setup_script {
                session.setup_script = Some(script);
            }
        }
        save_session(&self.session_dir(id), &self.sessions[idx])
    }

    /// Persist `Dead` status; the session stays listed until `delete`.
    pub fn mark_dead(&mut self, id: &str) -> anyhow::Result<()> {
        let idx = self.index_of(id)?;
        self.sessions[idx].status = Status::Dead;
        save_session(&self.session_dir(id), &self.sessions[idx])
    }

    /// Persist `Live` status on a dead session. Spawning the replacement shell
    /// is the lifecycle layer's job (TASK-009); the registry only records the
    /// transition.
    pub fn respawn(&mut self, id: &str) -> anyhow::Result<()> {
        let idx = self.index_of(id)?;
        self.sessions[idx].status = Status::Live;
        save_session(&self.session_dir(id), &self.sessions[idx])
    }

    /// Drop the session and remove its whole state dir, replay log included.
    pub fn delete(&mut self, id: &str) -> anyhow::Result<()> {
        let idx = self.index_of(id)?;
        let dir = self.session_dir(id);
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("cannot remove session dir {}", dir.display()));
            }
        }
        self.sessions.remove(idx);
        Ok(())
    }

    fn sessions_root(&self) -> PathBuf {
        self.data_dir.join(SESSIONS_DIR)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.sessions_root().join(id)
    }

    fn index_of(&self, id: &str) -> anyhow::Result<usize> {
        self.sessions
            .iter()
            .position(|s| s.id == id)
            .with_context(|| format!("no session with id {id}"))
    }

    fn load_session_dir(&mut self, dir: &Path) {
        let file = dir.join(SESSION_FILE);
        if !file.exists() {
            // Orphan state dir (e.g. a stray replay.log) — nothing persisted.
            return;
        }
        let raw = match fs::read_to_string(&file) {
            Ok(raw) => raw,
            Err(err) => {
                warn!("skipping {}: cannot read session.json: {err}", dir.display());
                return;
            }
        };
        match serde_json::from_str::<Session>(&raw) {
            Ok(session) => self.sessions.push(session),
            Err(err) => {
                warn!("quarantining corrupt {}: {err}", file.display());
                quarantine(&file);
            }
        }
    }
}

/// Write `session.json` atomically: tmp file in the same dir, then rename.
fn save_session(dir: &Path, session: &Session) -> anyhow::Result<()> {
    let final_path = dir.join(SESSION_FILE);
    let tmp_path = dir.join(format!("{SESSION_FILE}.tmp"));
    let body = serde_json::to_string_pretty(session).context("serialize session")?;
    fs::write(&tmp_path, body)
        .with_context(|| format!("cannot write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("cannot rename {}", tmp_path.display()))?;
    Ok(())
}

/// Rename a corrupt `session.json` aside so it is never loaded again.
fn quarantine(file: &Path) {
    let quarantined = file.with_file_name(format!("{SESSION_FILE}.corrupt-{}", Uuid::new_v4()));
    if let Err(err) = fs::rename(file, &quarantined) {
        warn!("cannot quarantine {}: {err}", file.display());
    }
}
