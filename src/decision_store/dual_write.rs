//! Writing a decision beside a session event, and repairing a lost append.
//!
//! `koto overrides record`, `koto next --to`, `koto decider answer` and
//! `koto decisions record <name>` each write their own session event and a
//! decision. The session log can't share a transaction with the store, so
//! the order is fixed (DESIGN-decision-record.md, Decision 3):
//!
//! 1. resolve the decider before anything is written, refusing a gate pass
//!    nobody can be named for (only a `refused` decision is written then);
//! 2. build the decision, redact it and check it the way the writer will,
//!    so the copy the event carries is exactly the line the store gets;
//! 3. append the session event carrying that copy in `decision_records`;
//! 4. append the decision to the store. A failure here warns and changes
//!    nothing else: the event already holds the whole decision.
//!
//! [`repair`] closes the gap step 4 can leave. It runs at the start of every
//! `koto next` and before a terminal tick removes its session, finds the
//! copies the session's events carry that the store lacks, and appends each
//! one marked `repaired_from` with the session's name. It never appends a
//! `reverse` line, whatever an event carries, so a forged event can't
//! record a reversal past the authority rules; it skips a copy that doesn't
//! parse, doesn't validate, or whose identifier names a month other than
//! its own time's; and it never fails the tick that runs it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::decider::{agent_slot, run_mode_slot, Observed};
use super::record::{Authority, Chain, DecidedBy, DeciderKind, Decision, Kind, Subject};
use super::{
    id_month, new_id, now_to_second, redact_decision, time_month, writer, Check, FieldError, Store,
    StoreError, WriteOutcome, DECISION_FORMAT_VERSION,
};
use crate::engine::types::Event;

/// Set to `append` to make the store append after a session event fail, so
/// a test can watch the repair tick put the decision back. Only a debug
/// build reads it, so it is not a setting.
pub const STORE_FAULT_ENV: &str = "KOTO_TEST_DECISION_STORE_FAULT";

/// The refusal reason a gate pass with no decider records.
pub const DECIDER_MISSING: &str = super::decider::DeciderError::MISSING_REASON;

/// The most bytes of a command's own free text (an override rationale, a
/// recorded choice) a decision copies. The session event keeps the whole
/// text; the decision keeps this much, so a long rationale can't push the
/// line past the store's line cap and fail the command.
pub const MAX_COPIED_TEXT_BYTES: usize = 16 * 1024;

/// `text` cut to [`MAX_COPIED_TEXT_BYTES`] on a character boundary, with a
/// trailing ellipsis when it was cut.
pub fn clip(text: &str) -> String {
    if text.len() <= MAX_COPIED_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_COPIED_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\u{2026}", &text[..end])
}

/// Who decided, under what authority and through which chain, resolved
/// before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub decided_by: DecidedBy,
    pub chain: Chain,
    pub authority: Authority,
}

fn decision(
    kind: Kind,
    outcome: &str,
    subject: Subject,
    identity: &Identity,
    evidence: Vec<String>,
    reason: Option<String>,
) -> Result<Decision, FieldError> {
    let time = now_to_second();
    Ok(Decision {
        v: DECISION_FORMAT_VERSION,
        id: new_id(&time)?,
        time,
        kind,
        outcome: outcome.to_string(),
        subject,
        decided_by: identity.decided_by.clone(),
        authority: identity.authority.clone(),
        chain: identity.chain.clone(),
        basis: Vec::new(),
        evidence,
        reason,
        reverses: None,
        original: None,
        repaired_from: None,
        import_source: None,
        recorded_at: None,
        compensating: None,
        replacement: None,
    })
}

/// A `gate-pass` subject: the workflow, the session id in its state-file
/// header (left out when it has none), the state and the gate. The session
/// id is what lets the reversal check in `koto next` tell this run of the
/// workflow from another of the same name.
pub fn gate_subject(workflow: &str, session_id: &str, state: &str, gate: &str) -> Subject {
    Subject {
        workflow: Some(workflow.to_string()),
        workflow_session_id: Some(session_id.to_string()).filter(|s| !s.is_empty()),
        state: Some(state.to_string()),
        gate: Some(gate.to_string()),
        ..Default::default()
    }
}

/// A `gate-pass` decided by `identity`.
pub fn gate_pass(
    identity: &Identity,
    subject: Subject,
    evidence: Vec<String>,
    reason: Option<String>,
) -> Result<Decision, FieldError> {
    decision(
        Kind::GatePass,
        "passed",
        subject,
        identity,
        evidence,
        reason,
    )
}

