//! Asciicast-v3 replay log writer.
//!
//! A `.cast` file is NDJSON: a `{"version":3,"term":{...}}` header line
//! followed by one `[interval, code, data]` event per line. Intervals are
//! relative to log creation. With a size cap set, each append trims the
//! oldest whole events so the file stays under the cap — the retained
//! window is the newest output. Rewrites happen in place (same inode), so
//! a clean or trim mid-attach never leaves a torn frame boundary: readers
//! see either the old or the new prefix-free content.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One parsed replay event. Covers the asciicast v3 codes this log emits;
/// `i` (input) events are skipped by the reader rather than surfaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Output(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Marker(String),
    Exit(i32),
}

/// Appending writer for one session's `.cast` file.
pub struct ReplayLog {
    path: PathBuf,
    file: File,
    start: Instant,
    header: String,
    cap: Option<u64>,
    size: u64,
}

impl ReplayLog {
    /// Create a fresh log at `path` (truncating any existing file) and
    /// write the v3 header recording the terminal size.
    pub fn create(path: &Path, cols: u16, rows: u16) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let header = format!(
            "{{\"version\":3,\"term\":{{\"cols\":{cols},\"rows\":{rows},\"type\":\"xterm-256color\"}}}}\n"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(header.as_bytes())?;
        let size = header.len() as u64;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            start: Instant::now(),
            header,
            cap: None,
            size,
        })
    }

    /// Cap the file size; oldest events are dropped after every append
    /// that pushes the file over `cap`.
    pub fn set_cap(&mut self, cap: u64) {
        self.cap = Some(cap);
    }

    /// Current size of the log in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn append_output(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.append_event("o", &String::from_utf8_lossy(bytes))
    }

    pub fn append_resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.append_event("r", &format!("{cols}x{rows}"))
    }

    pub fn append_marker(&mut self, label: &str) -> io::Result<()> {
        self.append_event("m", label)
    }

    pub fn append_exit(&mut self, code: i32) -> io::Result<()> {
        self.append_event("x", &code.to_string())
    }

    fn append_event(&mut self, code: &str, data: &str) -> io::Result<()> {
        let event = serde_json::json!([self.start.elapsed().as_secs_f64(), code, data]);
        let mut line = serde_json::to_string(&event)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        line.push('\n');
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        if let Some(cap) = self.cap {
            if self.size > cap {
                self.size = trim_file(&self.path, cap)?;
            }
        }
        Ok(())
    }

    /// Return the newest whole events whose on-disk lines fit within
    /// `n_bytes` — always at least the newest event, always ending at the
    /// current end of log, never a torn frame.
    pub fn tail(&self, n_bytes: u64) -> io::Result<Vec<Event>> {
        let parsed = crate::replay::reader::read_events_sized(&self.path)?;
        let mut kept: Vec<Event> = Vec::new();
        let mut used = 0u64;
        for (event, len) in parsed.into_iter().rev() {
            if !kept.is_empty() && used + len > n_bytes {
                break;
            }
            used += len;
            kept.push(event);
        }
        kept.reverse();
        Ok(kept)
    }

    /// Reset the log to just its header — the per-session clean. Writers
    /// keep appending afterwards; readers attaching later see only
    /// post-clean events.
    pub fn truncate_fresh(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(self.header.as_bytes())?;
        self.file.set_len(self.header.len() as u64)?;
        self.size = self.header.len() as u64;
        self.start = Instant::now();
        Ok(())
    }
}

/// Drop the oldest whole event lines of the `.cast` at `path` until it
/// fits `cap`, keeping the header and the newest events. Rewrites in
/// place; returns the new file size. Shared with the retention enforcer.
pub(crate) fn trim_file(path: &Path, cap: u64) -> io::Result<u64> {
    let contents = fs::read(path)?;
    let Some(header_end) = contents.iter().position(|&b| b == b'\n').map(|i| i + 1) else {
        // no newline at all — not something we can split into header + events
        return Ok(contents.len() as u64);
    };
    let header = &contents[..header_end];
    let events = &contents[header_end..];
    let budget = cap.saturating_sub(header.len() as u64);

    // Byte ranges of complete (newline-terminated) event lines; a trailing
    // fragment is torn and gets dropped on rewrite.
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    while pos < events.len() {
        match events[pos..].iter().position(|&b| b == b'\n') {
            Some(rel) => {
                lines.push((pos, rel + 1));
                pos += rel + 1;
            }
            None => break,
        }
    }

    let mut kept_len = 0u64;
    let mut kept_from = events.len();
    for &(start, len) in lines.iter().rev() {
        if kept_len + len as u64 > budget {
            break;
        }
        kept_len += len as u64;
        kept_from = start;
    }

    let new_len = header.len() + (events.len() - kept_from);
    if new_len == contents.len() {
        return Ok(new_len as u64);
    }
    let file = OpenOptions::new().write(true).open(path)?;
    (&file).write_all(header)?;
    (&file).write_all(&events[kept_from..])?;
    file.set_len(new_len as u64)?;
    Ok(new_len as u64)
}

#[cfg(test)]
mod tests {
    use super::{Event, ReplayLog};
    use crate::replay::reader::read_events;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn log_path(dir: &TempDir) -> PathBuf {
        dir.path().join("replay.cast")
    }

    fn file_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn output_payloads(events: &[Event]) -> Vec<&[u8]> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Output(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn writer_emits_asciicast_v3_header_with_term_size() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"hi").unwrap();

