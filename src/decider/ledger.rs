//! The decider's records, rebuilt from a session's own log.
//!
//! `koto decider report` joins five kinds of record: every consultation,
//! every agent answer to a consulted visit, every `koto next --to` that
//! leaves such a visit unanswered, every decider-check consultation, and
//! every override that moved past a failed decider check. Each of them is
//! already in the session that made it, so [`records_from_session`] reads
//! them from there:
//!
//! | Record | Read from |
//! |---|---|
//! | `consulted` | a `decider_consulted` event |
//! | `answered` | an `evidence_submitted` event paired with its visit's `decider_consulted`, as the run journal's `decider_answered` relay pairs them |
//! | `directed_exit` | a `directed_transition` event leaving a consulted visit unanswered |
//! | `checked` | a `decider_checked` event |
//! | `check_overridden` | a `gate_override_recorded` event and the `decider_checked` events of its visit |
//!
//! The host-wide ledger file koto once appended these records to
//! (`~/.koto/_decider_ledger.jsonl`) is no longer used: nothing writes,
//! reads or deletes it, and one that still exists is left as it is.
//!
//! Records pair on `(session_id, visit_seq)`, because session names are
//! reused across runs. A session whose header has no `session_id` gives
//! records with `None`, which are counted but never paired.
//!
//! Everything here is pure: the caller reads the log.

use std::collections::BTreeMap;

use super::check::DeciderCheck;
use super::record::{ConsultationOutcome, DeciderConsultation};
use crate::engine::decider::{prior_consultation, visit_start_index};
use crate::engine::persistence::arrival_index;
use crate::engine::types::{Event, EventPayload};

/// The keys every record kind carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEnvelope {
    /// The timestamp of the event the record was read from, RFC 3339 UTC.
    pub at: String,
    /// The session name. Names are reused across runs; don't join on it.
    pub session: String,
    /// The session header's `session_id`, or `None` when the header has
    /// none.
    pub session_id: Option<String>,
}

impl RecordEnvelope {
    /// An envelope for an event recorded `at`. An empty `session_id`
    /// becomes `None`.
    pub fn new(at: &str, session: &str, session_id: Option<&str>) -> Self {
        RecordEnvelope {
            at: at.to_string(),
            session: session.to_string(),
            session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
        }
    }
}

/// One consultation: the envelope plus every [`DeciderConsultation`] field.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsultedRecord {
    pub envelope: RecordEnvelope,
    pub consultation: DeciderConsultation,
}

/// The agent's answer on a visit whose consultation wasn't applied.
#[derive(Debug, Clone, PartialEq)]
pub struct AnsweredRecord {
    pub envelope: RecordEnvelope,
    /// The state the evidence was submitted on.
    pub state: String,
    /// The consultation's `visit_seq`, which this answer pairs with.
    pub visit_seq: u64,
    /// The submitted value of each field the consultation covered.
    pub values: BTreeMap<String, serde_json::Value>,
}

/// `koto next --to` leaving a visit whose consultation wasn't applied,
/// before the agent answered it (koto#254).
///
/// It is not an answer: the agent's value for the declared fields is
/// unknown, so it never forms a paired observation. It is kept because
/// `--to` is how an agent overrides a routing decision, so these are the
/// visits where it most likely disagreed with the decider.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectedExitRecord {
    pub envelope: RecordEnvelope,
    /// The state the agent left.
    pub state: String,
    /// The consultation's `visit_seq`, which this exit pairs with.
    pub visit_seq: u64,
    /// The state `--to` named.
    pub target: String,
}

/// One consultation of one decider-check criterion: the envelope plus every
/// [`DeciderCheck`] field (DESIGN-koto-decider-checks.md, Decision 5).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedRecord {
    pub envelope: RecordEnvelope,
    pub check: DeciderCheck,
}

/// What an override of a blocking decider check moved past, for one
/// criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckOverrideKind {
    /// The criterion was failing on a verdict: the override is a candidate
    /// false fail for whoever judges the criterion's accuracy.
    CandidateFalseFail,
    /// The criterion was unanswered: the override moved past a checker
    /// fault, not a judgment. A missing verdict no longer blocks, so no
    /// override earns it; the report keeps its column.
    OverriddenUnanswered,
}

