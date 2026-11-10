//! Standing, refusals and racing reversals (PRD R17 to R21, R44;
//! DESIGN-decision-record.md, Decision 1's "Decide" step).
//!
//! - [`judge`] applies R17 to a reversal: where it is recorded (R4), its
//!   standing and the chain path that gave it, or the reason it is refused.
//! - [`Index`] holds every readable reversal in the local store, keyed by
//!   the decision it is aimed at, and computes each decision's status with
//!   R19's race: the earliest time wins, then the reverser session whose
//!   name sorts first by bytes, then the id that sorts first. Every reader
//!   builds one and applies the same rule, so they agree.
//! - [`settle`] is R19's repeat check: a session that recorded a reversal
//!   that has since lost records a `refused` naming the winner.
//!
//! Supersede hops (R39 to R43) aren't followed here.

use std::cell::OnceCell;
use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::body::{self, BasisRef, DecisionBody, PathStep, Refusal, Request, SessionRef, Target};
use super::decider::{Authority, Decider};
use super::read::{self, Entry};
use super::store::{self, Area};
use super::{chain, codes, DecisionError, FORMAT_VERSION};
use crate::session::{Backend, SessionBackend};

/// The standing a recorded reversal carries.
pub const VERIFIED: &str = "verified";
pub const ASSERTED: &str = "asserted";

/// The reasons a reversal is refused.
pub mod reasons {
    /// No decision of that id in that session.
    pub const NOT_FOUND: &str = "not_found";
    /// The decision is there but koto can't read it.
    pub const UNREADABLE: &str = "unreadable";
    /// The decision is itself a `reverse` (R18).
    pub const REVERSES_REVERSAL: &str = "reverses_reversal";
    /// The decision is a `refused` (R18).
    pub const REVERSES_REFUSAL: &str = "reverses_refusal";
    /// A winning reversal of it already exists (R18).
    pub const ALREADY_REVERSED: &str = "already_reversed";
    /// The reverser is on the chain but no R17 rule gives it standing.
    pub const NO_STANDING: &str = "no_standing";
    /// Neither the reverser's session nor its anchor is on the chain, and
    /// the chain is complete.
    pub const NOT_ON_CHAIN: &str = "not_on_chain";
    /// A competing reversal won R19's race.
    pub const LOST_RACE: &str = "lost_race";
}

/// R19's order: time, then reverser session name, then id.
fn race_key(b: &DecisionBody) -> (&str, &str, &str) {
    (&b.time, &b.session.name, &b.id)
}

/// The reversal that wins R19's race among `reversals`.
pub fn winner<'a>(
    reversals: impl IntoIterator<Item = &'a DecisionBody>,
) -> Option<&'a DecisionBody> {
    reversals
        .into_iter()
        .min_by(|a, b| race_key(a).cmp(&race_key(b)))
}

fn same(a: &DecisionBody, b: &DecisionBody) -> bool {
    a.session.name == b.session.name && a.id == b.id
}

fn reference(b: &DecisionBody) -> BasisRef {
    BasisRef {
        session: b.session.name.clone(),
        id: b.id.clone(),
    }
}

/// Every session in the local store's decisions, and the readable
/// reversals among them keyed by `(target session id, decision id)`.
pub struct Index {
    sessions: BTreeMap<String, Vec<Entry>>,
    reversals: BTreeMap<(String, String), Vec<DecisionBody>>,
}

impl Index {
    /// Read every session in the local store. With `remote` set, under the
    /// cloud backend each session's bucket objects are listed too, once.
    pub fn scan(backend: &Backend, remote: bool) -> Self {
        let sessions = read::local_sessions(backend)
            .into_iter()
            .map(|s| {
                let entries = read::read_session_with(backend, &s, remote);
                (s, entries)
            })
            .collect();
        Self::from_sessions(sessions)
    }

