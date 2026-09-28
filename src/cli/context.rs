use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::Result;

use crate::cache::sha256_hex;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::session::context::ContextStore;
use crate::session::context_log::{self, ContextReadRecord, READER_CLI, WRITER_AGENT};
use crate::session::SessionBackend;

/// Read content from stdin and store it under the given key with writer
/// `agent`, then emit a `context_added` event naming that writer to the
/// session log. A failed append fails the command, as it always has.
///
/// When `from_file` is provided, reads from that path instead of stdin.
pub fn handle_add(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
    from_file: Option<&str>,
) -> Result<()> {
    // The append below refuses a log that has no header, but by then the
    // store would already hold the content. Check the header first so a
    // refused write stores nothing.
    //
    // This read is load-bearing on the cloud backend for a second reason: it
    // pulls the remote copy over the local one, so an append that follows
    // sees what S3 has. A session whose directory is missing locally still
    // fails here, because the pull writes no directory of its own.
    backend.read_header(session)?;

    let content = match from_file {
        Some(path) => {
            fs::read(path).map_err(|e| anyhow::anyhow!("failed to read file '{}': {}", path, e))?
        }
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| anyhow::anyhow!("failed to read stdin: {}", e))?;
            buf
        }
    };

    store.add_with_writer(session, key, &content, WRITER_AGENT)?;

    let hash = sha256_hex(&content);
    let size = content.len() as u64;
    let event = EventPayload::ContextAdded {
        key: key.to_string(),
        hash,
        size,
        writer: Some(WRITER_AGENT.to_string()),
    };
    backend.append_event(session, &event, &now_iso8601())?;

    Ok(())
}

/// Restore `key` from the session log when a transition assigned it and the
/// store write did not land (koto#204), so a read after a failed write still
/// returns the assigned value.
///
/// Best-effort and silent: a session that does not exist, a log that cannot be
/// read, or a key that is not a usable key leaves the store as it is, and the
/// read that follows reports whatever it finds.
///
/// Returns the log it read, so the read that follows can name the session's
/// current state without reading it again; `None` exactly when nothing was
/// read, which is also when the read logs nothing. The check here is
/// bookkeeping and logs no read of its own.
pub fn restore_assigned(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
) -> Option<Vec<Event>> {
    if crate::session::validate::validate_context_key(key).is_err() || !backend.exists(session) {
        return None;
    }
    let (_, events) = backend.read_events(session).ok()?;
    if let Err(e) = crate::engine::context_assign::reconcile(store, session, &events, Some(key)) {
        eprintln!(
            "warning: failed to restore assigned context value {:?}: {}",
            key, e
        );
    }
    Some(events)
}

/// Append a `koto context get`/`exists` read to the session's log as
/// `reader: "cli"`, best-effort, naming the state `events` ends in. A read
/// with no log behind it (`events` is `None`) logs nothing.
fn log_cli_read(
    backend: &dyn SessionBackend,
    session: &str,
    events: Option<&[Event]>,
    read: Option<ContextReadRecord>,
) {
    let (Some(events), Some(read)) = (events, read) else {
        return;
    };
    let state = derive_state_from_log(events).unwrap_or_default();
    context_log::append_to_session_best_effort(
        backend,
        session,
        &read.into_event(READER_CLI, &state, None),
    );
}

/// Retrieve stored content and write it to stdout.
///
/// When `to_file` is provided, writes to that path instead of stdout.
///
/// The read is logged as a `context_read` (`reader: "cli"`, `access:
/// "content"`) when `events` holds the session's log, including a read of an
/// absent key, which still fails the command.
pub fn handle_get(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    events: Option<&[Event]>,
    session: &str,
    key: &str,
    to_file: Option<&str>,
) -> Result<()> {
    let result = store.get(session, key);
    log_cli_read(
        backend,
        session,
        events,
        ContextReadRecord::content(key, result.as_deref().ok()),
    );
    let content = result?;

    match to_file {
        Some(path) => {
            if let Some(parent) = PathBuf::from(path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("failed to create parent directory for '{}': {}", path, e)
                    })?;
                }
            }
            fs::write(path, &content)
                .map_err(|e| anyhow::anyhow!("failed to write file '{}': {}", path, e))?;
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&content)
                .map_err(|e| anyhow::anyhow!("failed to write to stdout: {}", e))?;
        }
    }

    Ok(())
}

/// What the store can say about a key: it is here, it is not here, or it is not
/// a key at all.
///
/// The third arm is why this is not a bool. `ctx_exists` reports a key that
/// fails the grammar as absent, so the two negatives used to arrive
/// indistinguishable and a caller probing with a substituted key would decide a
/// key was missing when koto had in fact refused to look for it (Issue #227).
pub enum KeyPresence {
    /// The key is in the store.
    Present,
    /// The key is a usable key and the store does not have it.
    Absent,
    /// The key is not usable, carrying the reason from
    /// [`crate::session::validate::unusable_context_key_reason`].
    Unusable(String),
}

