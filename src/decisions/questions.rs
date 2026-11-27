//! Questions: the open reads, the report on `koto next`, and the one
//! read-time resolution every read and every writer applies.
//!
//! A question is an `escalate` decision with a typed recipient. Its
//! identity is the raising session's id with the decision's id, never the
//! session's name, so a later session that reuses the name raises
//! different questions. Every entry aimed at a question (an `answer` with
//! outcome `answered`, `settled` or `withdrawn`, or a `readdress`) is
//! recorded in the session it was given in, filed under that session's
//! `koto/aimed/<raiser's session id>/`; a settle or a withdrawal is given
//! in the raiser, so it sits in the raiser's own aimed area.
//!
//! Nothing records a question as closed. [`resolve`] works it out on every
//! read, in this order:
//!
//! 1. The addressee history: the recipient the question was raised with,
//!    then one per pass-up that moves it (the causal walk [`addressees`]
//!    describes), where a pass-up to a session or anchor must also name
//!    the dispatcher it recorded when it was written. A pass-up left out
//!    reads not counted.
//! 2. The closing entries, each judged against the addressee the question
//!    has now: a verified person answer always counts; a session's
//!    `answered` answer counts only from the session or anchor the
//!    question waits on, or a session continuing it, and never while it
//!    waits on a person; a `withdrawn` or `settled` entry counts only from
//!    the raiser or a session continuing it, by a decider that isn't a
//!    person, a withdrawal with a reason and a settle with the evidence
//!    its question needs. An entry that doesn't count is still returned,
//!    with the reason.
//! 3. The entry in effect: among counted verified person answers, the one
//!    no other cites, the later in time when several are uncited; with
//!    none, the earliest counted entry by time, then the recording
//!    session's name, then id.
//! 4. Open: a question in effect with no entry in effect.
//!
//! Every read looks only at the local store, the way [`Index::scan`] does
//! with `remote` unset: no lock, no pull, no bucket listing, no write.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::{BasisRef, DecisionBody, Recipient, Target};
use super::chain;
use super::decider::Decider;
use super::read::{self, Entry};
use super::standing::{self, Index};
use super::FORMAT_VERSION;
use crate::session::{origin, Backend};

/// Set in a test to have `report` say on stderr when it scans the store.
pub const SCAN_PROBE_ENV: &str = "KOTO_TEST_QUESTIONS_SCAN_PROBE";

/// The most open questions one raising session, with the sessions it
/// continues, holds waiting on a person, across every handle.
pub const PERSON_LIMIT: usize = 20;

/// Why an entry aimed at a question doesn't count.
pub mod reasons {
    /// A session's answer from a session that isn't, and doesn't
    /// continue, the session or anchor the question waits on now.
    pub const NOT_ADDRESSEE: &str = "not_addressee";
    /// A session's answer to a question that waits on a person now.
    pub const WAITS_ON_PERSON: &str = "waits_on_person";
    /// A settle or a withdrawal recorded outside the raiser and the
    /// sessions continuing it.
    pub const NOT_RAISER: &str = "not_raiser";
    /// A settle or a withdrawal by a person decider.
    pub const PERSON_DECIDER: &str = "person_decider";
    /// A withdrawal with no reason.
    pub const NO_REASON: &str = "no_reason";
    /// A settle of a `credential` or `reserved-step` need.
    pub const NEED: &str = "need_not_settleable";
    /// A settle without the evidence its question needs.
    pub const SETTLE_BASIS: &str = "settle_basis";
    /// An entry whose question isn't in the local store, or isn't a
    /// question.
    pub const NO_QUESTION: &str = "no_question";
    /// A verified person answer the store reads as in effect on a question
    /// it no longer holds ([`super::gone_question_answer`]). It is the one
    /// reason an entry carries while it counts: it says the question it
    /// counts toward is gone.
    pub const QUESTION_GONE: &str = "question_gone";
    /// An entry written in a fenced session after its fence: it reads
    /// `refused` and counts toward nothing.
    pub const AFTER_FENCE: &str = "after_fence";
    /// A pass-up to a session or anchor other than the dispatcher it
    /// recorded.
    pub const DESTINATION: &str = "destination";
    /// A pass-up the history's ordering leaves out: one recorded by a
    /// session that was neither the raiser nor the addressee at that
    /// point, one naming the addressee the question already had, or the
    /// later of two written against the same addressee.
    pub const SKIPPED: &str = "skipped";
}

/// A question's identity: `<raiser's session id>/<decision id>`.
pub fn identity(body: &DecisionBody) -> String {
    format!("{}/{}", body.session.id, body.id)
}

pub(crate) fn subject_str<'a>(body: &'a DecisionBody, field: &str) -> Option<&'a str> {
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

/// What a question asks for: its `need` subject field, or `decision` for
/// one raised without it.
pub fn need_of(question: &DecisionBody) -> &str {
    subject_str(question, "need").unwrap_or("decision")
}

/// Whether `question` is a need its raiser can't settle for itself.
pub fn unsettleable(question: &DecisionBody) -> bool {
    need_of(question) != "decision"
}

/// A question's `label` subject field, when it has one.
pub fn label_of(question: &DecisionBody) -> Option<&str> {
    subject_str(question, "label")
}

/// The handle of a person decider.
pub fn person_handle(body: &DecisionBody) -> Option<&str> {
    match &body.decider {
        Decider::Person { handle, .. } => Some(handle.value.as_str()),
        _ => None,
    }
}

pub(crate) fn same(a: &DecisionBody, b: &DecisionBody) -> bool {
    a.session.id == b.session.id && a.id == b.id
}

pub(crate) fn order_key(b: &DecisionBody) -> (&str, &str, &str) {
    (&b.time, &b.session.name, &b.id)
}

