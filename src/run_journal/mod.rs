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
//! `Z`) and `session` (the session name), then `koto.session.id` and
//! `koto.run.id` when known (a session from before session ids has
//! neither). The kinds and their own fields:
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
//! Those five are the identity kinds. Four more are relayed from check
//! events, and each carries `koto.seq` (the relayed event's sequence
//! number in the session's log) and `koto.visit.seq` (the sequence number
//! of the event that opened the visit it belongs to, by
//! `persistence::visit_start`'s rule):
//!
//! - `check_evaluated`: one per `gate_evaluated` that isn't pending and per
//!   `default_action_executed` (check `__action__`), with the outcome,
//!   attempt numbers, rule count, logged finding count (and whether the
//!   findings were cut) and run time; a decider check's also carries its
//!   awaiting and agent-answered criterion counts.
//! - `check_rule`: one per key of that event's `rule_counts`, after its
//!   `check_evaluated`, with the rule's visit and session counts. A rule id
//!   outside the name shape is written as its digest ([`name_or_digest`]).
//! - `check_finding`: one per distinct (rule id, level) among that event's
//!   logged findings, passing or failing, after its `check_rule` records,
//!   with the gate as `koto.check.name` and how many findings had the pair
//!   (see [`with_findings`]).
//! - `check_pending`: one per pending `gate_evaluated` of a polling gate or
//!   of a decider check waiting on the agent, never an outcome. A polling
//!   gate's carries the poll's run count and elapsed seconds; a decider
//!   check's carries the two criterion counts instead.
//!
//! Eight more are relayed the same way, with the same two sequence
//! numbers, from decider, evidence, override and dispute events:
//!
//! - `decider_consulted`: one per `decider_consulted` event, with the
//!   provider, model, outcome, token counts and timings, the field count,
//!   an `input_unavailable` consultation's reason and its input's label
//!   and size, and, for a consultation of one field, that field's name,
//!   the decider's answer, its mode and whether it met its threshold.
//! - `decider_field`: for a consultation of several fields, one per field
//!   after its `decider_consulted`, with those answer keys and the
//!   consultation's `koto.seq` as `koto.decider.consulted_seq`.
//! - `decider_answered`: one per consulted field an agent's
//!   `evidence_submitted` holds, on a visit whose consultation wasn't
//!   applied, with the field, the agent's answer (a declared value, or
//!   `other`) and `koto.decider.consulted_seq` (see `paired`).
//! - `decider_checked`: one per `decider_checked` event (one criterion of a
//!   decider check), with the gate as `koto.check`, the criterion's id as
//!   `koto.rule.id` (digested like a rule id), the mode, outcome, reason,
//!   whether it blocked, tokens and timings. A verdict koto reuses appends
//!   no event, so it writes no record.
//! - `decider_fallback`: one per `decider_fallback` event (a criterion
//!   handed to the agent), with the gate as `koto.check`, the criterion's id
//!   as `koto.rule.id`, the mode, the reason and the slice's size.
//! - `decider_check_answered`: one per `decider_check_answered` event (the
//!   agent's answer to a fallback), with the gate, the criterion's id, the
//!   answer, and the `seq` and reason of the fallback it answers.
//! - `check_overridden`: one per `gate_override_recorded`, with the state
//!   and gate only.
//! - `check_disputed`: one per `finding_disputed`, with the state, the gate
//!   as `koto.check.name`, the disputed evaluation's attempt numbers, the
//!   rule id (digested like a rule id) and who disputed it, never the
//!   reason.
//!
//! None of them copies a probability, hash, threshold, endpoint, rule
//! reference, rationale, dispute reason, override value or output, and `koto.visit.seq` is
//! always koto's visit, never the `visit_seq` a decider event carries. An
//! answer, a field name and an input label are names the template declares
//! (an agent's value outside them is written `other`) or koto's own words;
//! nothing the session wrote is copied.
//!
//! One more sums a visit's `context_read` events when the visit ends (see
//! `context_reads`). It carries `koto.seq` (the sequence number of the
//! event that ended the visit) and `koto.visit.seq` (the visit that ended):
//!
//! - `context_reads`: one per distinct (key, reader, access) read in the
//!   visit, in first-read order, with read and present counts and the first
//!   and last present hashes; past 50 tuples, one overflow record sums the
//!   rest. A key outside the name shape is written as its digest.
//!
//! And one relays no event: `response`, written after `koto next` or
//! `koto status` prints a non-terminal state's directive, with the byte
//! lengths of the printed `directive` and `details` (never their text).
//! Its `koto.seq` is the highest sequence number in the session's log when
//! it is written, and its `koto.visit.seq` the visit the log is in.
//!
//! A field with no value is left out, never written as null or empty.
//! Ids must match `^[A-Za-z0-9._:-]{1,128}$`, and names
//! `^[A-Za-z0-9._:/@-]{1,128}$` without a leading `/` or `~`. Names cover
//! template and state names, `koto.check`, the decider's provider and
//! model, and the short vocabulary values the other kinds carry
//! (`koto.check.outcome`, `koto.poll.status`, `koto.check.error`, the
//! `koto.decider.` outcome, mode, reason and error class, and
//! `koto.response.source`). A value outside its shape is left out and the
//! record is still written, with one exception: a rule id outside the name
//! shape, a decider criterion's id included, is written as its digest
//! ([`name_or_digest`]), never dropped, and so is a `context_reads` key,
//! reader or access.
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
//!   `directed_transition` and `rewound` event, `cancelled` for
//!   `workflow_cancelled`, the check kinds for `gate_evaluated` and
//!   `default_action_executed`, and the decider and override kinds for
//!   `decider_consulted`, `decider_checked`, `decider_fallback`,
//!   `decider_check_answered`, `gate_override_recorded` and
//!   `finding_disputed`, and `decider_answered` for an agent's
//!   `evidence_submitted` that answers a consultation.
//!   A commit that ends a visit (an arrival, a rewind, a cancel, or the
//!   terminal tick's `request_store.result`) writes the visit's
//!   `context_reads` just before its own record. Before those, on any
//!   committed payload, it writes `driver_seen` when the process's driver
//!   (`CLAUDE_CODE_SESSION_ID`, when id-shaped) differs from the one the
//!   session's sidecar caches, or the sidecar caches none, and then caches
//!   the new driver. No driver is never a change. Two commands racing under
//!   one new driver can each write a `driver_seen`; readers take the
//!   distinct values.
//! - [`terminal`], from the terminal tick, writes `terminal` once per
//!   arrival.
//! - [`response`], from `koto next`'s two print sites and from
//!   `koto status`, writes `response` after a directive is printed. It
//!   describes output rather than a commit, so it follows the print.
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
//! on. Nothing about the command's output, exit code, session state or
//! gate decisions changes.
//!
//! Writers take an advisory lock on the journal for each commit's records,
//! written as one batch, so a record is never interleaved with another.
//! Every writer waits for that lock at most [`JOURNAL_LOCK_WAIT`] (five
//! seconds; a `koto status` response, 30 milliseconds), then skips its
//! batch with the same one warning, so a process that stopped while holding
//! the lock can't hang a command. Once one write has given up, the
//! process's later writes try the lock once without waiting until one takes
//! it again, so a held lock costs a command one wait however many batches
//! it writes. The wait covers taking the lock only: nothing done while
//! holding it is cut short, and a writer that rotated the journal gets a
//! full wait of its own for the fresh file's lock.
//!
//! A write cut short (a crash, a full disk) can still leave a partial last
//! line: the next writer sees, under the lock, that the file doesn't end in
//! a newline and starts its record on a line of its own, so only the
//! partial line is lost. Readers skip lines that don't parse as JSON.
//!
//! A batch holding an identity record is flushed to disk; a batch of
//! relayed or response records alone isn't, and is made durable by the next
//! flush (see [`write`]).
//!
//! ## Files
//!
//! The journal is the one file koto writes for external readers. Each
//! session also gets a sidecar, `run-journal.json` in its session
//! directory, which koto does read back: it caches the session's run id
//! and the driver last recorded for it (see `sidecar`). Neither is session
//! state. Cloud sync and `koto session import` carry neither: they move the
//! state log, version, template and context files only.

