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
    // fails here, because the pull writes no directory of its own. (Inside a
    // tick on this session there is no pull: the tick pulled, and the local
    // copy holds writes the bucket doesn't have yet.)
    backend.read_header(session)?;

    // A reserved key is refused by the store itself; checking here as well
    // refuses it before stdin is read, rather than after.
    crate::session::links::check_writer_key(key, WRITER_AGENT)?;

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

/// Where a `koto context get`/`exists` read of a session is logged
/// (DESIGN-session-retention.md, Decision 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadLogTarget {
    /// The session read: the reader's anchor is the session read or holds
    /// it, so the read is the session's own.
    Session,
    /// The reader's anchor, naming the session read in the event's
    /// `session` field, so the session read stays byte-identical.
    Anchor(String),
    /// No reader's anchor: `KOTO_ANCHOR` is unset or names no anchor here.
    /// Recorded by [`record_anchorless_read`].
    Anchorless,
}

/// The reader's anchor `koto context get` and `exists` log by: the
/// `KOTO_ANCHOR` the command runs with, on the local backend.
///
/// On the cloud backend it is `None` for now, so every read there takes the
/// anchorless row. The cloud read path is being reworked to record reads of
/// another session on the reading host; once that lands, it hooks in here,
/// passing the same anchor to [`read_log_target`] and recording through
/// [`log_cli_read`].
pub fn reader_anchor(is_cloud: bool) -> Option<String> {
    if is_cloud {
        return None;
    }
    crate::session::anchor::anchor_from_env()
}

/// Where a read of `session` by a reader whose anchor is `reader_anchor`
/// (the `KOTO_ANCHOR` the command runs with) is logged:
///
/// | Reader anchor | Logged in |
/// |---|---|
/// | names an existing anchor that is `session`, or that `session` holds through `koto/links/held-by/<anchor>` | the session read |
/// | names an existing anchor otherwise | the reader's anchor |
/// | unset, or names no existing anchor | [`ReadLogTarget::Anchorless`] |
///
/// Nothing here writes, and the read session's header and manifest are read
/// in place.
pub fn read_log_target(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    reader_anchor: Option<&str>,
) -> ReadLogTarget {
    use crate::session::links::{link_key, LinkKind};

    let Some(anchor) = reader_anchor else {
        return ReadLogTarget::Anchorless;
    };
    if !crate::session::anchor::is_anchor(backend, anchor) {
        return ReadLogTarget::Anchorless;
    }
    if anchor == session {
        return ReadLogTarget::Session;
    }
    if store.ctx_exists(session, &link_key(LinkKind::HeldBy, anchor)) {
        return ReadLogTarget::Session;
    }
    ReadLogTarget::Anchor(anchor.to_string())
}

/// Append a `koto context get`/`exists` read to the log [`read_log_target`]
/// picks, as `reader: "cli"`, best-effort, naming the state `events` (the
/// read session's log) ends in. A read with no log behind it (`events` is
/// `None`) logs nothing. A read recorded in the reader's anchor carries the
/// session it read in `session`; an append that fails there warns on
/// stderr and the read goes on, as any read's append does.
fn log_cli_read(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    events: Option<&[Event]>,
    reader_anchor: Option<&str>,
    read: Option<ContextReadRecord>,
) {
    let (Some(events), Some(read)) = (events, read) else {
        return;
    };
    let state = derive_state_from_log(events).unwrap_or_default();
    match read_log_target(store, backend, session, reader_anchor) {
        ReadLogTarget::Session => {
            context_log::append_to_session_best_effort(
                backend,
                session,
                &read.into_event(READER_CLI, &state, None),
            );
        }
        ReadLogTarget::Anchor(anchor) => {
            context_log::append_to_session_best_effort(
                backend,
                &anchor,
                &read.into_anchor_event(READER_CLI, &state, session),
            );
        }
        ReadLogTarget::Anchorless => {
            record_anchorless_read(backend, session, &read.into_event(READER_CLI, &state, None));
        }
    }
}

