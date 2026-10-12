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
//! Those five are the identity kinds. Three more are relayed from check
//! events, and each carries `koto.seq` (the relayed event's sequence
//! number in the session's log) and `koto.visit.seq` (the sequence number
//! of the event that opened the visit it belongs to, by
//! `persistence::visit_start`'s rule):
//!
//! - `check_evaluated`: one per `gate_evaluated` that isn't pending and per
//!   `default_action_executed` (check `__action__`), with the outcome,
//!   attempt numbers, rule count and run time.
//! - `check_rule`: one per key of that event's `rule_counts`, after its
//!   `check_evaluated`, with the rule's visit and session counts. A rule id
//!   outside the name shape is written as its digest ([`name_or_digest`]).
//! - `check_pending`: one per pending `gate_evaluated` of a polling gate,
//!   with the poll's run count and elapsed seconds, never an outcome.
//!
//! Three more are relayed the same way, with the same two sequence
//! numbers, from decider and override events:
//!
//! - `decider_consulted`: one per `decider_consulted` event, with the
//!   provider, model, outcome, token counts and timings.
//! - `decider_checked`: one per `decider_checked` event (one criterion of a
//!   decider check), with the gate as `koto.check`, the criterion's id as
//!   `koto.rule.id` (digested like a rule id), the mode, outcome, reason,
//!   whether it blocked, tokens and timings. A verdict koto reuses appends
//!   no event, so it writes no record.
//! - `check_overridden`: one per `gate_override_recorded`, with the state
//!   and gate only.
//!
//! None of them copies a probability, hash, threshold, endpoint, rule
//! reference, rationale, override value or output, and `koto.visit.seq` is
//! always koto's visit, never the `visit_seq` a decider event carries.
//!
//! A field with no value is left out, never written as null or empty.
//! Ids must match `^[A-Za-z0-9._:-]{1,128}$`, and names
//! `^[A-Za-z0-9._:/@-]{1,128}$` without a leading `/` or `~`. Names cover
//! template and state names, `koto.check`, the decider's provider and
//! model, and the short vocabulary values the relayed kinds carry
//! (`koto.check.outcome`, `koto.poll.status`, `koto.check.error`, and the
//! `koto.decider.` outcome, mode, reason and error class). A value outside
//! its shape is left out and the record is still written, with one
//! exception: a rule id outside the name shape, a decider criterion's id
//! included, is written as its digest ([`name_or_digest`]), never dropped.
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
//!   `decider_consulted`, `decider_checked` and `gate_override_recorded`.
//!   Before those, on any committed payload, it writes `driver_seen` when
//!   the process's driver (`CLAUDE_CODE_SESSION_ID`, when id-shaped)
//!   differs from the one the session's sidecar caches, or the sidecar
//!   caches none, and then caches the new driver. No driver is never a
//!   change. Two commands racing under one new driver can each write a
//!   `driver_seen`; readers take the distinct values.
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
//! on. Nothing about the command's output, exit code, session state or
//! gate decisions changes.
//!
//! Writers take an advisory lock on the journal for each commit's records,
//! written as one batch, so a record is never interleaved with another. A
//! write cut short (a crash, a full disk) can still leave a partial last
//! line: the next writer sees, under the lock, that the file doesn't end in
//! a newline and starts its record on a line of its own, so only the
//! partial line is lost. Readers skip lines that don't parse as JSON.
//!
//! A batch holding an identity record is flushed to disk; a batch of
//! relayed records alone isn't, and is made durable by the next flush (see
//! [`write`]).
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
/// decider and override events never are on their own (see [`write`]).
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
    /// (batches written, batches flushed to disk) on this thread.
    static TEST_BATCHES: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
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

/// Reset this thread's line cap and warnings.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    TEST_MAX_LINE.with(|c| c.set(MAX_LINE_BYTES));
    TEST_WARNED.with(|w| w.set(false));
    TEST_WARNINGS.with(|w| w.borrow_mut().clear());
    TEST_BATCHES.with(|c| c.set((0, 0)));
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
/// The batch is flushed to disk only when it holds an identity record
/// ([`IDENTITY_KINDS`]). Records relayed from check, decider and override
/// events are never flushed on their own: a flush takes milliseconds, and a blocked tick
/// relays several. They reach the kernel at once, so a process that dies
/// loses none of them, and the next identity record's flush (a state entry,
/// a terminal, a cancel) makes them durable with it, since a flush covers
/// the whole file. Only an OS crash or power loss before then can lose
/// them.
fn write(root: &Path, records: &[Record]) {
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
    // The batch goes to one file whole: a rotation happens before it or
    // after it, never between two of its lines.
    #[cfg(unix)]
    let written = rotate::append(root, &refs, max, flush);
    #[cfg(not(unix))]
    let written =
        crate::engine::jsonl_append::append_bounded_lines_no_follow(root, &path, &refs, max, flush);
    match written {
        Ok(flushed) => {
            if !refs.is_empty() {
                count_batch(flushed);
            }
        }
        Err(e) => warn_once(&format!("{}: {}", path.display(), e.root_cause())),
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

/// Whether a payload writes a record, without reading anything. A
/// `context_read`, the commonest payload, never does.
fn journals(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. }
            | EventPayload::WorkflowCancelled { .. }
    ) || relays(payload)
}

