use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::engine::types::Event;

/// Append one event as a JSON line to the state file at `path`.
///
/// Creates the file if it doesn't exist; appends otherwise.
pub fn append_event(path: &Path, event: &Event) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let line = serde_json::to_string(event)
        .map_err(|e| anyhow::anyhow!("failed to serialize event: {}", e))?;

    writeln!(file, "{}", line)
        .map_err(|e| anyhow::anyhow!("failed to write event to {}: {}", path.display(), e))?;

    Ok(())
}

/// Read all events from the JSONL state file at `path`.
///
/// Skips malformed lines with a warning to stderr.
pub fn read_events(path: &Path) -> anyhow::Result<Vec<Event>> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (i, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "warning: failed to read line {} in {}: {}",
                    i + 1,
                    path.display(),
                    e
                );
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Event>(trimmed) {
            Ok(event) => events.push(event),
            Err(e) => {
                eprintln!(
                    "warning: skipping malformed event on line {} in {}: {}",
                    i + 1,
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(events)
}

/// Derive the current state from an event log.
///
/// Returns `Some(state)` from the last event, or `None` if the log is empty.
pub fn derive_state(events: &[Event]) -> Option<String> {
    events.last().map(|e| e.state.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::Event;
    use tempfile::TempDir;

    fn make_event(event_type: &str, state: &str) -> Event {
        Event {
            event_type: event_type.to_string(),
            state: state.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            template: None,
            template_hash: None,
        }
    }

    #[test]
    fn append_creates_then_extends_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-test.state.jsonl");

        assert!(!path.exists());

        append_event(&path, &make_event("init", "gather")).unwrap();
        assert!(path.exists());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);

        append_event(&path, &make_event("rewind", "gather")).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn read_events_returns_correct_sequence() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-seq.state.jsonl");

        append_event(&path, &make_event("init", "gather")).unwrap();
        append_event(&path, &make_event("transition", "plan")).unwrap();
        append_event(&path, &make_event("transition", "review")).unwrap();

        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, "init");
        assert_eq!(events[0].state, "gather");
        assert_eq!(events[1].state, "plan");
        assert_eq!(events[2].state, "review");
    }

    #[test]
    fn read_events_skips_malformed_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-malformed.state.jsonl");

        // Write one valid line, one malformed, one valid.
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"init","state":"gather","timestamp":"2026-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(f, "not valid json {{{{").unwrap();
        writeln!(
            f,
            r#"{{"type":"transition","state":"plan","timestamp":"2026-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        drop(f);

        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].state, "gather");
        assert_eq!(events[1].state, "plan");
    }

    #[test]
    fn derive_state_returns_last_state() {
        let events = vec![
            make_event("init", "gather"),
            make_event("transition", "plan"),
            make_event("transition", "review"),
        ];
        assert_eq!(derive_state(&events), Some("review".to_string()));
    }

    #[test]
    fn derive_state_returns_none_for_empty() {
        assert_eq!(derive_state(&[]), None);
    }
}