/// A `judgment` about `what`, decided by `identity`.
pub fn judgment(
    identity: &Identity,
    what: String,
    pointer: Option<String>,
    evidence: Vec<String>,
    reason: Option<String>,
) -> Result<Decision, FieldError> {
    let subject = Subject {
        what: Some(what),
        pointer,
        ..Default::default()
    };
    decision(
        Kind::Judgment,
        "settled",
        subject,
        identity,
        evidence,
        reason,
    )
}

/// The `refused` decision for an attempt of `attempted_kind` on `attempted`
/// that had no decider. The decider resolver returns nothing to build it
/// from, so it names no session: a `session` decider with an empty session
/// slot, the agent and run mode koto could observe, and an unknown chain.
pub fn refused_no_decider(
    attempted_kind: Kind,
    attempted: Subject,
) -> Result<Decision, FieldError> {
    let observed = Observed::from_host();
    let mut authority = Authority::for_decider(DeciderKind::Session);
    authority.run_mode = run_mode_slot(&observed, None);
    decision(
        Kind::Refused,
        "refused",
        Subject {
            attempted_kind: Some(attempted_kind.as_str().to_string()),
            attempted: Some(Box::new(attempted)),
            refusal_reason: Some(DECIDER_MISSING.to_string()),
            ..Default::default()
        },
        &Identity {
            decided_by: DecidedBy::session(Vec::new(), agent_slot(&observed, None), false),
            authority,
            chain: Chain::unknown(),
        },
        Vec::new(),
        None,
    )
}

/// Redact `decisions` and check each the way the writer will, returning the
/// copies a session event carries. Called before the event is appended, so
/// a decision the store would refuse stops the command with nothing
/// written.
pub fn prepare(
    store: &Store,
    decisions: &mut [Decision],
) -> Result<Vec<serde_json::Value>, StoreError> {
    let mut out = Vec::with_capacity(decisions.len());
    for d in decisions.iter_mut() {
        redact_decision(d, store.redactor());
        writer::assemble_line(d)?;
        out.push(
            serde_json::to_value(&*d)
                .map_err(|e| FieldError::new("decision", format!("serializing: {}", e)))?,
        );
    }
    Ok(out)
}

/// Append each decision after its session event committed. A failure warns
/// on stderr and is otherwise ignored: the event carries the decision and
/// the next `koto next` on `workflow` repairs it. Returns the identifiers
/// written or already present.
pub fn append_after_event(store: &Store, workflow: &str, decisions: &[Decision]) -> Vec<String> {
    let mut written = Vec::new();
    for d in decisions {
        let result = if fault_injected() {
            Err(StoreError::io(
                "appending to the decision store",
                std::io::Error::other(format!("{} is set", STORE_FAULT_ENV)),
            ))
        } else {
            store.append(d.clone(), |_| Ok(Check::Append))
        };
        match result {
            Ok(WriteOutcome::Appended { id } | WriteOutcome::Skipped { id }) => written.push(id),
            Ok(WriteOutcome::Refused { .. }) => {}
            Err(e) => eprintln!(
                "warning: decision {} was not written to the decision store ({}); the next \
                 `koto next {}` writes it from the session's log",
                d.id, e, workflow
            ),
        }
    }
    written
}

fn fault_injected() -> bool {
    cfg!(debug_assertions) && std::env::var(STORE_FAULT_ENV).is_ok_and(|v| v == "append")
}

/// Append a `refused` decision. A failure warns: the command is refused
/// either way. Returns the identifier when it was written.
pub fn record_refusal(store: &Store, refused: Decision) -> Option<String> {
    match store.append(refused, |_| Ok(Check::Append)) {
        Ok(WriteOutcome::Appended { id } | WriteOutcome::Skipped { id }) => Some(id),
        Ok(WriteOutcome::Refused { .. }) => None,
        Err(e) => {
            eprintln!(
                "warning: the refusal was not written to the decision store: {}",
                e
            );
            None
        }
    }
}

/// What a repair pass did.
#[derive(Debug, Default)]
pub struct Repair {
    /// Identifiers appended from the session's events.
    pub appended: Vec<String>,
    /// Copies skipped: unparseable, invalid, a `reverse` line, or an
    /// identifier whose month isn't its own time's.
    pub skipped: usize,
    /// The error that stopped the pass, when one did; the next tick retries.
    pub error: Option<StoreError>,
}

