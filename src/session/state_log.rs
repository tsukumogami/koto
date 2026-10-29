//! Two copies of one session's state log, compared by their events
//! (koto#477).
//!
//! Under the cloud backend a session written from more than one directory
//! has one bucket copy per directory's prefix, and every workspace on a host
//! shares one local copy. A pull that wrote the fetched copy over the local
//! log without asking whether it held the local events dropped whatever the
//! copy lacked, a rebind among them. [`compare`] is the question a pull and
//! a push ask first.
//!
//! The header line is left out of the prefix test: a rebind rewrites it, so
//! a copy that holds every event of another can still start with a different
//! first line. Events are append-only, so one copy's events being a byte
//! prefix of the other's is what "the other has everything this one has"
//! means; it is the strict-prefix rule `koto session resolve --children auto`
//! applies to children, applied to the events.

/// How two copies of a log relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogOrder {
    /// The same bytes.
    Same,
    /// The same events under different header lines.
    HeaderOnly,
    /// The second copy holds every event of the first, and more.
    RemoteExtends,
    /// The first copy holds every event of the second, and more.
    LocalExtends,
    /// Each copy holds an event the other lacks.
    Diverged,
    /// The two headers name different sessions (`session_id`): a session
    /// re-created under a name whose older copy is still in the bucket. Not
    /// one log's histories, so this module doesn't judge them; callers keep
    /// what they did before koto#477.
    Unrelated,
}

/// The `session_id` a header line records, when it records a non-empty one.
fn session_id(header: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(header).ok()?;
    let id = value.get("session_id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The header line (with its newline) and the event bytes after it.
fn split(log: &[u8]) -> (&[u8], &[u8]) {
    match log.iter().position(|&b| b == b'\n') {
        Some(at) => log.split_at(at + 1),
        None => (log, &[]),
    }
}

/// How `remote` relates to `local`, by their events.
pub fn compare(local: &[u8], remote: &[u8]) -> LogOrder {
    if local == remote {
        return LogOrder::Same;
    }
    let (local_header, local_events) = split(local);
    let (remote_header, remote_events) = split(remote);
    if let (Some(l), Some(r)) = (session_id(local_header), session_id(remote_header)) {
        if l != r {
            return LogOrder::Unrelated;
        }
    }
    if local_events == remote_events {
        debug_assert_ne!(local_header, remote_header);
        LogOrder::HeaderOnly
    } else if remote_events.starts_with(local_events) {
        LogOrder::RemoteExtends
    } else if local_events.starts_with(remote_events) {
        LogOrder::LocalExtends
    } else {
        LogOrder::Diverged
    }
}

/// A log's length in bytes and events, and its last event's seq.
fn describe(log: &[u8]) -> String {
    let (_, events) = split(log);
    let lines: Vec<&[u8]> = events
        .split(|&b| b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .collect();
    let last_seq = lines
        .last()
        .and_then(|l| serde_json::from_slice::<serde_json::Value>(l).ok())
        .and_then(|e| e.get("seq").and_then(serde_json::Value::as_u64));
    format!(
        "{} events, {} bytes, last seq {}",
        lines.len(),
        log.len(),
        last_seq.map_or_else(|| "none".to_string(), |s| s.to_string())
    )
}

/// The refusal when this host's log of `id` and the bucket's copy at `key`
/// have each an event the other lacks. `doing` says what was refused. The
/// message starts `session conflict:`, which makes it a hard sync error.
pub fn conflict(id: &str, key: &str, doing: &str, local: &[u8], remote: &[u8]) -> anyhow::Error {
    anyhow::anyhow!(
        "session conflict: session '{id}' has diverged between this host's log ({}) and the \
         bucket's copy at {key} ({}); {doing}. Nothing was overwritten. Run `koto session \
         resolve {id} --keep local` or `--keep remote` from the session's directory to keep \
         one of them",
        describe(local),
        describe(remote)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "{\"workflow\":\"wf\",\"execution_dir\":\"/a\"}\n";
    const H2: &str = "{\"workflow\":\"wf\",\"execution_dir\":\"/b\"}\n";
    const E1: &str = "{\"seq\":1,\"type\":\"x\"}\n";
    const E2: &str = "{\"seq\":2,\"type\":\"y\"}\n";
    const E2B: &str = "{\"seq\":2,\"type\":\"z\"}\n";

    fn log(parts: &[&str]) -> Vec<u8> {
        parts.concat().into_bytes()
    }

    #[test]
    fn compares_by_events_and_ignores_the_header_line() {
        let base = log(&[H1, E1]);
        assert_eq!(compare(&base, &base), LogOrder::Same);
        assert_eq!(compare(&base, &log(&[H2, E1])), LogOrder::HeaderOnly);
        assert_eq!(compare(&base, &log(&[H2, E1, E2])), LogOrder::RemoteExtends);
        assert_eq!(compare(&log(&[H2, E1, E2]), &base), LogOrder::LocalExtends);
        assert_eq!(
            compare(&log(&[H1, E1, E2]), &log(&[H1, E1, E2B])),
            LogOrder::Diverged
        );
        assert_eq!(compare(&log(&[H1]), &base), LogOrder::RemoteExtends);
    }

    #[test]
    fn headers_naming_different_sessions_are_unrelated() {
        let a = "{\"workflow\":\"wf\",\"session_id\":\"a\"}\n";
        let b = "{\"workflow\":\"wf\",\"session_id\":\"b\"}\n";
        assert_eq!(
            compare(&log(&[a, E1, E2]), &log(&[b, E1, E2B])),
            LogOrder::Unrelated
        );
        // One side without an id is judged by its events.
        assert_eq!(
            compare(&log(&[a, E1]), &log(&[H1, E1, E2])),
            LogOrder::RemoteExtends
        );
    }

    #[test]
    fn a_conflict_names_both_lengths_and_last_seqs() {
        let local = log(&[H1, E1, E2]);
        let remote = log(&[H1, E1, E2B, E1]);
        let msg = conflict(
            "wf",
            "p/wf/koto-wf.state.jsonl",
            "the pull",
            &local,
            &remote,
        )
        .to_string();
        assert!(msg.starts_with("session conflict:"), "{msg}");
        assert!(
            msg.contains(&format!("2 events, {} bytes, last seq 2", local.len())),
            "{msg}"
        );
        assert!(
            msg.contains(&format!("3 events, {} bytes, last seq 1", remote.len())),
            "{msg}"
        );
        assert!(
            msg.contains("koto session resolve wf --keep local"),
            "{msg}"
        );
    }
}
