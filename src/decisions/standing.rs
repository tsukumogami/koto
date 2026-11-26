//! Standing, refusals and racing reversals (PRD R17 to R21, R44;
//! DESIGN-decision-record.md, Decision 1's "Decide" step).
//!
//! - [`judge`] applies R17 to a reversal: where it is recorded (R4), its
//!   standing and the chain path that gave it, or the reason it is refused.
//! - [`Index`] holds every readable reversal in the local store, keyed by
//!   the decision it is aimed at, and computes each decision's status with
//!   R19's race: the earliest time wins, then the reverser session whose
//!   name sorts first by bytes, then the id that sorts first. Every reader
//!   builds one and applies the same rule, so they agree. [`list`] and
//!   [`show`] print decisions with the statuses it gives them.
//! - [`settle`] is R19's repeat check: every decision command that writes
//!   calls it for its session first, so a reversal entered through that
//!   session that has since lost gets a `refused` naming the winner there.
//!
//! Succession (R37, R39 to R41, R43): when neither the reverser nor its
//! anchor is on the original's chain, [`place`] looks for a chain session
//! whose confirmed supersede hops ([`chain::hops_to`]) reach one of them.
//! The one reached stands exactly in that session's position, R17's rules
//! apply as if the reversal were recorded there, and the path records each
//! hop. A reach that needs a hop koto can't confirm is refused
//! `supersede_unconfirmed`, never `asserted`. [`Index`] reads each fenced
//! session's marker and treats a decision there with an id above the
//! marker's `decisions_through` as written after the fence: `refused`,
//! with no standing and no win. The reversal read at `koto next` (R22) and
//! R42 aren't handled here.
//!
//! This module sits above [`store`] and [`read`]; neither calls into it.

use std::cell::OnceCell;
use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::body::{self, BasisRef, DecisionBody, PathStep, Refusal, Request, SessionRef, Target};
use super::decider::{Authority, Decider};
use super::read::{self, Entry};
use super::store::{self, Area};
use super::{chain, codes, marker, DecisionError, FORMAT_VERSION};
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
    /// The reverser would stand on the chain only through a supersede hop
    /// koto can't confirm (R40).
    pub const SUPERSEDE_UNCONFIRMED: &str = "supersede_unconfirmed";
    /// The decision is an `answer`, which a reversal doesn't overrule.
    pub const REVERSES_ANSWER: &str = "reverses_answer";
    /// The decision is a `readdress`, which a reversal doesn't overrule.
    pub const REVERSES_READDRESS: &str = "reverses_readdress";
}

/// R19's order: time, then the reverser's session name, then id, then the
/// holding session so the order is total. The reverser is the session the
/// decider entered through, which differs from the holder when the
/// reversal sits in the reverser's anchor (R4). Times compare as strings,
/// which orders them correctly only because every writer stamps
/// fixed-width RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
fn race_key(b: &DecisionBody) -> (&str, &str, &str, &str) {
    (&b.time, reverser_of(b), &b.id, &b.session.name)
}

/// The reverser's session: the one the decider entered through, or the
/// holding session when the body names none.
fn reverser_of(b: &DecisionBody) -> &str {
    match entered_through(&b.decider) {
        "" => &b.session.name,
        entered => entered,
    }
}