/// Parse one event copy, or `None` when the repair must skip it.
fn candidate(value: &serde_json::Value) -> Option<Decision> {
    let d: Decision = serde_json::from_value(value.clone()).ok()?;
    if d.kind == Kind::Reverse || d.repaired_from.is_some() || d.import_source.is_some() {
        return None;
    }
    if id_month(&d.id)? != time_month(&d.time)? {
        return None;
    }
    writer::assemble_line(&d).ok()?;
    Some(d)
}

/// Which of `ids` (all naming `month`) are already in that month file. A
/// raw byte search over the one file, so a session's tick reads only the
/// months its own decisions name.
fn present_in_month(
    root: &Path,
    month: &str,
    ids: &BTreeSet<String>,
) -> Result<BTreeSet<String>, StoreError> {
    let path = root.join(format!("{}.jsonl", month));
    let content = match std::fs::read(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(StoreError::io(format!("reading {}", path.display()), e)),
    };
    let needles: Vec<String> = ids
        .iter()
        .map(|id| format!("\"id\":{}", serde_json::to_string(id).unwrap_or_default()))
        .collect();
    let finder = aho_corasick::AhoCorasick::new(&needles)
        .map_err(|e| StoreError::io("building the identifier search", std::io::Error::other(e)))?;
    let ids: Vec<&String> = ids.iter().collect();
    Ok(finder
        .find_overlapping_iter(&content)
        .map(|m| ids[m.pattern().as_usize()].clone())
        .collect())
}

/// Append every decision `events` carry that the store at `store` lacks,
/// each marked `repaired_from: session`. Never returns an error: a failure
/// stops the pass, is reported in [`Repair::error`], and the next tick
/// retries.
pub fn repair(store: &Store, session: &str, events: &[Event]) -> Repair {
    let mut out = Repair::default();
    let mut by_month: BTreeMap<String, Vec<Decision>> = BTreeMap::new();
    for value in events.iter().flat_map(|e| e.payload.decision_records()) {
        match candidate(value) {
            Some(d) => {
                let month = time_month(&d.time).unwrap_or_default();
                by_month.entry(month).or_default().push(d);
            }
            None => out.skipped += 1,
        }
    }
    if by_month.is_empty() {
        return out;
    }
    for (month, decisions) in by_month {
        let ids: BTreeSet<String> = decisions.iter().map(|d| d.id.clone()).collect();
        let present = match present_in_month(store.root(), &month, &ids) {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        };
        let mut seen = BTreeSet::new();
        for mut d in decisions {
            if present.contains(&d.id) || !seen.insert(d.id.clone()) {
                continue;
            }
            d.repaired_from = Some(session.to_string());
            let id = d.id.clone();
            let result = store.append(d, |locked| {
                if locked.find(&id)?.is_some() {
                    Ok(Check::Skip)
                } else {
                    Ok(Check::Append)
                }
            });
            match result {
                Ok(WriteOutcome::Appended { id }) => out.appended.push(id),
                Ok(_) => {}
                // The original append landed after the presence check: the
                // decision is on the record.
                Err(StoreError::IdConflict { .. }) => {}
                Err(e) => {
                    out.error = Some(e);
                    return out;
                }
            }
        }
    }
    out
}