/// `bodies` with each decision once, by identity: a copy held in a second
/// session is the same entry.
fn distinct(bodies: Vec<&DecisionBody>) -> Vec<&DecisionBody> {
    let mut seen = BTreeSet::new();
    bodies
        .into_iter()
        .filter(|b| seen.insert((b.session.id.clone(), b.id.clone())))
        .collect()
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

    /// The backend the runs are read from.
    pub fn backend(&self) -> &'a Backend {
        self.backend
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

/// Whether the session `recorder` (whose header id is `recorder_id`) is
/// `question`'s raiser, by session id, or continues it.
pub fn raised_by(runs: &Runs, question: &DecisionBody, recorder: &str, recorder_id: &str) -> bool {
    // The question is read from the raiser's own directory, so the session
    // now holding the raiser's name is the raiser.
    recorder_id == question.session.id
        || runs.stands_for(recorder)[1..].contains(&question.session.name)
}

/// Whether the session `recorder` is, or continues, the session or anchor
/// `current` names.
pub fn addressed_to(runs: &Runs, current: &Recipient, recorder: &str) -> bool {
    runs.stands_for(recorder)
        .iter()
        .any(|s| waits_on_session(current, s))
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
    raised_by(runs, question, recorder, recorder_id) || addressed_to(runs, current, recorder)
}

/// The dispatcher the destination rule accepts for a pass-up recorded in
/// `recorder`: the session its nearest dispatched-by link names, or, for a
/// successor with no link of its own, that of the nearest session it
/// continues that has one.
pub fn dispatcher_for(backend: &Backend, recorder: &str) -> Option<String> {
    chain::dispatcher(backend, recorder).or_else(|| {
        chain::run_predecessors(backend, recorder)
            .iter()
            .find_map(|p| chain::dispatcher(backend, p))
    })
}

/// The dispatcher a stored pass-up was judged against when it was
/// written: its `dispatcher` subject field, or, for a pass-up written
/// before koto recorded one, the dispatcher its authority names.
fn recorded_dispatcher(pass: &DecisionBody) -> Option<&str> {
    subject_str(pass, "dispatcher").or_else(|| {
        let d = pass.authority.dispatcher.as_str();
        (!d.is_empty() && d != "none").then_some(d)
    })
}

/// Whether a pass-up's destination keeps the rule: a person, or the
/// session or anchor its recorded dispatcher names. Judged from what the
/// entry recorded, so a later change to the recorder's links moves no
/// question.
pub fn destination_ok(to: &Recipient, dispatcher: Option<&str>) -> bool {
    match to {
        Recipient::Person(_) => true,
        Recipient::Session(n) | Recipient::Anchor(n) => dispatcher == Some(n.as_str()),
        Recipient::Untyped(_) => false,
    }
}

/// The addressee history and the pass-ups that built it; see
/// [`addressees`].
fn walk<'i>(
    index: &'i Index,
    runs: &Runs,
    question: &DecisionBody,
) -> (Vec<Recipient>, Vec<&'i DecisionBody>) {
    let Some(first) = recipient_of(question) else {
        return (Vec::new(), Vec::new());
    };
    let mut history = vec![first];
    let mut applied = Vec::new();
    let mut pending = distinct(index.readdresses_of(&question.session.id, &question.id));
    loop {
        let current = history
            .last()
            .expect("the history starts with one recipient");
        let next = pending.iter().position(|r| {
            recipient_of(r).is_some_and(|to| {
                !matches!(to, Recipient::Untyped(_))
                    && &to != current
                    && destination_ok(&to, recorded_dispatcher(r))
            }) && may_pass(runs, question, current, &r.session.name, &r.session.id)
        });
        let Some(i) = next else { break };
        let r = pending.remove(i);
        history.push(recipient_of(r).expect("checked above"));
        applied.push(r);
    }
    (history, applied)
}

/// The recipients `question` has had, in order: the one it was raised
/// with, then one per pass-up that moved it. The last is the one it waits
/// on now. Computed every time it is read; nothing stores it.
///
/// The history is built by causality, not by clock: it repeatedly applies
/// the earliest (time, then the recording session's name, then id) pass-up
/// not yet applied that [`may_pass`] accepts against the current addressee,
/// that names a typed recipient other than it, and whose destination keeps
/// the rule ([`destination_ok`]), until none applies. A pass-up whose
/// recorded time sorts before the one it depends on is still applied once
/// that one is. The tie-break only decides between two pass-ups that are
/// both valid against the same addressee, which can only happen when they
/// were written at once; the later one is then skipped, so the history
/// stays a single line.
pub fn addressees(index: &Index, runs: &Runs, question: &DecisionBody) -> Vec<Recipient> {
    walk(index, runs, question).0
}

/// The addressee history as `<kind>:<name>` strings, for an error message.
pub fn history_labels(index: &Index, runs: &Runs, question: &DecisionBody) -> Vec<String> {
    addressees(index, runs, question)
        .iter()
        .map(label)
        .collect()
}

/// A recipient as `<kind>:<name>`.
pub fn label(r: &Recipient) -> String {
    match r {
        Recipient::Session(n) => format!("session:{n}"),
        Recipient::Anchor(n) => format!("anchor:{n}"),
        Recipient::Person(h) => format!("person:{h}"),
        Recipient::Untyped(v) => v.clone(),
    }
}

/// Whether `recipient` names the session or anchor called `name`.
pub fn waits_on_session(recipient: &Recipient, name: &str) -> bool {
    matches!(recipient, Recipient::Session(n) | Recipient::Anchor(n) if n == name)
}

/// Whether `answer` is a person's answer this host verified: an
/// `answered` answer by a person decider carrying a prompt that the
/// host-local note of one of its holders lists. A copy that arrived by
/// import or by a pull reads as a remote session's answer instead, and is
/// judged as an answer of the session holding it.
pub fn verified_person(index: &Index, backend: &Backend, answer: &DecisionBody) -> bool {
    answer.kind == "answer"
        && answer.outcome == "answered"
        && answer.prompt.is_some()
        && person_handle(answer).is_some()
        && index
            .holders(answer)
            .iter()
            .any(|h| standing::verified_in(backend, h, answer))
}

/// The person answer in effect among `persons`: the one no other of them
/// cites in its basis, the later by time (then session name, then id) when
/// several are uncited, and the later of all of them when every one is
/// cited.
fn person_winner<'i>(persons: &[&'i DecisionBody]) -> Option<&'i DecisionBody> {
    let uncited: Vec<&'i DecisionBody> = persons
        .iter()
        .copied()
        .filter(|p| !persons.iter().any(|o| !same(o, p) && cites(o, p)))
        .collect();
    let pool = if uncited.is_empty() {
        persons.to_vec()
    } else {
        uncited
    };
    pool.into_iter()
        .max_by(|a, b| order_key(a).cmp(&order_key(b)))
}

/// The verified person answer in effect on the question whose identity is
/// `(raiser's session id, question id)`, if any.
pub fn person_in_effect<'i>(
    index: &'i Index,
    backend: &Backend,
    session_id: &str,
    id: &str,
) -> Option<&'i DecisionBody> {
    let persons: Vec<&DecisionBody> = distinct(index.answers_of(session_id, id))
        .into_iter()
        .filter(|a| verified_person(index, backend, a))
        .collect();
    person_winner(&persons)
}