/// Check whether a key exists, distinguishing a key the store will not accept
/// from one it accepts and does not have.
///
/// The key is checked here rather than in the store because the store's own
/// answer is a bool with nowhere to put a reason, and because the context gate
/// already checks caller-side -- one mechanism for the question rather than two.
/// The wording is not composed here either: both callers share
/// [`crate::session::validate::unusable_context_key_reason`] so they cannot
/// drift into describing the same key differently.
///
/// The caller is responsible for mapping the outcome to exit codes.
///
/// A usable key's answer is logged as a `context_read` (`reader: "cli"`,
/// `access: "presence"`) when `events` holds the session's log.
pub fn handle_exists(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    events: Option<&[Event]>,
    session: &str,
    key: &str,
) -> KeyPresence {
    if let Some(reason) = crate::session::validate::unusable_context_key_reason(key) {
        return KeyPresence::Unusable(reason);
    }
    let present = store.ctx_exists(session, key);
    log_cli_read(
        backend,
        session,
        events,
        ContextReadRecord::presence(store, session, key, present),
    );
    if present {
        KeyPresence::Present
    } else {
        KeyPresence::Absent
    }
}

/// Remove a key and its content from the store, then emit a `context_removed`
/// event to the session log.
///
/// Idempotent: removing a key that is not there succeeds, matching
/// `ContextStore::remove`'s own contract and the usual shape of a delete verb.
/// A caller that needs to distinguish the two cases probes with
/// `context exists` first.
///
/// The event is emitted unconditionally, including on the idempotent no-op.
/// The log is the authoritative record of what happened to a session, and
/// `add` already writes one; a removal that left no trace would leave a log
/// asserting a key was added and never saying it went away, which is worse than
/// an occasional event for a key that was already absent.
pub fn handle_remove(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
) -> Result<()> {
    // As in `handle_add`: refuse a log with no header before the store
    // changes, so a refused removal leaves the store as it was.
    backend.read_header(session)?;

    store.remove(session, key)?;

    let event = EventPayload::ContextRemoved {
        key: key.to_string(),
        writer: Some(WRITER_AGENT.to_string()),
    };
    backend.append_event(session, &event, &now_iso8601())?;

    Ok(())
}

/// List all keys as a JSON array, optionally filtered by prefix.
pub fn handle_list(store: &dyn ContextStore, session: &str, prefix: Option<&str>) -> Result<()> {
    let keys = store.list_keys(session, prefix)?;
    println!("{}", serde_json::to_string(&keys)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::context_log::fail_best_effort_appends;
    use crate::session::local::LocalBackend;
    use crate::session::state_file_name;

    /// A session `s` in state `work`, with its log written directly.
    fn session(dir: &std::path::Path) -> LocalBackend {
        let backend = LocalBackend::with_base_dir(dir.to_path_buf());
        std::fs::create_dir_all(dir.join("s")).unwrap();
        std::fs::write(
            dir.join("s").join(state_file_name("s")),
            concat!(
                r#"{"schema_version":1,"workflow":"s","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":null,"to":"work","condition_type":"auto"}}"#,
                "\n",
            ),
        )
        .unwrap();
        backend
    }

    fn event_types(backend: &LocalBackend) -> Vec<String> {
        let (_, events) = backend.read_events("s").unwrap();
        events.into_iter().map(|e| e.event_type).collect()
    }

    #[test]
    fn get_and_exists_log_one_read_each_naming_the_current_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        backend
            .add_with_writer("s", "k", b"v", WRITER_AGENT)
            .unwrap();
        let out = dir.path().join("out.txt");

        let events = restore_assigned(&backend, &backend, "s", "k");
        handle_get(
            &backend,
            &backend,
            events.as_deref(),
            "s",
            "k",
            Some(out.to_str().unwrap()),
        )
        .unwrap();
        assert!(matches!(
            handle_exists(&backend, &backend, events.as_deref(), "s", "k"),
            KeyPresence::Present
        ));
        let (_, events) = backend.read_events("s").unwrap();
        let reads: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::ContextRead {
                    reader,
                    state,
                    access,
                    hash,
                    ..
                } => Some((
                    reader.as_str(),
                    state.as_str(),
                    access.clone(),
                    hash.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            reads,
            vec![
                (
                    "cli",
                    "work",
                    Some("content".to_string()),
                    Some(sha256_hex(b"v"))
                ),
                (
                    "cli",
                    "work",
                    Some("presence".to_string()),
                    Some(sha256_hex(b"v"))
                ),
            ]
        );
    }

    /// With every best-effort append failing, reads and writes behave as they
    /// did before reads were logged: `get` returns the content, `exists`
    /// answers, `add` stores and logs its own `context_added` (which is not
    /// best-effort), and only the reads go unrecorded.
    #[test]
    fn failing_read_appends_change_nothing_the_commands_return() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        let input = dir.path().join("in.txt");
        std::fs::write(&input, b"content").unwrap();
        let out = dir.path().join("out.txt");
        let _hook = fail_best_effort_appends();

        handle_add(&backend, &backend, "s", "k", Some(input.to_str().unwrap())).unwrap();
        let events = restore_assigned(&backend, &backend, "s", "k");
        handle_get(
            &backend,
            &backend,
            events.as_deref(),
            "s",
            "k",
            Some(out.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"content");
        assert!(matches!(
            handle_exists(&backend, &backend, events.as_deref(), "s", "k"),
            KeyPresence::Present
        ));
        assert!(handle_get(&backend, &backend, events.as_deref(), "s", "absent", None).is_err());

        assert_eq!(event_types(&backend), vec!["transitioned", "context_added"]);
        assert_eq!(
            backend.meta("s", "k").unwrap().writer.as_deref(),
            Some(WRITER_AGENT)
        );
    }

    #[test]
    fn a_session_with_no_log_logs_no_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        assert!(restore_assigned(&backend, &backend, "nope", "k").is_none());
        assert!(handle_get(&backend, &backend, None, "nope", "k", None).is_err());
    }
}
