//! The decisions koto's own commands record alongside the events they
//! already append (DESIGN-decision-record.md, Decision 3; PRD R24, R25).
//!
//! `koto overrides record`, `koto next --to` past a gate that hasn't passed
//! and `koto decider answer` with `pass` each record a `gate-pass`. Its
//! decider is the one `--by` names, else the session decider marked
//! `defaulted`, else none: the pass is refused with `decider_missing`, the
//! command appends nothing, and a `refused` decision is recorded whose
//! decider is marked `unestablished`. The gate-passing commands write their
//! event and the decision in one critical section, the event carrying the
//! body as `decision` ([`record_with_event`]), so a writer that stops
//! between the two leaves the event for the session's next command to
//! repair the key from ([`super::store::repair`]).

use std::collections::BTreeMap;

use serde_json::Value;

use super::body::{self, BasisRef, DecisionBody, Request, SessionRef};
use super::chain::{self, ChainSnapshot};
use super::decider::{self, Authority, Decider, Flags, Observed};
use super::{codes, store, DecisionError, FORMAT_VERSION};
use crate::engine::types::EventPayload;
use crate::session::{Backend, SessionBackend};

/// The code a command reports when the event it appends inside the
/// critical section couldn't be appended. Nothing was published.
pub const EVENT_APPEND_FAILED: &str = "event_append_failed";

/// Where a decision is recorded: the deciding session, its id, and the
/// state and log position it was taken at.
#[derive(Debug, Clone, Copy)]
pub struct At<'a> {
    pub session: &'a str,
    pub session_id: &'a str,
    pub state: &'a str,
    pub log_seq: u64,
}

impl At<'_> {
    /// The subject of a `gate-pass` at `gate` here.
    pub fn gate_pass_subject(&self, gate: &str) -> Vec<(String, String)> {
        vec![
            ("workflow".to_string(), self.session.to_string()),
            ("state".to_string(), self.state.to_string()),
            ("gate".to_string(), gate.to_string()),
        ]
    }
}

/// A decision checked and stamped with everything but its id and time.
#[derive(Debug, Clone)]
pub struct Prepared {
    session: SessionRef,
    state: String,
    log_seq: u64,
    kind: String,
    outcome: String,
    subject: BTreeMap<String, Value>,
    decider: Decider,
    authority: Authority,
    chain: ChainSnapshot,
    basis: Vec<BasisRef>,
    evidence: Vec<Value>,
    reason: Option<String>,
}

impl Prepared {
    /// Check `req` as `koto decisions record --kind` checks a caller's
    /// request, and stamp the authority and chain a fresh record gets.
    pub fn new(
        backend: &Backend,
        at: At<'_>,
        req: Request,
        decider: Decider,
        flags: &Flags,
        observed: &Observed,
    ) -> Result<Self, DecisionError> {
        let subject = body::validate(&req)?;
        let basis = body::parse_basis(&req.basis)?;
        let evidence = body::parse_evidence(&req.evidence)?;
        let run_mode = decider::run_mode(flags, observed)?;
        let anchor = match &decider {
            Decider::Session { unestablished, .. } if *unestablished => None,
            _ => observed.anchor.as_deref(),
        };
        let authority = decider::authority(
            backend,
            &decider,
            anchor,
            chain::dispatcher(backend, at.session),
            run_mode,
        );
        Ok(Self {
            session: SessionRef {
                name: at.session.to_string(),
                id: at.session_id.to_string(),
            },
            state: at.state.to_string(),
            log_seq: at.log_seq,
            kind: req.kind,
            outcome: req.outcome,
            subject,
            decider,
            authority,
            chain: chain::snapshot(backend, at.session),
            basis,
            evidence,
            reason: req.reason,
        })
    }

    fn body(&self, id: &str) -> DecisionBody {
        DecisionBody {
            v: FORMAT_VERSION,
            id: id.to_string(),
            session: self.session.clone(),
            time: crate::engine::types::now_iso8601(),
            state: self.state.clone(),
            log_seq: self.log_seq,
            kind: self.kind.clone(),
            outcome: self.outcome.clone(),
            subject: self.subject.clone(),
            decider: self.decider.clone(),
            authority: self.authority.clone(),
            chain: self.chain.clone(),
            basis: self.basis.clone(),
            evidence: self.evidence.clone(),
            reason: self.reason.clone(),
            tool_call: None,
        }
    }
}

