//! Tolerant asciicast-v3 reader.
//!
//! Parsing stops at the first line that is not a complete, well-formed
//! `[interval, code, data]` event — a torn tail left by a crash mid-append
//! or a concurrent in-place trim must never panic or yield a partial
//! frame. Well-formed events with codes we do not surface (`i`, unknown)
//! and `#` comment lines are skipped.

use crate::replay::log::Event;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Parse all complete events from the `.cast` file at `path`, skipping
/// the header line. Errors only when the file cannot be read at all.
pub fn read_events(path: &Path) -> io::Result<Vec<Event>> {
    Ok(read_events_sized(path)?
        .into_iter()
        .map(|(event, _)| event)
        .collect())
}

/// Like [`read_events`] but pairs each event with the byte length of its
/// on-disk line.
pub(crate) fn read_events_sized(path: &Path) -> io::Result<Vec<(Event, u64)>> {
    let contents = fs::read(path)?;
    let mut events = Vec::new();
    for line in contents[header_end(&contents)..].split_inclusive(|&b| b == b'\n') {
        if line.first() == Some(&b'#') {
            continue;
        }
        // Lossy decode keeps a crash mid-multibyte write from failing the
        // whole read; the torn line fails to parse and stops the scan.
        let text = String::from_utf8_lossy(line);
        match parse_event_line(text.trim_end()) {
            Some(Some(event)) => events.push((event, line.len() as u64)),
            Some(None) => continue,
            None => break,
        }
    }
    Ok(events)
}

/// Offset just past the header line. The spec requires the header on the
/// first line, but tolerate leading `#` comment lines by skipping them.
fn header_end(contents: &[u8]) -> usize {
    let mut pos = 0usize;
    while pos < contents.len() {
        match contents[pos..].iter().position(|&b| b == b'\n') {
            Some(rel) => {
                let end = pos + rel + 1;
                if contents[pos] == b'#' {
                    pos = end;
                } else {
                    return end;
                }
            }
            None => return contents.len(),
        }
    }
    contents.len()
}

/// Newest whole events whose on-disk lines fit in `n_bytes` — always at
/// least the newest event, never a torn frame. Reads only a suffix
/// window of the file, widening it until the byte budget is settled.
pub(crate) fn read_tail(path: &Path, n_bytes: u64) -> io::Result<Vec<Event>> {
    let mut file = fs::File::open(path)?;
    let mut window = n_bytes.max(1);
    loop {
        let file_len = file.metadata()?.len();
        let start = file_len.saturating_sub(window);
        // A window starting right after a newline opens on a line
        // boundary: its first chunk is a whole line, not a fragment.
        let whole_first_line = if start == 0 {
            true
        } else {
            file.seek(SeekFrom::Start(start - 1))?;
            let mut prev = [0u8; 1];
            file.read_exact(&mut prev)?;
            prev[0] == b'\n'
        };
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; (file_len - start) as usize];
        match file.read_exact(&mut buf) {
            // A concurrent in-place trim shrank the file under the read —
            // re-stat and try again rather than fail the attach.
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => continue,
            result => result?,
        }
        let (events, settled) = tail_window(&buf, whole_first_line, start == 0, n_bytes);
        if settled || start == 0 {
            return Ok(events);
        }
        window = window.saturating_mul(2);
    }
}

/// Raw bytes of the newest complete lines of `path` — plus any torn
/// tail after them — that fit `n_bytes`, never reaching above `floor`
/// (which must sit on a line boundary). Same EOF-windowed scan as
/// [`read_tail`]: the window starts at `n_bytes` and doubles until the
/// budget or `floor` settles it, so only ~the kept bytes are ever read.
pub(crate) fn read_tail_bytes(path: &Path, floor: u64, n_bytes: u64) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut window = n_bytes.max(1);
    loop {
        let file_len = file.metadata()?.len();
        if file_len <= floor {
            return Ok(Vec::new());
        }
        let start = file_len.saturating_sub(window).max(floor);
        let whole_first_line = if start == 0 {
            true
        } else {
            file.seek(SeekFrom::Start(start - 1))?;
            let mut prev = [0u8; 1];
            file.read_exact(&mut prev)?;
            prev[0] == b'\n'
        };
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; (file_len - start) as usize];
        match file.read_exact(&mut buf) {
            // A concurrent in-place trim shrank the file under the read —
            // re-stat and try again.
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => continue,
            result => result?,
        }
        let (kept, settled) = tail_bytes_window(&buf, whole_first_line, start <= floor, n_bytes);
        if settled || start <= floor {
            return Ok(kept.to_vec());
        }
        window = window.saturating_mul(2);
    }
}

/// Scan one EOF window for the newest complete newline-terminated lines
/// fitting `n_bytes`. The returned slice runs from the oldest kept line
/// to the end of `buf`, preserving any torn tail bytes; the flag has
/// [`tail_window`]'s meaning (`false` — a wider window may keep more).
fn tail_bytes_window(
    buf: &[u8],
    whole_first_line: bool,
    at_region_start: bool,
    n_bytes: u64,
) -> (&[u8], bool) {
    // Bytes after the last newline are a torn tail — kept verbatim when
    // any line is kept, so an in-flight append is not cut mid-line.
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return (&buf[buf.len()..], at_region_start);
    };
    let mut used = 0u64;
    let mut kept_from = buf.len();
    let mut end = last_nl + 1;
    loop {
        let line_start = buf[..end - 1]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let len = (end - line_start) as u64;
        if used + len > n_bytes {
            // Budget full — a fragment crossing the window edge only
            // grows, so nothing older fits either.
            return (&buf[kept_from..], true);
        }
        if line_start == 0 && !whole_first_line {
            // The window opened mid-line: the fragment may still fit
            // once its true length is known — a wider window decides.
            return (&buf[kept_from..], false);
        }
        used += len;
        kept_from = line_start;
        if line_start == 0 {
            return (&buf[kept_from..], at_region_start);
        }
        end = line_start;
    }
}