mod context_reads;
mod driver;
mod fixture;
mod pairing;
#[cfg(unix)]
pub(crate) mod rotate;
mod run_id;
mod sidecar;

use std::path::Path;

use serde_json::Value;

use crate::engine::persistence::CommitInfo;
use crate::engine::types::{now_iso8601, CheckEventFields, Event, EventPayload, StateFileHeader};
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

/// `value` when it has the name shape, otherwise `sha256:` and the first 32
/// lowercase hex characters of the SHA-256 of its UTF-8 bytes. Used for
/// rule ids and decider criterion ids, which koto doesn't restrict to the
/// name shape: the same id gives the same value
/// in every session, and the result always has the name shape, so an odd
/// id keeps its identity without its text reaching the journal.
pub(crate) fn name_or_digest(value: &str) -> String {
    if is_name(value) {
        return value.to_string();
    }
    use sha2::{Digest, Sha256};
    let hex = hex::encode(Sha256::digest(value.as_bytes()));
    format!("sha256:{}", &hex[..32])
}

/// The record kinds that describe a session's identity and progress. A
/// batch holding one is flushed to disk; the kinds relayed from check,
/// decider and override events, and `response`, never are on their own
/// (see [`write`]).
const IDENTITY_KINDS: &[&str] = &[
    "session_started",
    "state_entered",
    "terminal",
    "cancelled",
    "driver_seen",
];

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

    /// Add `key` when `value` is present.
    fn opt_flag(self, key: &'static str, value: Option<bool>) -> Self {
        match value {
            Some(v) => self.flag(key, v),
            None => self,
        }
    }

    /// Add `key` when `value` is present.
    fn int(mut self, key: &'static str, value: Option<u64>) -> Self {
        if let Some(v) = value {
            self.fields.push((key, Value::from(v)));
        }
        self
    }

    /// Whether this is one of the five identity kinds, which are flushed to
    /// disk with the batch that carries them.
    fn is_identity(&self) -> bool {
        self.fields
            .first()
            .and_then(|(_, kind)| kind.as_str())
            .is_some_and(|kind| IDENTITY_KINDS.contains(&kind))
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

/// Set when a journal write in this process has given up waiting for the
/// lock, and cleared by the next write that takes it. A lock still held
/// after a full wait most likely belongs to a writer that stopped, so while
/// this is set every write tries the lock once, without waiting, and skips
/// its batch (with the warning already printed) if it is still held: a held
/// lock costs a command one wait, not one per batch. A write that takes the
/// lock clears it, so a long-running process (a `koto next` polling a gate)
/// waits normally again after a stall has passed. A rotating writer that
/// can't lock the fresh file after its own move doesn't set it: that wait
/// says nothing about a stopped writer (see [`write_within`]).
#[cfg(not(test))]
static GAVE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(not(test))]
fn gave_up() -> bool {
    GAVE_UP.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(test))]
fn set_gave_up(value: bool) {
    GAVE_UP.store(value, std::sync::atomic::Ordering::Relaxed);
}

// In unit tests the cap, the warn-once flag and the gave-up flag are per
// thread, so a test sees exactly the warnings and waits its own commands
// caused. Unit tests reach a journal only through a store built with
// `LocalBackend::with_base_dir_and_journal` on a temporary directory.
#[cfg(test)]
thread_local! {
    static TEST_MAX_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_LINE_BYTES) };
    static TEST_WARNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_GAVE_UP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// (batches written, batches flushed to disk) on this thread.
    static TEST_BATCHES: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static TEST_LOCK_WAIT: std::cell::Cell<std::time::Duration> =
        const { std::cell::Cell::new(JOURNAL_LOCK_WAIT) };
}

/// Shorten (or restore) this thread's [`journal_lock_wait`].
#[cfg(test)]
pub(crate) fn set_lock_wait_for_test(wait: std::time::Duration) {
    TEST_LOCK_WAIT.with(|c| c.set(wait));
}

/// Count one written batch, and whether the append reported a flush.
#[cfg(test)]
fn count_batch(flushed: bool) {
    TEST_BATCHES.with(|c| {
        let (written, synced) = c.get();
        c.set((written + 1, synced + usize::from(flushed)));
    });
}

#[cfg(not(test))]
fn count_batch(_flushed: bool) {}

