//! Questions: the open read for a person and the report on `koto next`.
//!
//! A question is an `escalate` decision. Its identity is the raising
//! session's id with the decision's id, never the session's name, so a
//! later session that reuses the name raises different questions. An
//! answer is an `answer` decision recorded in the session it was given in,
//! filed under that session's `koto/aimed/<raiser's session id>/`, and
//! never written into the asker's session.
//!
//! Whether a question is open is computed every time it is read: an
//! in-effect escalation with no `answered` answer aimed at it is open.
//! Nothing records a question as closed. In this version any `answered`
//! answer closes its question, and the one in effect is the earliest by
//! time, then the answering session's name, then id. Which answers count
//! (a person's answer needs verification koto doesn't do yet) and which
//! one governs when several disagree are not settled here.
//!
//! Both reads look only at the local store, the way [`Index::scan`] does
//! with `remote` unset: no lock, no pull, no bucket listing, no write.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::{DecisionBody, Recipient};
use super::chain;
use super::read::{self, Entry};
use super::standing::Index;
use super::FORMAT_VERSION;
use crate::session::{origin, Backend};

/// Set in a test to have `report` say on stderr when it scans the store.
pub const SCAN_PROBE_ENV: &str = "KOTO_TEST_QUESTIONS_SCAN_PROBE";

/// A question's identity: `<raiser's session id>/<decision id>`.
pub fn identity(body: &DecisionBody) -> String {
    format!("{}/{}", body.session.id, body.id)
}

fn subject_str<'a>(body: &'a DecisionBody, field: &str) -> Option<&'a str> {
    body.subject.get(field).and_then(Value::as_str)
}

/// The typed recipient `body` names: an escalation's, or the one a
/// pass-up moves its question to.
pub fn recipient_of(body: &DecisionBody) -> Option<Recipient> {
    subject_str(body, "recipient").map(Recipient::parse)
}

/// Whether `body` is a question this version answers: an `escalate` with a
/// typed recipient. An escalation recorded before recipients were typed
/// isn't one, so a run holding only those reads nothing more.
pub fn is_question(body: &DecisionBody) -> bool {
    body.kind == "escalate"
        && matches!(
            recipient_of(body),
            Some(Recipient::Session(_) | Recipient::Anchor(_) | Recipient::Person(_))
        )
}

/// The questions the run's decisions cite an answer of, as `(raiser's
/// session id, question id)`.
///
/// A basis entry names a session only by name, and a name can be reused
/// after its session is removed, so each citation is resolved by reading
/// the cited decision: it counts only when it is an `answered` answer and
/// was recorded no later than the decision citing it. A later session that
/// reuses the name, and an id, therefore never stands in for the answer
/// that was cited.
fn cited_questions(backend: &Backend, decisions: &[&DecisionBody]) -> BTreeSet<(String, String)> {
    let mut sessions: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut out = BTreeSet::new();
    for d in decisions {
        for c in &d.basis {
            let entries = sessions
                .entry(c.session.clone())
                .or_insert_with(|| read::read_session_with(backend, &c.session, false));
            let cited = entries.iter().find_map(|e| match e {
                Entry::Decision(b) if b.id == c.id => Some(&**b),
                _ => None,
            });
            let Some(a) = cited else { continue };
            if a.kind != "answer" || a.outcome != "answered" || a.time > d.time {
                continue;
            }
            if let Some(t) = &a.target {
                out.insert((t.id.clone(), t.decision.clone()));
            }
        }
    }
    out
}

/// The names each session stands for: itself and the earlier members of
/// its run (the sessions it continues through a confirmed succession),
/// looked up once per session and kept for the rest of the read.
pub struct Runs<'a> {
    backend: &'a Backend,
    cache: RefCell<BTreeMap<String, Vec<String>>>,
}

