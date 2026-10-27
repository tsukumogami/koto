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

/// The log of `session`, read for a `koto context get` or `exists` of `key`
/// to answer from and to name the session's current state in.
///
/// Read-only: a value a transition assigned to `key` whose store write did
/// not land (koto#204) is answered from this log by [`handle_get`] and
/// [`handle_exists`], never stored. The session's own next `koto next`
/// stores it, since that tick reconciles the store with the log before it
/// evaluates anything, so a read never writes into the session it reads.
///
/// Best-effort and silent: a session that does not exist, a log that cannot
/// be read, or a key that is not a usable key gives `None`, and the read that
/// follows answers from the store alone and logs nothing.
pub fn read_session_log(
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
) -> Option<Vec<Event>> {
    if crate::session::validate::validate_context_key(key).is_err() || !backend.exists(session) {
        return None;
    }
    backend.read_events(session).ok().map(|(_, events)| events)
}

/// The value a transition assigned to `key` that the store should hold, from
/// the session's log: `Some` exactly when the latest event touching `key`
/// in `events` is a transition's assignment. A read answers it in place of
/// what the store holds, which differs only after a lost store write.
fn assigned_value(events: Option<&[Event]>, key: &str) -> Option<String> {
    crate::engine::context_assign::outstanding_assignments(events?).remove(key)
}

/// Where a `koto context get`/`exists` read of a session is logged
/// (DESIGN-session-retention.md, Decision 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadLogTarget {
    /// The session read: the read runs inside the session's own tick (a
    /// gate script or default action reading its own session).
    Session,
    /// The reader's anchor, naming the session read in the event's
    /// `session` field, so the session read stays byte-identical.
    Anchor(String),
    /// Outside the session's own tick with no other agent session reading:
    /// `KOTO_ANCHOR` is unset, names no anchor here, or names an anchor
    /// that is the session read or holds it. Recorded by
    /// [`record_anchorless_read`].
    Anchorless,
}

/// The reader's anchor `koto context get` and `exists` log by: the
/// `KOTO_ANCHOR` the command runs with, on either backend.
pub fn reader_anchor() -> Option<String> {
    crate::session::anchor::anchor_from_env()
}