/// The verified person answer in effect on the question whose identity is
/// `(raiser's session id, question id)`, read for a question the local
/// store no longer holds because its raiser was removed.
///
/// A raiser's settles and withdrawals sat in the raiser's own area and went
/// with it, and a verified person answer wins over any session's answer, so
/// the person answer in effect ([`person_in_effect`]) is the answer in
/// effect, except when what is left still says otherwise:
///
/// - a reversal of the question stored anywhere this store reads (one
///   recorded `--in` another session) withdraws every answer to it;
/// - an answer of any kind aimed at the question that cites the person
///   answer in its `basis` has replaced it, whether or not that answer reads
///   as verified here.
///
/// What this can't see: a reversal recorded in the raiser itself went with
/// the raiser, and a replacing answer whose session was forced out (retention
/// keeps a session holding a verified person answer otherwise) leaves the
/// one it replaced looking in effect.
pub fn gone_question_answer<'i>(
    index: &'i Index,
    backend: &Backend,
    session_id: &str,
    id: &str,
) -> Option<&'i DecisionBody> {
    if index.winner_of(session_id, id).is_some() {
        return None;
    }
    let w = person_in_effect(index, backend, session_id, id)?;
    let replaced = index
        .answers_of(session_id, id)
        .into_iter()
        .any(|o| !same(o, w) && cites(o, w));
    (!replaced).then_some(w)
}

/// Whether `by` names `p` in its `basis`.
fn cites(by: &DecisionBody, p: &DecisionBody) -> bool {
    by.basis
        .iter()
        .any(|c| c.id == p.id && c.session == p.session.name)
}

/// The question `target` names, as the local store holds it: read from
/// the raiser's directory when its session id matches, else from any copy
/// carrying that identity.
pub fn question_of<'i>(index: &'i Index, target: &Target) -> Option<&'i DecisionBody> {
    let matches = |b: &DecisionBody| b.session.id == target.id && b.id == target.decision;
    if let Some(Entry::Decision(b)) = index.find(&target.name, &target.decision) {
        if matches(b) {
            return Some(b);
        }
    }
    index.sessions().values().flatten().find_map(|e| match e {
        Entry::Decision(b) if matches(b) => Some(&**b),
        _ => None,
    })
}

/// Why a settle of `question` doesn't stand, or `Ok` when it does.
///
/// A question that waits on `person:<h>` now is settled only from a basis
/// naming a verified answer of `<h>`'s that is in effect on another
/// question carrying the same `label`; when the store no longer holds that
/// question (its raiser was removed), the label koto copied onto the
/// answer stands in for the question's. One that waits on a session or
/// anchor is settled from a basis naming a decision that reads back and
/// is neither the question itself nor an entry aimed at it, or an `event`
/// evidence pointer. A cited person answer stands only while the question
/// it answers reads in effect, so reversing that question withdraws the
/// basis. A cited decision recorded after `at` (the
/// settle's time) is never taken for the one cited, so a later session
/// reusing a removed session's name, and an id, stands in for nothing.
pub fn settle_basis(
    index: &Index,
    backend: &Backend,
    question: &DecisionBody,
    current: &Recipient,
    basis: &[BasisRef],
    evidence: &[Value],
    at: &str,
) -> Result<(), String> {
    let cited = |c: &BasisRef| match index.find(&c.session, &c.id) {
        Some(Entry::Decision(b)) if b.time.as_str() <= at => Some(&**b),
        _ => None,
    };
    let Recipient::Person(handle) = current else {
        let event = evidence
            .iter()
            .any(|e| e.get("kind").and_then(Value::as_str) == Some("event"));
        if event {
            return Ok(());
        }
        // The question itself, and an entry aimed at it (an answer, a
        // settle, a withdrawal, a pass-up or a reversal of it), are about
        // the question, not evidence for its answer.
        let about_question = |b: &DecisionBody| {
            same(b, question)
                || b.target
                    .as_ref()
                    .is_some_and(|t| t.id == question.session.id && t.decision == question.id)
        };
        let mut why = Vec::new();
        for c in basis {
            let named = format!("{}:{}", c.session, c.id);
            match cited(c) {
                None => why.push(format!("{named} doesn't read back")),
                Some(b) if same(b, question) => why.push(format!("{named} is the question itself")),
                Some(b) if about_question(b) => {
                    why.push(format!("{named} is an entry aimed at the question"))
                }
                Some(_) => return Ok(()),
            }
        }
        let tail = if why.is_empty() {
            String::new()
        } else {
            format!(": {}", why.join("; "))
        };
        return Err(format!(
            "a settle of a question waiting on {} needs a --basis naming a decision that reads \
             back and isn't the question or an entry aimed at it, or an --evidence \
             event:<session>:<seq> pointer{tail}",
            label(current)
        ));
    };
    let Some(own_label) = label_of(question) else {
        return Err(format!(
            "the question waits on person:{handle} and has no label, so no answer to another \
             question settles it"
        ));
    };
    if basis.is_empty() {
        return Err(format!(
            "a settle of a question waiting on person:{handle} needs a --basis naming an answer \
             of person:{handle}'s that is in effect on a question labelled '{own_label}'"
        ));
    }
    let mut why = Vec::new();
    for c in basis {
        let named = format!("{}:{}", c.session, c.id);
        let Some(a) = cited(c) else {
            why.push(format!("{named} doesn't read back"));
            continue;
        };
        if a.kind != "answer" || a.outcome != "answered" || a.prompt.is_none() {
            why.push(format!("{named} isn't a person's answer"));
            continue;
        }
        match person_handle(a) {
            Some(h) if h == handle => {}
            Some(h) => {
                why.push(format!(
                    "{named} is an answer of person:{h}, not person:{handle}"
                ));
                continue;
            }
            None => {
                why.push(format!("{named} isn't a person's answer"));
                continue;
            }
        }
        if !standing::verified_in(backend, &c.session, a) {
            why.push(format!("{named} reads as a remote session's answer"));
            continue;
        }
        let Some(t) = &a.target else {
            why.push(format!("{named} answers no question"));
            continue;
        };
        let held = question_of(index, t);
        let in_effect = match held {
            Some(_) => person_in_effect(index, backend, &t.id, &t.decision),
            None => gone_question_answer(index, backend, &t.id, &t.decision),
        };
        if in_effect.is_none_or(|w| !same(w, a)) {
            why.push(format!(
                "{named} isn't the answer in effect on its own question"
            ));
            continue;
        }
        // A question removed with its raiser stands on the label koto
        // copied onto its answer. A reversal of it recorded in another
        // session, or an answer citing this one as replaced, still
        // withdraws it (`gone_question_answer`); one recorded in the raiser
        // went with the raiser.
        let answered_label = match held {
            Some(answered) => {
                let status = index.status(answered);
                if status != "in-effect" {
                    why.push(format!(
                        "{named} answers a question that reads '{status}', not in effect"
                    ));
                    continue;
                }
                label_of(answered)
            }
            None => subject_str(a, "label"),
        };
        match answered_label {
            Some(l) if l == own_label => return Ok(()),
            Some(l) => why.push(format!(
                "{named} answers a question labelled '{l}', not '{own_label}'"
            )),
            None => why.push(format!("{named} answers a question with no label")),
        }
    }
    Err(format!(
        "no --basis settles a question waiting on person:{handle}: {}",
        why.join("; ")
    ))
}

