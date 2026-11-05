//! Retention enforcement for session replay logs.
//!
//! Sessions keep their log at `<sessions_dir>/<session>/replay.cast`.
//! `enforce` applies a size and/or age policy to every such file;
//! `clean_all` resets every existing log to a fresh header. Only files
//! named exactly `replay.cast` are touched — session metadata and other
//! files are left alone, and no log is ever fabricated.

use crate::replay::log::trim_file;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// File name every session's replay log uses.
pub const LOG_FILE_NAME: &str = "replay.cast";

/// Limits enforced on each replay log. `None` disables that check.
#[derive(Debug, Clone, Default)]
pub struct RetentionPolicy {
    /// Largest permitted file size; over-cap logs keep only the newest
    /// events that fit.
    pub max_size_bytes: Option<u64>,
    /// Oldest permitted modification time; over-age logs are reset to a
    /// fresh header entirely.
    pub max_age: Option<Duration>,
}

/// Apply `policy` to every replay log under `dir`.
pub fn enforce(dir: &Path, policy: &RetentionPolicy) -> io::Result<()> {
    for path in session_logs(dir)? {
        let meta = fs::metadata(&path)?;
        if let Some(max_age) = policy.max_age {
            let over_age = meta
                .modified()
                .ok()
                .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
                .is_some_and(|age| age > max_age);
            if over_age {
                reset_log(&path)?;
                continue;
            }
        }
        if let Some(cap) = policy.max_size_bytes {
            if meta.len() > cap {
                trim_file(&path, cap)?;
            }
        }
    }
    Ok(())
}

/// Reset every replay log under `dir` to a fresh header, preserving each
/// log's own header line (terminal size). Sessions without a log are
/// skipped — nothing is created.
pub fn clean_all(dir: &Path) -> io::Result<()> {
    for path in session_logs(dir)? {
        reset_log(&path)?;
    }
    Ok(())
}

/// All replay logs under `dir`: `dir/replay.cast` plus `dir/<session>/replay.cast`.
fn session_logs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut logs = Vec::new();
    let top = dir.join(LOG_FILE_NAME);
    if top.is_file() {
        logs.push(top);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let candidate = entry.path().join(LOG_FILE_NAME);
            if candidate.is_file() {
                logs.push(candidate);
            }
        }
    }
    Ok(logs)
}

/// Truncate a log to just its header line, in place. A missing or
/// non-v3 header falls back to a minimal default so the file stays a
/// valid asciicast.
fn reset_log(path: &Path) -> io::Result<()> {
    let header = header_line(path)?;
    let file = OpenOptions::new().write(true).open(path)?;
    (&file).write_all(&header)?;
    file.set_len(header.len() as u64)?;
    Ok(())
}