    /// An index over sessions already read.
    pub fn from_sessions(sessions: BTreeMap<String, Vec<Entry>>) -> Self {
        let mut reversals: BTreeMap<(String, String), Vec<DecisionBody>> = BTreeMap::new();
        for entries in sessions.values() {
            for entry in entries {
                let Entry::Decision(b) = entry else { continue };
                if let (true, Some(t)) = (b.kind == "reverse", &b.target) {
                    reversals
                        .entry((t.id.clone(), t.decision.clone()))
                        .or_default()
                        .push((**b).clone());
                }
            }
        }
        Self {
            sessions,
            reversals,
        }
    }

    /// `session`'s entries: from the scan, or read now (bucket included)
    /// for a session this host holds no copy of.
    pub fn entries(&self, backend: &Backend, session: &str) -> Vec<Entry> {
        match self.sessions.get(session) {
            Some(entries) => entries.clone(),
            None => read::read_session(backend, session),
        }
    }

    /// The decision `id` in `session` as the scan read it.
    pub fn find(&self, session: &str, id: &str) -> Option<&Entry> {
        self.sessions.get(session)?.iter().find(|e| match e {
            Entry::Decision(b) => b.id == id,
            Entry::Unreadable { id: Some(i), .. } => i == id,
            Entry::Unreadable { id: None, .. } => false,
        })
    }

    /// Every reversal aimed at decision `id` of the session whose id is
    /// `session_id`.
    pub fn reversals_of(&self, session_id: &str, id: &str) -> &[DecisionBody] {
        self.reversals
            .get(&(session_id.to_string(), id.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The reversal of `session_id`'s decision `id` that wins R19's race.
    pub fn winner_of(&self, session_id: &str, id: &str) -> Option<&DecisionBody> {
        winner(self.reversals_of(session_id, id))
    }

    /// A readable decision's status (PRD Terms).
    pub fn status(&self, body: &DecisionBody) -> &'static str {
        match body.kind.as_str() {
            "refused" => "refused",
            "reverse" => {
                let won = body
                    .target
                    .as_ref()
                    .and_then(|t| self.winner_of(&t.id, &t.decision))
                    .is_none_or(|w| same(w, body));
                if won {
                    "in-effect"
                } else {
                    "refused"
                }
            }
            _ if self.winner_of(&body.session.id, &body.id).is_some() => "reversed",
            _ => "in-effect",
        }
    }

    /// An entry as `list` and `show` print it: the stored decision, its
    /// status, and what R20 and R21 add: the winning reversal of a reversed
    /// decision, the winner a refused reversal lost to, an escalation's
    /// effect, and the decisions whose `basis` names a reversed acceptance.
    pub fn entry_json(&self, entry: &Entry, scope: &Scope<'_>) -> Value {
        let mut out = entry.to_json();
        let Entry::Decision(body) = entry else {
            return out;
        };
        out["status"] = json!(self.status(body));
        match body.kind.as_str() {
            "refused" => {}
            "reverse" => {
                let Some(t) = &body.target else { return out };
                let win = self.winner_of(&t.id, &t.decision);
                match win {
                    Some(w) if !same(w, body) => out["winner"] = json!(reference(w)),
                    _ => {
                        let original = match self.find(&t.name, &t.decision) {
                            Some(Entry::Decision(o)) if o.session.id == t.id => Some(o),
                            _ => None,
                        };
                        if original.is_some_and(|o| is_acceptance(o)) {
                            out["affected"] = json!(scope.dependents(self, &t.name, &t.decision));
                        }
                    }
                }
            }
            _ => {
                let Some(w) = self.winner_of(&body.session.id, &body.id) else {
                    return out;
                };
                out["reversed_by"] = reversal_json(w);
                if is_acceptance(body) {
                    out["affected"] = json!(scope.dependents(self, &body.session.name, &body.id));
                }
                if let Some(effect) = effect(body, w) {
                    out["effect"] = effect;
                }
            }
        }
        out
    }
}

fn is_acceptance(b: &DecisionBody) -> bool {
    b.kind == "accept" && b.outcome == "accepted"
}

/// The winning reversal as a reversed decision names it (R20, R44).
fn reversal_json(w: &DecisionBody) -> Value {
    let mut out = json!({
        "session": w.session.name,
        "id": w.id,
        "time": w.time,
        "decider": w.decider,
        "reason": w.reason,
        "standing": w.standing,
        "path": w.path,
    });
    if let Some(c) = &w.compensating {
        out["compensating"] = json!(c);
    }
    if let Some(r) = &w.replacement {
        out["replacement"] = json!(r);
    }
    out
}

/// What reversing an escalation means for its question (R21).
fn effect(body: &DecisionBody, w: &DecisionBody) -> Option<Value> {
    let question = body.subject.get("question")?.clone();
    match body.kind.as_str() {
        "escalate" => Some(json!({
            "question": question,
            "withdrawn": true,
            "recipient": body.subject.get("recipient"),
        })),
        "decline-escalation" => Some(json!({
            "question": question,
            "escalated_to": w.session.name,
        })),
        _ => None,
    }
}

/// The sessions R21's dependents are looked for in: the lister's own
/// session and every session below it, walked only when needed.
pub struct Scope<'a> {
    backend: &'a Backend,
    root: String,
    sessions: OnceCell<Vec<String>>,
}

impl<'a> Scope<'a> {
    pub fn new(backend: &'a Backend, root: &str) -> Self {
        Self {
            backend,
            root: root.to_string(),
            sessions: OnceCell::new(),
        }
    }