/// The batches this thread has written since `reset_for_test`, and how many
/// of them were flushed to disk.
#[cfg(test)]
pub(crate) fn batches_for_test() -> (usize, usize) {
    TEST_BATCHES.with(|c| c.get())
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

#[cfg(test)]
fn gave_up() -> bool {
    TEST_GAVE_UP.with(|g| g.get())
}

#[cfg(test)]
fn set_gave_up(value: bool) {
    TEST_GAVE_UP.with(|g| g.set(value));
}

/// Reset this thread's line cap, warnings and lock-wait state.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    TEST_MAX_LINE.with(|c| c.set(MAX_LINE_BYTES));
    TEST_WARNED.with(|w| w.set(false));
    TEST_GAVE_UP.with(|g| g.set(false));
    TEST_WARNINGS.with(|w| w.borrow_mut().clear());
    TEST_BATCHES.with(|c| c.set((0, 0)));
    TEST_LOCK_WAIT.with(|c| c.set(JOURNAL_LOCK_WAIT));
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

/// Append `records` in order to the journal under `root`, as one batch: one
/// lock, one write per line. A record over the line cap is skipped with a
/// warning and the rest are still written.
///
/// The lock is waited for at most [`journal_lock_wait`] ([`JOURNAL_LOCK_WAIT`]
/// outside tests). A lock still held after that is a failed write like any
/// other: the batch is skipped with the one warning and this returns
/// normally, so a writer that stopped while holding the lock can't hang the
/// command. That includes the terminal tick, whose cleanup runs after its
/// `terminal` record. After one write has given up, later writes in the
/// process don't wait until one takes the lock again (see [`GAVE_UP`]).
///
/// The batch is flushed to disk only when it holds an identity record
/// ([`IDENTITY_KINDS`]). Records relayed from check, decider and override
/// events, and `response` records, are never flushed on their own: a flush
/// takes milliseconds, and a blocked tick writes several. They reach the
/// kernel at once, so a process that dies loses none of them, and the next
/// identity record's flush (a state entry, a terminal, a cancel) makes them
/// durable with it, since a flush covers the whole file. Only an OS crash
/// or power loss before then can lose them.
fn write(root: &Path, records: &[Record]) {
    write_within(root, records, journal_lock_wait())
}

/// How long a journal write waits for the journal's lock before its batch
/// is skipped, unless the caller passes a tighter bound. Generous enough
/// that ordinary contention between koto commands never skips a record;
/// short enough that a writer stopped while holding the lock can't hang a
/// command for long. A process waits it out once per stall: after a write
/// gives up, later writes don't wait until one takes the lock again (see
/// [`GAVE_UP`]). Not configurable.
///
/// The bound covers acquiring the lock only. Once a writer holds the lock,
/// what it does under it (the size check, a rotation's flush and rename,
/// retention, the batch's own writes and flush) runs to the end however
/// long it takes: a slow step there is never cut short, and it is the
/// writers waiting behind it whose bound runs out.
pub(crate) const JOURNAL_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a `koto status` `response` record waits for the journal's lock
/// before it is skipped (see [`response`]).
const RESPONSE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(30);

/// The lock wait a debug build reads from the environment in place of
/// [`JOURNAL_LOCK_WAIT`], in milliseconds, so the binary's tests can hold
/// the lock past the bound without sleeping for seconds. Release builds
/// don't read it, so it is not a setting.
#[cfg(all(not(test), debug_assertions))]
const LOCK_WAIT_TEST_ENV: &str = "KOTO_TEST_JOURNAL_LOCK_WAIT_MS";

/// The bound every journal write waits for the lock within, when its
/// caller passes no tighter one: [`JOURNAL_LOCK_WAIT`].
#[cfg(not(test))]
fn journal_lock_wait() -> std::time::Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var(LOCK_WAIT_TEST_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return std::time::Duration::from_millis(ms);
    }
    JOURNAL_LOCK_WAIT
}

#[cfg(test)]
fn journal_lock_wait() -> std::time::Duration {
    TEST_LOCK_WAIT.with(|c| c.get())
}