/// First line of `path` (including its newline) if it is a v3 header;
/// otherwise a default `80x24` v3 header.
fn header_line(path: &Path) -> io::Result<Vec<u8>> {
    const DEFAULT: &[u8] = b"{\"version\":3,\"term\":{\"cols\":80,\"rows\":24}}\n";
    let contents = fs::read(path)?;
    let end = contents
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(contents.len());
    let first = &contents[..end];
    let is_v3 = std::str::from_utf8(first)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s.trim_end()).ok())
        .is_some_and(|v| v["version"] == 3);
    if is_v3 && !first.is_empty() {
        Ok(first.to_vec())
    } else {
        Ok(DEFAULT.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_all, enforce, RetentionPolicy};
    use crate::replay::log::{Event, ReplayLog};
    use crate::replay::reader::read_events;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    /// Sessions keep their replay log in their own state dir.
    fn session_log(dir: &Path, session: &str) -> PathBuf {
        let session_dir = dir.join(session);
        fs::create_dir_all(&session_dir).unwrap();
        session_dir.join("replay.cast")
    }

    fn grow(path: &Path, chunks: usize, chunk_size: usize) {
        let mut log = ReplayLog::create(path, 80, 24).unwrap();
        for i in 0..chunks {
            let mut payload = format!("fill-{i:04}-").into_bytes();
            payload.extend(std::iter::repeat(b'z').take(chunk_size));
            log.append_output(&payload).unwrap();
        }
    }

    fn valid_header_only(path: &Path) -> bool {
        let contents = fs::read_to_string(path).unwrap();
        let mut lines = contents.lines();
        match lines.next() {
            None => return false,
            Some(first) => {
                let header: serde_json::Value = serde_json::from_str(first).unwrap();
                if header["version"] != 3 {
                    return false;
                }
            }
        }
        lines.next().is_none()
    }

    #[test]
    fn enforce_trims_log_over_size_cap_keeping_newest() {
        let dir = TempDir::new().unwrap();
        let path = session_log(dir.path(), "sess-a");
        grow(&path, 16, 1024);
        let before = fs::metadata(&path).unwrap().len();
        assert!(before > 8 * 1024);

        enforce(
            dir.path(),
            &RetentionPolicy {
                max_size_bytes: Some(8 * 1024),
                max_age: None,
            },
        )
        .unwrap();

        let after = fs::metadata(&path).unwrap().len();
        assert!(after <= 8 * 1024, "enforcer left log over cap: {after}");
        let events = read_events(&path).unwrap();
        assert!(!events.is_empty(), "enforcer must keep newest events");
        assert!(
            matches!(events.last(), Some(Event::Output(bytes)) if bytes.starts_with(b"fill-0015")),
            "newest chunk must survive enforcement"
        );
        let first_line = fs::read_to_string(&path).unwrap();
        let header: serde_json::Value =
            serde_json::from_str(first_line.lines().next().unwrap()).unwrap();
        assert_eq!(header["version"], 3, "trimmed log stays a valid v3 file");
    }

    #[test]
    fn enforce_resets_log_older_than_max_age() {
        let dir = TempDir::new().unwrap();
        let path = session_log(dir.path(), "sess-old");
        grow(&path, 8, 1024);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(30 * 24 * 3600))
            .unwrap();

        enforce(
            dir.path(),
            &RetentionPolicy {
                max_size_bytes: None,
                max_age: Some(Duration::from_secs(7 * 24 * 3600)),
            },
        )
        .unwrap();

        assert!(
            valid_header_only(&path),
            "over-age log must be reset to a fresh header"
        );
        assert!(read_events(&path).unwrap().is_empty());
    }

    #[test]
    fn enforce_leaves_logs_within_limits_untouched() {
        let dir = TempDir::new().unwrap();
        let path = session_log(dir.path(), "sess-ok");
        grow(&path, 2, 256);
        let before = fs::read(&path).unwrap();

        enforce(
            dir.path(),
            &RetentionPolicy {
                max_size_bytes: Some(1024 * 1024),
                max_age: Some(Duration::from_secs(365 * 24 * 3600)),
            },
        )
        .unwrap();

        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn enforce_ignores_non_log_files() {
        let dir = TempDir::new().unwrap();
        let session_dir = dir.path().join("sess-meta");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("session.json"), b"{}").unwrap();
        fs::write(dir.path().join("notes.txt"), b"keep me").unwrap();

        enforce(
            dir.path(),
            &RetentionPolicy {
                max_size_bytes: Some(1),
                max_age: Some(Duration::from_secs(0)),
            },
        )
        .unwrap();

        assert_eq!(
            fs::read(session_dir.join("session.json")).unwrap(),
            b"{}",
            "enforcer must not touch session metadata"
        );
        assert_eq!(
            fs::read(dir.path().join("notes.txt")).unwrap(),
            b"keep me",
            "enforcer must not touch non-log files"
        );
    }

    #[test]
    fn clean_all_resets_every_session_log() {
        let dir = TempDir::new().unwrap();
        let first = session_log(dir.path(), "sess-a");
        let second = session_log(dir.path(), "sess-b");
        grow(&first, 4, 512);
        grow(&second, 4, 512);

        clean_all(dir.path()).unwrap();

        for path in [&first, &second] {
            assert!(
                valid_header_only(path),
                "global clean must reset {path:?} to a fresh header"
            );
            assert!(read_events(path).unwrap().is_empty());
        }
    }

    #[test]
    fn clean_all_skips_sessions_without_logs() {
        let dir = TempDir::new().unwrap();
        let empty_session = dir.path().join("sess-empty");
        fs::create_dir_all(&empty_session).unwrap();
        fs::write(empty_session.join("session.json"), b"{}").unwrap();
        let with_log = session_log(dir.path(), "sess-live");
        grow(&with_log, 2, 256);

        clean_all(dir.path()).unwrap();

        assert!(
            !empty_session.join("replay.cast").exists(),
            "clean must not fabricate a log for a session without one"
        );
        assert!(valid_header_only(&with_log));
    }
}