    fn sessions(&self) -> &[String] {
        self.sessions.get_or_init(|| {
            let mut all = vec![self.root.clone()];
            all.extend(read::sessions_below(self.backend, &self.root));
            all
        })
    }

    /// The decisions in scope whose `basis` names `session`'s `id`.
    pub fn dependents(&self, index: &Index, session: &str, id: &str) -> Vec<BasisRef> {
        let named = BasisRef {
            session: session.to_string(),
            id: id.to_string(),
        };
        let mut out = Vec::new();
        for s in self.sessions() {
            for entry in index.entries(self.backend, s) {
                if let Entry::Decision(b) = entry {
                    if b.basis.contains(&named) {
                        out.push(reference(&b));
                    }
                }
            }
        }
        out
    }
}

/// Where a reversal is recorded and the standing it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub session: String,
    pub standing: &'static str,
    pub path: Vec<PathStep>,
}

/// Whether `session` is the original's deciding session or on the chain
/// the original recorded.
fn on_chain(original: &DecisionBody, session: &str) -> bool {
    session == original.session.name || above(original, session)
}

fn above(original: &DecisionBody, session: &str) -> bool {
    original.chain.links.iter().any(|l| l.session == session)
}

/// R17's rules, for a reversal recorded in `recorded_in` by `decider`.
fn rule_matches(original: &DecisionBody, recorded_in: &str, decider: &Decider) -> bool {
    if recorded_in == original.session.name {
        return match (&original.decider, decider) {
            (Decider::Session { .. }, Decider::Session { .. }) => true,
            (Decider::Check { name: a, .. }, Decider::Check { name: b, .. }) => a.value == b.value,
            (Decider::Person { .. }, Decider::Person { .. }) => true,
            _ => false,
        };
    }
    if !above(original, recorded_in) {
        return false;
    }
    // From above: a person through the session for any original, and the
    // session alone for an original decided by a session or a check.
    matches!(
        (&original.decider, decider),
        (_, Decider::Person { .. })
            | (
                Decider::Session { .. } | Decider::Check { .. },
                Decider::Session { .. }
            )
    )
}

/// Judge a reversal of `original` by `decider`, whose session is
/// `reverser` and whose anchor, if any, is `anchor`. Returns where it is
/// recorded and its standing, or the reason it is refused.
pub fn judge(
    store: &dyn SessionBackend,
    original: &DecisionBody,
    reverser: &str,
    decider: &Decider,
    anchor: Option<&str>,
) -> Result<Placement, &'static str> {
    let live = Live {
        path_to: &|to: &str| chain::path(store, &original.session.name, to),
        chain_incomplete: &|| chain::snapshot(store, &original.session.name).incomplete,
        reverser_below: &|| {
            chain::snapshot(store, reverser)
                .links
                .iter()
                .any(|l| l.session == original.session.name)
        },
    };
    place(original, reverser, decider, anchor, &live)
}