/// [`write`], waiting at most `lock_wait` for the journal's lock, or not at
/// all once an earlier write in this process has given up on it. A lock
/// still held after that is a failed write: one warning, and the batch is
/// skipped.
fn write_within(root: &Path, records: &[Record], lock_wait: std::time::Duration) {
    if records.is_empty() {
        return;
    }
    let path = root.join(JOURNAL_FILE);
    let max = max_line_bytes();
    let mut lines = Vec::with_capacity(records.len());
    let mut flush = false;
    for record in records {
        let line = record.to_line();
        if line.len() + 1 > max {
            warn_once(&format!(
                "{}: a {} record exceeds the {}-byte line cap",
                path.display(),
                record
                    .fields
                    .first()
                    .and_then(|(_, k)| k.as_str())
                    .unwrap_or("journal"),
                max
            ));
            continue;
        }
        flush |= record.is_identity();
        lines.push(line);
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    // A zero wait still tries the lock once, so a lock that was freed in
    // the meantime is taken and the batch written. Only the waiting is
    // dropped: a writer that takes the lock does everything it would have
    // under it, a rotation included.
    let lock_wait = if gave_up() {
        std::time::Duration::ZERO
    } else {
        lock_wait
    };
    // The batch goes to one file whole: a rotation happens before it or
    // after it, never between two of its lines. The lock wait covers the
    // whole append, the fresh file's lock after a rotation included, but
    // only the waiting: work done while holding the lock isn't timed.
    #[cfg(unix)]
    let written = rotate::append(root, &refs, max, flush, lock_wait);
    #[cfg(not(unix))]
    let written = crate::engine::jsonl_append::append_bounded_lines_no_follow_within(
        root,
        &path,
        &refs,
        max,
        flush,
        Some(lock_wait),
    );
    match written {
        Ok(flushed) => {
            // No lines opens nothing; any other success took the lock.
            if !refs.is_empty() {
                count_batch(flushed);
                set_gave_up(false);
            }
        }
        Err(e) => {
            // Only a wait that ran out on another writer's lock means a
            // writer may have stopped. A rotating writer whose own move
            // left it short of the fresh file's lock had its full wait
            // there and simply lost a race.
            if crate::engine::jsonl_append::lock_timeout(&e).is_some_and(|t| !t.after_own_move()) {
                set_gave_up(true);
            }
            warn_once(&format!("{}: {}", path.display(), e.root_cause()))
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

/// Whether a payload writes a record of its own, without reading anything.
/// A `context_read`, the commonest payload, never does. (A payload that
/// ends a visit can also write that visit's reads; see [`after_commit`].)
fn journals(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. }
            | EventPayload::WorkflowCancelled { .. }
    ) || relays(payload)
        || pairs(payload)
}

/// Whether a payload relays records of its own (see [`relayed`]).
fn relays(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::GateEvaluated { .. }
            | EventPayload::DefaultActionExecuted { .. }
            | EventPayload::DeciderConsulted(_)
            | EventPayload::DeciderChecked(_)
            | EventPayload::DeciderFallback(_)
            | EventPayload::GateOverrideRecorded { .. }
            | EventPayload::FindingDisputed(_)
    )
}

/// The check name a default action's records carry.
const ACTION_CHECK: &str = "__action__";

// The names the check-lifecycle records add. They are provisional, so each
// is written once here and every writer uses the constant.

/// One record per distinct (rule id, level) among an evaluation's logged
/// findings (see [`with_findings`]).
const CHECK_FINDING: &str = "check_finding";
/// One record per `finding_disputed` event.
const CHECK_DISPUTED: &str = "check_disputed";
/// On `check_finding` and `check_disputed`: the gate's name (or
/// `__action__`). The path an exporter that nests dotted names gives
/// `check_evaluated`'s scalar `koto.check`, so joins read one path; the
/// older kinds keep `koto.check` unchanged.
const CHECK_NAME_FIELD: &str = "koto.check.name";
/// On `check_evaluated`: how many findings the event logged, 0 included.
const FINDING_COUNT_FIELD: &str = "koto.check.finding_count";
/// On `check_evaluated`, when `true`: the check reported more findings than
/// the event logged.
const FINDINGS_TRUNCATED_FIELD: &str = "koto.check.findings_truncated";
/// On `check_finding`: the findings' level.
const FINDING_LEVEL_FIELD: &str = "koto.finding.level";
/// On `check_finding`: how many logged findings had the record's rule id
/// and level.
const FINDING_PAIR_COUNT_FIELD: &str = "koto.finding.count";
/// On `check_disputed`: who disputed the finding.
const DISPUTE_SOURCE_FIELD: &str = "koto.dispute.source";
/// The most `check_finding` records one evaluation writes.
const MAX_FINDING_RECORDS: usize = 50;

/// What every record placed in a session's log shares: the records relayed
/// from one committed event, or a `response` record.
struct Relay<'a> {
    session: &'a str,
    session_id: &'a str,
    run_id: Option<&'a str>,
    /// For relayed records, the committed event's own `seq`; for a
    /// `response`, the highest `seq` in the log when it is written.
    seq: u64,
    /// The visit the event belongs to, or, for a `response`, the visit the
    /// log is in when it is written.
    visit_seq: Option<u64>,
}

impl Relay<'_> {
    /// A record of `kind` placed in the log: the identity fields, then
    /// `koto.seq` and `koto.visit.seq`.
    fn record(&self, kind: &str) -> Record {
        Record::new(kind, self.session, self.session_id, self.run_id)
            .int("koto.seq", Some(self.seq))
            .int("koto.visit.seq", self.visit_seq)
    }
}

/// The records a committed event relays. A check event writes a
/// `check_pending` for a pending polling gate; otherwise a
/// `check_evaluated`, then one `check_rule` per key of the event's
/// `rule_counts`, in their order, then its `check_finding` records. A
/// `decider_consulted` event writes one record of its own name, followed by
/// a `decider_field` per field when it asked about several; a
/// `decider_checked` or `decider_fallback` event writes one record of its
/// own name, a `gate_override_recorded` one `check_overridden`, and a
/// `finding_disputed` one `check_disputed`. Every other payload relays
/// nothing here: `decider_answered` and `decider_check_answered`, which
/// need the earlier event they answer, come from [`paired`].
///
/// Each field is copied by name from the event, never by stripping it, so
/// nothing the event carries beyond these (output, streams, a finding's
/// message, path or reference, commands, probabilities, hashes, thresholds,
/// rationales, dispute reasons, override values) can reach the journal.
fn relayed(payload: &EventPayload, relay: &Relay<'_>) -> Vec<Record> {
    match payload {
        EventPayload::GateEvaluated {
            state,
            gate,
            output,
            outcome,
            check,
            poll,
            ..
        } => {
            // Only a decider check's output carries an `unanswered` list. A
            // decider check also counts the criteria waiting on the agent
            // and the ones an agent's answer decided, 0 included, on its
            // pending and its evaluated records alike.
            let unanswered = output.get("unanswered").and_then(Value::as_array);
            let decider_count = |key: &str| {
                unanswered.map(|_| {
                    output
                        .get(key)
                        .and_then(Value::as_array)
                        .map_or(0, |ids| ids.len() as u64)
                })
            };
            if outcome == "pending" {
                // A polling gate's wait, or a decider check waiting on the
                // agent. Only the first has a poll to describe.
                return vec![relay
                    .record("check_pending")
                    .name("koto.state", Some(state.as_str()))
                    .name("koto.check", Some(gate.as_str()))
                    .name("koto.poll.status", poll.as_ref().map(|_| "pending"))
                    .int(
                        "koto.poll.evaluations",
                        poll.as_ref().map(|p| p.evaluations),
                    )
                    .int(
                        "koto.poll.elapsed_secs",
                        poll.as_ref().map(|p| p.elapsed_secs),
                    )
                    .int("koto.decider.awaiting_count", decider_count("awaiting"))
                    .int(
                        "koto.decider.agent_answered_count",
                        decider_count("agent_answered"),
                    )];
            }
            let error = unanswered
                .and(output.get("error"))
                .and_then(Value::as_str)
                .filter(|e| !e.is_empty());
            let evaluated = check_evaluated(relay, state, gate, outcome, check)
                .name("koto.poll.status", poll.as_ref().map(|p| p.status.as_str()))
                .int(
                    "koto.check.unanswered_count",
                    unanswered.map(|u| u.len() as u64),
                )
                .name("koto.check.error", error)
                .int("koto.decider.awaiting_count", decider_count("awaiting"))
                .int(
                    "koto.decider.agent_answered_count",
                    decider_count("agent_answered"),
                );
            let mut records = with_rules(evaluated, relay, state, gate, check);
            records.extend(with_findings(relay, state, gate, check));
            records
        }
        EventPayload::DefaultActionExecuted {
            state,
            exit_code,
            check,
            ..
        } => {
            // A capture failure after exit 0 always carries `rule_counts`.
            let failed = *exit_code != 0 || check.rule_counts.is_some();
            let outcome = if failed { "failed" } else { "passed" };
            let evaluated = check_evaluated(relay, state, ACTION_CHECK, outcome, check);
            let mut records = with_rules(evaluated, relay, state, ACTION_CHECK, check);
            records.extend(with_findings(relay, state, ACTION_CHECK, check));
            records
        }
        EventPayload::DeciderConsulted(c) => decider_consulted(relay, c),
        EventPayload::DeciderChecked(c) => vec![decider_checked(relay, c)],
        EventPayload::DeciderFallback(f) => vec![decider_fallback(relay, f)],
        // The override event doesn't say which rules it moved past; a reader
        // joins it to the evaluation it overrode (the reference's "Joining
        // an override"). Its rationale and values stay in the log.
        EventPayload::GateOverrideRecorded { state, gate, .. } => vec![relay
            .record("check_overridden")
            .name("koto.state", Some(state.as_str()))
            .name("koto.check", Some(gate.as_str()))],
        // The reason stays in the log. The attempt numbers are the disputed
        // evaluation's, which the event carries, so a reader joins the
        // dispute to exactly that `check_evaluated`.
        EventPayload::FindingDisputed(d) => vec![relay
            .record(CHECK_DISPUTED)
            .name("koto.state", Some(d.state.as_str()))
            .name(CHECK_NAME_FIELD, Some(d.gate.as_str()))
            .int("koto.attempt", d.attempt)
            .int("koto.visit_attempt", d.visit_attempt)
            .name("koto.rule.id", Some(name_or_digest(&d.rule_id).as_str()))
            .name(DISPUTE_SOURCE_FIELD, Some(d.source.as_str()))],
        _ => Vec::new(),
    }
}

