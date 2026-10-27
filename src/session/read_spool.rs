//! Reads of a session made with no reader's anchor, and the pulls such reads
//! make, kept on this host and never in the session's log (koto#428,
//! koto#431, DESIGN-session-retention.md Decision 5).
//!
//! A `koto context get` or `exists` logs its read as a `context_read`, so a
//! visit's reads can be folded into the run journal when the visit closes.
//! Logged into the read session's own log, a read from a shell with no
//! anchor makes the reader a writer of that session: on the cloud backend a
//! push of its whole state file for every read, from a host that may not own
//! the session (koto#412, koto#410). So such a read is appended here instead,
//! beside the session in the local store, under either backend, and never
//! pushed. There are two writers: `koto context get` and `exists` spool an
//! anchorless read themselves (`src/cli/context.rs`), on both backends, and
//! the cloud backend's `append_event` spools any other anchorless
//! `context_read` made outside the session's own tick, as a backstop for a
//! caller that doesn't route its reads. `koto context get` and `exists` also
//! spool a read whose reader's anchor is the session or holds it, since it
//! runs outside the session's tick. Only a read in the session's own tick
//! goes into its log; a read another agent session's anchor records of it
//! goes into that anchor's log.
//!
//! A read of a key another host changed downloads it into this host's local
//! store first, and the cloud backend records that pull as a `context_added`
//! whose writer is `sync`. The pull is the read's doing, so the same rule
//! holds (koto#431): from anything but the session's own tick it is spooled
//! here, beside the read, and the session's log and the bucket are left as
//! they were. Only this host's local mirror of the key and its sync stamp
//! change, which keeps the next read of the session current.
//!
//! The spool is `context-reads.jsonl` in the session's local directory: one
//! line per read or pull, `{"timestamp": ..., "type": "context_read" |
//! "context_added", "payload": {...}}`, an event line without a `seq`,
//! appended by whichever process read. It is host-local: nothing uploads
//! it, and a read made on another host isn't in it.
//!
//! So the spool holds `context_added` lines as well as reads, and [`read`]
//! skips them on purpose: its callers fold every line it returns as a read,
//! and a pull is what a read cost, not a read. A reader that wants the pulls
//! finds them in the file, beside the reads that made them.
//!
//! [`read`] is its one reader of reads and [`append`] its one writer
//! function, which two production call sites use: the read routing of
//! `koto context get` and `koto context exists` for an anchorless read, on
//! either backend, and the cloud backend's `append_event` in
//! `crate::session::cloud`, for reads and pulls. Whatever consumes these reads (today the run
//! journal's visit fold, `crate::run_journal`) goes through `read`, so the
//! spool can move or change shape by changing these two functions and both
//! call sites.
//! A line names no session: the spool's place beside the session says
//! which, so a move that keeps reads elsewhere adds the session to the
//! line and to `read`'s arguments.
//!
//! The spool is not pruned: every read stays until the session's local
//! directory is removed, and each visit close reads the file whole.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::types::EventPayload;
use crate::session::context_log::WRITER_SYNC;

/// File name of the spool inside a session's local directory.
pub const SPOOL_FILE: &str = "context-reads.jsonl";

/// The event type of a spooled read, the only lines [`read`] returns.
const CONTEXT_READ: &str = "context_read";

/// Whether `payload` belongs in the spool rather than the read session's
/// log, when anything but the session's own tick logs it: a read with no
/// reader's anchor (`session` unset), or the pull a read made (a
/// `context_added` written by `sync`). A read that names the session it
/// read is an anchor's own record and goes into that anchor's log.
pub fn spooled(payload: &EventPayload) -> bool {
    match payload {
        EventPayload::ContextRead { session: None, .. } => true,
        EventPayload::ContextAdded { writer, .. } => writer.as_deref() == Some(WRITER_SYNC),
        _ => false,
    }
}

/// One spooled line as written: when it was made, and the payload the log
/// would have held.
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

/// Append `payload`, made at `timestamp`, to the spool of the session whose
/// local directory is `session_dir`. A payload [`spooled`] doesn't accept is
/// refused: only anchorless reads and their pulls go here.
pub fn append(session_dir: &Path, timestamp: &str, payload: &EventPayload) -> anyhow::Result<()> {
    if !spooled(payload) {
        anyhow::bail!("only an anchorless context_read or a sync context_added is spooled");
    }
    let line = serde_json::to_string(&SpooledRead {
        timestamp,
        event_type: payload.type_name(),
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

/// The spool's reads, as written, that were made at or after `since` (an
/// RFC 3339 timestamp as `now_iso8601` writes it, which sorts as text), in
/// the order they were appended. A spooled pull is not a read and is left
/// out. A missing spool is empty; a line that doesn't parse (a torn last
/// write) is skipped.
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
            session: None,
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

    fn added_payload(key: &str, writer: &str) -> EventPayload {
        EventPayload::ContextAdded {
            key: key.to_string(),
            hash: "cd".repeat(32),
            size: 3,
            writer: Some(writer.to_string()),
        }
    }

    #[test]
    fn only_an_anchorless_read_or_a_sync_pull_is_spooled() {
        let dir = tempfile::TempDir::new().unwrap();
        let other = EventPayload::WorkflowCancelled {
            state: "work".to_string(),
            reason: "x".to_string(),
        };
        let anchored = match read_payload("a") {
            EventPayload::ContextRead {
                key,
                reader,
                state,
                present,
                hash,
                access,
                gate,
                ..
            } => EventPayload::ContextRead {
                key,
                reader,
                state,
                present,
                hash,
                access,
                gate,
                session: Some("worker".to_string()),
            },
            _ => unreachable!(),
        };
        assert!(append(dir.path(), "t", &other).is_err());
        assert!(append(dir.path(), "t", &added_payload("a", "agent")).is_err());
        assert!(append(dir.path(), "t", &anchored).is_err());
        assert!(!dir.path().join(SPOOL_FILE).exists());
        assert!(spooled(&read_payload("a")));
        assert!(!spooled(&anchored));
        assert!(spooled(&added_payload("a", WRITER_SYNC)));
    }

    #[test]
    fn a_spooled_pull_is_written_as_context_added_and_is_not_a_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let pull = added_payload("a", WRITER_SYNC);
        append(dir.path(), "2026-10-23T10:00:00.000Z", &pull).unwrap();
        append(dir.path(), "2026-10-23T10:00:00.001Z", &read_payload("a")).unwrap();
        let text = std::fs::read_to_string(dir.path().join(SPOOL_FILE)).unwrap();
        let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["type"], "context_added");
        assert_eq!(first["payload"]["writer"], WRITER_SYNC);
        let reads = read(dir.path(), "");
        assert_eq!(reads.len(), 1);
        assert!(reads[0].contains("\"context_read\""));
    }
}