/// Run [`repair`] against the default store, warning on stderr when it
/// stops early. Used by `koto next` at the start of a tick and before a
/// terminal tick's cleanup; it never fails the tick or passes a gate.
/// Returns `false` when a decision the events carry may still be missing
/// from the store, so the terminal tick keeps the session (and its events)
/// for the next tick to retry rather than removing the only copy.
pub fn repair_or_warn(session: &str, events: &[Event]) -> bool {
    if !events
        .iter()
        .any(|e| !e.payload.decision_records().is_empty())
    {
        return true;
    }
    let store = match Store::open_default() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: the decision store couldn't be opened to repair '{}': {}",
                session, e
            );
            return false;
        }
    };
    let r = repair(&store, session, events);
    match r.error {
        Some(e) => {
            eprintln!(
                "warning: repairing the decisions of '{}' stopped ({}); the next `koto next` retries",
                session, e
            );
            false
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_store::record::Sourced;
    use crate::engine::types::EventPayload;

    fn identity() -> Identity {
        Identity {
            decided_by: DecidedBy::session(
                vec![Sourced::observed("sess-1")],
                vec![Sourced::observed("main")],
                true,
            ),
            chain: Chain::unknown(),
            authority: Authority::for_decider(DeciderKind::Session),
        }
    }

    fn store(dir: &Path) -> Store {
        Store::new(
            dir.join("decisions"),
            crate::redact::Redactor::new(Vec::new()),
        )
    }

    fn event(seq: u64, records: Vec<serde_json::Value>) -> Event {
        Event {
            seq,
            timestamp: "2026-10-19T00:00:00.000Z".to_string(),
            event_type: "gate_override_recorded".to_string(),
            payload: EventPayload::GateOverrideRecorded {
                state: "s".into(),
                gate: "g".into(),
                rationale: "r".into(),
                override_applied: serde_json::Value::Null,
                actual_output: serde_json::Value::Null,
                timestamp: "t".into(),
                decision_records: records,
            },
            idempotency_hash: None,
        }
    }

    fn pass() -> Decision {
        gate_pass(
            &identity(),
            gate_subject("wf", "sid", "review", "panel"),
            Vec::new(),
            Some("why".into()),
        )
        .unwrap()
    }

    #[test]
    fn a_lost_append_is_repaired_once_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let mut ds = vec![pass()];
        let copies = prepare(&s, &mut ds).unwrap();
        let events = vec![event(1, copies)];
        let r = repair(&s, "wf", &events);
        assert_eq!(r.appended, vec![ds[0].id.clone()]);
        assert!(r.error.is_none());
        let stored = crate::decision_store::reader::find_decision(s.root(), &ds[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.repaired_from.as_deref(), Some("wf"));
        let mut expected = ds[0].clone();
        expected.repaired_from = Some("wf".into());
        assert_eq!(stored, expected);
        // A second pass finds it present and writes nothing.
        assert!(repair(&s, "wf", &events).appended.is_empty());
    }

    #[test]
    fn a_decision_already_on_the_record_is_not_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let mut ds = vec![pass()];
        let copies = prepare(&s, &mut ds).unwrap();
        assert_eq!(append_after_event(&s, "wf", &ds), vec![ds[0].id.clone()]);
        let r = repair(&s, "wf", &[event(1, copies)]);
        assert!(r.appended.is_empty());
        let stored = crate::decision_store::reader::find_decision(s.root(), &ds[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.repaired_from, None);
    }

    #[test]
    fn reverse_lines_bad_months_and_junk_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let mut reverse = serde_json::to_value(pass()).unwrap();
        reverse["kind"] = "reverse".into();
        let mut wrong_month = pass();
        wrong_month.id = format!("d-202001-{}", "a".repeat(32));
        let mut invalid = serde_json::to_value(pass()).unwrap();
        invalid["outcome"] = "maybe".into();
        let events = vec![event(
            1,
            vec![
                reverse,
                serde_json::to_value(wrong_month).unwrap(),
                invalid,
                serde_json::json!("not a decision"),
            ],
        )];
        let r = repair(&s, "wf", &events);
        assert!(r.appended.is_empty());
        assert_eq!(r.skipped, 4);
        assert!(crate::decision_store::reader::month_files(s.root())
            .unwrap()
            .is_empty());
        assert!(!s
            .root()
            .join(crate::decision_store::REVERSALS_FILE)
            .exists());
    }

    #[test]
    fn prepare_refuses_a_decision_the_writer_would_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let mut bad = pass();
        bad.subject.gate = None;
        let err = prepare(&s, &mut [bad]).unwrap_err();
        assert_eq!(err.field(), Some("gate"));
    }

    #[test]
    fn long_text_is_clipped_on_a_character_boundary() {
        assert_eq!(clip("short"), "short");
        let long = "\u{e9}".repeat(MAX_COPIED_TEXT_BYTES);
        let c = clip(&long);
        assert!(c.len() <= MAX_COPIED_TEXT_BYTES + 3);
        assert!(c.ends_with('\u{2026}'));
    }

    #[test]
    fn a_refusal_names_no_session_and_the_missing_decider() {
        let r = refused_no_decider(Kind::GatePass, gate_subject("wf", "", "s", "g")).unwrap();
        assert_eq!(r.kind, Kind::Refused);
        assert!(r.decided_by.session.is_empty());
        assert_eq!(r.subject.refusal_reason.as_deref(), Some(DECIDER_MISSING));
        assert_eq!(r.subject.attempted_kind.as_deref(), Some("gate-pass"));
        assert!(r.chain.incomplete);
        writer::assemble_line(&r).unwrap();
    }
}
