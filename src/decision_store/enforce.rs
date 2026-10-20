//! The per-tick check for reversed gate passes.
//!
//! `koto next` asks, on every tick, whether a gate pass this workflow
//! relied on has been reversed. The question is answered from the reversal
//! log alone: each reversal line carries the subject key of the decision it
//! reversed, and a `gate-pass` key names the workflow and its session id.
//! A workflow with no reversed gate passes opens no month file.
//!
//! A reversal blocks the workflow until either
//!
//! - the workflow is rewound, after the reversal, to the gated state or to a
//!   state it had been in before it last entered the gated state; or
//! - a new `gate-pass` for the same workflow session, state and gate is
//!   recorded, after the reversal, by a decider [`authorize`] lets reverse
//!   the reverser, with standing that meets the reversal's own.
//!
//! Decisions record their time to the second, so a pass in the same second
//! as the reversal can't be told from one just before it. It doesn't clear
//! the block: an unclear order keeps the workflow stopped rather than
//! letting it through. A rewind's event time has milliseconds, and one in
//! the same second as the reversal counts as after it, since a rewind only
//! moves the workflow back.
//!
//! The reversal log fails closed: a complete line in it that can't be read,
//! or a reversal line with no copy of its original, blocks the workflow as
//! `decision_store_unreadable` rather than letting a pass through.

use std::collections::HashMap;
use std::path::Path;

use super::authority::{authorize, Standing};
use super::reader::{self, Needles, StrictError};
use super::record::{gate_pass_key, Decision, Kind};
use super::time_month;
use crate::engine::types::{Event, EventPayload};

/// One reversed gate pass that still blocks the workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocking {
    /// The reversal's identifier.
    pub reversal: String,
    /// The reversal's standing.
    pub standing: Standing,
    /// When the reversal was recorded.
    pub time: String,
    /// The state the reversed pass let the workflow past.
    pub state: String,
    /// The gate it passed.
    pub gate: String,
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing blocks the workflow.
    Clear,
    /// Each reversed pass still in effect.
    Blocked(Vec<Blocking>),
    /// The reversal log couldn't be read; the message says why without
    /// quoting any line.
    Unreadable(String),
}

/// Check a workflow named `workflow`, whose state file's session id is
/// `session_id`, against the store at `root`, using its event log to find
/// rewinds. A workflow with an empty session id can't be named by a gate
/// pass's subject key, so nothing blocks it.
pub fn check(root: &Path, workflow: &str, session_id: &str, events: &[Event]) -> Verdict {
    if session_id.is_empty() {
        return Verdict::Clear;
    }
    let reversals = match reader::read_reversals_strict(root) {
        Ok(r) => r,
        Err(StrictError::Unreadable { line }) => {
            return Verdict::Unreadable(format!(
                "line {} of the reversal log is not a decision koto can read",
                line
            ))
        }
        Err(StrictError::Store(e)) => return Verdict::Unreadable(e.to_string()),
    };

    let mut relevant: Vec<(&Decision, String, String)> = Vec::new();
    for r in &reversals {
        if r.kind != Kind::Reverse {
            continue;
        }
        let Some(original) = &r.original else {
            return Verdict::Unreadable(format!(
                "reversal {} carries no copy of the decision it reverses",
                r.id
            ));
        };
        if original.kind != Kind::GatePass {
            continue;
        }
        let Some([w, sid, state, gate]) = original.subject_key.as_deref().and_then(gate_pass_key)
        else {
            return Verdict::Unreadable(format!(
                "reversal {} of a gate pass carries no readable subject key",
                r.id
            ));
        };
        if w == workflow && sid == session_id {
            relevant.push((r, state, gate));
        }
    }
    if relevant.is_empty() {
        return Verdict::Clear;
    }

    let mut blocking = Vec::new();
    let mut passes: Option<Vec<Decision>> = None;
    for (r, state, gate) in relevant {
        let Some(standing) = r.authority.standing.as_deref().and_then(Standing::parse) else {
            return Verdict::Unreadable(format!("reversal {} carries no standing", r.id));
        };
        if rewound_past(events, &r.time, &state) {
            continue;
        }
        let passes = match &passes {
            Some(p) => p,
            None => match later_gate_passes(root, session_id, &r.time) {
                Ok(p) => passes.insert(p),
                Err(e) => return Verdict::Unreadable(e.to_string()),
            },
        };
        let cleared = passes.iter().any(|g| {
            g.time.as_str() > r.time.as_str()
                && Some(&g.id) != r.reverses.as_ref()
                && g.subject.workflow.as_deref() == Some(workflow)
                && g.subject.workflow_session_id.as_deref() == Some(session_id)
                && g.subject.state.as_deref() == Some(state.as_str())
                && g.subject.gate.as_deref() == Some(gate.as_str())
                && authorize(&r.decided_by, &r.chain, &g.decided_by)
                    .is_ok_and(|s| s.meets(standing))
        });
        if !cleared {
            blocking.push(Blocking {
                reversal: r.id.clone(),
                standing,
                time: r.time.clone(),
                state,
                gate,
            });
        }
    }
    if blocking.is_empty() {
        Verdict::Clear
    } else {
        Verdict::Blocked(blocking)
    }
}