/// One closing entry, judged.
#[derive(Debug, Clone)]
pub struct Judged<'i> {
    pub entry: &'i DecisionBody,
    pub counted: bool,
    pub reason: Option<&'static str>,
    /// A verified person answer.
    pub person: bool,
}

/// What [`resolve`] works out for one question.
#[derive(Debug, Clone)]
pub struct Resolution<'i> {
    /// The recipients the question has had; the last is the one it waits
    /// on now.
    pub history: Vec<Recipient>,
    /// The pass-ups that built the history.
    pub applied: Vec<&'i DecisionBody>,
    /// Every answer aimed at the question, once each, judged.
    pub closing: Vec<Judged<'i>>,
    /// The closing entry in effect: `None` while the question is open.
    pub in_effect: Option<&'i DecisionBody>,
}

impl Resolution<'_> {
    /// The recipient the question waits on now.
    pub fn current(&self) -> Option<&Recipient> {
        self.history.last()
    }
}

/// Judge one closing entry of `question`, whose current addressee is
/// `current`: whether it counts, why not, and whether it is a verified
/// person answer.
fn judge_closing(
    index: &Index,
    runs: &Runs,
    question: &DecisionBody,
    current: &Recipient,
    e: &DecisionBody,
) -> (bool, Option<&'static str>, bool) {
    let backend = runs.backend();
    let holders = index.holders(e);
    match e.outcome.as_str() {
        "answered" => {
            if verified_person(index, backend, e) {
                return (true, None, true);
            }
            if matches!(current, Recipient::Person(_)) {
                return (false, Some(reasons::WAITS_ON_PERSON), false);
            }
            if holders.iter().any(|h| addressed_to(runs, current, h)) {
                (true, None, false)
            } else {
                (false, Some(reasons::NOT_ADDRESSEE), false)
            }
        }
        "withdrawn" | "settled" => {
            if !holders
                .iter()
                .any(|h| raised_by(runs, question, h, &e.session.id))
            {
                return (false, Some(reasons::NOT_RAISER), false);
            }
            if person_handle(e).is_some() {
                return (false, Some(reasons::PERSON_DECIDER), false);
            }
            if e.outcome == "withdrawn" {
                return if e.reason.as_deref().is_some_and(|r| !r.trim().is_empty()) {
                    (true, None, false)
                } else {
                    (false, Some(reasons::NO_REASON), false)
                };
            }
            if unsettleable(question) {
                return (false, Some(reasons::NEED), false);
            }
            match settle_basis(
                index,
                backend,
                question,
                current,
                &e.basis,
                &e.evidence,
                &e.time,
            ) {
                Ok(()) => (true, None, false),
                Err(_) => (false, Some(reasons::SETTLE_BASIS), false),
            }
        }
        _ => (false, Some(reasons::NO_QUESTION), false),
    }
}

/// Work out `question`'s addressee history, which entries aimed at it
/// count, and which one is in effect. This is koto's one computation of
/// what closes a question: every open read, the `koto next` report, the
/// `counted` and `governs` fields of a listing and every writer's check
/// apply it.
pub fn resolve<'i>(index: &'i Index, runs: &Runs, question: &DecisionBody) -> Resolution<'i> {
    let (history, applied) = walk(index, runs, question);
    let Some(current) = history.last().cloned() else {
        return Resolution {
            history,
            applied,
            closing: Vec::new(),
            in_effect: None,
        };
    };
    let closing: Vec<Judged> = distinct(index.answers_of(&question.session.id, &question.id))
        .into_iter()
        .map(|e| {
            let (counted, reason, person) = judge_closing(index, runs, question, &current, e);
            Judged {
                entry: e,
                counted,
                reason,
                person,
            }
        })
        .collect();
    let persons: Vec<&DecisionBody> = closing
        .iter()
        .filter(|j| j.counted && j.person)
        .map(|j| j.entry)
        .collect();
    let in_effect = person_winner(&persons).or_else(|| {
        closing
            .iter()
            .filter(|j| j.counted)
            .map(|j| j.entry)
            .min_by(|a, b| order_key(a).cmp(&order_key(b)))
    });
    Resolution {
        history,
        applied,
        closing,
        in_effect,
    }
}

/// The closing entry in effect on `question` (an `answered`, `settled` or
/// `withdrawn` answer that counts and wins under [`resolve`]), or `None`
/// while nothing closes it.
///
/// This is the public entry point for "what closes this question". Every
/// caller outside this module that needs it calls this, never its own
/// reading of the aimed entries: retention and cleanup call it, as the open
/// reads and the `koto next` report do. [`Index::answers_of`] returns every
/// entry aimed at a question, whatever its outcome and whether it counts,
/// so taking the first of them as closing is wrong.
pub fn answer_in_effect<'i>(
    index: &'i Index,
    runs: &Runs,
    question: &DecisionBody,
) -> Option<&'i DecisionBody> {
    resolve(index, runs, question).in_effect
}

/// koto's one test of open: `body` is a question (an `escalate` with a
/// typed recipient), it reads in effect, and [`answer_in_effect`] finds
/// nothing closing it.
///
/// This is the public entry point for "is this question open". The open
/// reads and the limit of twenty go through it, and retention and cleanup
/// call it rather than a test of their own, so the rule has one place to
/// change. The `koto next` report, which needs the closing entry itself,
/// calls [`answer_in_effect`].
pub fn is_open(index: &Index, runs: &Runs, body: &DecisionBody) -> bool {
    is_question(body)
        && index.status(body) == "in-effect"
        && answer_in_effect(index, runs, body).is_none()
}

/// How a listing shows an `answer` or `readdress` entry: whether it counts
/// toward its question, the reason when it doesn't, and, on an answer,
/// whether it is the entry in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryJudgment {
    pub counted: bool,
    pub reason: Option<&'static str>,
    pub governs: Option<bool>,
}

/// Judge the stored `answer` or `readdress` `body` against its question as
/// the local store holds it now.
pub fn judge_entry(index: &Index, backend: &Backend, body: &DecisionBody) -> EntryJudgment {
    let answer = body.kind == "answer";
    if index.after_fence(body) {
        return EntryJudgment {
            counted: false,
            reason: Some(reasons::AFTER_FENCE),
            governs: answer.then_some(false),
        };
    }
    let lost = EntryJudgment {
        counted: false,
        reason: Some(reasons::NO_QUESTION),
        governs: answer.then_some(false),
    };
    let Some(target) = body.target.as_ref() else {
        return lost;
    };
    let Some(question) = question_of(index, target) else {
        // The question went with its raiser. The answer the precedent read
        // and a settle take as in effect reads that way here too.
        let in_effect = answer
            && gone_question_answer(index, backend, &target.id, &target.decision)
                .is_some_and(|w| same(w, body));
        return if in_effect {
            EntryJudgment {
                counted: true,
                reason: Some(reasons::QUESTION_GONE),
                governs: Some(true),
            }
        } else {
            lost
        };
    };
    if !is_question(question) {
        return lost;
    }
    let runs = Runs::new(backend);
    judge_resolved(index, body, &resolve(index, &runs, question))
}