impl<'a> Runs<'a> {
    pub fn new(backend: &'a Backend) -> Self {
        Runs {
            backend,
            cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// `name` and every session it continues.
    pub fn stands_for(&self, name: &str) -> Vec<String> {
        if let Some(hit) = self.cache.borrow().get(name) {
            return hit.clone();
        }
        let mut out = vec![name.to_string()];
        out.extend(chain::run_predecessors(self.backend, name));
        self.cache
            .borrow_mut()
            .insert(name.to_string(), out.clone());
        out
    }
}

/// Whether the session `recorder` (whose header id is `recorder_id`) may
/// pass `question` up while it waits on `current`: the recorder is the
/// raiser (by session id) or continues it, or it is, or continues, the
/// session or anchor `current` names.
pub fn may_pass(
    runs: &Runs,
    question: &DecisionBody,
    current: &Recipient,
    recorder: &str,
    recorder_id: &str,
) -> bool {
    if recorder_id == question.session.id {
        return true;
    }
    let stands = runs.stands_for(recorder);
    // The question is read from the raiser's own directory, so the session
    // now holding the raiser's name is the raiser.
    stands[1..].contains(&question.session.name)
        || stands.iter().any(|s| waits_on_session(current, s))
}

/// The recipients `question` has had, in order: the one it was raised
/// with, then one per pass-up that moved it. The last is the one it waits
/// on now. Computed every time it is read; nothing stores it.
///
/// The history is built by causality, not by clock: it repeatedly applies
/// the earliest (time, then the recording session's name, then id) pass-up
/// not yet applied that [`may_pass`] accepts against the current addressee
/// and that names a typed recipient other than it, until none applies. A
/// pass-up whose recorded time sorts before the one it depends on is still
/// applied once that one is. The tie-break only decides between two
/// pass-ups that are both valid against the same addressee, which can only
/// happen when they were written at once; the later one is then skipped,
/// so the history stays a single line.
pub fn addressees(index: &Index, runs: &Runs, question: &DecisionBody) -> Vec<Recipient> {
    history(index, runs, question)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// [`addressees`] with the pass-up that moved the question to each
/// recipient: `None` for the first, which it was raised with.
fn history<'i>(
    index: &'i Index,
    runs: &Runs,
    question: &DecisionBody,
) -> Vec<(Recipient, Option<&'i DecisionBody>)> {
    let Some(first) = recipient_of(question) else {
        return Vec::new();
    };
    let mut out = vec![(first, None)];
    let mut pending = index.readdresses_of(&question.session.id, &question.id);
    loop {
        let (current, _) = out.last().expect("the history starts with one recipient");
        let next = pending.iter().position(|r| {
            recipient_of(r).is_some_and(|to| !matches!(to, Recipient::Untyped(_)) && &to != current)
                && may_pass(runs, question, current, &r.session.name, &r.session.id)
        });
        let Some(i) = next else { break };
        let r = pending.remove(i);
        out.push((recipient_of(r).expect("checked above"), Some(r)));
    }
    out
}

/// The addressee history as `<kind>:<name>` strings, for an error message.
pub fn history_labels(index: &Index, runs: &Runs, question: &DecisionBody) -> Vec<String> {
    addressees(index, runs, question)
        .iter()
        .map(|r| match r {
            Recipient::Session(n) => format!("session:{n}"),
            Recipient::Anchor(n) => format!("anchor:{n}"),
            Recipient::Person(h) => format!("person:{h}"),
            Recipient::Untyped(v) => v.clone(),
        })
        .collect()
}

/// Whether `recipient` names the session or anchor called `name`.
pub fn waits_on_session(recipient: &Recipient, name: &str) -> bool {
    matches!(recipient, Recipient::Session(n) | Recipient::Anchor(n) if n == name)
}

/// Whether `b` is a question that still stands: an `escalate` with a
/// typed recipient whose status is in effect (not reversed, not past a
/// fence).
fn stands(index: &Index, b: &DecisionBody) -> bool {
    is_question(b) && index.status(b) == "in-effect"
}

/// The answer in effect for `question`: the earliest `answered` answer
/// aimed at it, by time, then the answering session's name, then id.
pub fn answer_in_effect<'i>(index: &'i Index, question: &DecisionBody) -> Option<&'i DecisionBody> {
    index
        .answers_of(&question.session.id, &question.id)
        .into_iter()
        .next()
}