/// Write `prepared` and, inside the same critical section, append the
/// event `make_event` builds around the body, after the body is built and
/// before its key is published.
pub fn record_with_event(
    backend: &Backend,
    prepared: &Prepared,
    timestamp: &str,
    mut make_event: impl FnMut(Value) -> EventPayload,
) -> Result<DecisionBody, DecisionError> {
    let session = prepared.session.name.as_str();
    let mut commit = |body: &DecisionBody| -> Result<(), DecisionError> {
        let value = serde_json::to_value(body)
            .map_err(|e| DecisionError::new(EVENT_APPEND_FAILED, e.to_string()))?;
        backend
            .append_event(session, &make_event(value), timestamp)
            .map_err(|e| DecisionError::new(EVENT_APPEND_FAILED, e.to_string()))
    };
    store::write_with(
        backend,
        session,
        |id| Ok(prepared.body(id)),
        Some(&mut commit),
    )
}

/// Write `prepared` on its own, after the command's event is appended.
pub fn record(backend: &Backend, prepared: &Prepared) -> Result<DecisionBody, DecisionError> {
    store::write(backend, &prepared.session.name, |id| Ok(prepared.body(id)))
}

/// Resolve the decider of a gate pass at `gate` (R24). When nothing
/// establishes one, record the `refused` decision and return the
/// `decider_missing` refusal for the command to print; the command then
/// appends nothing. A malformed flag is refused with nothing recorded.
pub fn resolve_or_refuse(
    backend: &Backend,
    at: At<'_>,
    gate: &str,
    flags: &Flags,
    observed: &Observed,
) -> Result<Decider, DecisionError> {
    decider::run_mode(flags, observed)?;
    match decider::resolve_gate_pass(at.session, flags, observed) {
        Err(e) if e.code == codes::DECIDER_MISSING => {
            record_refused(backend, at, gate, flags, observed, &e.message);
            Err(e)
        }
        other => other,
    }
}

/// Record the `refused` decision of a gate pass with no decider: the
/// attempted `gate-pass` subject as a JSON string, the refusal as the
/// reason, and the unestablished session decider. A failed write warns;
/// the command's refusal stands either way.
fn record_refused(
    backend: &Backend,
    at: At<'_>,
    gate: &str,
    flags: &Flags,
    observed: &Observed,
    message: &str,
) {
    let attempted: serde_json::Map<String, Value> = at
        .gate_pass_subject(gate)
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let req = Request {
        kind: "refused".to_string(),
        outcome: "refused".to_string(),
        subject: vec![
            ("kind".to_string(), "gate-pass".to_string()),
            ("subject".to_string(), Value::Object(attempted).to_string()),
        ],
        reason: Some(message.to_string()),
        ..Request::default()
    };
    let written = Prepared::new(
        backend,
        at,
        req,
        decider::unestablished(at.session),
        flags,
        observed,
    )
    .and_then(|p| record(backend, &p));
    if let Err(e) = written {
        eprintln!(
            "warning: the refused gate pass in '{}' wasn't recorded as a decision: {}",
            at.session, e.message
        );
    }
}

/// The decider of a `judgment` a command records for its caller (R25):
/// the one `--by` names, else the session.
pub fn judgment_decider(
    session: &str,
    flags: &Flags,
    observed: &Observed,
) -> Result<Decider, DecisionError> {
    if flags.by.is_some() {
        return decider::resolve(session, flags, observed);
    }
    let flags = Flags {
        by: Some("session".to_string()),
        ..flags.clone()
    };
    decider::resolve(session, &flags, observed)
}

/// A `judgment` request about `what`.
pub fn judgment(what: String, reason: Option<String>, evidence: Vec<String>) -> Request {
    Request {
        kind: "judgment".to_string(),
        outcome: "settled".to_string(),
        subject: vec![("what".to_string(), what)],
        evidence,
        reason,
        ..Request::default()
    }
}

/// A `gate-pass` request at `gate`.
pub fn gate_pass(at: At<'_>, gate: &str, reason: Option<String>) -> Request {
    Request {
        kind: "gate-pass".to_string(),
        outcome: "passed".to_string(),
        subject: at.gate_pass_subject(gate),
        reason,
        ..Request::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_pass_subject_names_the_workflow_state_and_gate() {
        let at = At {
            session: "w",
            session_id: "sid",
            state: "review",
            log_seq: 3,
        };
        let req = gate_pass(at, "ci", None);
        let subject = body::validate(&req).unwrap();
        assert_eq!(subject["workflow"], "w");
        assert_eq!(subject["state"], "review");
        assert_eq!(subject["gate"], "ci");
    }

    #[test]
    fn a_judgment_with_no_by_is_decided_by_the_session_and_not_defaulted() {
        let d = judgment_decider("w", &Flags::default(), &Observed::default()).unwrap();
        assert_eq!(d.kind(), "session");
        assert!(!d.defaulted());
        assert!(body::validate(&judgment("x".to_string(), None, vec![])).is_ok());
    }
}