/// [`judge_entry`] for an entry aimed at a question whose resolution the
/// caller already holds, so a read judging every entry about one question
/// resolves it once rather than once per entry. `r` must be the question's
/// resolution against the same `index`.
pub(crate) fn judge_resolved(index: &Index, body: &DecisionBody, r: &Resolution) -> EntryJudgment {
    let answer = body.kind == "answer";
    if index.after_fence(body) {
        return EntryJudgment {
            counted: false,
            reason: Some(reasons::AFTER_FENCE),
            governs: answer.then_some(false),
        };
    }
    if answer {
        let Some(j) = r.closing.iter().find(|j| same(j.entry, body)) else {
            return EntryJudgment {
                counted: false,
                reason: Some(reasons::NO_QUESTION),
                governs: Some(false),
            };
        };
        return EntryJudgment {
            counted: j.counted,
            reason: j.reason,
            governs: Some(r.in_effect.is_some_and(|w| same(w, body))),
        };
    }
    if r.applied.iter().any(|p| same(p, body)) {
        return EntryJudgment {
            counted: true,
            reason: None,
            governs: None,
        };
    }
    let destination =
        recipient_of(body).is_some_and(|to| !destination_ok(&to, recorded_dispatcher(body)));
    EntryJudgment {
        counted: false,
        reason: Some(if destination {
            reasons::DESTINATION
        } else {
            reasons::SKIPPED
        }),
        governs: None,
    }
}

pub(crate) fn decisions(index: &Index) -> impl Iterator<Item = &DecisionBody> {
    index.sessions().values().flatten().filter_map(|e| match e {
        Entry::Decision(b) => Some(&**b),
        Entry::Unreadable { .. } => None,
    })
}

/// The open questions in the local store, each with its addressee
/// history, sorted by time, then the raising session's name, then id.
/// Open is [`is_open`]'s test.
fn open_questions<'i>(index: &'i Index, runs: &Runs) -> Vec<(&'i DecisionBody, Vec<Recipient>)> {
    let mut seen = BTreeSet::new();
    let mut found: Vec<(&DecisionBody, Vec<Recipient>)> = decisions(index)
        .filter(|b| seen.insert((b.session.id.clone(), b.id.clone())))
        .filter(|b| is_open(index, runs, b))
        .map(|b| (b, addressees(index, runs, b)))
        .collect();
    found.sort_by(|(a, _), (b, _)| order_key(a).cmp(&order_key(b)));
    found
}

/// How many open questions ([`is_open`]) raised by the sessions whose ids
/// are in `raisers` wait on a person now, across every handle, leaving out
/// the question `except` names by `(raiser's session id, id)`.
pub fn open_to_person(
    index: &Index,
    runs: &Runs,
    raisers: &BTreeSet<String>,
    except: Option<(&str, &str)>,
) -> usize {
    let mut seen = BTreeSet::new();
    decisions(index)
        .filter(|b| raisers.contains(&b.session.id))
        .filter(|b| except != Some((b.session.id.as_str(), b.id.as_str())))
        .filter(|b| seen.insert((b.session.id.clone(), b.id.clone())))
        .filter(|b| is_open(index, runs, b))
        .filter(|b| {
            matches!(
                addressees(index, runs, b).last(),
                Some(Recipient::Person(_))
            )
        })
        .count()
}

/// The run a question's raiser belongs to, as the limit of twenty counts
/// it: the session ids this host holds for the raiser, the sessions it
/// continues and the sessions that continue it, and the id of the run's
/// earliest session, which keys the run's lock.
pub struct Run {
    pub ids: Vec<String>,
    pub root: String,
}