/// `koto overrides record` moving past one blocking decider-check criterion.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOverriddenRecord {
    pub envelope: RecordEnvelope,
    pub state: String,
    /// The visit the overridden consultation belongs to.
    pub visit_seq: u64,
    pub gate: String,
    pub rule_id: String,
    pub declaration_hash: String,
    pub override_kind: CheckOverrideKind,
}

/// One decider record.
#[derive(Debug, Clone, PartialEq)]
pub enum LedgerRecord {
    Consulted(ConsultedRecord),
    Answered(AnsweredRecord),
    DirectedExit(DirectedExitRecord),
    Checked(CheckedRecord),
    CheckOverridden(CheckOverriddenRecord),
}

impl LedgerRecord {
    /// The record's kind.
    pub fn kind(&self) -> &'static str {
        match self {
            LedgerRecord::Consulted(_) => "consulted",
            LedgerRecord::Answered(_) => "answered",
            LedgerRecord::DirectedExit(_) => "directed_exit",
            LedgerRecord::Checked(_) => "checked",
            LedgerRecord::CheckOverridden(_) => "check_overridden",
        }
    }
}

/// What one session's log holds for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionRecords {
    /// Records in log order.
    pub records: Vec<LedgerRecord>,
    /// `decider_consulted` and `decider_checked` events whose payload this
    /// build can't read (they parse as `Unknown`).
    pub malformed: u64,
}

/// Rebuild the decider records of session `session`, whose header carries
/// `session_id`, from its `events` in log order.
///
/// At each event the log before it is the log the command that appended it
/// saw, so each record follows the rule its old writer followed:
///
/// - an agent's `evidence_submitted` on a visit whose consultation (the
///   visit's first `decider_consulted`) wasn't applied is an `answered`
///   record holding the submitted values of the fields the consultation
///   covered, and none when it holds no such field;
/// - a `directed_transition` from such a visit, to another state, before
///   any submission answered one of those fields is a `directed_exit`; a
///   `--to` naming the current state starts a new lap, not an exit;
/// - a `gate_override_recorded` is one `check_overridden` per criterion
///   [`override_records`] finds.
pub fn records_from_session(
    session: &str,
    session_id: Option<&str>,
    events: &[Event],
) -> SessionRecords {
    let mut out = SessionRecords::default();
    // Only a consulted session can hold an answer or a directed exit, so
    // the visit lookups below run only for one.
    let consulted = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::DeciderConsulted(_)));
    for (i, e) in events.iter().enumerate() {
        let before = &events[..i];
        let envelope = || RecordEnvelope::new(&e.timestamp, session, session_id);
        match &e.payload {
            EventPayload::DeciderConsulted(c) => {
                out.records.push(LedgerRecord::Consulted(ConsultedRecord {
                    envelope: envelope(),
                    consultation: c.clone(),
                }));
            }
            EventPayload::EvidenceSubmitted {
                state,
                fields,
                source: None,
                ..
            } if consulted => {
                let Some(c) = unapplied_consultation(before, state) else {
                    continue;
                };
                let values: BTreeMap<String, serde_json::Value> = fields
                    .iter()
                    .filter(|(f, _)| c.fields.contains_key(*f))
                    .map(|(f, v)| (f.clone(), v.clone()))
                    .collect();
                if values.is_empty() {
                    continue;
                }
                out.records.push(LedgerRecord::Answered(AnsweredRecord {
                    envelope: envelope(),
                    state: state.clone(),
                    visit_seq: c.visit_seq,
                    values,
                }));
            }
            EventPayload::DirectedTransition { from, to, .. } if consulted && from != to => {
                let Some(c) = unapplied_consultation(before, from) else {
                    continue;
                };
                if answered_in_visit(before, from, c) {
                    continue;
                }
                out.records
                    .push(LedgerRecord::DirectedExit(DirectedExitRecord {
                        envelope: envelope(),
                        state: from.clone(),
                        visit_seq: c.visit_seq,
                        target: to.clone(),
                    }));
            }
            EventPayload::DeciderChecked(check) => {
                out.records.push(LedgerRecord::Checked(CheckedRecord {
                    envelope: envelope(),
                    check: check.clone(),
                }));
            }
            EventPayload::GateOverrideRecorded {
                state,
                gate,
                actual_output,
                ..
            } => {
                out.records.extend(override_records(
                    before,
                    state,
                    gate,
                    actual_output,
                    &envelope(),
                ));
            }
            EventPayload::Unknown { type_name, .. }
                if type_name == "decider_consulted" || type_name == "decider_checked" =>
            {
                out.malformed += 1;
            }
            _ => {}
        }
    }
    out
}