/// What [`place`] reads from the links as they stand now, each asked only
/// when needed.
pub struct Live<'a> {
    /// The steps the walk from the original's session takes to reach a
    /// session, or `None` when it doesn't reach it.
    pub path_to: &'a dyn Fn(&str) -> Option<Vec<PathStep>>,
    /// Whether the walk from the original's session is incomplete now.
    pub chain_incomplete: &'a dyn Fn() -> bool,
    /// Whether the original's deciding session is on the reverser's chain.
    pub reverser_below: &'a dyn Fn() -> bool,
}

/// [`judge`] over facts already read.
///
/// A reversal is `verified` only when an R17 rule matches and, for a
/// session above, the live walk from the original reaches the recording
/// session, so the path it records is one the links hold now. When the
/// recorded chain lists a session the live walk no longer reaches, no
/// rule gives standing: the reversal is `asserted` if the chain, recorded
/// or live, is incomplete and the reverser isn't below the original, and
/// refused otherwise.
pub fn place(
    original: &DecisionBody,
    reverser: &str,
    decider: &Decider,
    anchor: Option<&str>,
    live: &Live<'_>,
) -> Result<Placement, &'static str> {
    let recorded_in = if on_chain(original, reverser) {
        Some(reverser)
    } else {
        anchor.filter(|a| on_chain(original, a))
    };
    if let Some(r) = recorded_in {
        if rule_matches(original, r, decider) {
            let path = if r == original.session.name {
                Some(Vec::new())
            } else {
                (live.path_to)(r)
            };
            if let Some(path) = path {
                return Ok(Placement {
                    session: r.to_string(),
                    standing: VERIFIED,
                    path,
                });
            }
        }
    }
    // No rule gave standing. Beyond a broken chain the reversal is
    // asserted, unless the reverser sits below the original's deciding
    // session.
    if reverser != original.session.name
        && (original.chain.incomplete || (live.chain_incomplete)())
        && !(live.reverser_below)()
    {
        return Ok(Placement {
            session: reverser.to_string(),
            standing: ASSERTED,
            path: Vec::new(),
        });
    }
    Err(if recorded_in.is_some() {
        reasons::NO_STANDING
    } else {
        reasons::NOT_ON_CHAIN
    })
}

/// What a decision written in `session` stamps from its state file.
pub struct Stamp {
    pub session: SessionRef,
    pub state: String,
    pub log_seq: u64,
}

/// Read `session`'s header id, current state and last log sequence.
pub fn stamp(backend: &Backend, session: &str) -> Result<Stamp, DecisionError> {
    let (header, events) = backend.read_events(session).map_err(|e| {
        DecisionError::new(
            "decision_write_failed",
            format!("'{session}' can't be read: {e}"),
        )
    })?;
    Ok(Stamp {
        session: SessionRef {
            name: session.to_string(),
            id: header.session_id,
        },
        state: crate::engine::persistence::derive_state_from_log(&events).unwrap_or_default(),
        log_seq: events.last().map(|e| e.seq).unwrap_or(0),
    })
}

/// What a `refused` reversal records beyond its decider.
pub struct RefusedReversal {
    /// `<session>:<id>` of the decision the reversal was aimed at.
    pub aimed_at: String,
    pub target: Option<Target>,
    pub reason: String,
    pub refusal: Refusal,
}

/// The body of a `refused` reversal with id `id` in `stamp`'s session.
pub fn refused_body(
    id: &str,
    stamp: &Stamp,
    decider: Decider,
    authority: Authority,
    chain: chain::ChainSnapshot,
    what: RefusedReversal,
) -> Result<DecisionBody, DecisionError> {
    let subject = body::validate(&Request {
        kind: "refused".to_string(),
        outcome: "refused".to_string(),
        subject: vec![
            ("kind".to_string(), "reverse".to_string()),
            ("subject".to_string(), what.aimed_at),
        ],
        reason: Some(what.reason.clone()),
        ..Request::default()
    })?;
    Ok(DecisionBody {
        v: FORMAT_VERSION,
        id: id.to_string(),
        session: stamp.session.clone(),
        time: crate::engine::types::now_iso8601(),
        state: stamp.state.clone(),
        log_seq: stamp.log_seq,
        kind: "refused".to_string(),
        outcome: "refused".to_string(),
        subject,
        decider,
        authority,
        chain,
        basis: Vec::new(),
        evidence: Vec::new(),
        reason: Some(what.reason),
        tool_call: None,
        target: what.target,
        standing: None,
        path: None,
        compensating: None,
        replacement: None,
        refusal: Some(what.refusal),
    })
}