/// A `decider_consulted` record: the routing decider's provider, model,
/// outcome, tokens and timings, how many fields it was asked, and, for an
/// `input_unavailable` consultation, the reason and the input's label and
/// size. The event's own `visit_seq` isn't copied: `koto.visit.seq` comes
/// from `relay`, by koto's visit rule, like every relayed record's.
///
/// A consultation of one field (every shipped template declares one per
/// state) carries that field's answer keys itself (see [`with_field`]). One
/// of several fields carries none, and a `decider_field` record per field
/// follows it, keyed to it by `koto.decider.consulted_seq`, so the
/// consultation's tokens and timings are still written once.
fn decider_consulted(relay: &Relay<'_>, c: &crate::decider::DeciderConsultation) -> Vec<Record> {
    let consulted = relay
        .record("decider_consulted")
        .name("koto.state", Some(c.state.as_str()))
        .name("koto.decider.provider", Some(c.provider.as_str()))
        .name("koto.decider.model", Some(c.model.as_str()))
        .name("koto.decider.outcome", Some(c.outcome.as_str()))
        .name(
            "koto.decider.error_class",
            c.error_class.as_ref().map(|e| e.as_str()),
        )
        .int("koto.decider.input_tokens", c.input_tokens)
        .int("koto.decider.output_tokens", c.output_tokens)
        .int(
            "koto.decider.unread_usage_attempts",
            Some(u64::from(c.unread_usage_attempts)),
        )
        .int("koto.decider.latency_ms", Some(c.latency_ms))
        .int("koto.decider.directive_bytes", Some(c.directive_bytes))
        .name("koto.decider.reason", c.reason.map(|r| r.as_str()))
        .name(
            "koto.decider.input_label",
            c.input_label.as_deref().map(name_or_digest).as_deref(),
        )
        .int("koto.decider.input_bytes", c.input_bytes)
        .int("koto.decider.field_count", Some(c.fields.len() as u64));
    if c.fields.len() == 1 {
        let (field, fc) = c.fields.iter().next().expect("one field");
        return vec![with_field(consulted, c.outcome, field, fc)];
    }
    let mut records = vec![consulted];
    for (field, fc) in &c.fields {
        records.push(with_field(
            relay
                .record(DECIDER_FIELD)
                .name("koto.state", Some(c.state.as_str()))
                .int(CONSULTED_SEQ_FIELD, Some(relay.seq)),
            c.outcome,
            field,
            fc,
        ));
    }
    records
}

/// One record per declared field of a consultation of several fields.
const DECIDER_FIELD: &str = "decider_field";
/// One record per field an agent answered on a consulted visit (see
/// [`paired`]).
const DECIDER_ANSWERED: &str = "decider_answered";
/// One record per `decider_check_answered` event (see [`paired`]).
const DECIDER_CHECK_ANSWERED: &str = "decider_check_answered";
/// On `decider_field` and `decider_answered`: the `koto.seq` of the
/// `decider_consulted` record the row belongs to.
const CONSULTED_SEQ_FIELD: &str = "koto.decider.consulted_seq";
/// The answer written for an agent's value that isn't one of the field's
/// declared values, so no submitted text reaches the journal.
const OTHER_ANSWER: &str = "other";

/// `record` with one field's answer keys: the field's name, the decider's
/// answer, the declared mode for it, and whether it met its threshold.
///
/// The answer is the column `koto decider report` puts the field in: the
/// winning declared value when it met its threshold (applied or not), and
/// otherwise koto's own word for what happened: `below_threshold`,
/// `escape`, or `no_answer` when the consultation got no usable answer.
/// The mode is the winner's when it is a
/// declared value, otherwise the one mode every declared value shares, and
/// otherwise absent. `koto.decider.threshold_met` is written only when a
/// declared value won, so never with `escape` or `no_answer`. The field and a value are names the template
/// declares, digested like rule ids when outside the name shape; nothing
/// the session wrote is copied.
fn with_field(
    record: Record,
    outcome: crate::decider::ConsultationOutcome,
    field: &str,
    fc: &crate::decider::FieldConsultation,
) -> Record {
    // The report's own column, so the export and `koto decider report`
    // never read one consultation two ways. It is a declared value or one
    // of the report's fixed words, all of them name-shaped but a declared
    // value outside the shape, which is digested.
    let column = crate::decider::report::ledger_column(outcome, fc);
    // A threshold was met or missed only by a declared value that won: an
    // escape or no answer has none to report.
    let threshold_met = (column != crate::decider::report::COL_ESCAPE
        && column != crate::decider::report::COL_NO_ANSWER)
        .then_some(fc.at_threshold);
    let answer = name_or_digest(&column);
    let declared_winner = fc.winning.as_deref().filter(|w| fc.modes.contains_key(*w));
    let mode = declared_winner
        .and_then(|w| fc.modes.get(w).copied())
        .or_else(|| {
            let mut modes = fc.modes.values();
            let first = *modes.next()?;
            modes.all(|m| *m == first).then_some(first)
        });
    record
        .name("koto.decider.field", Some(name_or_digest(field).as_str()))
        .name("koto.decider.answer", Some(answer.as_str()))
        .name("koto.decider.mode", mode.map(|m| m.as_str()))
        .opt_flag("koto.decider.threshold_met", threshold_met)
}