/// Whether `b` is an open question: one that stands with no answer in
/// effect.
///
/// This is koto's one test of open. The open reads, the terminal tick's
/// retention, the retention engine's keepers and the cleanup refusal all
/// come here (the last three through [`raised_open`] and [`holds_of`]), so
/// a change to what counts as open, or to which answer is in effect,
/// reaches every one of them at once.
pub fn is_open(index: &Index, b: &DecisionBody) -> bool {
    stands(index, b) && answer_in_effect(index, b).is_none()
}

/// The open questions in the local store: in-effect escalations with a
/// typed recipient that no answer has closed, each with its addressee
/// history, sorted by time, then the raising session's name, then id.
fn open_questions<'i>(index: &'i Index, runs: &Runs) -> Vec<(&'i DecisionBody, Vec<Recipient>)> {
    let mut found: Vec<(&DecisionBody, Vec<Recipient>)> = index
        .sessions()
        .values()
        .flatten()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
        .filter(|b| is_open(index, b))
        .map(|b| (b, addressees(index, runs, b)))
        .collect();
    found.sort_by(|(a, _), (b, _)| {
        (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id))
    });
    found
}

/// One element of an open read: the question, the recipient it was raised
/// with, the one it waits on now and every one it has had.
fn element(b: &DecisionBody, history: &[Recipient]) -> Value {
    json!({
        "identity": identity(b),
        "session": b.session.name,
        "id": b.id,
        "question": subject_str(b, "question"),
        "text": subject_str(b, "text"),
        "recipient": recipient_of(b).map(|r| r.to_json()),
        "addressee": history.last().map(Recipient::to_json),
        "addressees": history.iter().map(Recipient::to_json).collect::<Vec<_>>(),
        "time": b.time,
    })
}

/// `koto decisions open --person <handle>`: every open question in the
/// local store that waits on `person:<handle>` now, whoever it was raised
/// to. It walks no chain: a question is found wherever it was raised,
/// whatever links its session has or lacks.
pub fn open(backend: &Backend, person: &str) -> Value {
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    let want = Recipient::Person(person.to_string());
    let questions: Vec<Value> = open_questions(&index, &runs)
        .iter()
        .filter(|(_, h)| h.last() == Some(&want))
        .map(|(b, h)| element(b, h))
        .collect();
    json!({
        "v": FORMAT_VERSION,
        "person": person,
        "questions": questions,
    })
}

/// `koto decisions open --at <session-or-anchor>`: every open question in
/// the local store raised at the position, waiting on it now, or raised in
/// a session below it, each listed once with how the position stands to
/// it in `relation`.
///
/// The position is the named session or anchor together with the earlier
/// members of its run (the sessions it continues through koto's confirmed
/// succession), so a coordinator's successor holds what was addressed to,
/// raised in or raised below its predecessor. "Below" is the downward walk
/// of [`origin::below`] over every session in the local store: a session
/// is directly below the dispatcher `koto session origin` names for it and
/// below the anchor holding it, matched by name.
///
/// `relation` is `both` when the question was raised at the position and
/// waits on it, `raised` or `addressee` when only one holds, and `below`
/// when it was raised below the position and waits elsewhere.
pub fn open_at(backend: &Backend, position: &str) -> Value {
    let index = Index::scan(backend, false);
    let mut at: BTreeSet<String> = BTreeSet::new();
    at.insert(position.to_string());
    at.extend(chain::run_predecessors(backend, position));
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    let below = origin::below(local, &read::local_sessions(backend), &at);
    let runs = Runs::new(backend);
    let questions: Vec<Value> = open_questions(&index, &runs)
        .iter()
        .filter_map(|(b, h)| {
            let raised = at.contains(&b.session.name);
            let addressed = h
                .last()
                .is_some_and(|r| at.iter().any(|p| waits_on_session(r, p)));
            let relation = match (raised, addressed) {
                (true, true) => "both",
                (true, false) => "raised",
                (false, true) => "addressee",
                (false, false) if below.contains(&b.session.name) => "below",
                (false, false) => return None,
            };
            let mut e = element(b, h);
            e["relation"] = json!(relation);
            Some(e)
        })
        .collect();
    json!({
        "v": FORMAT_VERSION,
        "at": position,
        "questions": questions,
    })
}