        let lines = file_lines(&path);
        assert!(lines.len() >= 2);
        let header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header["version"], 3);
        assert_eq!(header["term"]["cols"], 80);
        assert_eq!(header["term"]["rows"], 24);
    }

    #[test]
    fn writer_appends_output_resize_marker_exit_events() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"hello").unwrap();
        log.append_resize(120, 40).unwrap();
        log.append_marker("checkpoint").unwrap();
        log.append_exit(0).unwrap();

        let lines = file_lines(&path);
        assert_eq!(lines.len(), 5, "header plus four event lines");

        let events: Vec<serde_json::Value> = lines[1..]
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let codes: Vec<&str> = events
            .iter()
            .map(|event| event[1].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["o", "r", "m", "x"]);

        for event in &events {
            assert!(
                event[0].as_f64().unwrap() >= 0.0,
                "event timestamp must be a non-negative relative interval"
            );
        }
        assert_eq!(events[0][2], "hello");
        assert_eq!(events[1][2], "120x40");
        assert_eq!(events[2][2], "checkpoint");
    }

    #[test]
    fn appending_past_cap_keeps_file_under_cap_with_newest_events() {
        const CAP: u64 = 8 * 1024;
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.set_cap(CAP);

        let mut appended: Vec<Vec<u8>> = Vec::new();
        for i in 0..24u32 {
            let mut payload = format!("chunk-{i:04}-").into_bytes();
            payload.extend(std::iter::repeat(b'x').take(1024));
            log.append_output(&payload).unwrap();
            appended.push(payload);
        }

        let on_disk = fs::metadata(&path).unwrap().len();
        assert!(
            on_disk <= CAP,
            "file grew past cap: {on_disk} > {CAP}"
        );
        assert!(log.size() <= CAP, "reported size exceeds cap");

        let events = read_events(&path).unwrap();
        let outputs = output_payloads(&events);
        assert!(!outputs.is_empty(), "trim must keep the newest events");
        assert_eq!(
            *outputs.last().unwrap(),
            appended.last().unwrap().as_slice(),
            "newest event must survive the trim"
        );
        assert!(
            !outputs.iter().any(|b| *b == appended[0].as_slice()),
            "oldest event must be trimmed away"
        );

        let lines = file_lines(&path);
        let header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header["version"], 3, "trimmed log must keep a v3 header");
        for line in &lines[1..] {
            serde_json::from_str::<serde_json::Value>(line)
                .expect("every retained event line must be complete JSON");
        }
    }

    #[test]
    fn tail_returns_contiguous_suffix_of_complete_events() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        let mut appended: Vec<Vec<u8>> = Vec::new();
        for i in 0..10u8 {
            let mut payload = format!("event-{i:02}-").into_bytes();
            payload.extend(std::iter::repeat(b'y').take(128));
            log.append_output(&payload).unwrap();
            appended.push(payload);
        }

        let events = log.tail(400).unwrap();
        assert!(!events.is_empty());

        let all = read_events(&path).unwrap();
        assert!(
            events.len() < all.len(),
            "a small tail window must not return the whole log"
        );
        assert_eq!(
            events.last(),
            all.last(),
            "tail always includes the newest event"
        );
        let kept = events.len();
        assert_eq!(
            events.as_slice(),
            &all[all.len() - kept..],
            "tail must return a contiguous suffix of whole events"
        );
        for event in &events {
            if let Event::Output(bytes) = event {
                assert!(
                    appended.iter().any(|p| p == bytes),
                    "tail returned a payload that is not an originally appended event — torn frame"
                );
            }
        }
    }

    #[test]
    fn tail_tolerates_torn_final_line() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"first").unwrap();
        log.append_output(b"second").unwrap();

        // simulate a crash mid-write: append an unterminated event line
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"[9.999,\"o\",\"partial").unwrap();
        drop(file);

        let events = log.tail(1 << 40).unwrap();
        assert_eq!(
            events,
            vec![
                Event::Output(b"first".to_vec()),
                Event::Output(b"second".to_vec())
            ],
            "torn tail must be excluded, complete events preserved"
        );
    }

    #[test]
    fn truncate_fresh_resets_log_to_header_only() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"before-clean-aaaa").unwrap();
        log.append_output(b"before-clean-bbbb").unwrap();

        log.truncate_fresh().unwrap();

        let lines = file_lines(&path);
        assert_eq!(lines.len(), 1, "fresh log contains only the header");
        let header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header["version"], 3);
        assert!(
            read_events(&path).unwrap().is_empty(),
            "no pre-clean events may survive"
        );
    }

    #[test]
    fn writer_continues_appending_after_truncate_fresh() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"before-clean").unwrap();
        log.truncate_fresh().unwrap();

        log.append_output(b"after-clean").unwrap();
        log.append_resize(100, 30).unwrap();

        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::Output(b"after-clean".to_vec()));
        assert!(
            matches!(events[1], Event::Resize { cols: 100, rows: 30 }),
            "post-clean events must append to the fresh log"
        );
    }

    #[test]
    fn clean_during_attach_yields_only_post_clean_frames() {
        let dir = TempDir::new().unwrap();
        let path = log_path(&dir);
        let mut log = ReplayLog::create(&path, 80, 24).unwrap();
        log.append_output(b"\x1b[2J\x1b[Hprompt$ ").unwrap();
        log.append_output(b"\x1b[31mpre-clean red\x1b[0m").unwrap();

        // clean lands while a client is attached
        log.truncate_fresh().unwrap();

        // live stream continues uninterrupted
        log.append_output(b"\x1b[32mpost-clean\x1b[0m line").unwrap();

        let tailed = log.tail(1 << 40).unwrap();
        assert_eq!(
            tailed,
            vec![Event::Output(
                b"\x1b[32mpost-clean\x1b[0m line".to_vec()
            )],
            "reattach must replay only post-clean data"
        );
        let reread = read_events(&path).unwrap();
        assert_eq!(
            reread, tailed,
            "replay boundary must equal the on-disk log — no torn frame in between"
        );
    }
}