/// Whether a payload relays records of its own (see [`relayed`]).
fn relays(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::GateEvaluated { .. }
            | EventPayload::DefaultActionExecuted { .. }
            | EventPayload::DeciderConsulted(_)
            | EventPayload::DeciderChecked(_)
            | EventPayload::GateOverrideRecorded { .. }
    )
}

/// The check name a default action's records carry.
const ACTION_CHECK: &str = "__action__";

/// What every record relayed from one committed event shares.
struct Relay<'a> {
    session: &'a str,
    session_id: &'a str,
    run_id: Option<&'a str>,
    /// The event's own `seq`.
    seq: u64,
    /// The visit the event belongs to.
    visit_seq: Option<u64>,
}

impl Relay<'_> {
    /// A relayed record of `kind`: the identity fields, then `koto.seq` and
    /// `koto.visit.seq`.
    fn record(&self, kind: &str) -> Record {
        Record::new(kind, self.session, self.session_id, self.run_id)
            .int("koto.seq", Some(self.seq))
            .int("koto.visit.seq", self.visit_seq)
    }
}

/// The records a committed event relays. A check event writes a
/// `check_pending` for a pending polling gate; otherwise a
/// `check_evaluated`, then one `check_rule` per key of the event's
/// `rule_counts`, in their order. A `decider_consulted` or
/// `decider_checked` event writes one record of its own name, and a
/// `gate_override_recorded` one `check_overridden`. Every other payload
/// relays nothing.
///
/// Each field is copied by name from the event, never by stripping it, so
/// nothing the event carries beyond these (output, streams, findings,
/// commands, probabilities, hashes, thresholds, rationales, override
/// values) can reach the journal.
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
            if outcome == "pending" {
                return vec![relay
                    .record("check_pending")
                    .name("koto.state", Some(state.as_str()))
                    .name("koto.check", Some(gate.as_str()))
                    .name("koto.poll.status", Some("pending"))
                    .int(
                        "koto.poll.evaluations",
                        poll.as_ref().map(|p| p.evaluations),
                    )
                    .int(
                        "koto.poll.elapsed_secs",
                        poll.as_ref().map(|p| p.elapsed_secs),
                    )];
            }
            // Only a decider check's output carries an `unanswered` list.
            let unanswered = output.get("unanswered").and_then(Value::as_array);
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
                .name("koto.check.error", error);
            with_rules(evaluated, relay, state, gate, check)
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
            with_rules(evaluated, relay, state, ACTION_CHECK, check)
        }
        EventPayload::DeciderConsulted(c) => vec![decider_consulted(relay, c)],
        EventPayload::DeciderChecked(c) => vec![decider_checked(relay, c)],
        // The override event doesn't say which rules it moved past; a reader
        // joins it to the evaluation it overrode (the reference's "Joining
        // an override"). Its rationale and values stay in the log.
        EventPayload::GateOverrideRecorded { state, gate, .. } => vec![relay
            .record("check_overridden")
            .name("koto.state", Some(state.as_str()))
            .name("koto.check", Some(gate.as_str()))],
        _ => Vec::new(),
    }
}

/// A `decider_consulted` record: the routing decider's provider, model,
/// outcome, tokens and timings. The event's own `visit_seq` isn't copied:
/// `koto.visit.seq` comes from `relay`, by koto's visit rule, like every
/// relayed record's.
fn decider_consulted(relay: &Relay<'_>, c: &crate::decider::DeciderConsultation) -> Record {
    relay
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
/// harmless. Without a driver change, payloads other than state entries,
/// cancels and check events write nothing and read nothing beyond the
/// sidecar.
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
    if new_driver.is_none() && !journals(payload) {
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
    records.extend(record_for(payload, session, &header, run_id.as_deref()));
    // The visit is worked out only here, for a payload that relays records
    // of its own: every other commit skips the scan of the log.
    if relays(payload) {
        records.extend(relayed(
            payload,
            &Relay {
                session,
                session_id: &header.session_id,
                run_id: run_id.as_deref(),
                seq: commit.seq,
                visit_seq: commit.visit_seq_for(payload),
            },
        ));
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