/// Scan one EOF window for the newest whole events fitting `n_bytes`.
/// The returned flag is `false` only when the scan ran off the window's
/// oldest edge — a wider window may still add events.
fn tail_window(
    buf: &[u8],
    whole_first_line: bool,
    at_file_start: bool,
    n_bytes: u64,
) -> (Vec<Event>, bool) {
    // Bytes after the last newline are a torn tail — drop them.
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return (Vec::new(), at_file_start);
    };
    let mut lines = buf[..=last_nl].split_inclusive(|&b| b == b'\n');
    // A window opening mid-file holds a leading fragment, not an event.
    if !whole_first_line {
        lines.next();
    }
    let mut kept: Vec<Event> = Vec::new();
    let mut used = 0u64;
    let mut settled = at_file_start;
    for line in lines.rev() {
        if line.first() == Some(&b'#') {
            continue;
        }
        let text = String::from_utf8_lossy(line);
        match parse_event_line(text.trim_end()) {
            Some(Some(event)) => {
                let len = line.len() as u64;
                if !kept.is_empty() && used + len > n_bytes {
                    // Budget full — a wider window only adds older lines
                    // that fit even less.
                    settled = true;
                    break;
                }
                used += len;
                kept.push(event);
            }
            Some(None) => continue,
            // Malformed or header line — same stop rule as the forward
            // reader; nothing older is reachable.
            None => {
                settled = true;
                break;
            }
        }
    }
    kept.reverse();
    (kept, settled)
}

/// `None` — malformed line, stop reading. `Some(None)` — well-formed but
/// not surfaced (input or unknown code). `Some(Some(_))` — a real event.
fn parse_event_line(line: &str) -> Option<Option<Event>> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let arr = value.as_array()?;
    if arr.len() < 3 || arr[0].as_f64().is_none() {
        return None;
    }
    let code = arr[1].as_str()?;
    let data = arr[2].as_str()?;
    let event = match code {
        "o" => Some(Event::Output(data.as_bytes().to_vec())),
        "r" => {
            let (cols, rows) = data.split_once('x')?;
            Some(Event::Resize {
                cols: cols.parse().ok()?,
                rows: rows.parse().ok()?,
            })
        }
        "m" => Some(Event::Marker(data.to_string())),
        "x" => Some(Event::Exit(data.parse().ok()?)),
        // "i" and unknown codes parse fine but carry no replayable frame
        _ => None,
    };
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::read_events;
    use crate::replay::log::Event;
    use std::fs;
    use tempfile::TempDir;

    fn write_log(dir: &TempDir, contents: &str) -> std::path::PathBuf {
        let path = dir.path().join("replay.cast");
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn reader_parses_header_and_typed_events() {
        let dir = TempDir::new().unwrap();
        let path = write_log(
            &dir,
            concat!(
                "{\"version\":3,\"term\":{\"cols\":80,\"rows\":24}}\n",
                "[0.000000,\"o\",\"hello\"]\n",
                "[0.100000,\"r\",\"120x40\"]\n",
                "[0.200000,\"m\",\"checkpoint\"]\n",
                "[0.300000,\"x\",\"0\"]\n"
            ),
        );

        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], Event::Output(b"hello".to_vec()));
        assert!(
            matches!(events[1], Event::Resize { cols: 120, rows: 40 }),
            "resize event data uses the asciicast <cols>x<rows> form"
        );
        assert_eq!(events[2], Event::Marker("checkpoint".to_string()));
        assert!(matches!(events[3], Event::Exit(_)));
    }

    #[test]
    fn reader_stops_at_truncated_final_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("torn_tail.cast");
        fs::write(&path, include_str!("fixtures/torn_tail.cast")).unwrap();

        let events = read_events(&path).unwrap();
        assert_eq!(
            events,
            vec![
                Event::Output(b"prompt$ ".to_vec()),
                Event::Output(b"first output\r\n".to_vec())
            ],
            "torn tail must stop the reader, complete events survive"
        );
    }

    #[test]
    fn reader_stops_at_corrupt_tail_line() {
        let dir = TempDir::new().unwrap();
        let path = write_log(
            &dir,
            concat!(
                "{\"version\":3,\"term\":{\"cols\":80,\"rows\":24}}\n",
                "[0.000000,\"o\",\"ok\"]\n",
                "this is not an event line\n"
            ),
        );

        let events = read_events(&path).unwrap();
        assert_eq!(events, vec![Event::Output(b"ok".to_vec())]);
    }

    #[test]
    fn reader_returns_empty_events_for_header_only_log() {
        let dir = TempDir::new().unwrap();
        let path = write_log(
            &dir,
            "{\"version\":3,\"term\":{\"cols\":80,\"rows\":24}}\n",
        );
        let events = read_events(&path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn reader_errors_on_missing_file() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such.cast");
        assert!(read_events(&missing).is_err());
    }
}
