//! Tolerant asciicast-v3 reader.
//!
//! Parsing stops at the first line that is not a complete, well-formed
//! `[interval, code, data]` event — a torn tail left by a crash mid-append
//! or a concurrent in-place trim must never panic or yield a partial
//! frame. Well-formed events with codes we do not surface (`i`, unknown)
//! are skipped.

use crate::replay::log::Event;
use std::fs;
use std::io;
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
/// on-disk line so `ReplayLog::tail` can size a suffix window.
pub(crate) fn read_events_sized(path: &Path) -> io::Result<Vec<(Event, u64)>> {
    let contents = fs::read_to_string(path)?;
    let header_end = contents
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(contents.len());
    let mut events = Vec::new();
    for line in contents[header_end..].split_inclusive('\n') {
        match parse_event_line(line.trim_end()) {
            Some(Some(event)) => events.push((event, line.len() as u64)),
            Some(None) => continue,
            None => break,
        }
    }
    Ok(events)
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