/// Whether `session` already holds a `refused` for its reversal `lost`.
fn refusal_recorded(entries: &[Entry], lost: &BasisRef) -> bool {
    entries.iter().any(|e| match e {
        Entry::Decision(b) => b
            .refusal
            .as_ref()
            .is_some_and(|r| r.reversal.as_ref() == Some(lost)),
        _ => false,
    })
}

/// Record in `session` the `refused` for its reversal `lost`, which lost
/// to `winner`, unless one is already there. The decider and authority
/// are the losing reversal's.
pub fn record_lost(
    backend: &Backend,
    session: &str,
    lost: &DecisionBody,
    winner: &DecisionBody,
) -> Result<Option<DecisionBody>, DecisionError> {
    let lost_ref = reference(lost);
    let stamp = stamp(backend, session)?;
    let target = lost.target.clone();
    let aimed_at = target
        .as_ref()
        .map(|t| format!("{}:{}", t.name, t.decision))
        .unwrap_or_default();
    let written = store::write_in(backend, session, &Area::Decisions, |id| {
        if refusal_recorded(&read::read_session_with(backend, session, false), &lost_ref) {
            return Err(DecisionError::new(
                codes::DECISION_EXISTS,
                "already recorded",
            ));
        }
        refused_body(
            id,
            &stamp,
            lost.decider.clone(),
            lost.authority.clone(),
            chain::snapshot(backend, session),
            RefusedReversal {
                aimed_at: aimed_at.clone(),
                target: target.clone(),
                reason: format!(
                    "reversal {}:{} of {aimed_at} lost to {}:{}, recorded earlier",
                    lost.session.name, lost.id, winner.session.name, winner.id
                ),
                refusal: Refusal {
                    reason: reasons::LOST_RACE.to_string(),
                    winner: Some(reference(winner)),
                    reversal: Some(lost_ref.clone()),
                },
            },
        )
    });
    match written {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.code == codes::DECISION_EXISTS => Ok(None),
        Err(e) => Err(e),
    }
}

/// The session a decider entered through: the deciding session for a
/// session, the session that ran a check, a person's entering session.
fn entered_through(decider: &Decider) -> &str {
    match decider {
        Decider::Session { session, .. } | Decider::Check { session, .. } => &session.value,
        Decider::Person { entering, .. } => &entering.session.value,
    }
}