/// Why `lost` lost to `winner`, for the refusal's message.
pub fn lost_message(lost: &DecisionBody, winner: &DecisionBody) -> String {
    let why = if winner.time != lost.time {
        "was recorded earlier"
    } else if reverser_of(winner) != reverser_of(lost) {
        "has the same time and a reverser session name that sorts first"
    } else {
        "has the same time and reverser and an id that sorts first"
    };
    format!(
        "reversal {}:{} lost to {}:{}, which {why}",
        lost.session.name, lost.id, winner.session.name, winner.id
    )
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
/// reversals and answers among them keyed by `(target session id,
/// decision id)`.
pub struct Index {
    sessions: BTreeMap<String, Vec<Entry>>,
    reversals: BTreeMap<(String, String), Vec<DecisionBody>>,
    answers: BTreeMap<(String, String), Vec<DecisionBody>>,
    readdresses: BTreeMap<(String, String), Vec<DecisionBody>>,
    /// Each fenced session's `decisions_through`, by session name (R37).
    cutoffs: BTreeMap<String, Option<String>>,
}

/// Whether `body` sits in a fenced session whose marker's
/// `decisions_through` is below its id, or is `null` (R37). Decision ids
/// are fixed-width digits, so their byte order is the order a session
/// recorded them in.
fn past_cutoff(cutoffs: &BTreeMap<String, Option<String>>, body: &DecisionBody) -> bool {
    cutoffs
        .get(&body.session.name)
        .is_some_and(|through| through.as_deref().is_none_or(|t| body.id.as_str() > t))
}

impl Index {
    /// Read every session in the local store. With `remote` set, under the
    /// cloud backend each session's bucket objects are listed too, once.
    /// Each session's supersede marker is read from its local copy.
    pub fn scan(backend: &Backend, remote: bool) -> Self {
        let mut cutoffs = BTreeMap::new();
        let sessions = read::local_sessions(backend)
            .into_iter()
            .map(|s| {
                if let Ok(Some(m)) = marker::read(&backend.session_dir(&s).join("ctx")) {
                    cutoffs.insert(s.clone(), m.decisions_through);
                }
                let entries = read::read_session_with(backend, &s, remote);
                (s, entries)
            })
            .collect();
        Self::from_sessions_cut(sessions, cutoffs)
    }

    /// An index over sessions already read.
    pub fn from_sessions(sessions: BTreeMap<String, Vec<Entry>>) -> Self {
        Self::from_sessions_cut(sessions, BTreeMap::new())
    }

    /// An index over sessions already read, with the `decisions_through`
    /// of each fenced one. A decision past its session's cutoff is never
    /// counted as a reversal.
    pub fn from_sessions_cut(
        sessions: BTreeMap<String, Vec<Entry>>,
        cutoffs: BTreeMap<String, Option<String>>,
    ) -> Self {
        let mut reversals: BTreeMap<(String, String), Vec<DecisionBody>> = BTreeMap::new();
        let mut answers: BTreeMap<(String, String), Vec<DecisionBody>> = BTreeMap::new();
        let mut readdresses: BTreeMap<(String, String), Vec<DecisionBody>> = BTreeMap::new();
        for entries in sessions.values() {
            for entry in entries {
                let Entry::Decision(b) = entry else { continue };
                if past_cutoff(&cutoffs, b) {
                    continue;
                }
                let Some(t) = &b.target else { continue };
                let into = match b.kind.as_str() {
                    "reverse" => &mut reversals,
                    "answer" if b.outcome == "answered" => &mut answers,
                    "readdress" if b.outcome == "passed" => &mut readdresses,
                    _ => continue,
                };
                into.entry((t.id.clone(), t.decision.clone()))
                    .or_default()
                    .push((**b).clone());
            }
        }
        Self {
            sessions,
            reversals,
            answers,
            readdresses,
            cutoffs,
        }
    }

    /// Whether `body` was written in a fenced session after its fence, by
    /// the marker's `decisions_through` (R37).
    pub fn after_fence(&self, body: &DecisionBody) -> bool {
        past_cutoff(&self.cutoffs, body)
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

    /// Every `answered` answer aimed at decision `id` of the session whose
    /// id is `session_id`, wherever it was recorded, earliest first by
    /// time, then the answering session's name, then id.
    pub fn answers_of(&self, session_id: &str, id: &str) -> Vec<&DecisionBody> {
        let mut out: Vec<&DecisionBody> = self
            .answers
            .get(&(session_id.to_string(), id.to_string()))
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        out.sort_by(|a, b| {
            (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id))
        });
        out
    }

    /// The sessions whose directories this scan read `body` from: its
    /// recording session, and any copy of it this host holds.
    pub fn holders(&self, body: &DecisionBody) -> Vec<&str> {
        self.sessions
            .iter()
            .filter(|(_, entries)| {
                entries.iter().any(|e| {
                    matches!(e, Entry::Decision(b)
                        if b.session.id == body.session.id && b.id == body.id)
                })
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Every `passed` pass-up aimed at decision `id` of the session whose
    /// id is `session_id`, wherever it was recorded, earliest first by
    /// time, then the recording session's name, then id: the order the
    /// addressee history is built in.
    pub fn readdresses_of(&self, session_id: &str, id: &str) -> Vec<&DecisionBody> {
        let mut out: Vec<&DecisionBody> = self
            .readdresses
            .get(&(session_id.to_string(), id.to_string()))
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        out.sort_by(|a, b| {
            (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id))
        });
        out
    }

    /// Every session the scan read, with its entries.
    pub fn sessions(&self) -> &BTreeMap<String, Vec<Entry>> {
        &self.sessions
    }

    /// The reversal of `session_id`'s decision `id` that wins R19's race.
    pub fn winner_of(&self, session_id: &str, id: &str) -> Option<&DecisionBody> {
        winner(self.reversals_of(session_id, id))
    }

    /// A readable decision's status (PRD Terms).
    pub fn status(&self, body: &DecisionBody) -> &'static str {
        if self.after_fence(body) {
            return "refused";
        }
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
    /// `holder` is the session whose directory the entry was read from.
    pub fn entry_json(&self, holder: &str, entry: &Entry, scope: &Scope<'_>) -> Value {
        let mut out = entry.to_json();
        let Entry::Decision(body) = entry else {
            return out;
        };
        out["status"] = json!(self.status(body));
        if body.kind == "escalate" || body.kind == "readdress" {
            if let Some(r) = body.subject.get("recipient").and_then(Value::as_str) {
                out["recipient"] = body::Recipient::parse(r).to_json();
            }
            if body.subject.contains_key("packet") {
                out["prompt_header"] = json!(super::prompt::tag(&super::questions::identity(body)));
            }
        }
        if body.prompt.is_some() {
            // The stored mark reads verified only in the session directory
            // of the host that checked the prompt; a copy that arrived by
            // import or by a pull, or under another name, reads it as a
            // remote session's answer. The record isn't opened.
            if verified_in(scope.backend, holder, body) {
                out["reading"] = json!(super::prompt::VERIFIED);
            } else {
                out["reading"] = json!(super::prompt::REMOTE_READING);
                out["decision"]["prompt"]["mark"] =
                    json!(super::decider::Sourced::observed(super::prompt::REMOTE));
            }
        }
        if self.after_fence(body) {
            // Written after the fence: it reads refused and nothing more.
            out["after_fence"] = json!(true);
            return out;
        }
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
pub(crate) fn reversal_json(w: &DecisionBody) -> Value {
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

/// `koto decisions list <session> --format v1`, or with `below` the
/// decisions of every session below `session`. Each status comes from the
/// reversals found anywhere in the local store (R19, R20).
pub fn list(backend: &Backend, session: &str, below: bool, filters: &read::Filters) -> Value {
    let index = Index::scan(backend, true);
    let sessions = if below {
        read::sessions_below(backend, session)
    } else {
        vec![session.to_string()]
    };
    let mut entries: Vec<(String, Entry)> = sessions
        .iter()
        .flat_map(|s| {
            index
                .entries(backend, s)
                .into_iter()
                .map(move |e| (s.clone(), e))
        })
        .filter(|(_, e)| match e {
            Entry::Decision(body) => filters.matches(body),
            Entry::Unreadable { .. } => true,
        })
        .collect();
    entries.sort_by_key(|(_, e)| read::sort_key(e));
    let scope = Scope::new(backend, session);
    let mut out = json!({
        "v": FORMAT_VERSION,
        "session": session,
        "decisions": entries
            .iter()
            .map(|(holder, e)| index.entry_json(holder, e, &scope))
            .collect::<Vec<_>>(),
    });
    if below {
        out["below"] = json!(true);
        out["sessions"] = json!(sessions);
    }
    out
}

/// `koto decisions show <session> <id>`: the entry for `id`, or `None`
/// when the session holds no file of that name in either area.
pub fn show(backend: &Backend, session: &str, id: &str) -> Option<Value> {
    let index = Index::scan(backend, true);
    let entries = index.entries(backend, session);
    let found = entries.iter().find(|e| match e {
        Entry::Decision(body) => body.id == id,
        Entry::Unreadable { id: Some(i), .. } => i == id,
        Entry::Unreadable { id: None, .. } => false,
    });
    let whole_session_unreadable = entries
        .iter()
        .find(|e| matches!(e, Entry::Unreadable { id: None, .. }));
    let entry = found.or(whole_session_unreadable)?;
    let mut out = index.entry_json(session, entry, &Scope::new(backend, session));
    out["v"] = json!(FORMAT_VERSION);
    out["id"] = json!(id);
    if let Entry::Decision(body) = entry {
        if super::attempts::is_provisional(body) {
            // The review comes from the local store, as `decisions
            // attempts` reads it, so the two never disagree.
            let local;
            let reading = if matches!(backend, Backend::Cloud(_)) {
                local = Index::scan(backend, false);
                &local
            } else {
                &index
            };
            out["review"] = super::attempts::review(backend, reading, body);
        }
    }
    Some(out)
}

/// Whether this host checked the prompt of answer `body`, as held in
/// session `holder`'s directory: the host-local note there lists it,
/// sealed to this host's identity (DESIGN-decision-record.md, "A person's
/// answer and the question content area").
pub fn verified_in(backend: &Backend, holder: &str, body: &DecisionBody) -> bool {
    super::prompt::verified_here(
        crate::koto_home::koto_home().as_deref(),
        &crate::session::SessionBackend::session_dir(backend, holder),
        holder,
        body,
    )
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
        hops_to: &|from: &str, to: &str| chain::hops_to(store, from, to),
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
    /// How the supersede hops followed forward from a chain session reach
    /// a session ([`chain::hops_to`]).
    pub hops_to: &'a dyn Fn(&str, &str) -> chain::Reach,
}

/// What following supersede hops from the original's chain found for a
/// reverser off it (R39, R40).
enum Succession {
    /// A confirmed reach with a rule that matches and a live path.
    Placed(Placement),
    /// A rule would match only through a hop koto can't confirm.
    Unconfirmed,
    /// A confirmed reach, but no rule gives standing.
    NoRule,
    /// A reach only through a hop koto can't confirm, where no rule would
    /// give standing either: refused, and never asserted (R40).
    UnconfirmedNoRule,
    /// No hop from the chain reaches it.
    Off,
}

/// Look for a session on the original's chain (its own session, then its
/// recorded links in order) whose supersede hops reach the reverser, or
/// else its anchor. The one reached stands exactly in that session's
/// position: R17's rules apply as if the reversal were recorded there, and
/// the path is the live walk from the original to it plus one step per
/// hop. The reversal is recorded in the session reached. The successor's
/// own links are never followed.
fn succession(
    original: &DecisionBody,
    reverser: &str,
    decider: &Decider,
    anchor: Option<&str>,
    live: &Live<'_>,
) -> Succession {
    let chain_sessions: Vec<&str> = std::iter::once(original.session.name.as_str())
        .chain(original.chain.links.iter().map(|l| l.session.as_str()))
        .collect();
    let mut found = Succession::Off;
    for cand in std::iter::once(reverser).chain(anchor) {
        for &c in &chain_sessions {
            if c == cand {
                continue;
            }
            match (live.hops_to)(c, cand) {
                chain::Reach::No => {}
                chain::Reach::Unconfirmed => {
                    if rule_matches(original, c, decider) {
                        found = Succession::Unconfirmed;
                    } else if matches!(found, Succession::Off | Succession::NoRule) {
                        found = Succession::UnconfirmedNoRule;
                    }
                }
                chain::Reach::Confirmed(hops) => {
                    if !rule_matches(original, c, decider) {
                        if matches!(found, Succession::Off) {
                            found = Succession::NoRule;
                        }
                        continue;
                    }
                    let base = if c == original.session.name {
                        Some(Vec::new())
                    } else {
                        (live.path_to)(c)
                    };
                    if let Some(mut path) = base {
                        path.extend(hops);
                        return Succession::Placed(Placement {
                            session: cand.to_string(),
                            standing: VERIFIED,
                            path,
                        });
                    }
                    if matches!(found, Succession::Off) {
                        found = Succession::NoRule;
                    }
                }
            }
        }
    }
    found
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
///
/// A reverser (or anchor) off the chain that a chain session's confirmed
/// supersede hops reach is judged in that session's position (R39, R43),
/// and one reached only through an unconfirmed hop is refused
/// `supersede_unconfirmed` before the asserted fallback is considered
/// (R40).
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
    // Off the chain: a later member of a chain session's run stands in
    // its position (R39). An unconfirmed hop gives no standing and is
    // never asserted (R40), so it is refused before the fallback below.
    let mut through_hop = false;
    if recorded_in.is_none() {
        match succession(original, reverser, decider, anchor, live) {
            Succession::Placed(p) => return Ok(p),
            Succession::Unconfirmed => return Err(reasons::SUPERSEDE_UNCONFIRMED),
            Succession::UnconfirmedNoRule => return Err(reasons::NO_STANDING),
            Succession::NoRule => through_hop = true,
            Succession::Off => {}
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
    Err(if recorded_in.is_some() || through_hop {
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
        prompt: None,
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
                reason: format!("{} (aimed at {aimed_at})", lost_message(lost, winner)),
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
/// Every command that writes a decision calls it for its session before
/// writing. It reads local copies only. A failure is reported on stderr
/// and otherwise dropped: the check runs before an unrelated decision and
/// must not block it.
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
                if let Err(e) = record_lost(backend, session, lost, w) {
                    eprintln!(
                        "koto: couldn't record in '{session}' that reversal {}:{} lost: {e}",
                        lost.session.name, lost.id
                    );
                }
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
            unestablished: false,
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
            prompt: None,
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
            hops_to: &|_, _| chain::Reach::No,
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
        let reached = |to: &str| Some(vec![PathStep::link("w", "init", to)]);
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

    fn hop(fenced: &str, successor: &str) -> PathStep {
        PathStep {
            session: fenced.to_string(),
            via: chain::VIA_SUPERSEDE.to_string(),
            to: successor.to_string(),
            successor_id: Some(format!("{successor}-id")),
            actor: Some(crate::engine::types::SupersedeActor {
                claude_session: None,
                host: Some("h".to_string()),
                user: Some("u".to_string()),
            }),
            time: Some("2026-11-10T00:00:05.000Z".to_string()),
        }
    }

    fn never() -> bool {
        false
    }

    fn always() -> bool {
        true
    }

    static LINK_TO: fn(&str) -> Option<Vec<PathStep>> = link_to;
    static P_TO_S: fn(&str, &str) -> chain::Reach = p_to_s;
    static NEVER: fn() -> bool = never;
    static ALWAYS: fn() -> bool = always;

    /// Facts where the live walk reaches every session it is asked for
    /// with one link step and supersede hops reach as `hops_to` says.
    fn hops_live<'a>(
        hops_to: &'a dyn Fn(&str, &str) -> chain::Reach,
        chain_incomplete: &'a dyn Fn() -> bool,
    ) -> Live<'a> {
        Live {
            path_to: &LINK_TO,
            chain_incomplete,
            reverser_below: &NEVER,
            hops_to,
        }
    }

    fn p_to_s(from: &str, to: &str) -> chain::Reach {
        match (from, to) {
            ("p", "s") => chain::Reach::Confirmed(vec![hop("p", "s")]),
            ("p", "s2") => chain::Reach::Confirmed(vec![hop("p", "s"), hop("s", "s2")]),
            _ => chain::Reach::No,
        }
    }

    fn link_to(to: &str) -> Option<Vec<PathStep>> {
        Some(vec![PathStep::link("w", "leg", to)])
    }

    #[test]
    fn a_successor_stands_in_the_fenced_sessions_position() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["p", "x"],
        );
        let live = hops_live(&P_TO_S, &NEVER);
        let p = place(&w, "s", &session_decider("s"), None, &live).unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert_eq!(p.session, "s", "the reversal is recorded in the successor");
        assert_eq!(p.path, vec![PathStep::link("w", "leg", "p"), hop("p", "s")]);
        assert!(p.path[1].is_hop());
        assert_eq!(p.path[1].successor_id.as_deref(), Some("s-id"));
        assert!(p.path[1].actor.is_some() && p.path[1].time.is_some());

        // Two hops: both are named, in order.
        let p = place(&w, "s2", &session_decider("s2"), None, &live).unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert_eq!(
            p.path,
            vec![
                PathStep::link("w", "leg", "p"),
                hop("p", "s"),
                hop("s", "s2")
            ]
        );
    }

    #[test]
    fn a_successor_reverses_the_fenced_sessions_own_session_decision() {
        let own = body(
            "p",
            "000000000001",
            "judgment",
            session_decider("p"),
            &["x"],
        );
        let live = hops_live(&P_TO_S, &NEVER);
        let p = place(&own, "s", &session_decider("s"), None, &live).unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert_eq!(p.session, "s");
        assert_eq!(p.path, vec![hop("p", "s")]);
    }

    #[test]
    fn succession_does_not_change_who_may_reverse_a_person() {
        let persons = body("p", "000000000001", "judgment", person("ana", "p"), &["x"]);
        let live = hops_live(&P_TO_S, &NEVER);
        assert_eq!(
            place(&persons, "s", &session_decider("s"), None, &live),
            Err(reasons::NO_STANDING),
            "a later member, as a session, can't reverse a person"
        );
        let p = place(&persons, "s", &person("bea", "s"), None, &live).unwrap();
        assert_eq!(p.standing, VERIFIED);
        assert_eq!(p.session, "s");
        assert_eq!(p.path, vec![hop("p", "s")]);

        // Entered through the person's own session's successor from
        // above, too.
        let w_persons = body("w", "000000000002", "judgment", person("ana", "w"), &["p"]);
        assert_eq!(
            place(&w_persons, "s", &session_decider("s"), None, &live),
            Err(reasons::NO_STANDING)
        );
        let p = place(&w_persons, "s", &person("bea", "s"), None, &live).unwrap();
        assert_eq!(p.path.last(), Some(&hop("p", "s")));
    }

    #[test]
    fn a_successor_gains_nothing_beside_or_above_the_fenced_session() {
        let live = hops_live(&P_TO_S, &NEVER);
        // A sibling of P under X: no hop from Q's chain reaches S.
        let sibling = body(
            "q",
            "000000000001",
            "judgment",
            session_decider("q"),
            &["x"],
        );
        assert_eq!(
            place(&sibling, "s", &session_decider("s"), None, &live),
            Err(reasons::NOT_ON_CHAIN)
        );
        // The session above P.
        let above_p = body("x", "000000000001", "judgment", session_decider("x"), &[]);
        assert_eq!(
            place(&above_p, "s", &session_decider("s"), None, &live),
            Err(reasons::NOT_ON_CHAIN)
        );
    }

    #[test]
    fn an_unconfirmed_hop_is_refused_and_never_asserted() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["p"],
        );
        let unconfirmed = |from: &str, to: &str| match (from, to) {
            ("p", "s") => chain::Reach::Unconfirmed,
            _ => chain::Reach::No,
        };
        // A broken chain would assert a reverser off it; an unconfirmed
        // hop comes first.
        for incomplete in [&NEVER, &ALWAYS] {
            let live = hops_live(&unconfirmed, incomplete);
            assert_eq!(
                place(&w, "s", &session_decider("s"), None, &live),
                Err(reasons::SUPERSEDE_UNCONFIRMED)
            );
        }
        let mut broken = w.clone();
        broken.chain.incomplete = true;
        let live = hops_live(&unconfirmed, &ALWAYS);
        assert_eq!(
            place(&broken, "s", &session_decider("s"), None, &live),
            Err(reasons::SUPERSEDE_UNCONFIRMED)
        );
        // Where no rule would match even through the hop (a session alone
        // against a person's decision), the reversal is still never
        // asserted, however broken the chain.
        let mut ruling = body("w", "000000000002", "judgment", person("ana", "w"), &["p"]);
        ruling.chain.incomplete = true;
        assert_eq!(
            place(&ruling, "s", &session_decider("s"), None, &live),
            Err(reasons::NO_STANDING)
        );
    }

    #[test]
    fn a_decision_past_the_fences_cutoff_reads_refused_and_wins_nothing() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["p"],
        );
        let before = body("p", "000000000001", "judgment", session_decider("p"), &[]);
        let after = body("p", "000000000002", "judgment", session_decider("p"), &[]);
        let late = reversal("p", "000000000003", "2026-11-10T00:00:01.000Z", &w);
        let sessions: BTreeMap<String, Vec<Entry>> = [
            ("w".to_string(), vec![Entry::Decision(Box::new(w.clone()))]),
            (
                "p".to_string(),
                [&before, &after, &late]
                    .into_iter()
                    .map(|b| Entry::Decision(Box::new(b.clone())))
                    .collect(),
            ),
        ]
        .into_iter()
        .collect();
        let cut = |through: Option<&str>| {
            Index::from_sessions_cut(
                sessions.clone(),
                [("p".to_string(), through.map(str::to_string))]
                    .into_iter()
                    .collect(),
            )
        };
        let index = cut(Some("000000000001"));
        assert_eq!(index.status(&before), "in-effect");
        assert_eq!(index.status(&after), "refused");
        assert_eq!(index.status(&late), "refused");
        assert!(index.winner_of("w-id", "000000000001").is_none());
        assert_eq!(
            index.status(&w),
            "in-effect",
            "a late reversal blocks nothing"
        );

        let index = cut(None);
        assert_eq!(index.status(&before), "refused", "a null cutoff saw none");

        // Without a marker the same reversal wins.
        let index = Index::from_sessions(sessions.clone());
        assert_eq!(index.status(&w), "reversed");
        assert_eq!(index.status(&after), "in-effect");
    }

    #[test]
    fn the_fenced_filter_keeps_reversals_with_a_hop_from_that_session() {
        let w = body(
            "w",
            "000000000001",
            "judgment",
            session_decider("w"),
            &["p"],
        );
        let mut through = reversal("s", "000000000001", "2026-11-10T00:00:01.000Z", &w);
        through.path = Some(vec![PathStep::link("w", "leg", "p"), hop("p", "s")]);
        let mut direct = reversal("p", "000000000001", "2026-11-10T00:00:01.000Z", &w);
        direct.path = Some(vec![PathStep::link("w", "leg", "p")]);
        let filters = read::Filters {
            fenced: Some("p".to_string()),
            ..read::Filters::default()
        };
        assert!(filters.matches(&through));
        assert!(!filters.matches(&direct), "a link step from p isn't a hop");
        assert!(!filters.matches(&w));
        let other = read::Filters {
            fenced: Some("s".to_string()),
            ..read::Filters::default()
        };
        assert!(!other.matches(&through));

        // A body written before hops were recorded still parses.
        let old = r#"{"session":"w","via":"leg","to":"p"}"#;
        let step: PathStep = serde_json::from_str(old).unwrap();
        assert_eq!(step, PathStep::link("w", "leg", "p"));
        assert!(!serde_json::to_string(&step).unwrap().contains("actor"));
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

    #[test]
    fn equal_times_go_to_the_reverser_name_not_the_holder() {
        let o = body(
            "c",
            "000000000001",
            "judgment",
            session_decider("c"),
            &["anchor-a", "anchor-b"],
        );
        let time = "2026-11-10T00:00:01.000Z";
        // Held in anchors whose names sort the other way round from the
        // sessions that entered the reversals through them.
        let mut by_zed = reversal("anchor-a", "000000000001", time, &o);
        by_zed.decider = session_decider("zed");
        let mut by_amy = reversal("anchor-b", "000000000001", time, &o);
        by_amy.decider = person("dana", "amy");
        for order in [[&by_zed, &by_amy], [&by_amy, &by_zed]] {
            assert!(same(winner(order).unwrap(), &by_amy));
        }
        let sessions = [("anchor-a", &by_zed), ("anchor-b", &by_amy), ("c", &o)]
            .into_iter()
            .map(|(s, b)| (s.to_string(), vec![Entry::Decision(Box::new(b.clone()))]))
            .collect();
        let index = Index::from_sessions(sessions);
        assert!(same(
            index.winner_of("c-id", "000000000001").unwrap(),
            &by_amy
        ));
        assert_eq!(index.status(&o), "reversed");
        assert_eq!(index.status(&by_amy), "in-effect");
        assert_eq!(index.status(&by_zed), "refused");
        assert!(lost_message(&by_zed, &by_amy).contains("reverser session name"));
    }
}