/// The `questions` array `koto next <session>` carries, or `None` when it
/// carries none.
///
/// The questions reported are the ones raised in `session` or in an
/// earlier member of its run (its confirmed predecessors through koto's
/// succession) that an answer has closed and that no decision in the run
/// cites yet: a decision whose `basis` names an answer of the question,
/// recorded no later than that decision, ends the report. Each element
/// names the answer in effect.
///
/// The store is scanned only while the run holds a question no decision
/// in it has cited an answer of. A run with no question, or with only
/// untyped escalations, or whose every question is cited, costs a read of
/// its own decision directories, its continues link and the decisions it
/// cites, and nothing more. The read never writes, never takes a lock and
/// makes no network call, so it never blocks the tick it rides on.
pub fn report(backend: &Backend, session: &str) -> Option<Value> {
    let mut run = vec![session.to_string()];
    run.extend(chain::run_predecessors(backend, session));
    let entries: Vec<Entry> = run
        .iter()
        .flat_map(|s| read::read_session_with(backend, s, false))
        .collect();
    let decisions: Vec<&DecisionBody> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
        .collect();
    if !decisions.iter().any(|b| is_question(b)) {
        return None;
    }
    let cited = cited_questions(backend, &decisions);
    let pending: Vec<&DecisionBody> = decisions
        .iter()
        .copied()
        .filter(|q| is_question(q) && !cited.contains(&(q.session.id.clone(), q.id.clone())))
        .collect();
    if pending.is_empty() {
        return None;
    }
    if std::env::var_os(SCAN_PROBE_ENV).is_some() {
        eprintln!("koto: questions: store scanned");
    }
    let index = Index::scan(backend, false);
    let mut out = Vec::new();
    for q in pending {
        if !stands(&index, q) {
            continue;
        }
        let Some(answer) = answer_in_effect(&index, q) else {
            continue;
        };
        out.push(json!({
            "identity": identity(q),
            "session": q.session.name,
            "id": q.id,
            "question": subject_str(q, "question"),
            "state": "answered",
            "answer": {
                "session": answer.session.name,
                "id": answer.id,
                "text": subject_str(answer, "text"),
                "decider": answer.decider,
                "run_mode": answer.authority.run_mode,
                "time": answer.time,
            },
        }));
    }
    (!out.is_empty()).then_some(Value::Array(out))
}

/// What a session holds that an open question depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Held {
    /// It raised the question, which is open.
    Raised,
    /// It holds the answer in effect for the question.
    Answer,
    /// It holds a pass-up the question's current addressee rests on.
    PassUp,
}

impl Held {
    /// The `held` value a refusal and `orphaned_questions` print.
    pub fn as_str(self) -> &'static str {
        match self {
            Held::Raised => "raised",
            Held::Answer => "answer",
            Held::PassUp => "pass_up",
        }
    }

    /// What removing the session loses for the question.
    pub fn loss(self) -> &'static str {
        match self {
            Held::Raised => "the question itself: no open read returns it once its raiser is gone",
            Held::Answer => "the answer in effect: the question reads open again",
            Held::PassUp => "a pass-up: the question waits on the addressee it had before it",
        }
    }

    /// What the session holds of question `identity`, for a refusal's
    /// message: `it <describe>`.
    pub fn describe(self, identity: &str) -> String {
        match self {
            Held::Raised => format!("raised the open question {identity}"),
            Held::Answer => format!("holds the answer in effect for the question {identity}"),
            Held::PassUp => format!(
                "holds a pass-up that the current addressee of the question {identity} rests on"
            ),
        }
    }
}

/// One question a session holds something of, by its identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hold {
    pub identity: String,
    pub held: Held,
}

/// The readable decisions the scan read in session directory `session`.
fn decisions_in<'i>(index: &'i Index, session: &str) -> impl Iterator<Item = &'i DecisionBody> {
    index
        .sessions()
        .get(session)
        .into_iter()
        .flatten()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
}