/// Whether a payload writes records that pair it with an earlier event in
/// its log (see [`paired`]). Evidence the decider itself applied never does.
fn pairs(payload: &EventPayload) -> bool {
    match payload {
        EventPayload::EvidenceSubmitted { source, .. } => {
            source.as_deref() != Some(crate::engine::decider::DECIDER_SOURCE)
        }
        EventPayload::DeciderCheckAnswered(_) => true,
        _ => false,
    }
}

/// The records that pair a committed event with an earlier one in
/// `prior_log` (the log before it):
///
/// - Evidence the agent submitted on a visit whose consultation wasn't
///   applied writes one `decider_answered` per consulted field it holds:
///   the visits the ledger's `answered` record pairs. Each carries the
///   field, the agent's answer and `koto.decider.consulted_seq`, the
///   consultation's `koto.seq`. The answer is the submitted value only when
///   it is one of the field's declared values, read from the consultation's
///   own record of them; any other value is written `other`.
/// - A `decider_check_answered` event writes a `decider_check_answered`
///   with the gate, the criterion, the agent's answer, and the `seq` and
///   reason of the `decider_fallback` it answers, the reason read from that
///   event.
///
/// Every other payload writes nothing, and so does one that finds nothing
/// to pair with.
fn paired(payload: &EventPayload, relay: &Relay<'_>, prior_log: &str) -> Vec<Record> {
    match payload {
        EventPayload::EvidenceSubmitted { state, fields, .. } => {
            let Some((consulted_seq, c)) = pairing::consultation(prior_log, state) else {
                return Vec::new();
            };
            if c.outcome == crate::decider::ConsultationOutcome::Applied {
                return Vec::new();
            }
            c.fields
                .iter()
                .filter_map(|(field, fc)| {
                    let answer = agent_answer(fields.get(field)?, fc);
                    Some(
                        relay
                            .record(DECIDER_ANSWERED)
                            .name("koto.state", Some(state.as_str()))
                            .name("koto.decider.field", Some(name_or_digest(field).as_str()))
                            .name("koto.decider.agent_answer", Some(answer.as_str()))
                            .int(CONSULTED_SEQ_FIELD, Some(consulted_seq)),
                    )
                })
                .collect()
        }
        EventPayload::DeciderCheckAnswered(a) => {
            let reason = pairing::fallback(prior_log, a.fallback_seq).map(|f| f.reason);
            vec![relay
                .record(DECIDER_CHECK_ANSWERED)
                .name("koto.state", Some(a.state.as_str()))
                .name("koto.check", Some(a.gate.as_str()))
                .name("koto.rule.id", Some(name_or_digest(&a.rule_id).as_str()))
                .name("koto.decider.answer", Some(a.answer.as_str()))
                .name("koto.decider.reason", reason.map(|r| r.as_str()))
                .int("koto.decider.fallback_seq", Some(a.fallback_seq))]
        }
        _ => Vec::new(),
    }
}

