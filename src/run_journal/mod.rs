//! The run journal: a local record of session identity and progress for
//! external tooling.
//!
//! koto appends one JSON object per line to `<koto home>/_run_journal.jsonl`
//! (the same `~/.koto` the decider ledger uses) as sessions are created,
//! enter states, reach a terminal, are cancelled and change driver. The
//! journal is append-only: koto never rewrites, deletes or reads back its
//! records, so its lines outlive the sessions they describe, including
//! sessions koto removes at their terminal tick or on
//! `koto session cleanup`. `docs/reference/run-journal.md` documents the
//! format for readers.
//!
//! ## Where the journal lives
//!
//! The run journal lives with the session store it describes. The user's
//! store (`LocalBackend::new`, `~/.koto/sessions`, which the cloud store
//! also keeps its local copies in) journals into `~/.koto`. A store built on
//! an explicit base directory (`LocalBackend::with_base_dir`, which is what
//! `KOTO_SESSIONS_BASE` builds) journals inside that base, so every store
//! is journaled and a redirected store never writes the real home's
//! journal. Tests that want the journal somewhere else use
//! `LocalBackend::with_base_dir_and_journal`.
//!
//! The switch belongs to the store's constructor, not to the
//! `SessionBackend` trait: each writer below takes the journal's root as a
//! parameter. The commit hooks and the import hook take it from the
//! `LocalBackend` they run in; the terminal tick takes it from the backend
//! the command built (`Backend::journal_root`), which for the cloud backend
//! is the local store it keeps its copies in.
//!
//! ## Records
//!
//! Every line carries `kind`, `v` (`1`), `at` (UTC, RFC 3339, milliseconds,
//! `Z`), `session` (the session name), `koto.session.id` and, when known,
//! `koto.run.id`. The kinds and their own fields:
//!
//! - `session_started`: `koto.parent.session.id` (children),
//!   `koto.driver.session.id` (the Claude Code session that created it),
//!   `koto.template.name`, `koto.template.hash`, `koto.fixture`, and
//!   `koto.imported_from.session.id` for `koto session import`.
//! - `state_entered`: `koto.state`.
//! - `terminal`: `koto.terminal`.
//! - `cancelled`: nothing more.
//! - `driver_seen`: `koto.driver.session.id`, the Claude Code session now
//!   driving the session, when it differs from the last one recorded.
//!
//! A field with no value is left out, never written as null or empty. Ids
//! must match `^[A-Za-z0-9._:-]{1,128}$` and names (template and state
//! names) `^[A-Za-z0-9._:/@-]{1,128}$` without a leading `/`; a value
//! outside its shape is left out and the record is still written.
//!
//! ## When records are written
//!
//! Always after the session's own log has committed the change they
//! describe, never before, so a failed commit writes nothing:
//!
//! - [`after_init`], from `LocalBackend::init_state_file`, writes
//!   `session_started` and a `state_entered` for the initial state.
//! - [`after_commit`], from `LocalBackend::append_event` next to the
//!   `/workflows` hook, writes `state_entered` for each `transitioned`,
//!   `directed_transition` and `rewound` event and `cancelled` for
//!   `workflow_cancelled`. Before those, on any committed payload, it
//!   writes `driver_seen` when the process's driver (`CLAUDE_CODE_SESSION_ID`,
//!   when id-shaped) differs from the one the session's sidecar caches, or
//!   the sidecar caches none, and then caches the new driver. No driver is
//!   never a change. Two commands racing under one new driver can each
//!   write a `driver_seen`; readers take the distinct values.
//! - [`terminal`], from the terminal tick, writes `terminal` once per
//!   arrival.
//! - [`imported`], from `koto session import`, writes the imported
//!   session's `session_started` and a `state_entered` for the state it is
//!   in, once per imported session.
//!
//! ## Failure
//!
//! Writing is best-effort. When a record can't be written (a koto home that
//! can't be created, a read-only or full disk, a symlink at the journal's
//! path, a record over [`MAX_LINE_BYTES`]) the process prints one warning
//! line naming the run journal on stderr, the first time only, and carries
//! on. Nothing about
//! the command's output, exit code, session state or gate decisions
//! changes.
//!
//! Writers take an advisory lock on the journal for each record, so a
//! record is never interleaved with another. A write cut short (a crash, a
//! full disk) can still leave a partial last line: the next writer sees,
//! under the lock, that the file doesn't end in a newline and starts its
//! record on a line of its own, so only the partial line is lost. Readers
//! skip lines that don't parse as JSON.
//!
//! ## Files
//!
//! The journal is the one file koto writes for external readers. Each
//! session also gets a sidecar, `run-journal.json` in its session
//! directory, which koto does read back: it caches the session's run id
//! and the driver last recorded for it (see `sidecar`). Neither is session
//! state. Cloud sync and `koto session import` carry neither: they move the
//! state log, version, template and context files only.