/// The whole run of `raiser` (whose header id is `raiser_id`): its own
/// session id, those of the sessions it continues
/// ([`chain::run_predecessors`]) and those of the sessions that continue it
/// ([`chain::run_successors`]), as far as this host holds them. A pass-up
/// recorded by an addressee and an `escalate` recorded by a successor
/// therefore count the same questions and take the same lock.
pub fn run_ids(backend: &Backend, raiser: &str, raiser_id: &str) -> Run {
    let mut ids = vec![raiser_id.to_string()];
    let mut root = raiser_id.to_string();
    for p in chain::run_predecessors(backend, raiser) {
        if let Some(id) = read::local_session_id(backend, &p) {
            if !ids.contains(&id) {
                ids.push(id.clone());
            }
            root = id;
        }
    }
    for s in chain::run_successors(backend, raiser) {
        if let Some(id) = read::local_session_id(backend, &s) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Run { ids, root }
}

/// The directory under the koto root that holds the limit's locks, one file
/// per run. The files are never removed: each is empty, and one left by a
/// run that has ended only costs a directory entry.
pub const PERSON_LIMIT_LOCKS_DIR: &str = "person-limit";

/// How long a writer waits for another writer of the same run to release
/// the limit's lock before it refuses.
pub const PERSON_LIMIT_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The limit's lock on one run, held until dropped.
pub struct LimitLock {
    _file: std::fs::File,
}

/// Take the limit's lock for the run whose earliest session id is `root`
/// ([`Run::root`]), waiting at most [`PERSON_LIMIT_LOCK_WAIT`]. A writer
/// that would add a question waiting on a person holds it from the count to
/// the write, so two writers on this host can't both count nineteen and
/// both write. Every member of a run keys it by the same earliest session,
/// so a successor, the sessions it continues and an addressee passing up
/// one of their questions share it. It is a file under the koto root,
/// outside every session's synced files, and serializes writers on this
/// host only. A lock still held when the wait ends is an error of kind
/// `TimedOut` naming the file.
pub fn lock_person_limit(home: &std::path::Path, root: &str) -> std::io::Result<LimitLock> {
    use sha2::{Digest, Sha256};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = home.join(PERSON_LIMIT_LOCKS_DIR);
    crate::koto_home::ensure_private_root(&dir)?;
    let path = dir.join(hex::encode(Sha256::digest(root.as_bytes())));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let deadline = std::time::Instant::now() + PERSON_LIMIT_LOCK_WAIT;
    loop {
        // SAFETY: flock on a descriptor borrowed from `file`, which
        // outlives the call; the lock is released when `file` closes.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(LimitLock { _file: file });
        }
        let err = std::io::Error::last_os_error();
        match err.kind() {
            std::io::ErrorKind::Interrupted => continue,
            std::io::ErrorKind::WouldBlock => {}
            _ => return Err(err),
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "another koto command has held {} for more than {} seconds",
                    path.display(),
                    PERSON_LIMIT_LOCK_WAIT.as_secs()
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// One element of an open read: the question, what it asks for, the
/// recipient it was raised with, the one it waits on now and every one it
/// has had.
fn element(b: &DecisionBody, history: &[Recipient]) -> Value {
    let mut e = json!({
        "identity": identity(b),
        "session": b.session.name,
        "id": b.id,
        "question": subject_str(b, "question"),
        "text": subject_str(b, "text"),
        "need": need_of(b),
        "recipient": recipient_of(b).map(|r| r.to_json()),
        "addressee": history.last().map(Recipient::to_json),
        "addressees": history.iter().map(Recipient::to_json).collect::<Vec<_>>(),
        "time": b.time,
    });
    // The workflow state and visit the question was raised in, only when
    // the escalate carries one, so an element for one raised without it
    // prints as it did before the field existed.
    if let Some(state) = subject_str(b, "state") {
        e["state"] = json!(state);
    }
    e
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

/// The decisions the run's decisions cite, as `(session id, id)` of the
/// cited body.
///
/// A basis entry names a session only by name, and a name can be reused
/// after its session is removed, so each citation is resolved by reading
/// the cited decision: it counts only when it was recorded no later than
/// the decision citing it. A later session that reuses the name, and an
/// id, therefore never stands in for the decision that was cited.
pub(crate) fn cited(index: &Index, decisions: &[&DecisionBody]) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for d in decisions {
        for c in &d.basis {
            if let Some(Entry::Decision(a)) = index.find(&c.session, &c.id) {
                if a.time <= d.time {
                    out.insert((a.session.id.clone(), a.id.clone()));
                }
            }
        }
    }
    out
}

/// The `questions` array `koto next <session>` carries, or `None` when it
/// carries none.
///
/// The questions reported are the ones raised in `session` or in an
/// earlier member of its run (its confirmed predecessors through koto's
/// succession) whose entry in effect is an `answered` answer that no
/// decision in the run cites yet: a decision whose `basis` names that
/// entry, recorded no later than that decision, ends the report. The
/// report follows the entry in effect, so a raiser that cited a session's
/// answer is told again when a person's answer later governs. A question
/// the run closed itself, by a `settled` or `withdrawn` entry, isn't
/// reported, and an entry that doesn't count never is.
///
/// The store is scanned whenever the run holds a question: one read of
/// every local session's decision directories per tick, since a person's
/// answer can arrive in any session at any time and replace the one in
/// effect. A run with no question, or with only untyped escalations,
/// costs a read of its own decision directories and its continues link,
/// and nothing more. The read never writes, never takes a lock and makes
/// no network call, so it never blocks the tick it rides on.
pub fn report(backend: &Backend, session: &str) -> Option<Value> {
    let mut run = vec![session.to_string()];
    run.extend(chain::run_predecessors(backend, session));
    let entries: Vec<Entry> = run
        .iter()
        .flat_map(|s| read::read_session_with(backend, s, false))
        .collect();
    let own: Vec<&DecisionBody> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
        .collect();
    if !own.iter().any(|b| is_question(b)) {
        return None;
    }
    if std::env::var_os(SCAN_PROBE_ENV).is_some() {
        eprintln!("koto: questions: store scanned");
    }
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    let cited = cited(&index, &own);
    let mut out = Vec::new();
    for q in own.iter().copied().filter(|q| is_question(q)) {
        if index.status(q) != "in-effect" {
            continue;
        }
        let Some(answer) = answer_in_effect(&index, &runs, q) else {
            continue;
        };
        if answer.outcome != "answered"
            || cited.contains(&(answer.session.id.clone(), answer.id.clone()))
        {
            continue;
        }
        let mut answer_json = json!({
            "session": answer.session.name,
            "id": answer.id,
            "text": subject_str(answer, "text"),
            "decider": answer.decider,
            "run_mode": answer.authority.run_mode,
            "time": answer.time,
        });
        if answer.prompt.is_some() {
            // The same reading `show` and `list` give: verified only where
            // this host's note in a holder's directory lists the answer.
            let here = index
                .holders(answer)
                .iter()
                .any(|h| standing::verified_in(backend, h, answer));
            answer_json["reading"] = json!(if here {
                super::prompt::VERIFIED
            } else {
                super::prompt::REMOTE_READING
            });
        }
        out.push(json!({
            "identity": identity(q),
            "session": q.session.name,
            "id": q.id,
            "question": subject_str(q, "question"),
            "state": "answered",
            "answer": answer_json,
        }));
    }
    (!out.is_empty()).then_some(Value::Array(out))
}

/// What a session holds that an open question depends on, or that keeps
/// it on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Held {
    /// It raised the question, which is open.
    Raised,
    /// It holds the answer in effect for the question.
    Answer,
    /// It holds a pass-up the question's current addressee rests on.
    PassUp,
    /// It holds a person's answer whose reading is `verified` on this host
    /// ([`verified_answers_in`]). It keeps the session whether or not the
    /// question is open and whether or not its raiser is still here.
    PersonAnswer,
}

impl Held {
    /// The `held` value a refusal and `orphaned_questions` print.
    pub fn as_str(self) -> &'static str {
        match self {
            Held::Raised => "raised",
            Held::Answer => "answer",
            Held::PassUp => "pass_up",
            Held::PersonAnswer => "person_answer",
        }
    }

    /// What removing the session loses for the question.
    pub fn loss(self) -> &'static str {
        match self {
            Held::Raised => "the question itself: no open read returns it once its raiser is gone",
            Held::Answer => "the answer in effect: the question reads open again",
            Held::PassUp => "a pass-up: the question waits on the addressee it had before it",
            Held::PersonAnswer => {
                "a person's answer this host verified against its prompt: no other copy of it \
                 reads verified here"
            }
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
            Held::PersonAnswer => {
                format!("holds a verified person answer to the question {identity}")
            }
        }
    }
}

/// One question a session holds something of, by its identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hold {
    pub identity: String,
    pub held: Held,
    /// The session directory that raised the question: the holder itself
    /// for [`Held::Raised`].
    pub raiser: String,
    /// For [`Held::PersonAnswer`], the answer: `<holder's session
    /// id>/<answer decision id>`. `None` for every other hold.
    pub answer: Option<String>,
}

impl Hold {
    /// What the session holds, for a message: `it <describe>`, sanitized,
    /// naming the answer of a [`Held::PersonAnswer`].
    pub fn describe(&self) -> String {
        use crate::session::hierarchy::sanitize;
        let identity = sanitize(&self.identity);
        match &self.answer {
            Some(a) => format!(
                "holds the verified person answer {} to the question {identity}",
                sanitize(a)
            ),
            None => self.held.describe(&identity),
        }
    }
}