/// Every `gate-pass` naming `session_id`, from the month of `since` on.
fn later_gate_passes(
    root: &Path,
    session_id: &str,
    since: &str,
) -> Result<Vec<Decision>, super::StoreError> {
    let from = time_month(since).unwrap_or_default();
    let quoted = serde_json::to_string(session_id).unwrap_or_default();
    let needles = Needles::new([
        b"\"kind\":\"gate-pass\"".to_vec(),
        quoted.as_bytes()[1..quoted.len() - 1].to_vec(),
    ]);
    let mut out = Vec::new();
    for (month, path) in reader::month_files(root)? {
        if month < from {
            continue;
        }
        out.extend(
            reader::read_file(&path, &needles)?
                .decisions
                .into_iter()
                .filter(|d| d.kind == Kind::GatePass),
        );
    }
    Ok(out)
}

/// An event time cut to the second, the precision decisions record, so
/// the two compare as strings.
fn to_second(ts: &str) -> &str {
    ts.get(..19).unwrap_or(ts)
}

/// Whether, at or after `since`, the workflow was rewound to `state` or to
/// a state it had entered before it last entered `state`.
fn rewound_past(events: &[Event], since: &str, state: &str) -> bool {
    let since = to_second(since);
    let mut entered: HashMap<&str, u64> = HashMap::new();
    for e in events {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. }
            | EventPayload::DirectedTransition { to, .. }
            | EventPayload::Rewound { to, .. } => to.as_str(),
            _ => continue,
        };
        if matches!(e.payload, EventPayload::Rewound { .. }) && to_second(&e.timestamp) >= since {
            let reached_gate = to == state;
            let before_gate = match (entered.get(to), entered.get(state)) {
                (Some(t), Some(g)) => t < g,
                _ => false,
            };
            if reached_gate || before_gate {
                return true;
            }
        }
        entered.insert(to, e.seq);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, ts: &str, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: ts.to_string(),
            event_type: payload.type_name().to_string(),
            idempotency_hash: None,
            payload,
        }
    }

    fn tr(seq: u64, ts: &str, from: Option<&str>, to: &str) -> Event {
        ev(
            seq,
            ts,
            EventPayload::Transitioned {
                from: from.map(str::to_string),
                to: to.to_string(),
                condition_type: "auto".into(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
        )
    }

    fn rw(seq: u64, ts: &str, from: &str, to: &str) -> Event {
        ev(
            seq,
            ts,
            EventPayload::Rewound {
                from: from.into(),
                to: to.into(),
                rationale: None,
            },
        )
    }

    const T0: &str = "2026-10-20T10:00:00.000Z";
    const T2: &str = "2026-10-20T10:00:02.500Z";
    const T3: &str = "2026-10-20T10:00:03.000Z";

    fn path() -> Vec<Event> {
        vec![
            tr(1, T0, None, "a"),
            tr(2, T0, Some("a"), "gated"),
            tr(3, T0, Some("gated"), "b"),
            tr(4, T0, Some("b"), "c"),
        ]
    }

    #[test]
    fn a_rewind_before_the_reversal_does_not_clear() {
        let mut events = path();
        events.push(rw(5, T0, "c", "gated"));
        assert!(!rewound_past(&events, "2026-10-20T10:00:01Z", "gated"));
    }

    #[test]
    fn a_rewind_to_the_gated_state_clears() {
        let mut events = path();
        events.push(rw(5, T2, "c", "b"));
        assert!(!rewound_past(&events, "2026-10-20T10:00:01Z", "gated"));
        events.push(rw(6, T3, "b", "gated"));
        assert!(rewound_past(&events, "2026-10-20T10:00:01Z", "gated"));
    }

    #[test]
    fn a_rewind_to_a_state_before_the_gate_clears() {
        let mut events = path();
        events.push(rw(5, T2, "c", "a"));
        assert!(rewound_past(&events, "2026-10-20T10:00:01Z", "gated"));
    }

    #[test]
    fn a_rewind_in_the_same_second_as_the_reversal_counts_as_after_it() {
        let mut events = path();
        events.push(rw(5, "2026-10-20T10:00:01.900Z", "c", "gated"));
        assert!(rewound_past(&events, "2026-10-20T10:00:01Z", "gated"));
    }

    #[test]
    fn an_empty_session_id_is_never_blocked() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check(dir.path(), "wf", "", &path()), Verdict::Clear);
    }

    #[test]
    fn no_store_is_clear() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check(&dir.path().join("absent"), "wf", "sid", &path()),
            Verdict::Clear
        );
    }
}