mod driver;
mod fixture;
mod run_id;
mod sidecar;

use std::path::Path;

use serde_json::Value;

use crate::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use crate::session::SessionBackend;

pub(crate) use driver::driver;
pub(crate) use run_id::child_lineage;

/// The journal's file name inside the koto home.
pub(crate) const JOURNAL_FILE: &str = "_run_journal.jsonl";

/// The most bytes one journal line may take, newline included. Within
/// `PIPE_BUF`, so concurrent appends never interleave.
pub(crate) const MAX_LINE_BYTES: usize = 4096;

/// The line format's version, the `v` every record carries.
const FORMAT_VERSION: u64 = 1;

/// The longest id or name, in bytes. Both shapes are ASCII-only, so this
/// is also the character limit.
const MAX_VALUE_BYTES: usize = 128;

/// Whether `value` has the id shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : -`.
pub(crate) fn is_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Whether `value` has the name shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : / @ -`, not starting with `/` or `~`, so an absolute
/// path can't pass as a template or state name.
pub(crate) fn is_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && !value.starts_with('/')
        && !value.starts_with('~')
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'-')
        })
}

/// One journal record, its fields in the order they are written.
#[derive(Debug, Clone)]
struct Record {
    fields: Vec<(&'static str, Value)>,
}

impl Record {
    fn new(kind: &str, session: &str, session_id: &str, run_id: Option<&str>) -> Self {
        let mut r = Record {
            fields: vec![
                ("kind", Value::from(kind)),
                ("v", Value::from(FORMAT_VERSION)),
                ("at", Value::from(now_iso8601())),
            ],
        };
        if !session.is_empty() {
            r.fields.push(("session", Value::from(session)));
        }
        r.id("koto.session.id", Some(session_id))
            .id("koto.run.id", run_id)
    }

    /// Add `key` when `value` is id-shaped.
    fn id(mut self, key: &'static str, value: Option<&str>) -> Self {
        if let Some(v) = value.filter(|v| is_id(v)) {
            self.fields.push((key, Value::from(v)));
        }
        self
    }

    /// Add `key` when `value` is name-shaped.
    fn name(mut self, key: &'static str, value: Option<&str>) -> Self {
        if let Some(v) = value.filter(|v| is_name(v)) {
            self.fields.push((key, Value::from(v)));
        }
        self
    }

    fn flag(mut self, key: &'static str, value: bool) -> Self {
        self.fields.push((key, Value::from(value)));
        self
    }

    fn to_line(&self) -> String {
        let mut line = String::from("{");
        for (i, (key, value)) in self.fields.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            // Keys are fixed ASCII and values are serde_json values, so
            // neither can fail to serialize or carry a raw newline.
            line.push_str(&Value::from(*key).to_string());
            line.push(':');
            line.push_str(&value.to_string());
        }
        line.push('}');
        line
    }
}

// ----- Writing -----

#[cfg(not(test))]
fn max_line_bytes() -> usize {
    MAX_LINE_BYTES
}

/// Warn about a failed write once per process.
#[cfg(not(test))]
fn warn_once(message: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("warning: run journal write failed ({})", message);
    }
}

// In unit tests the cap and the warn-once flag are per thread, so a test
// sees exactly the warnings its own commands caused. Unit tests reach a
// journal only through a store built with
// `LocalBackend::with_base_dir_and_journal` on a temporary directory.
#[cfg(test)]
thread_local! {
    static TEST_MAX_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_LINE_BYTES) };
    static TEST_WARNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn max_line_bytes() -> usize {
    TEST_MAX_LINE.with(|c| c.get())
}

#[cfg(test)]
fn warn_once(message: &str) {
    if !TEST_WARNED.with(|w| w.replace(true)) {
        TEST_WARNINGS.with(|w| {
            w.borrow_mut()
                .push(format!("warning: run journal write failed ({})", message))
        });
    }
}

/// Reset this thread's line cap and warnings.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    TEST_MAX_LINE.with(|c| c.set(MAX_LINE_BYTES));
    TEST_WARNED.with(|w| w.set(false));
    TEST_WARNINGS.with(|w| w.borrow_mut().clear());
}

/// Lower (or restore) this thread's line cap.
#[cfg(test)]
pub(crate) fn set_max_line_for_test(max: usize) {
    TEST_MAX_LINE.with(|c| c.set(max));
}

/// The warning lines this thread has printed since `reset_for_test`.
#[cfg(test)]
pub(crate) fn warnings_for_test() -> Vec<String> {
    TEST_WARNINGS.with(|w| w.borrow().clone())
}

/// Append `records` in order to the journal under `root`. A record that
/// can't be written is skipped and the rest are still tried.
fn write(root: &Path, records: &[Record]) {
    let path = root.join(JOURNAL_FILE);
    let max = max_line_bytes();
    for record in records {
        let line = record.to_line();
        if let Err(e) =
            crate::engine::jsonl_append::append_bounded_line_no_follow(root, &path, &line, max)
        {
            warn_once(&format!("{}: {}", path.display(), e.root_cause()));
        }
    }
}

// ----- The hooks -----

/// The `state_entered` or `cancelled` record a committed payload maps to,
/// or `None` for a payload that writes nothing.
fn record_for(
    payload: &EventPayload,
    session: &str,
    header: &StateFileHeader,
    run_id: Option<&str>,
) -> Option<Record> {
    let state_entered = |to: &str| {
        Record::new("state_entered", session, &header.session_id, run_id)
            .name("koto.state", Some(to))
    };
    match payload {
        EventPayload::Transitioned { to, .. }
        | EventPayload::DirectedTransition { to, .. }
        | EventPayload::Rewound { to, .. } => Some(state_entered(to)),
        EventPayload::WorkflowCancelled { .. } => Some(Record::new(
            "cancelled",
            session,
            &header.session_id,
            run_id,
        )),
        _ => None,
    }
}

/// Whether a payload writes a record, without reading anything.
fn journals(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. }
            | EventPayload::WorkflowCancelled { .. }
    )
}

fn session_started(
    session: &str,
    header: &StateFileHeader,
    run_id: Option<&str>,
    driver: Option<&str>,
    imported_from: Option<&str>,
) -> Record {
    let hash = Some(header.template_hash.as_str()).filter(|h| !h.is_empty());
    Record::new("session_started", session, &header.session_id, run_id)
        .id(
            "koto.parent.session.id",
            header.parent_session_id.as_deref(),
        )
        .id("koto.driver.session.id", driver)
        .name("koto.template.name", header.template_name.as_deref())
        .id("koto.template.hash", hash)
        .flag(
            "koto.fixture",
            fixture::is_fixture(header.template_source_dir.as_deref()),
        )
        // The source session's id: the same `from_session_id` the import's
        // `session_imported` event carries, under the same name.
        .id("koto.imported_from.session.id", imported_from)
}

/// The session's run id: the sidecar's cached value, or derived from the
/// header (and the parent chain for an older child) and cached now, next
/// to the driver the sidecar already holds. A sidecar with no run id (one
/// written for a driver before the run id resolved) is derived again, so a
/// parent header that was briefly unreadable is tried again on the next
/// record.
fn cached_run_id(
    backend: &dyn SessionBackend,
    session_dir: &Path,
    header: &StateFileHeader,
) -> Option<String> {
    let cached = sidecar::read(session_dir);
    if let Some(run_id) = cached.as_ref().and_then(|c| c.run_id.clone()) {
        return Some(run_id);
    }
    let run_id = run_id::run_id(backend, header);
    if run_id.is_some() {
        let _ = sidecar::write(
            session_dir,
            &sidecar::Sidecar {
                run_id: run_id.clone(),
                driver: cached.and_then(|c| c.driver),
            },
        );
    }
    run_id
}

/// The driver of this process when it differs from the one the session's
/// sidecar caches, or when the sidecar caches none: a change of driving
/// session that [`after_commit`] records. `None` when this process has no
/// driver, since no driver is never a change.
fn changed_driver(session_dir: &Path) -> Option<String> {
    let current = driver()?;
    let cached = sidecar::read(session_dir).and_then(|s| s.driver);
    (cached.as_deref() != Some(current.as_str())).then_some(current)
}

/// Journal a session that `init_state_file` has just committed:
/// `session_started`, then a `state_entered` for each state its initial
/// events enter. Writes the session's sidecar with its run id and creation
/// driver. `journal_root` is the store's journal home; `None` writes
/// nothing.
pub(crate) fn after_init(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    initial_events: &[Event],
) {
    let Some(root) = journal_root else {
        return;
    };
    let run_id = run_id::run_id(backend, header);
    let driver = driver();
    let mut records = vec![session_started(
        session,
        header,
        run_id.as_deref(),
        driver.as_deref(),
        None,
    )];
    records.extend(
        initial_events
            .iter()
            .filter_map(|e| record_for(&e.payload, session, header, run_id.as_deref())),
    );
    let _ = sidecar::write(
        &backend.session_dir(session),
        &sidecar::Sidecar { run_id, driver },
    );
    write(root, &records);
}

/// Journal a payload `append_event` has just committed to `session`'s log.
///
/// When this process runs under a driver other than the one the session's
/// sidecar caches (or the sidecar caches none), a `driver_seen` record comes
/// first, whatever the payload, and the sidecar is then rewritten with the
/// new driver. Two commands racing under the same new driver can each
/// write one; readers take the distinct values, so the duplicate is
/// harmless. Without a driver change, payloads other than state entries
/// and cancels write nothing and read nothing beyond the sidecar; a driver
/// change or a journaled payload also reads the session's header.
/// `journal_root` is the store's journal home; `None` writes nothing.
pub(crate) fn after_commit(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    payload: &EventPayload,
) {
    let Some(root) = journal_root else {
        return;
    };
    let session_dir = backend.session_dir(session);
    let new_driver = changed_driver(&session_dir);
    if new_driver.is_none() && !journals(payload) {
        return;
    }
    let header = match backend.read_header(session) {
        Ok(h) => h,
        Err(e) => {
            warn_once(&format!(
                "could not read the header of session {}: {}",
                session,
                e.root_cause()
            ));
            return;
        }
    };
    let run_id = cached_run_id(backend, &session_dir, &header);
    let mut records = Vec::new();
    if let Some(d) = new_driver.as_deref() {
        records.push(
            Record::new(
                "driver_seen",
                session,
                &header.session_id,
                run_id.as_deref(),
            )
            .id("koto.driver.session.id", Some(d)),
        );
    }
    records.extend(record_for(payload, session, &header, run_id.as_deref()));
    write(root, &records);
    // The cache moves only after the journal write, so a crash between the
    // two repeats the `driver_seen` on the next command rather than losing
    // it. A write that failed (and warned) is not retried: the cache moves
    // either way, as every journal write is best-effort.
    if new_driver.is_some() {
        let _ = sidecar::write(
            &session_dir,
            &sidecar::Sidecar {
                run_id,
                driver: new_driver,
            },
        );
    }
}

/// Journal an arrival at the terminal state `final_state`. The terminal
/// tick calls this once per arrival, after the state's `state_entered`
/// and before any cleanup, with the root of the command's backend
/// (`Backend::journal_root`); `None` writes nothing.
pub(crate) fn terminal(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    final_state: &str,
) {
    let Some(root) = journal_root else {
        return;
    };
    let run_id = cached_run_id(backend, &backend.session_dir(session), header);
    write(
        root,
        &[
            Record::new("terminal", session, &header.session_id, run_id.as_deref())
                .name("koto.terminal", Some(final_state)),
        ],
    );
}

/// Journal a session `koto session import` has just built: its
/// `session_started`, then one `state_entered` for `current_state`, the
/// state its carried log leaves it in, timed at the import as
/// [`after_init`] times a new session's initial state. The imported session
/// is a new run: its own id is its run id, and `imported_from` (the
/// `from_session_id` of the import's `session_imported` event) is recorded
/// as `koto.imported_from.session.id`. None of the source's records are
/// copied.
///
/// The import calls this once, after the session is in place and before
/// the source is marked, on the branch that built it; a retry that adopts
/// an already-placed session, or an import that rolls back, writes nothing.
pub(crate) fn imported(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    current_state: Option<&str>,
    imported_from: &str,
) {
    let Some(root) = journal_root else {
        return;
    };
    let run_id = Some(header.session_id.clone()).filter(|v| is_id(v));
    let driver = driver();
    let mut records = vec![session_started(
        session,
        header,
        run_id.as_deref(),
        driver.as_deref(),
        Some(imported_from),
    )];
    if let Some(state) = current_state {
        records.push(
            Record::new(
                "state_entered",
                session,
                &header.session_id,
                run_id.as_deref(),
            )
            .name("koto.state", Some(state)),
        );
    }
    let _ = sidecar::write(
        &backend.session_dir(session),
        &sidecar::Sidecar { run_id, driver },
    );
    write(root, &records);
}

#[cfg(test)]
mod tests;