/// The way out a refusal for `holds` names before `--force`: answering,
/// reversing or passing the question, except when every hold is a verified
/// person answer, which nothing but a forced removal ends.
pub fn way_out(holds: &[Hold]) -> &'static str {
    if !holds.is_empty() && holds.iter().all(|h| h.held == Held::PersonAnswer) {
        "A verified person answer is kept until its session is removed"
    } else {
        "Answer, reverse or pass the question first"
    }
}

/// Whether `b`, read from session directory `holder`, is a person's answer
/// whose `reading` is `verified` here: an answer aimed at a question that
/// carries a prompt (only a person's answer does), judged by
/// [`standing::verified_in`] for `holder`, the test `koto decisions show`
/// prints `reading` from.
fn verified_held(backend: &Backend, holder: &str, b: &DecisionBody) -> bool {
    b.kind == "answer"
        && b.target.is_some()
        && b.prompt.is_some()
        && standing::verified_in(backend, holder, b)
}

/// A person's answer a session holds whose reading is `verified` here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VerifiedAnswer {
    /// The question it answers: `<raiser's session id>/<question id>`.
    pub question: String,
    /// The answer: `<holder's session id>/<answer decision id>`.
    pub answer: String,
    /// The raiser's session name, as the answer names it.
    pub raiser: String,
}

fn own_decisions(entries: &[Entry]) -> impl Iterator<Item = &DecisionBody> {
    entries.iter().filter_map(|e| match e {
        Entry::Decision(b) => Some(&**b),
        Entry::Unreadable { .. } => None,
    })
}