/// R19's repeat check: for each reversal entered through `session` that
/// now loses to another reversal on this host, record in `session` the
/// `refused` naming the winner, once.
///
/// A reversal entered through `session` sits in `session` itself, or in
/// the anchor it was recorded in when `session` was off the original's
/// chain (R4). So the check looks in `session`, in the sessions on its
/// chain (its own anchor among them) and in the anchor `KOTO_ANCHOR`
/// names, keeping the reversals whose decider entered through `session`.
/// The refusal always goes in `session`: R19 puts it in the losing
/// reverser's own session, where its next decision command finds it.
///
/// It reads local copies only. Failures are dropped: the check runs
/// before an unrelated decision and must not block it.
pub fn settle(backend: &Backend, session: &str) {
    let own = read::read_session_with(backend, session, false);
    let mut holders: Vec<String> = chain::snapshot(backend, session)
        .links
        .into_iter()
        .map(|l| l.session)
        .collect();
    if let Some(anchor) = crate::session::anchor::anchor_from_env()
        .filter(|a| crate::session::anchor::is_anchor(backend, a))
    {
        holders.push(anchor);
    }
    holders.sort();
    holders.dedup();
    holders.retain(|h| h != session);
    let held: Vec<Entry> = holders
        .iter()
        .flat_map(|h| read::read_session_with(backend, h, false))
        .collect();
    let pending: Vec<&DecisionBody> = own
        .iter()
        .chain(held.iter())
        .filter_map(|e| match e {
            Entry::Decision(b) if b.kind == "reverse" => Some(&**b),
            _ => None,
        })
        .filter(|b| entered_through(&b.decider) == session)
        .filter(|b| !refusal_recorded(&own, &reference(b)))
        .collect();
    if pending.is_empty() {
        return;
    }
    let index = Index::scan(backend, false);
    for lost in pending {
        let Some(t) = &lost.target else { continue };
        if let Some(w) = index.winner_of(&t.id, &t.decision) {
            if !same(w, lost) {
                let _ = record_lost(backend, session, lost, w);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::chain::{ChainLink, ChainSnapshot};
    use crate::decisions::decider::{Entering, Source, Sourced};

    fn session_decider(s: &str) -> Decider {
        Decider::Session {
            session: Sourced::observed(s),
            harness: Sourced::observed(""),
            anchor: Sourced::observed(""),
            agent: Sourced::observed("unknown"),
            defaulted: false,
            note: None,
        }
    }

    fn person(handle: &str, through: &str) -> Decider {
        Decider::Person {
            handle: Sourced::handed(handle),
            surface: "terminal".to_string(),
            link: None,
            entering: Entering {
                session: Sourced::observed(through),
                harness: Sourced::observed(""),
                anchor: Sourced::observed(""),
            },
        }
    }

    fn check(name: &str, session: &str) -> Decider {
        Decider::Check {
            name: Sourced::handed(name),
            session: Sourced::observed(session),
        }
    }

    fn body(session: &str, id: &str, kind: &str, decider: Decider, chain: &[&str]) -> DecisionBody {
        DecisionBody {
            v: FORMAT_VERSION,
            id: id.to_string(),
            session: SessionRef {
                name: session.to_string(),
                id: format!("{session}-id"),
            },
            time: "2026-11-10T00:00:00.000Z".to_string(),
            state: String::new(),
            log_seq: 0,
            kind: kind.to_string(),
            outcome: body::outcomes_of(kind).unwrap()[0].to_string(),
            subject: Default::default(),
            decider,
            authority: Authority {
                kind: "dispatch".to_string(),
                level: None,
                grants: Vec::new(),
                source: Source::Handed,
                dispatcher: "none".to_string(),
                run_mode: Sourced::observed("unknown"),
            },
            chain: ChainSnapshot {
                links: chain
                    .iter()
                    .map(|s| ChainLink {
                        session: s.to_string(),
                        via: "init".to_string(),
                    })
                    .collect(),
                incomplete: false,
            },
            basis: Vec::new(),
            evidence: Vec::new(),
            reason: None,
            tool_call: None,
            target: None,
            standing: None,
            path: None,
            compensating: None,
            replacement: None,
            refusal: None,
        }
    }

    fn reversal(session: &str, id: &str, time: &str, of: &DecisionBody) -> DecisionBody {
        let mut b = body(session, id, "reverse", session_decider(session), &[]);
        b.time = time.to_string();
        b.target = Some(Target {
            name: of.session.name.clone(),
            id: of.session.id.clone(),
            decision: of.id.clone(),
        });
        b.standing = Some(VERIFIED.to_string());
        b.path = Some(Vec::new());
        b
    }

    #[test]
    fn the_same_decider_and_a_session_above_have_standing() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["c"],
        );
        assert!(rule_matches(&w, "w", &session_decider("w")));
        assert!(rule_matches(&w, "c", &session_decider("c")));
        assert!(rule_matches(&w, "c", &person("dana", "c")));
        assert!(!rule_matches(&w, "x", &session_decider("x")));
        assert!(!rule_matches(&w, "w", &person("dana", "w")));

        let v = body("g", "000000000001", "verdict", check("grader", "g"), &["c"]);
        assert!(rule_matches(&v, "g", &check("grader", "g")));
        assert!(!rule_matches(&v, "g", &check("other", "g")));
    }

    #[test]
    fn a_session_alone_cannot_reverse_a_person() {
        let d = body(
            "d",
            "000000000001",
            "judgment",
            person("ana", "d"),
            &["top"],
        );
        assert!(!rule_matches(&d, "top", &session_decider("top")));
        assert!(rule_matches(&d, "top", &person("bea", "top")));
        assert!(rule_matches(&d, "d", &person("bea", "d")));
        assert!(!rule_matches(&d, "d", &session_decider("d")));
    }

    fn live<'a>(
        path_to: &'a dyn Fn(&str) -> Option<Vec<PathStep>>,
        chain_incomplete: &'a dyn Fn() -> bool,
    ) -> Live<'a> {
        Live {
            path_to,
            chain_incomplete,
            reverser_below: &|| false,
        }
    }

    #[test]
    fn a_session_above_is_verified_only_when_the_live_walk_reaches_it() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["c"],
        );
        let reached = |to: &str| {
            Some(vec![PathStep {
                session: "w".to_string(),
                via: "init".to_string(),
                to: to.to_string(),
            }])
        };
        let p = place(
            &w,
            "c",
            &session_decider("c"),
            None,
            &live(&reached, &|| false),
        )
        .unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert_eq!(p.session, "c");
        assert_eq!(p.path.len(), 1);

        // The recorded chain lists c, but the links no longer reach it.
        let unreached = |_: &str| None;
        assert_eq!(
            place(
                &w,
                "c",
                &session_decider("c"),
                None,
                &live(&unreached, &|| false)
            ),
            Err(reasons::NO_STANDING),
            "a complete chain that no longer reaches the reverser gives no standing"
        );
        let p = place(
            &w,
            "c",
            &session_decider("c"),
            None,
            &live(&unreached, &|| true),
        )
        .unwrap();
        assert_eq!(
            p.standing, ASSERTED,
            "past a broken live chain it is asserted"
        );
        assert_eq!(p.session, "c");
        assert!(p.path.is_empty());

        // The same session needs no walk.
        let p = place(
            &w,
            "w",
            &session_decider("w"),
            None,
            &live(&unreached, &|| false),
        )
        .unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert!(p.path.is_empty());
    }

    #[test]
    fn the_earliest_reversal_wins_then_the_session_name_then_the_id() {
        let o = body(
            "c",
            "000000000001",
            "judgment",
            session_decider("c"),
            &["a", "b"],
        );
        let a = reversal("a", "000000000001", "2026-11-10T00:00:02.000Z", &o);
        let b = reversal("b", "000000000001", "2026-11-10T00:00:01.000Z", &o);
        assert!(same(winner([&a, &b]).unwrap(), &b));
        assert!(same(winner([&b, &a]).unwrap(), &b));
        let b = reversal("b", "000000000001", "2026-11-10T00:00:02.000Z", &o);
        assert!(same(winner([&b, &a]).unwrap(), &a));
        let a2 = reversal("a", "000000000002", "2026-11-10T00:00:02.000Z", &o);
        assert!(same(winner([&a2, &b, &a]).unwrap(), &a));
    }

    #[test]
    fn statuses_follow_the_race() {
        let o = body(
            "c",
            "000000000001",
            "judgment",
            session_decider("c"),
            &["a", "b"],
        );
        let a = reversal("a", "000000000001", "2026-11-10T00:00:02.000Z", &o);
        let b = reversal("b", "000000000001", "2026-11-10T00:00:01.000Z", &o);
        let sessions = [("a", &a), ("b", &b), ("c", &o)]
            .into_iter()
            .map(|(s, b)| (s.to_string(), vec![Entry::Decision(Box::new(b.clone()))]))
            .collect();
        let index = Index::from_sessions(sessions);
        assert_eq!(index.status(&o), "reversed");
        assert_eq!(index.status(&b), "in-effect");
        assert_eq!(index.status(&a), "refused");
    }
}