/// The answer a `decider_answered` record carries for `submitted`: the
/// value when it is one of the field's declared values (`"true"` or
/// `"false"` for a boolean), digested like a rule id when outside the name
/// shape, and `other` for anything else.
fn agent_answer(submitted: &Value, fc: &crate::decider::FieldConsultation) -> String {
    let value = match submitted {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    value
        .filter(|v| fc.modes.contains_key(v))
        .map_or(OTHER_ANSWER.to_string(), |v| name_or_digest(&v))
}

/// A `decider_checked` record: one decider-check criterion's verdict, under
/// its gate as `koto.check` and its id (or the id's digest) as
/// `koto.rule.id`. Like [`decider_consulted`], the event's own `visit_seq`
/// isn't copied.
fn decider_checked(relay: &Relay<'_>, c: &crate::decider::check::DeciderCheck) -> Record {
    relay
        .record("decider_checked")
        .name("koto.state", Some(c.state.as_str()))
        .name("koto.check", Some(c.gate.as_str()))
        .name("koto.rule.id", Some(name_or_digest(&c.rule_id).as_str()))
        .name("koto.decider.provider", Some(c.provider.as_str()))
        .name("koto.decider.model", Some(c.model.as_str()))
        .name("koto.decider.mode", Some(c.mode.as_str()))
        .name("koto.decider.outcome", Some(c.outcome.as_str()))
        .name("koto.decider.reason", c.reason.map(|r| r.as_str()))
        .flag("koto.decider.blocked", c.blocked)
        .name(
            "koto.decider.error_class",
            c.error_class.as_ref().map(|e| e.as_str()),
        )
        .int("koto.decider.input_tokens", c.input_tokens)
        .int("koto.decider.output_tokens", c.output_tokens)
        .int(
            "koto.decider.unread_usage_attempts",
            Some(u64::from(c.unread_usage_attempts)),
        )
        .int("koto.decider.attempts", Some(u64::from(c.attempts)))
        .int("koto.decider.latency_ms", Some(c.latency_ms))
        .int("koto.decider.input_bytes", c.input_bytes)
}

/// A `decider_fallback` record: one decider-check criterion handed to the
/// agent (DESIGN-decider-fallback.md, Decision 4), under its gate as
/// `koto.check` and its id (or the id's digest) as `koto.rule.id`, with its
/// mode, the reason it got no verdict and the slice's size. Like
/// [`decider_checked`], the event's own `visit_seq` isn't copied, and no
/// hash, question or slice is.
fn decider_fallback(relay: &Relay<'_>, f: &crate::decider::check::DeciderFallback) -> Record {
    relay
        .record("decider_fallback")
        .name("koto.state", Some(f.state.as_str()))
        .name("koto.check", Some(f.gate.as_str()))
        .name("koto.rule.id", Some(name_or_digest(&f.rule_id).as_str()))
        .name("koto.decider.mode", Some(f.mode.as_str()))
        .name("koto.decider.reason", Some(f.reason.as_str()))
        .int("koto.decider.input_bytes", f.input_bytes)
}

/// A `check_evaluated` record's fields common to gates and default actions.
fn check_evaluated(
    relay: &Relay<'_>,
    state: &str,
    check_name: &str,
    outcome: &str,
    check: &CheckEventFields,
) -> Record {
    let rule_count = check.rule_counts.as_ref().map_or(0, |r| r.0.len()) as u64;
    let mut record = relay
        .record("check_evaluated")
        .name("koto.state", Some(state))
        .name("koto.check", Some(check_name))
        .name("koto.check.outcome", Some(outcome))
        .int("koto.attempt", check.attempt)
        .int("koto.visit_attempt", check.visit_attempt)
        .int("koto.check.rule_count", Some(rule_count));
    if check.rule_counts_truncated {
        record = record.flag("koto.check.rule_counts_truncated", true);
    }
    record = record.int(FINDING_COUNT_FIELD, Some(check.findings.len() as u64));
    if check.findings_truncated {
        record = record.flag(FINDINGS_TRUNCATED_FIELD, true);
    }
    record.int("koto.check.duration_ms", check.duration_ms)
}

/// `evaluated`, followed by one `check_rule` per error rule.
fn with_rules(
    evaluated: Record,
    relay: &Relay<'_>,
    state: &str,
    check_name: &str,
    check: &CheckEventFields,
) -> Vec<Record> {
    let rules = check.rule_counts.as_ref().map_or(&[][..], |r| &r.0[..]);
    let mut records = Vec::with_capacity(1 + rules.len());
    records.push(evaluated);
    for (rule_id, count) in rules {
        records.push(
            relay
                .record("check_rule")
                .name("koto.state", Some(state))
                .name("koto.check", Some(check_name))
                .int("koto.attempt", check.attempt)
                .int("koto.visit_attempt", check.visit_attempt)
                .name("koto.rule.id", Some(name_or_digest(rule_id).as_str()))
                .int("koto.rule.visit_count", Some(count.visit))
                .int("koto.rule.session_count", Some(count.session)),
        );
    }
    records
}

/// One `check_finding` per distinct (rule id, level) among the event's
/// logged findings, whether the check passed or failed: `error` pairs
/// first, then `warning`, then `info`, then any other level, each level in
/// the order its pairs were first seen, at most [`MAX_FINDING_RECORDS`].
/// Each counts the logged findings with its pair. Nothing else of a finding
/// (message, path, location, rule reference) is copied.
fn with_findings(
    relay: &Relay<'_>,
    state: &str,
    check_name: &str,
    check: &CheckEventFields,
) -> Vec<Record> {
    // (rule id, level, count), in first-seen order.
    let mut pairs: Vec<(&str, &crate::findings::FindingLevel, u64)> = Vec::new();
    for f in &check.findings {
        match pairs
            .iter_mut()
            .find(|(id, level, _)| *id == f.rule_id && **level == f.level)
        {
            Some(pair) => pair.2 += 1,
            None => pairs.push((f.rule_id.as_str(), &f.level, 1)),
        }
    }
    // Stable, so each level keeps first-seen order.
    pairs.sort_by_key(|(_, level, _)| crate::findings::level_rank(level));
    pairs.truncate(MAX_FINDING_RECORDS);
    pairs
        .into_iter()
        .map(|(rule_id, level, count)| {
            relay
                .record(CHECK_FINDING)
                .name("koto.state", Some(state))
                .name(CHECK_NAME_FIELD, Some(check_name))
                .int("koto.attempt", check.attempt)
                .int("koto.visit_attempt", check.visit_attempt)
                .name("koto.rule.id", Some(name_or_digest(rule_id).as_str()))
                .name(FINDING_LEVEL_FIELD, Some(level.as_str()))
                .int(FINDING_PAIR_COUNT_FIELD, Some(count))
        })
        .collect()
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

/// The session's header: the first line of the log `commit` read under its
/// lock, or, should that not parse, the backend's own read.
fn header_of(
    commit: &CommitInfo,
    backend: &dyn SessionBackend,
    session: &str,
) -> anyhow::Result<StateFileHeader> {
    let parsed = commit
        .prior_log
        .lines()
        .next()
        .and_then(|line| serde_json::from_str::<StateFileHeader>(line.trim()).ok());
    match parsed {
        Some(header) => Ok(header),
        None => backend.read_header(session),
    }
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
/// harmless.
///
/// A payload that ends a visit (see `context_reads::closes_visit`: an
/// arrival, a rewind, a cancel, or the terminal tick's
/// `request_store.result`) first folds that visit's `context_read` events
/// out of `commit`'s copy of the log, and writes the resulting
/// `context_reads` after any `driver_seen` and before its own record. A
/// `request_store.result` writes nothing else, so when its visit has no
/// reads left to write, it reads nothing more. Without a driver change, a
/// payload that is none of these (a state entry, a cancel, that result, a
/// check, decider or override event) writes nothing and reads nothing
/// beyond the sidecar.
///
/// A check event (`gate_evaluated`, `default_action_executed`) relays its
/// `check_evaluated` and `check_rule` records, or a `check_pending`; a
/// decider or override event its one record (see [`relayed`]). Each carries
/// `commit`'s `seq` as `koto.seq` and the visit the event belongs to as
/// `koto.visit.seq`. The header comes from `commit`'s copy of the
/// log, so nothing is read again. A commit's records are written as one
/// batch, flushed only when it holds an identity record (see [`write`]).
/// `journal_root` is the store's journal home; `None` writes nothing.
pub(crate) fn after_commit(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    payload: &EventPayload,
    commit: &CommitInfo,
) {
    let Some(root) = journal_root else {
        return;
    };
    let session_dir = backend.session_dir(session);
    let new_driver = changed_driver(&session_dir);
    // The visit a closing commit ends, and its reads still to be written.
    // Folded before anything else is read, so a closing commit with nothing
    // to write (a `request_store.result` in a visit with no reads) stops
    // here.
    let reads = context_reads::closes_visit(payload)
        .then(|| commit.prior_visit_seq())
        .flatten()
        .and_then(|visit| {
            let fold = context_reads::fold_with_spool(&commit.prior_log, visit, &session_dir)?;
            Some((visit, fold))
        });
    if new_driver.is_none() && reads.is_none() && !journals(payload) {
        return;
    }
    let header = match header_of(commit, backend, session) {
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
    // A commit that ends a visit writes that visit's reads first, before
    // the `state_entered` or `cancelled` it maps to.
    if let Some((visit_seq, fold)) = reads {
        records.extend(fold.into_records(&Relay {
            session,
            session_id: &header.session_id,
            run_id: run_id.as_deref(),
            seq: commit.seq,
            visit_seq: Some(visit_seq),
        }));
    }
    records.extend(record_for(payload, session, &header, run_id.as_deref()));
    // The visit is worked out only here, for a payload that relays records
    // of its own. Apart from the closing commits above, every other commit
    // skips the scan of the log.
    if relays(payload) || pairs(payload) {
        let relay = Relay {
            session,
            session_id: &header.session_id,
            run_id: run_id.as_deref(),
            seq: commit.seq,
            visit_seq: commit.visit_seq_for(payload),
        };
        records.extend(relayed(payload, &relay));
        records.extend(paired(payload, &relay, &commit.prior_log));
    }
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
///
/// Like every journal write it always returns: a record it can't write,
/// one whose lock stays held past [`JOURNAL_LOCK_WAIT`] included, is
/// skipped with the one warning, and the tick goes on to its index entry,
/// parent notice and cleanup as it would have.
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

/// The command that printed a response: `koto next` or `koto status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseSource {
    Next,
    Status,
}

impl ResponseSource {
    fn as_str(self) -> &'static str {
        match self {
            ResponseSource::Next => "next",
            ResponseSource::Status => "status",
        }
    }
}

/// A `response` record: the state whose directive was printed, the command
/// that printed it, and the byte lengths of the printed `directive` and
/// `details` (0 when none was printed). Never the text itself.
fn response_record(
    relay: &Relay<'_>,
    state: &str,
    source: ResponseSource,
    directive: &str,
    details: Option<&str>,
) -> Record {
    relay
        .record("response")
        .name("koto.state", Some(state))
        .name("koto.response.source", Some(source.as_str()))
        .int(
            "koto.response.directive_bytes",
            Some(directive.len() as u64),
        )
        .int(
            "koto.response.details_bytes",
            Some(details.map_or(0, str::len) as u64),
        )
}

/// Journal a response that has just been printed: one `response` record for
/// `state`, the non-terminal state whose `directive` (and `details`, when it
/// carried them) the command printed. `koto next` calls this at its two
/// print sites, after the response and its `instructions_delivered` event,
/// and `koto status` at its end; a terminal, an error or a refusal prints no
/// directive and calls nothing. A chained `koto next` prints, and records,
/// only the state it stopped in.
///
/// The byte counts are the UTF-8 lengths of the strings the response
/// serialized, which are the printed JSON's strings after decoding: escapes
/// don't count, and text koto adds to the directive (the recovery pointer,
/// a notice) does.
///
/// The session's log is read once, straight from the session directory and
/// never through the backend's own read (which, for the cloud store, can
/// fetch the log from the remote). That one read gives the header, the
/// highest `seq` (`koto.seq`), and the state the log is in with the visit
/// that state's arrival opened (`koto.visit.seq`, by
/// [`persistence::visit_start`]'s rule). When the log's state isn't
/// `state`, another command advanced the session after this one printed,
/// and no record is written: its position would describe a different
/// state.
///
/// This hook writes nothing but the record. The run id comes from the
/// session's sidecar, or is derived from the header without caching it. For
/// `koto status` the journal's lock is waited for at most
/// [`RESPONSE_LOCK_WAIT`], so a writer that stopped while holding it can't
/// hold up the command an agent uses to recover; past that, the record is
/// skipped with the one warning. `koto next` waits as long as every other
/// journal write ([`JOURNAL_LOCK_WAIT`]), so its records aren't lost to a
/// busy journal but a stuck one can't hang it either. The record is written
/// without a flush, like the check records.
/// `journal_root` is the store's journal home; `None` writes nothing.
///
/// [`persistence::visit_start`]: crate::engine::persistence::visit_start
pub(crate) fn response(
    journal_root: Option<&Path>,
    backend: &dyn SessionBackend,
    session: &str,
    state: &str,
    source: ResponseSource,
    directive: &str,
    details: Option<&str>,
) {
    use crate::engine::persistence::{current_visit, last_seq};
    let Some(root) = journal_root else {
        return;
    };
    let session_dir = backend.session_dir(session);
    let log =
        match std::fs::read_to_string(session_dir.join(crate::session::state_file_name(session))) {
            Ok(log) => log,
            Err(e) => {
                warn_once(&format!(
                    "could not read the log of session {}: {}",
                    session, e
                ));
                return;
            }
        };
    let header = log
        .lines()
        .next()
        .and_then(|line| serde_json::from_str::<StateFileHeader>(line.trim()).ok());
    let (Some(header), Some(seq)) = (header, last_seq(&log)) else {
        warn_once(&format!(
            "could not read the header and events of session {}",
            session
        ));
        return;
    };
    let Some((current, visit_seq)) = current_visit(&log) else {
        return;
    };
    if current != state {
        return;
    }
    let run_id = sidecar::read(&session_dir)
        .and_then(|c| c.run_id)
        .or_else(|| run_id::run_id(backend, &header));
    let relay = Relay {
        session,
        session_id: &header.session_id,
        run_id: run_id.as_deref(),
        seq,
        visit_seq: Some(visit_seq),
    };
    let lock_wait = match source {
        ResponseSource::Status => RESPONSE_LOCK_WAIT.min(journal_lock_wait()),
        ResponseSource::Next => journal_lock_wait(),
    };
    write_within(
        root,
        &[response_record(&relay, state, source, directive, details)],
        lock_wait,
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
