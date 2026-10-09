//! The run journal: a local record of session identity and progress for
//! external tooling.
//!
//! koto appends one JSON object per line to `<koto home>/_run_journal.jsonl`
//! (the same `~/.koto` the decider ledger uses) as sessions are created,
//! enter states, reach a terminal and are cancelled. The journal is
//! append-only: koto never rewrites, reads back or deletes it, so its lines
//! outlive the sessions they describe, including sessions koto removes at
//! their terminal tick or on `koto session cleanup`.
//!
//! The user's session store and the store the koto CLI opens are journaled.
//! A store a test or an embedder builds elsewhere with
//! `LocalBackend::with_base_dir` is not, unless it opts in with
//! `LocalBackend::with_run_journal`.
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
//!   `imported_from` for `koto session import`.
//! - `state_entered`: `koto.state`.
//! - `terminal`: `koto.terminal`.
//! - `cancelled`: nothing more.
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
//!   `workflow_cancelled`.
//! - [`terminal`], from the terminal tick, writes `terminal` once per
//!   arrival.
//! - [`imported`], from `koto session import`, writes the imported
//!   session's `session_started`.
//!
//! ## Failure
//!
//! Writing is best-effort. When a record can't be written (no home, a
//! read-only or full disk, a symlink at the journal's path, a record over
//! [`MAX_LINE_BYTES`]) the process prints one warning line naming the run
//! journal on stderr, the first time only, and carries on. Nothing about
//! the command's output, exit code, session state or gate decisions
//! changes.

mod driver;
mod fixture;
mod run_id;
mod sidecar;

use std::path::{Path, PathBuf};

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

const MAX_VALUE_CHARS: usize = 128;

/// Whether `value` has the id shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : -`.
pub(crate) fn is_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_CHARS
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Whether `value` has the name shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : / @ -`, not starting with `/` or `~`, so an absolute
/// path can't pass as a template or state name.
pub(crate) fn is_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_CHARS
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

// ----- Where the journal goes -----

/// Where records go: nowhere (unit tests that didn't ask for a journal), or
/// the koto home, which is `None` when there is no home directory.
enum Target {
    #[cfg(test)]
    Off,
    Home(Option<PathBuf>),
}

#[cfg(not(test))]
fn target() -> Target {
    Target::Home(crate::cli::ledger_root())
}

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

// Unit tests never write the invoking user's journal: each test thread
// starts with the journal off, and a test that wants one points it at a
// temporary koto home with `set_home_for_test`. The cap and the warn-once
// flag are per thread too, so a test sees exactly the warnings its own
// commands caused.
#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<Option<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_MAX_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_LINE_BYTES) };
    static TEST_WARNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn target() -> Target {
    TEST_HOME.with(|h| match h.borrow().clone() {
        None => Target::Off,
        Some(home) => Target::Home(home),
    })
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

/// Point this thread's journal at `koto_home` (`Some(None)` simulates a
/// missing home directory), or turn it off with `None`. Resets the cap and
/// the warnings.
#[cfg(test)]
pub(crate) fn set_home_for_test(koto_home: Option<Option<&Path>>) {
    TEST_HOME.with(|h| *h.borrow_mut() = koto_home.map(|o| o.map(Path::to_path_buf)));
    TEST_MAX_LINE.with(|c| c.set(MAX_LINE_BYTES));
    TEST_WARNED.with(|w| w.set(false));
    TEST_WARNINGS.with(|w| w.borrow_mut().clear());
}

/// Lower (or restore) this thread's line cap.
#[cfg(test)]
pub(crate) fn set_max_line_for_test(max: usize) {
    TEST_MAX_LINE.with(|c| c.set(max));
}

/// The warning lines this thread has printed since `set_home_for_test`.
#[cfg(test)]
pub(crate) fn warnings_for_test() -> Vec<String> {
    TEST_WARNINGS.with(|w| w.borrow().clone())
}

/// Whether `backend`'s sessions are journaled: the store opted in (see
/// `LocalBackend::with_run_journal`) and, in unit tests, the thread asked
/// for a journal.
fn enabled(backend: &dyn SessionBackend) -> bool {
    if !backend.run_journal_enabled() {
        return false;
    }
    match target() {
        #[cfg(test)]
        Target::Off => false,
        Target::Home(_) => true,
    }
}

/// Append `records` in order. A record that can't be written is skipped
/// and the rest are still tried.
fn write(records: &[Record]) {
    let home = match target() {
        #[cfg(test)]
        Target::Off => return,
        Target::Home(None) => {
            warn_once("no home directory");
            return;
        }
        Target::Home(Some(home)) => home,
    };
    let path = home.join(JOURNAL_FILE);
    let max = max_line_bytes();
    for record in records {
        let line = record.to_line();
        if let Err(e) =
            crate::engine::jsonl_append::append_bounded_line_no_follow(&home, &path, &line, max)
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
        .id("imported_from", imported_from)
}

/// The session's run id: the sidecar's cached value, or derived from the
/// header (and the parent chain for an older child) and cached now.
fn cached_run_id(
    backend: &dyn SessionBackend,
    session_dir: &Path,
    header: &StateFileHeader,
) -> Option<String> {
    if let Some(cached) = sidecar::read(session_dir) {
        return cached.run_id;
    }
    let run_id = run_id::run_id(backend, header);
    let _ = sidecar::write(
        session_dir,
        &sidecar::Sidecar {
            run_id: run_id.clone(),
            driver: None,
        },
    );
    run_id
}

/// Journal a session that `init_state_file` has just committed:
/// `session_started`, then a `state_entered` for each state its initial
/// events enter. Writes the session's sidecar with its run id and creation
/// driver.
pub(crate) fn after_init(
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    initial_events: &[Event],
) {
    if !enabled(backend) {
        return;
    }
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
    write(&records);
}

/// Journal a payload `append_event` has just committed to `session`'s log.
/// Payloads other than state entries and cancels write nothing and read
/// nothing.
pub(crate) fn after_commit(backend: &dyn SessionBackend, session: &str, payload: &EventPayload) {
    if !journals(payload) || !enabled(backend) {
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
    let run_id = cached_run_id(backend, &backend.session_dir(session), &header);
    if let Some(record) = record_for(payload, session, &header, run_id.as_deref()) {
        write(&[record]);
    }
}

/// Journal an arrival at the terminal state `final_state`. The terminal
/// tick calls this once per arrival, after the state's `state_entered`
/// and before any cleanup.
pub(crate) fn terminal(
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    final_state: &str,
) {
    if !enabled(backend) {
        return;
    }
    let run_id = cached_run_id(backend, &backend.session_dir(session), header);
    write(&[
        Record::new("terminal", session, &header.session_id, run_id.as_deref())
            .name("koto.terminal", Some(final_state)),
    ]);
}

/// Journal a session `koto session import` has just built. The imported
/// session is a new run: its own id is its run id, and the source's id is
/// recorded as `imported_from`. None of the source's records are copied.
pub(crate) fn imported(
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
    imported_from: &str,
) {
    if !enabled(backend) {
        return;
    }
    let run_id = Some(header.session_id.clone()).filter(|v| is_id(v));
    let driver = driver();
    let record = session_started(
        session,
        header,
        run_id.as_deref(),
        driver.as_deref(),
        Some(imported_from),
    );
    let _ = sidecar::write(
        &backend.session_dir(session),
        &sidecar::Sidecar { run_id, driver },
    );
    write(&[record]);
}

#[cfg(test)]
mod tests;