/// Record a read of `session` made with no reader's anchor: the one place
/// the anchorless row is written.
///
/// Today it appends to the read session's own log, as koto always has, which
/// makes a read from a bare shell a known cross-session writer (koto#412).
/// It is meant to move to the reading host's read spool, a file beside the
/// read session that never leaves the host and is read back through
/// `session::read_spool::read`, once that reader lands; this function is
/// where its writer plugs in.
fn record_anchorless_read(backend: &dyn SessionBackend, session: &str, payload: &EventPayload) {
    context_log::append_to_session_best_effort(backend, session, payload);
}

/// Retrieve stored content and write it to stdout.
///
/// When `to_file` is provided, writes to that path instead of stdout.
///
/// The read is logged as a `context_read` (`reader: "cli"`, `access:
/// "content"`) when `events` holds the session's log, including a read of an
/// absent key, which still fails the command. It goes into the session's
/// log or the reader's anchor, as [`read_log_target`] decides for
/// `reader_anchor`.
pub fn handle_get(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    events: Option<&[Event]>,
    reader_anchor: Option<&str>,
    session: &str,
    key: &str,
    to_file: Option<&str>,
) -> Result<()> {
    let result = store.get(session, key);
    log_cli_read(
        store,
        backend,
        session,
        events,
        reader_anchor,
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
/// `access: "presence"`) when `events` holds the session's log, in the
/// session's log or the reader's anchor as [`read_log_target`] decides.
pub fn handle_exists(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    events: Option<&[Event]>,
    reader_anchor: Option<&str>,
    session: &str,
    key: &str,
) -> KeyPresence {
    if let Some(reason) = crate::session::validate::unusable_context_key_reason(key) {
        return KeyPresence::Unusable(reason);
    }
    let present = store.ctx_exists(session, key);
    log_cli_read(
        store,
        backend,
        session,
        events,
        reader_anchor,
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

    store.remove_with_writer(session, key, WRITER_AGENT)?;

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
    use crate::session::context_log::{fail_best_effort_appends, take_warnings};
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
            None,
            "s",
            "k",
            Some(out.to_str().unwrap()),
        )
        .unwrap();
        assert!(matches!(
            handle_exists(&backend, &backend, events.as_deref(), None, "s", "k"),
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
        take_warnings();

        // `add` makes no best-effort append of its own (its `context_added`
        // is a required write), so the hook has nothing to refuse and `add`
        // warns about nothing.
        handle_add(&backend, &backend, "s", "k", Some(input.to_str().unwrap())).unwrap();
        assert_eq!(take_warnings(), Vec::<String>::new());
        let events = restore_assigned(&backend, &backend, "s", "k");
        handle_get(
            &backend,
            &backend,
            events.as_deref(),
            None,
            "s",
            "k",
            Some(out.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"content");
        assert!(matches!(
            handle_exists(&backend, &backend, events.as_deref(), None, "s", "k"),
            KeyPresence::Present
        ));
        assert!(handle_get(
            &backend,
            &backend,
            events.as_deref(),
            None,
            "s",
            "absent",
            None
        )
        .is_err());
        let refused = "test hook: append refused";
        assert_eq!(
            take_warnings(),
            vec![
                format!("warning: failed to record context_read for context key \"k\": {refused}"),
                format!("warning: failed to record context_read for context key \"k\": {refused}"),
                format!(
                    "warning: failed to record context_read for context key \"absent\": {refused}"
                ),
            ]
        );

        assert_eq!(event_types(&backend), vec!["transitioned", "context_added"]);
        assert_eq!(
            backend.meta("s", "k").unwrap().writer.as_deref(),
            Some(WRITER_AGENT)
        );
    }

    /// A store that says every key exists but has no metadata and no
    /// readable content for any of them.
    struct Hashless;

    impl ContextStore for Hashless {
        fn add(&self, _: &str, _: &str, _: &[u8]) -> Result<()> {
            anyhow::bail!("read-only")
        }
        fn get(&self, _: &str, _: &str) -> Result<Vec<u8>> {
            anyhow::bail!("unreadable")
        }
        fn ctx_exists(&self, _: &str, _: &str) -> bool {
            true
        }
        fn remove(&self, _: &str, _: &str) -> Result<()> {
            anyhow::bail!("read-only")
        }
        fn list_keys(&self, _: &str, _: Option<&str>) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    /// `koto context exists` on a key with no hash to be had still answers
    /// present, and appends nothing to the log.
    #[test]
    fn exists_on_a_key_with_no_hash_logs_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        let (_, events) = backend.read_events("s").unwrap();
        assert!(matches!(
            handle_exists(&Hashless, &backend, Some(&events), None, "s", "k"),
            KeyPresence::Present
        ));
        assert_eq!(event_types(&backend), vec!["transitioned"]);
    }

    #[test]
    fn a_session_with_no_log_logs_no_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        assert!(restore_assigned(&backend, &backend, "nope", "k").is_none());
        assert!(handle_get(&backend, &backend, None, None, "nope", "k", None).is_err());
    }

    /// An anchor `name`, its header marked `kind: "anchor"`, with one event.
    fn anchor(dir: &std::path::Path, name: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(
            dir.join(name).join(state_file_name(name)),
            format!(
                concat!(
                    r#"{{"schema_version":1,"workflow":"{name}","template_hash":"h","created_at":"2026-01-01T00:00:00Z","kind":"anchor"}}"#,
                    "\n",
                    r#"{{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{{"from":null,"to":"open","condition_type":"auto"}}}}"#,
                    "\n",
                ),
                name = name
            ),
        )
        .unwrap();
    }

    /// Every file under `dir` and its bytes.
    fn files(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// The `context_read` events in `name`'s log, as (key, access, state,
    /// session).
    fn reads_in(
        backend: &LocalBackend,
        name: &str,
    ) -> Vec<(String, Option<String>, String, Option<String>)> {
        let (_, events) = backend.read_events(name).unwrap();
        events
            .into_iter()
            .filter_map(|e| match e.payload {
                EventPayload::ContextRead {
                    key,
                    access,
                    state,
                    session,
                    ..
                } => Some((key, access, state, session)),
                _ => None,
            })
            .collect()
    }

    /// Run `get` then `exists` of key `k` in `s` with `anchor` as the
    /// reader's anchor; both must answer as a plain read would.
    fn read_both(backend: &LocalBackend, dir: &std::path::Path, anchor: Option<&str>) {
        let out = dir.join("out.txt");
        let events = restore_assigned(backend, backend, "s", "k");
        handle_get(
            backend,
            backend,
            events.as_deref(),
            anchor,
            "s",
            "k",
            Some(out.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"v");
        assert!(matches!(
            handle_exists(backend, backend, events.as_deref(), anchor, "s", "k"),
            KeyPresence::Present
        ));
    }

    /// A reader whose anchor exists and doesn't hold the session read logs
    /// both reads in its anchor, naming the session and the key, and leaves
    /// the session read byte-identical.
    #[test]
    fn a_read_from_another_agent_session_is_recorded_in_the_readers_anchor() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        backend
            .add_with_writer("s", "k", b"v", WRITER_AGENT)
            .unwrap();
        anchor(dir.path(), "anchor-r");
        let before = files(&dir.path().join("s"));

        assert_eq!(
            read_log_target(&backend, &backend, "s", Some("anchor-r")),
            ReadLogTarget::Anchor("anchor-r".to_string())
        );
        read_both(&backend, dir.path(), Some("anchor-r"));

        assert_eq!(files(&dir.path().join("s")), before);
        let s = Some("s".to_string());
        assert_eq!(
            reads_in(&backend, "anchor-r"),
            vec![
                ("k".into(), Some("content".into()), "work".into(), s.clone()),
                ("k".into(), Some("presence".into()), "work".into(), s),
            ]
        );
    }

    /// An anchor the session read holds a held-by link to, or that is the
    /// session itself, is the session's own agent session: the read logs in
    /// the session, with no `session` field, and the anchor gets nothing.
    #[test]
    fn an_own_read_still_logs_in_the_session_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        backend
            .add_with_writer("s", "k", b"v", WRITER_AGENT)
            .unwrap();
        anchor(dir.path(), "anchor-own");
        crate::session::links::write_link(
            &backend,
            "s",
            crate::session::links::LinkKind::HeldBy,
            "anchor-own",
            &crate::session::links::LinkHints::default(),
        )
        .unwrap();
        let anchor_before = files(&dir.path().join("anchor-own"));

        assert_eq!(
            read_log_target(&backend, &backend, "s", Some("anchor-own")),
            ReadLogTarget::Session
        );
        read_both(&backend, dir.path(), Some("anchor-own"));
        assert_eq!(files(&dir.path().join("anchor-own")), anchor_before);
        assert_eq!(
            reads_in(&backend, "s"),
            vec![
                ("k".into(), Some("content".into()), "work".into(), None),
                ("k".into(), Some("presence".into()), "work".into(), None),
            ]
        );

        // An anchor reading itself is its own read too.
        assert_eq!(
            read_log_target(&backend, &backend, "anchor-own", Some("anchor-own")),
            ReadLogTarget::Session
        );
    }

    /// With no reader anchor, or a name that is no anchor here (absent, or
    /// an ordinary session, the read session included), the read takes the
    /// anchorless row, which still logs in the session read.
    #[test]
    fn no_reader_anchor_takes_the_anchorless_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        backend
            .add_with_writer("s", "k", b"v", WRITER_AGENT)
            .unwrap();
        std::fs::create_dir_all(dir.path().join("plain")).unwrap();
        std::fs::write(
            dir.path().join("plain").join(state_file_name("plain")),
            "{\"schema_version\":1,\"workflow\":\"plain\",\"template_hash\":\"h\",\"created_at\":\"2026-01-01T00:00:00Z\"}\n",
        )
        .unwrap();
        for anchor in [None, Some("no-such-anchor"), Some("plain"), Some("s")] {
            assert_eq!(
                read_log_target(&backend, &backend, "s", anchor),
                ReadLogTarget::Anchorless,
                "{anchor:?}"
            );
        }
        read_both(&backend, dir.path(), None);
        read_both(&backend, dir.path(), Some("no-such-anchor"));
        assert_eq!(reads_in(&backend, "s").len(), 4);
        assert!(reads_in(&backend, "s").iter().all(|r| r.3.is_none()));
    }

    /// An append to the reader's anchor that fails warns on stderr and the
    /// read still answers; the session read stays byte-identical.
    #[test]
    fn a_failed_append_to_the_readers_anchor_warns_and_never_fails_the_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = session(dir.path());
        backend
            .add_with_writer("s", "k", b"v", WRITER_AGENT)
            .unwrap();
        anchor(dir.path(), "anchor-r");
        let before = files(dir.path());
        let _hook = fail_best_effort_appends();
        take_warnings();

        read_both(&backend, dir.path(), Some("anchor-r"));
        let refused = "test hook: append refused";
        assert_eq!(
            take_warnings(),
            vec![
                format!("warning: failed to record context_read for context key \"k\": {refused}"),
                format!("warning: failed to record context_read for context key \"k\": {refused}"),
            ]
        );
        let after: Vec<_> = files(dir.path())
            .into_iter()
            .filter(|(p, _)| !p.ends_with("out.txt"))
            .collect();
        assert_eq!(after, before);
    }
}