/// Where a read of `session` by a reader whose anchor is `reader_anchor`
/// (the `KOTO_ANCHOR` the command runs with) is logged:
///
/// | Reader anchor | Logged in |
/// |---|---|
/// | any, from a command `session`'s own tick runs (a gate script's read) | the session read |
/// | names an existing anchor that is `session`, or that holds `session` (`session` carries `koto/links/held-by/<anchor>`) | [`ReadLogTarget::Anchorless`] |
/// | names an existing anchor otherwise | the reader's anchor |
/// | unset, or names no existing anchor | [`ReadLogTarget::Anchorless`] |
///
/// Only the session's own tick appends to the session it reads, so a read
/// outside that tick leaves the session byte-identical whoever runs it.
/// Nothing here writes to a session's log or context, and the read
/// session's header and manifest are read in place. Under the cloud backend
/// the held-by check may refresh the host-local sync cache beside the read
/// session, which is never pushed.
pub fn read_log_target(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    reader_anchor: Option<&str>,
) -> ReadLogTarget {
    use crate::session::links::{link_key, LinkKind};

    if crate::engine::reentrancy::enclosing_tick().as_deref() == Some(session) {
        return ReadLogTarget::Session;
    }
    let Some(anchor) = reader_anchor else {
        return ReadLogTarget::Anchorless;
    };
    if !crate::session::anchor::is_anchor(backend, anchor) {
        return ReadLogTarget::Anchorless;
    }
    if anchor == session || store.ctx_exists(session, &link_key(LinkKind::HeldBy, anchor)) {
        return ReadLogTarget::Anchorless;
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
            // A read in the session's own tick: into the session's log on
            // every backend, which `append_own_read` tells the cloud backend
            // so it logs the read rather than spooling it.
            context_log::append_best_effort(&read.into_event(READER_CLI, &state, None), |p| {
                backend.append_own_read(session, p, &crate::engine::types::now_iso8601())
            });
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

/// Record a read of `session` made outside its own tick by no other agent
/// session (no reader's anchor, or one that is or holds `session`): the one
/// place the anchorless row is written, on either backend.
///
/// It goes into the reading host's read spool beside the session's local
/// copy ([`crate::session::read_spool`]), never into the session's log, so
/// a read from a bare shell writes nothing of the session it read
/// (koto#412). The spool never leaves the host, counts as no session's
/// activity, and is read back only through `read_spool::read`, which the
/// run journal's visit fold uses. Best-effort: a spool that can't be
/// appended to warns on stderr and the read goes on.
fn record_anchorless_read(backend: &dyn SessionBackend, session: &str, payload: &EventPayload) {
    let dir = backend.session_dir(session);
    context_log::append_best_effort(payload, |p| {
        crate::session::read_spool::append(&dir, &crate::engine::types::now_iso8601(), p)
    });
}

/// Retrieve a key's content and write it to stdout.
///
/// When `to_file` is provided, writes to that path instead of stdout.
///
/// The content is the value a transition assigned to the key when `events`
/// (the session's log) says the store should hold one, and the store's
/// otherwise: after a lost store write the two differ, and the log is the
/// record. Nothing is stored either way.
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
    let result = match assigned_value(events, key) {
        Some(value) => Ok(value.into_bytes()),
        None => store.get(session, key),
    };
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
/// A key is present when the store has it or `events` (the session's log)
/// says a transition assigned it, the answer [`handle_get`] gives too; a
/// value a lost store write left behind is not stored.
///
/// A usable key's answer is logged as a `context_read` (`reader: "cli"`,
/// `access: "presence"`) when `events` holds the session's log, where
/// [`read_log_target`] decides.
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
    let assigned = assigned_value(events, key);
    let present = assigned.is_some() || store.ctx_exists(session, key);
    let record = match &assigned {
        Some(value) => {
            ContextReadRecord::content(key, Some(value.as_bytes())).map(|r| ContextReadRecord {
                presence_only: true,
                ..r
            })
        }
        None => ContextReadRecord::presence(store, session, key, present),
    };
    log_cli_read(store, backend, session, events, reader_anchor, record);
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

/// List all keys as a JSON array, optionally filtered by prefix, as
/// [`list_keys`] finds them.
pub fn handle_list(
    store: &dyn ContextStore,
    events: Option<&[Event]>,
    session: &str,
    prefix: Option<&str>,
) -> Result<()> {
    let keys = list_keys(store, events, session, prefix)?;
    println!("{}", serde_json::to_string(&keys)?);
    Ok(())
}

/// The keys `koto context list` prints, sorted: the store's and, when
/// `events` holds the session's log, the keys a transition assigned that the
/// store should hold, so a key a lost store write left out is listed as
/// [`handle_get`] answers it. Nothing is stored, and the listing logs no
/// read.
fn list_keys(
    store: &dyn ContextStore,
    events: Option<&[Event]>,
    session: &str,
    prefix: Option<&str>,
) -> Result<Vec<String>> {
    let mut keys = store.list_keys(session, prefix)?;
    if let Some(events) = events {
        let assigned = crate::engine::context_assign::outstanding_assignments(events);
        keys.extend(
            assigned
                .into_keys()
                .filter(|k| prefix.is_none_or(|p| k.starts_with(p))),
        );
        keys.sort();
        keys.dedup();
    }
    Ok(keys)
}

/// The error envelope `command` (`context get` or `context list`) prints
/// when its read fails. The error's text goes through
/// [`without_url_userinfo`], as the cloud backend's own errors do, so a URL
/// it names never prints its credentials (koto#469); text naming no URL is
/// printed as it is.
///
/// [`without_url_userinfo`]: crate::session::cloud::without_url_userinfo
pub(super) fn failed_read_envelope(command: &str, e: &anyhow::Error) -> serde_json::Value {
    serde_json::json!({
        "error": crate::session::cloud::without_url_userinfo(&e.to_string()),
        "command": command,
    })
}

/// Print [`failed_read_envelope`] and exit 3. Both the reads in this
/// workspace and the `--workspace` reads end here when `context get` or
/// `context list` fails.
pub(super) fn exit_with_failed_read(command: &str, e: &anyhow::Error) -> ! {
    super::exit_with_error_code(failed_read_envelope(command, e), super::EXIT_INFRASTRUCTURE);
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

    /// The payloads of the reads spooled beside `name`, through the spool's
    /// one reader.
    fn spooled(backend: &LocalBackend, name: &str) -> Vec<serde_json::Value> {
        crate::session::read_spool::read(&backend.session_dir(name), "")
            .iter()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["payload"].clone())
            .collect()
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

        let events = read_session_log(&backend, "s", "k");
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
        // With no reader's anchor both reads go to the spool, none to the log.
        assert_eq!(event_types(&backend), vec!["transitioned"]);
        let reads: Vec<_> = spooled(&backend, "s")
            .into_iter()
            .map(|r| {
                (
                    r["reader"].as_str().unwrap().to_string(),
                    r["state"].as_str().unwrap().to_string(),
                    r["access"].as_str().map(str::to_string),
                    r["hash"].as_str().map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            reads,
            vec![
                (
                    "cli".to_string(),
                    "work".to_string(),
                    Some("content".to_string()),
                    Some(sha256_hex(b"v"))
                ),
                (
                    "cli".to_string(),
                    "work".to_string(),
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
        let events = read_session_log(&backend, "s", "k");
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
        assert!(read_session_log(&backend, "nope", "k").is_none());
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
        let events = read_session_log(backend, "s", "k");
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
    /// session itself, is the session's own agent session, but its read runs
    /// outside the session's tick: it takes the anchorless row, so the
    /// session and the anchor stay byte-identical and the read is spooled.
    #[test]
    fn an_own_anchors_read_outside_the_tick_takes_the_anchorless_row() {
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
        let session_before = files(&dir.path().join("s"));

        assert_eq!(
            read_log_target(&backend, &backend, "s", Some("anchor-own")),
            ReadLogTarget::Anchorless
        );
        read_both(&backend, dir.path(), Some("anchor-own"));
        assert_eq!(files(&dir.path().join("anchor-own")), anchor_before);
        assert_eq!(unspooled_files(dir.path()), session_before);
        assert!(reads_in(&backend, "s").is_empty());
        assert_eq!(spooled(&backend, "s").len(), 2);

        // An anchor reading itself outside its tick is anchorless too.
        assert_eq!(
            read_log_target(&backend, &backend, "anchor-own", Some("anchor-own")),
            ReadLogTarget::Anchorless
        );
    }

    /// Every file of session `s` under `dir` but its read spool.
    fn unspooled_files(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let spool = crate::session::read_spool::SPOOL_FILE;
        files(&dir.join("s"))
            .into_iter()
            .filter(|(p, _)| !p.ends_with(spool))
            .collect()
    }

    /// `s` in state `work`, entered by a transition that assigned `k` the
    /// value `assigned` whose store write did not land: the store holds
    /// `stale` under `k`, or nothing when `stale` is `None`.
    fn session_with_a_lost_write(dir: &std::path::Path, stale: Option<&[u8]>) -> LocalBackend {
        let backend = LocalBackend::with_base_dir(dir.to_path_buf());
        std::fs::create_dir_all(dir.join("s")).unwrap();
        std::fs::write(
            dir.join("s").join(state_file_name("s")),
            concat!(
                r#"{"schema_version":1,"workflow":"s","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":null,"to":"start","condition_type":"auto"}}"#,
                "\n",
                r#"{"seq":2,"timestamp":"2026-01-01T00:00:01Z","type":"transitioned","payload":{"from":"start","to":"work","condition_type":"auto","context_assignments":{"k":"assigned"}}}"#,
                "\n",
            ),
        )
        .unwrap();
        if let Some(stale) = stale {
            backend
                .add_with_writer("s", "k", stale, WRITER_AGENT)
                .unwrap();
        }
        backend
    }

    /// After a lost store write (koto#204), `get`, `exists` and `list`
    /// answer the value the transition assigned, served from the log, and
    /// store nothing: the session's files are byte-identical but for the
    /// spooled reads, and the store still holds what it held.
    #[test]
    fn a_lost_assigned_value_is_answered_from_the_log_and_never_stored() {
        for stale in [None, Some(&b"stale"[..])] {
            let dir = tempfile::TempDir::new().unwrap();
            let backend = session_with_a_lost_write(dir.path(), stale);
            let before = unspooled_files(dir.path());

            let events = read_session_log(&backend, "s", "k");
            let out = dir.path().join("out.txt");
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
            assert_eq!(std::fs::read(&out).unwrap(), b"assigned", "{stale:?}");
            assert!(matches!(
                handle_exists(&backend, &backend, events.as_deref(), None, "s", "k"),
                KeyPresence::Present
            ));
            assert_eq!(
                list_keys(&backend, events.as_deref(), "s", None).unwrap(),
                vec!["k".to_string()]
            );
            assert!(list_keys(&backend, events.as_deref(), "s", Some("x"))
                .unwrap()
                .is_empty());

            assert_eq!(unspooled_files(dir.path()), before, "{stale:?}");
            assert_eq!(backend.get("s", "k").ok().as_deref(), stale);
            // Both reads are logged with the hash of the value answered.
            let hashes: Vec<_> = spooled(&backend, "s")
                .into_iter()
                .map(|r| r["hash"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(hashes, vec![sha256_hex(b"assigned"); 2]);
        }
    }

    /// With no reader anchor, or a name that is no anchor here (absent, or
    /// an ordinary session, the read session included), the read takes the
    /// anchorless row: the read goes to the spool beside the session and the
    /// session's own files stay byte-identical.
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
        let spool = crate::session::read_spool::SPOOL_FILE;
        let own_files = |d: &std::path::Path| -> Vec<(std::path::PathBuf, Vec<u8>)> {
            files(&d.join("s"))
                .into_iter()
                .filter(|(p, _)| !p.ends_with(spool))
                .collect()
        };
        let before = own_files(dir.path());
        read_both(&backend, dir.path(), None);
        read_both(&backend, dir.path(), Some("no-such-anchor"));
        assert_eq!(own_files(dir.path()), before);
        assert!(reads_in(&backend, "s").is_empty());
        let spooled = spooled(&backend, "s");
        assert_eq!(spooled.len(), 4);
        assert!(spooled
            .iter()
            .all(|r| r["key"] == "k" && r.get("session").is_none()));
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

    /// A failed read's envelope leaves out the userinfo of a URL its error
    /// names, and prints an error naming no URL as it is.
    #[test]
    fn a_failed_reads_envelope_prints_no_url_credentials() {
        let e = anyhow::anyhow!("endpoint http://user:secret@host/bucket refused the read");
        assert_eq!(
            failed_read_envelope("context get", &e),
            serde_json::json!({
                "error": "endpoint http://host/bucket refused the read",
                "command": "context get",
            })
        );
        let e = anyhow::anyhow!("failed to read context key 'k' for session 's': not found");
        assert_eq!(
            failed_read_envelope("context list", &e),
            serde_json::json!({
                "error": "failed to read context key 'k' for session 's': not found",
                "command": "context list",
            })
        );
    }
}