fn verified_among<'e>(
    backend: &Backend,
    session: &str,
    entries: impl Iterator<Item = &'e DecisionBody>,
) -> Vec<VerifiedAnswer> {
    let mut out: Vec<VerifiedAnswer> = entries
        .filter(|b| verified_held(backend, session, b))
        .filter_map(|b| {
            let t = b.target.as_ref()?;
            Some(VerifiedAnswer {
                question: format!("{}/{}", t.id, t.decision),
                answer: identity(b),
                raiser: t.name.clone(),
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The person's answers session directory `session` holds whose reading
/// is `verified` on this host, whatever their questions read and whether
/// their raisers are still here. Read from the session's own decisions; a
/// host-local note is read only for an entry carrying a prompt.
pub fn verified_answers_in(backend: &Backend, session: &str) -> Vec<VerifiedAnswer> {
    let own = read::read_session_with(backend, session, false);
    verified_among(backend, session, own_decisions(&own))
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

/// The question `target` names, when the local store holds it and it is
/// a question: [`question_of`], narrowed to questions.
fn aimed_question<'i>(index: &'i Index, target: &Target) -> Option<&'i DecisionBody> {
    question_of(index, target).filter(|q| is_question(q))
}

/// The identities of the open questions ([`is_open`]) raised in session
/// directory `session`, in the order it recorded them.
pub fn raised_open(index: &Index, runs: &Runs, session: &str) -> Vec<String> {
    decisions_in(index, session)
        .filter(|b| is_open(index, runs, b))
        .map(identity)
        .collect()
}

/// Every question session directory `session` holds something of that its
/// removal would lose: an open question it raised ([`is_open`]), the entry
/// [`answer_in_effect`] names for a question whose raiser is in the store,
/// and a counted pass-up (one [`resolve`] applied) the current addressee
/// of an open question rests on. An entry that doesn't count, or that a
/// person's answer outranks, holds nothing. A person's answer whose reading
/// is `verified` here holds as [`Held::PersonAnswer`] instead, whatever its
/// question reads and whether or not its raiser is in the store. Each is matched by identity,
/// never by name, so an answer aimed at a removed session's id holds
/// nothing once that session is gone, whatever now carries its name.
pub fn holds_of(index: &Index, runs: &Runs, session: &str) -> Vec<Hold> {
    let mut out: Vec<Hold> = raised_open(index, runs, session)
        .into_iter()
        .map(|identity| Hold {
            identity,
            held: Held::Raised,
            raiser: session.to_string(),
            answer: None,
        })
        .collect();
    for b in decisions_in(index, session) {
        let Some(t) = &b.target else { continue };
        // A person's answer that reads verified here holds on its own,
        // whatever its question reads and wherever its raiser is.
        if verified_held(runs.backend(), session, b) {
            out.push(Hold {
                identity: format!("{}/{}", t.id, t.decision),
                held: Held::PersonAnswer,
                raiser: t.name.clone(),
                answer: Some(identity(b)),
            });
            continue;
        }
        // By kind alone: whether the entry is the answer in effect, or a
        // pass-up in the question's history, is the shared computation's
        // to say.
        let held = match b.kind.as_str() {
            "answer" => Held::Answer,
            "readdress" => Held::PassUp,
            _ => continue,
        };
        let Some(q) = aimed_question(index, t) else {
            continue;
        };
        let holds = match held {
            // The entry in effect, whatever its outcome, for a question in
            // effect: an entry that doesn't count, or one outranked by a
            // person's answer, holds nothing. A settle or a withdrawal the
            // raiser recorded in its own directory goes with the question,
            // so it holds nothing apart from it.
            Held::Answer => {
                b.session.id != q.session.id
                    && index.status(q) == "in-effect"
                    && answer_in_effect(index, runs, q).is_some_and(|a| same(a, b))
            }
            // A counted pass-up (one the resolution applied to build the
            // addressee history) of a question that is still open.
            Held::PassUp => {
                is_open(index, runs, q)
                    && resolve(index, runs, q).applied.iter().any(|r| same(r, b))
            }
            Held::Raised | Held::PersonAnswer => false,
        };
        if holds {
            out.push(Hold {
                identity: identity(q),
                held,
                raiser: q.session.name.clone(),
                answer: None,
            });
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether `b` could make a hold [`holds_of`] counts: a question, or an
/// answer or pass-up aimed at one.
fn may_count(b: &DecisionBody) -> bool {
    is_question(b) || (b.target.is_some() && matches!(b.kind.as_str(), "answer" | "readdress"))
}

/// What keeps a finished session for the questions it holds, at its
/// terminal tick and in a parent's sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickHold {
    /// The identities of the open questions it raised.
    Open(Vec<String>),
    /// The person's answers it holds whose reading is `verified` here.
    PersonAnswers(Vec<VerifiedAnswer>),
}

/// What keeps session `session` at its terminal tick for its questions:
/// the open questions it raised, else the person's answers it holds that
/// read verified here, else nothing. One read of its own decisions: the
/// store is scanned only when it raised a question, and a host-local note
/// is read only when it holds an answer carrying a prompt, so a session
/// with neither pays that read alone.
pub fn tick_hold(backend: &Backend, session: &str) -> Option<TickHold> {
    let own = read::read_session_with(backend, session, false);
    if own_decisions(&own).any(is_question) {
        let open = raised_open(&Index::scan(backend, false), &Runs::new(backend), session);
        if !open.is_empty() {
            return Some(TickHold::Open(open));
        }
    }
    let answers = verified_among(backend, session, own_decisions(&own));
    (!answers.is_empty()).then_some(TickHold::PersonAnswers(answers))
}

/// What an explicit removal of a session would orphan, and what couldn't
/// be read to tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Orphans {
    /// [`holds_of`] the session.
    pub holds: Vec<Hold>,
    /// What couldn't be read, one line each: the session's own decision
    /// directory or files, and each question one of its aimed answers or
    /// pass-ups points at that sits in a session in the store but can't be
    /// read. Either could hide a hold, so an explicit removal is refused
    /// for them as it is for a hold.
    pub unreadable: Vec<String>,
}

impl Orphans {
    /// Whether an explicit removal is refused without `--force`.
    pub fn refuses(&self) -> bool {
        !self.holds.is_empty() || !self.unreadable.is_empty()
    }
}

/// One unreadable entry of `session`, as a line of [`Orphans::unreadable`].
fn unreadable_line(session: &str, id: Option<&str>, reason: &str) -> String {
    match id {
        Some(id) => format!("decision {id} of '{session}': {reason}"),
        None => format!("the decisions of '{session}': {reason}"),
    }
}

/// The session in the store, other than `except`, whose state-file header
/// names session id `session_id`.
fn session_with_id(
    backend: &Backend,
    index: &Index,
    session_id: &str,
    except: &str,
) -> Option<String> {
    index.sessions().keys().find_map(|s| {
        if s == except {
            return None;
        }
        let path = crate::session::SessionBackend::session_dir(backend, s)
            .join(crate::session::state_file_name(s));
        let id = crate::session::links::LinkHints::from_state_file(&path).session_id;
        (id.as_deref() == Some(session_id)).then(|| s.clone())
    })
}

/// What an explicit removal of session `session` would orphan
/// ([`holds_of`]) and what couldn't be read to tell, from the local store
/// only: nothing, at the cost of a read of its own decisions, when every
/// entry of its own reads and none is a question or an aimed answer or
/// pass-up.
pub fn orphans(backend: &Backend, session: &str) -> Orphans {
    let own = read::read_session_with(backend, session, false);
    let mut out = Orphans {
        unreadable: own
            .iter()
            .filter_map(|e| match e {
                Entry::Unreadable { id, reason, .. } => {
                    Some(unreadable_line(session, id.as_deref(), reason))
                }
                Entry::Decision(_) => None,
            })
            .collect(),
        ..Orphans::default()
    };
    let counts = own.iter().any(|e| match e {
        Entry::Decision(b) => may_count(b),
        Entry::Unreadable { .. } => false,
    });
    if !counts {
        return out;
    }
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    out.holds = holds_of(&index, &runs, session);
    // An aimed entry whose question can't be read, in the session the store
    // holds under that question's session id, may rest on an open question.
    for b in decisions_in(&index, session) {
        let Some(t) = &b.target else { continue };
        if !matches!(b.kind.as_str(), "answer" | "readdress") || aimed_question(&index, t).is_some()
        {
            continue;
        }
        let Some(raiser) = session_with_id(backend, &index, &t.id, session) else {
            continue;
        };
        for e in index.sessions().get(&raiser).into_iter().flatten() {
            if let Entry::Unreadable { id, reason, .. } = e {
                if id.as_deref().is_none_or(|id| id == t.decision) {
                    out.unreadable
                        .push(unreadable_line(&raiser, id.as_deref(), reason));
                }
            }
        }
    }
    out.unreadable.sort();
    out.unreadable.dedup();
    out
}

/// The members of `removing` a by-rule removal keeps for the questions they
/// hold, each with the holds whose raiser isn't removed with it: a holder
/// of the answer in effect or of a pass-up goes only with the raiser of
/// that question ([`holds_of`]), and a member kept here keeps the holders
/// that rest on it, to a fixed point. Asker and answerer removed together
/// both go. A holder of a person's answer that reads verified here
/// ([`Held::PersonAnswer`]) is kept whatever is removed with it. `may_hold`
/// says which members could hold anything (those with an aimed answer or
/// pass-up); when none could, nothing is read. Read from the local store
/// only.
pub fn kept_for_raisers(
    backend: &Backend,
    removing: &BTreeSet<String>,
    may_hold: impl Fn(&str) -> bool,
) -> BTreeMap<String, Vec<Hold>> {
    let holders: Vec<&String> = removing.iter().filter(|n| may_hold(n)).collect();
    let mut kept: BTreeMap<String, Vec<Hold>> = BTreeMap::new();
    if holders.is_empty() {
        return kept;
    }
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    let holds: Vec<(&String, Vec<Hold>)> = holders
        .into_iter()
        .map(|h| (h, holds_of(&index, &runs, h)))
        .filter(|(_, holds)| !holds.is_empty())
        .collect();
    loop {
        let mut changed = false;
        for (holder, held) in &holds {
            if kept.contains_key(*holder) {
                continue;
            }
            let apart: Vec<Hold> = held
                .iter()
                .filter(|h| {
                    h.held == Held::PersonAnswer
                        || !removing.contains(&h.raiser)
                        || kept.contains_key(&h.raiser)
                })
                .cloned()
                .collect();
            if !apart.is_empty() {
                kept.insert((*holder).clone(), apart);
                changed = true;
            }
        }
        if !changed {
            return kept;
        }
    }
}

/// The detail a by-rule removal prints for a member [`kept_for_raisers`]
/// keeps.
pub fn kept_for_raisers_text(holds: &[Hold]) -> String {
    use crate::session::hierarchy::sanitize;
    let each: Vec<String> = holds
        .iter()
        .map(|h| match h.held {
            Held::PersonAnswer => format!(
                "it {}, which keeps it whether or not its raiser {} stays (removing it would \
                 lose {})",
                h.describe(),
                sanitize(&h.raiser),
                h.held.loss()
            ),
            _ => format!(
                "it {}, and its raiser {} isn't removed with it (removing it would lose {})",
                h.describe(),
                sanitize(&h.raiser),
                h.held.loss()
            ),
        })
        .collect();
    format!("kept: {}", each.join("; "))
}

/// The open questions each session directory in the local store raised,
/// for the retention engine; sessions that raised none are left out.
pub fn open_by_session(backend: &Backend) -> BTreeMap<String, Vec<String>> {
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    index
        .sessions()
        .keys()
        .filter_map(|s| {
            let open = raised_open(&index, &runs, s);
            (!open.is_empty()).then(|| (s.clone(), open))
        })
        .collect()
}