/// The question `id` raised in the session whose header id is
/// `session_id`, when that session is in the store.
fn question_of<'i>(index: &'i Index, session_id: &str, id: &str) -> Option<&'i DecisionBody> {
    index
        .sessions()
        .values()
        .flatten()
        .find_map(|e| match e {
            Entry::Decision(b) if b.session.id == session_id && b.id == id && is_question(b) => {
                Some(&**b)
            }
            _ => None,
        })
}

fn same_entry(a: &DecisionBody, b: &DecisionBody) -> bool {
    a.session.id == b.session.id && a.id == b.id
}

/// The identities of the open questions raised in session directory
/// `session`, in the order it recorded them.
pub fn raised_open(index: &Index, session: &str) -> Vec<String> {
    decisions_in(index, session)
        .filter(|b| is_open(index, b))
        .map(identity)
        .collect()
}

/// Every question session directory `session` holds something of that its
/// removal would lose: an open question it raised, the answer in effect
/// for a question whose raiser is in the store, and a pass-up the current
/// addressee of an open question rests on. Each is matched by identity,
/// never by name, so an answer aimed at a removed session's id holds
/// nothing once that session is gone, whatever now carries its name.
pub fn holds_of(index: &Index, runs: &Runs, session: &str) -> Vec<Hold> {
    let mut out: Vec<Hold> = raised_open(index, session)
        .into_iter()
        .map(|identity| Hold {
            identity,
            held: Held::Raised,
        })
        .collect();
    for b in decisions_in(index, session) {
        let Some(t) = &b.target else { continue };
        let held = match (b.kind.as_str(), b.outcome.as_str()) {
            ("answer", "answered") => Held::Answer,
            ("readdress", "passed") => Held::PassUp,
            _ => continue,
        };
        let Some(q) = question_of(index, &t.id, &t.decision) else {
            continue;
        };
        let holds = match held {
            Held::Answer => {
                stands(index, q) && answer_in_effect(index, q).is_some_and(|a| same_entry(a, b))
            }
            Held::PassUp => {
                is_open(index, q)
                    && history(index, runs, q)
                        .iter()
                        .any(|(_, r)| r.is_some_and(|r| same_entry(r, b)))
            }
            Held::Raised => false,
        };
        if holds {
            out.push(Hold {
                identity: identity(q),
                held,
            });
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether session `session`'s own decisions could hold anything
/// [`holds_of`] counts: a question, or (with `aimed`) an answer or pass-up
/// aimed at one. A session with neither never pays for a scan of the
/// store.
fn may_hold(backend: &Backend, session: &str, aimed: bool) -> bool {
    read::read_session_with(backend, session, false)
        .iter()
        .any(|e| match e {
            Entry::Decision(b) => {
                is_question(b)
                    || (aimed
                        && b.target.is_some()
                        && matches!(b.kind.as_str(), "answer" | "readdress"))
            }
            Entry::Unreadable { .. } => false,
        })
}

/// The open questions session `session` raised, for its terminal tick:
/// empty, at the cost of a read of its own decisions, when it raised none.
pub fn raised_open_in(backend: &Backend, session: &str) -> Vec<String> {
    if !may_hold(backend, session, false) {
        return Vec::new();
    }
    raised_open(&Index::scan(backend, false), session)
}

/// What an explicit removal of session `session` would orphan
/// ([`holds_of`]), read from the local store only: empty, at the cost of a
/// read of its own decisions, when it holds no question and no aimed
/// answer or pass-up.
pub fn orphans(backend: &Backend, session: &str) -> Vec<Hold> {
    if !may_hold(backend, session, true) {
        return Vec::new();
    }
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    holds_of(&index, &runs, session)
}

/// The open questions each session directory in the local store raised,
/// for the retention engine; sessions that raised none are left out.
pub fn open_by_session(backend: &Backend) -> BTreeMap<String, Vec<String>> {
    let index = Index::scan(backend, false);
    index
        .sessions()
        .keys()
        .filter_map(|s| {
            let open = raised_open(&index, s);
            (!open.is_empty()).then(|| (s.clone(), open))
        })
        .collect()
}