/// The current visit's consultation of `state` in `before`, when it wasn't
/// applied.
fn unapplied_consultation<'a>(before: &'a [Event], state: &str) -> Option<&'a DeciderConsultation> {
    prior_consultation(before, state).filter(|c| c.outcome != ConsultationOutcome::Applied)
}

/// Whether an `evidence_submitted` in the current visit to `state` already
/// answered a field consultation `c` covered.
fn answered_in_visit(before: &[Event], state: &str, c: &DeciderConsultation) -> bool {
    let Some(start) = visit_start_index(before, state) else {
        return false;
    };
    before[start.index + 1..].iter().any(|e| match &e.payload {
        EventPayload::EvidenceSubmitted {
            state: s, fields, ..
        } => s == state && fields.keys().any(|f| c.fields.contains_key(f)),
        _ => false,
    })
}

/// The `check_overridden` records for an override of the decider check
/// `gate` in `state`, given the log `before` it: one `candidate_false_fail`
/// per criterion the check's last output (`actual_output`) lists as failed
/// by the decider. A criterion the agent answered `fail` is listed under
/// `agent_answered` too and gets none: an override of the agent's own
/// judgment says nothing about the decider. A criterion waiting on the
/// agent is never under `failed`, so it gets none either. Each record takes
/// its visit and declaration hash from the latest `decider_checked` for
/// that criterion in the current visit to `state`, the visit the override
/// applies to; a reused verdict appended none, but the consultation it
/// reused is in the same visit. A criterion with no such record in this
/// visit gets none, even when an earlier visit has one: that consultation
/// isn't what the override moved past. A gate that isn't a decider check
/// has no `decider_checked` events, so its overrides give none.
pub fn override_records(
    before: &[Event],
    state: &str,
    gate: &str,
    actual_output: &serde_json::Value,
    envelope: &RecordEnvelope,
) -> Vec<LedgerRecord> {
    let visit = match arrival_index(before, state) {
        Some(i) => &before[i + 1..],
        None => before,
    };
    let agent_answered: Vec<&str> = actual_output["agent_answered"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    let failed = actual_output["failed"].as_array();
    for rule_id in failed
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|id| !agent_answered.contains(id))
    {
        let latest = visit.iter().rev().find_map(|e| match &e.payload {
            EventPayload::DeciderChecked(c)
                if c.state == state && c.gate == gate && c.rule_id == rule_id =>
            {
                Some((c.visit_seq, c.declaration_hash.as_str()))
            }
            _ => None,
        });
        if let Some((visit_seq, declaration_hash)) = latest {
            out.push(LedgerRecord::CheckOverridden(CheckOverriddenRecord {
                envelope: envelope.clone(),
                state: state.to_string(),
                visit_seq,
                gate: gate.to_string(),
                rule_id: rule_id.to_string(),
                declaration_hash: declaration_hash.to_string(),
                override_kind: CheckOverrideKind::CandidateFalseFail,
            }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// Log events from JSON lines, numbered from 1.
    fn log(lines: &[Value]) -> Vec<Event> {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut l = l.clone();
                l["seq"] = json!(i as u64 + 1);
                l["timestamp"] = json!("2026-01-01T00:00:00Z");
                serde_json::from_value(l).unwrap()
            })
            .collect()
    }

    fn init() -> Value {
        json!({"type": "workflow_initialized",
            "payload": {"template_path": "t.json", "variables": {}}})
    }

    fn arrive(from: Option<&str>, to: &str) -> Value {
        json!({"type": "transitioned",
            "payload": {"from": from, "to": to, "condition_type": "auto"}})
    }

    fn consulted(visit_seq: u64, outcome: &str) -> Value {
        json!({"type": "decider_consulted", "payload": {
            "state": "review", "visit_seq": visit_seq, "provider": "jev",
            "model": "m", "outcome": outcome, "latency_ms": 1,
            "directive_bytes": 1, "endpoint_origin": "default",
            "fields": {"verdict": {"declaration_hash": "h",
                "modes": {"proceed": "shadow", "exit": "shadow"},
                "at_threshold": false}}}})
    }

    fn evidence(fields: Value) -> Value {
        json!({"type": "evidence_submitted",
            "payload": {"state": "review", "fields": fields}})
    }

    fn directed(from: &str, to: &str) -> Value {
        json!({"type": "directed_transition", "payload": {"from": from, "to": to}})
    }

    fn checked(visit_seq: u64, hash: &str) -> Value {
        json!({"type": "decider_checked", "payload": {
            "state": "review", "visit_seq": visit_seq, "gate": "comments",
            "rule_id": "r", "rule_ref": "ref", "declaration_hash": hash,
            "mode": "veto", "threshold": 0.9, "outcome": "fail", "blocked": true,
            "provider": "jev", "model": "m", "attempts": 1, "latency_ms": 1}})
    }

    fn override_of(output: Value) -> Value {
        json!({"type": "gate_override_recorded", "payload": {
            "state": "review", "gate": "comments", "rationale": "x",
            "override_applied": {}, "actual_output": output,
            "timestamp": "2026-01-01T00:00:00Z"}})
    }

    fn kinds(events: &[Event]) -> Vec<&'static str> {
        records_from_session("wf", Some("u"), events)
            .records
            .iter()
            .map(LedgerRecord::kind)
            .collect()
    }

    fn overridden(events: &[Event]) -> Vec<(u64, String)> {
        override_records(
            events,
            "review",
            "comments",
            &json!({"failed": ["r"], "unanswered": [], "error": ""}),
            &RecordEnvelope::new("t", "wf", None),
        )
        .into_iter()
        .map(|r| match r {
            LedgerRecord::CheckOverridden(c) => {
                assert_eq!(c.override_kind, CheckOverrideKind::CandidateFalseFail);
                (c.visit_seq, c.declaration_hash)
            }
            other => panic!("unexpected record {:?}", other),
        })
        .collect()
    }

    #[test]
    fn every_kind_is_read_from_its_event() {
        let events = log(&[
            init(),
            arrive(None, "review"),
            consulted(2, "not_applied"),
            evidence(json!({"verdict": "exit", "notes": "free text"})),
            checked(2, "h"),
            override_of(json!({"failed": ["r"]})),
            directed("review", "done"),
        ]);
        let read = records_from_session("wf", Some("u"), &events);
        assert_eq!(read.malformed, 0);
        assert_eq!(
            read.records
                .iter()
                .map(LedgerRecord::kind)
                .collect::<Vec<_>>(),
            vec!["consulted", "answered", "checked", "check_overridden"]
        );
        let LedgerRecord::Answered(a) = &read.records[1] else {
            panic!("kind");
        };
        assert_eq!(a.visit_seq, 2);
        assert_eq!(a.values, [("verdict".to_string(), json!("exit"))].into());
        assert_eq!(a.envelope.session_id.as_deref(), Some("u"));
        assert_eq!(a.envelope.at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn answered_needs_an_unapplied_consultation_and_a_consulted_field() {
        let answer = evidence(json!({"verdict": "exit"}));
        // No consultation on the visit.
        assert_eq!(
            kinds(&log(&[init(), arrive(None, "review"), answer.clone()])),
            Vec::<&str>::new()
        );
        // Applied.
        let applied = log(&[
            init(),
            arrive(None, "review"),
            consulted(2, "applied"),
            answer.clone(),
        ]);
        assert_eq!(kinds(&applied), vec!["consulted"]);
        // A consultation from an earlier visit doesn't count.
        let earlier = log(&[
            init(),
            arrive(None, "review"),
            consulted(2, "not_applied"),
            arrive(Some("review"), "review"),
            answer.clone(),
        ]);
        assert_eq!(kinds(&earlier), vec!["consulted"]);
        // Nothing the consultation covered was submitted.
        let undeclared = log(&[
            init(),
            arrive(None, "review"),
            consulted(2, "error"),
            evidence(json!({"notes": "x"})),
        ]);
        assert_eq!(kinds(&undeclared), vec!["consulted"]);
        // Two submissions in one visit: two answers.
        let twice = log(&[
            init(),
            arrive(None, "review"),
            consulted(2, "not_applied"),
            answer.clone(),
            answer,
        ]);
        assert_eq!(kinds(&twice), vec!["consulted", "answered", "answered"]);
    }

    #[test]
    fn an_empty_header_session_id_reads_none() {
        let events = log(&[init(), arrive(None, "review"), consulted(2, "not_applied")]);
        for id in [None, Some("")] {
            let read = records_from_session("wf", id, &events);
            let LedgerRecord::Consulted(c) = &read.records[0] else {
                panic!("kind");
            };
            assert_eq!(c.envelope.session_id, None);
        }
    }

    #[test]
    fn a_directed_exit_needs_an_unanswered_consulted_visit_left_for_another_state() {
        let base = || vec![init(), arrive(None, "review"), consulted(2, "not_applied")];
        let with = |extra: Vec<Value>| {
            let mut v = base();
            v.extend(extra);
            kinds(&log(&v))
        };
        assert_eq!(
            with(vec![directed("review", "done")]),
            vec!["consulted", "directed_exit"]
        );
        // A self-loop is a lap, not an exit.
        assert_eq!(with(vec![directed("review", "review")]), vec!["consulted"]);
        // Answered first: already a paired observation.
        assert_eq!(
            with(vec![
                evidence(json!({"verdict": "exit"})),
                directed("review", "done")
            ]),
            vec!["consulted", "answered"]
        );
        // An unconsulted visit.
        let unconsulted = log(&[init(), arrive(None, "review"), directed("review", "done")]);
        assert_eq!(kinds(&unconsulted), Vec::<&str>::new());
        // Only the consulted visit, not a later one.
        assert_eq!(
            with(vec![
                directed("review", "done"),
                directed("done", "review"),
                directed("review", "done"),
            ]),
            vec!["consulted", "directed_exit"]
        );
    }

    #[test]
    fn an_unreadable_decider_event_counts_as_malformed() {
        let events = log(&[
            init(),
            json!({"type": "decider_consulted", "payload": {"state": "review"}}),
            json!({"type": "decider_checked", "payload": {"visit_seq": "x"}}),
            json!({"type": "something_else", "payload": {}}),
        ]);
        let read = records_from_session("wf", Some("u"), &events);
        assert!(read.records.is_empty());
        assert_eq!(read.malformed, 2);
    }

    #[test]
    fn an_override_names_the_consultation_of_the_current_visit_only() {
        // Visit 1 (seq 2) failed r and was left; visit 2 (seq 5) failed it
        // again. The override names visit 2's consultation.
        let two_visits = log(&[
            init(),
            arrive(None, "review"),
            checked(2, "old"),
            arrive(Some("review"), "work"),
            arrive(Some("work"), "review"),
            checked(5, "new"),
        ]);
        assert_eq!(overridden(&two_visits), vec![(5, "new".to_string())]);

        // The current visit has no consultation of r: an earlier visit's is
        // not what the override moved past, so nothing is read. Looking
        // across the whole log would name visit 1 here.
        let stale = log(&[
            init(),
            arrive(None, "review"),
            checked(2, "old"),
            arrive(Some("review"), "work"),
            arrive(Some("work"), "review"),
        ]);
        assert_eq!(overridden(&stale), vec![]);

        // One visit: its consultation.
        let one = log(&[init(), arrive(None, "review"), checked(2, "only")]);
        assert_eq!(overridden(&one), vec![(2, "only".to_string())]);
    }

    #[test]
    fn an_override_skips_criteria_the_agent_answered() {
        let events = log(&[init(), arrive(None, "review"), checked(2, "h")]);
        let records = override_records(
            &events,
            "review",
            "comments",
            &json!({"failed": ["r"], "unanswered": [], "awaiting": [],
                "agent_answered": ["r"], "error": ""}),
            &RecordEnvelope::new("t", "wf", None),
        );
        assert!(records.is_empty());
    }

    /// No source file names the old host ledger file except in a comment
    /// saying it is no longer used.
    #[test]
    fn no_source_file_names_the_host_ledger() {
        let needle = ["_decider", "_ledger.jsonl"].concat();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lines: Vec<&str> = body.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.contains(&needle) {
                        continue;
                    }
                    let comment = line.trim_start().starts_with("//");
                    // The comment's sentence may end on the next line.
                    let says = lines[i..lines.len().min(i + 3)]
                        .iter()
                        .any(|l| l.contains("no longer used"));
                    if !(comment && says) {
                        offenders.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "{:?}", offenders);
    }
}
