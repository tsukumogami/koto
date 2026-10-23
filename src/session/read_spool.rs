//! Reads of a session made by anything but that session's own tick, kept on
//! this host and never in the session's log (koto#428).
//!
//! A `koto context get` or `exists` logs its read as a `context_read`, so a
//! visit's reads can be folded into the run journal when the visit closes.
//! Logged into the read session's own log, a read from another session's
//! tick, or from outside any tick, makes the reader a writer of that
//! session: on the cloud backend a push of its whole state file for every
//! read, from a host that may not own the session (koto#412, koto#410). So
//! the cloud backend appends such a read here instead, beside the session in
//! the local store, and never pushes it. A session's own tick still logs its
//! reads in the log, as before.
//!
//! The spool is `context-reads.jsonl` in the session's local directory: one
//! line per read, `{"timestamp": ..., "type": "context_read", "payload":
//! {...}}`, an event line without a `seq`, appended by whichever process
//! read. It is host-local: nothing uploads it, and a read made on another
//! host isn't in it.
//!
//! [`read`] is its one reader. Whatever consumes these reads (today the
//! run journal's visit fold, `crate::run_journal`) goes through it, so the
//! spool can move (to the reader's own side, as the session-retention work
//! plans) by changing this module alone.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::types::EventPayload;

/// File name of the spool inside a session's local directory.
pub const SPOOL_FILE: &str = "context-reads.jsonl";

/// The event type every spooled line carries.
const CONTEXT_READ: &str = "context_read";

/// One spooled read as written: when it was made, and the `context_read`
/// payload the log would have held.
#[derive(Serialize)]
struct SpooledRead<'a> {
    timestamp: &'a str,
    #[serde(rename = "type")]
    event_type: &'a str,
    payload: &'a EventPayload,
}

/// What [`read`] parses of a line. The payload is left to the consumer,
/// which parses it as it parses a log line: `EventPayload` is untagged, so
/// only the line's `type` says which payload it is.
#[derive(Deserialize)]
struct SpoolHead {
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
}

/// Append `payload`, read at `timestamp`, to the spool of the session whose
/// local directory is `session_dir`. A payload that isn't a `context_read`
/// is refused: only reads go here.
pub fn append(session_dir: &Path, timestamp: &str, payload: &EventPayload) -> anyhow::Result<()> {
    if !matches!(payload, EventPayload::ContextRead { .. }) {
        anyhow::bail!("only a context_read is spooled");
    }
    let line = serde_json::to_string(&SpooledRead {
        timestamp,
        event_type: CONTEXT_READ,
        payload,
    })?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(session_dir.join(SPOOL_FILE))?;
    // One write of the whole line, so concurrent appenders never interleave
    // within a line.
    file.write_all(format!("{}\n", line).as_bytes())?;
    Ok(())
}

/// The spool's lines, as written, that were made at or after `since` (an
/// RFC 3339 timestamp as `now_iso8601` writes it, which sorts as text), in
/// the order they were appended. A missing spool is empty; a line that
/// doesn't parse (a torn last write) is skipped.
pub fn read(session_dir: &Path, since: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(session_dir.join(SPOOL_FILE)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| {
            serde_json::from_str::<SpoolHead>(line).is_ok_and(|read| {
                read.event_type == CONTEXT_READ && read.timestamp.as_str() >= since
            })
        })
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_payload(key: &str) -> EventPayload {
        EventPayload::ContextRead {
            key: key.to_string(),
            reader: "cli".to_string(),
            state: "work".to_string(),
            present: true,
            hash: Some("ab".repeat(32)),
            access: None,
            gate: None,
        }
    }

    #[test]
    fn appended_reads_come_back_from_the_time_asked() {
        let dir = tempfile::TempDir::new().unwrap();
        append(dir.path(), "2026-10-23T10:00:00.000Z", &read_payload("a")).unwrap();
        append(dir.path(), "2026-10-23T10:00:01.000Z", &read_payload("b")).unwrap();
        let lines = read(dir.path(), "2026-10-23T10:00:00.500Z");
        assert_eq!(lines.len(), 1);
        let only: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(only["type"], "context_read");
        assert_eq!(only["payload"]["key"], "b");
        assert_eq!(read(dir.path(), "").len(), 2);
    }

    #[test]
    fn a_missing_spool_is_empty_and_a_torn_line_is_skipped() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read(dir.path(), "").is_empty());
        append(dir.path(), "2026-10-23T10:00:00.000Z", &read_payload("a")).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(SPOOL_FILE))
            .unwrap();
        f.write_all(b"{\"timestamp\":\"2026-10-23T10:00:0").unwrap();
        assert_eq!(read(dir.path(), "").len(), 1);
    }

    #[test]
    fn only_a_read_is_spooled() {
        let dir = tempfile::TempDir::new().unwrap();
        let other = EventPayload::WorkflowCancelled {
            state: "work".to_string(),
            reason: "x".to_string(),
        };
        assert!(append(dir.path(), "t", &other).is_err());
        assert!(!dir.path().join(SPOOL_FILE).exists());
    }
}
