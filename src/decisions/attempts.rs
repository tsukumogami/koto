//! Attempts to decide, and provisional decisions awaiting review.
//!
//! A `judgment` may carry two subject fields: `state`, the workflow state
//! and visit the caller says it was made in (`<state name>:<visit>`, stored
//! as handed and never compared with the session's own state), and
//! `result`, one of [`RESULTS`]. A judgment carrying `result` is an
//! attempt; one carrying neither field, which every judgment recorded
//! before the fields existed is, is not, and no read here returns it.
//!
//! An attempt is tied to a question through `basis`, in either direction:
//! the attempt names the question's `escalate`, or the `escalate` names an
//! attempt recorded in the raiser's own run (the raiser, the sessions it
//! continues and those that continue it). A raiser naming another
//! session's attempt doesn't tie it. A basis entry names its session by
//! name, which a later session can reuse, so a citation counts only when
//! the decision it names, read now, is the one meant: the right kind, and
//! recorded no later than the decision citing it. That is how the
//! `questions` report on `koto next` resolves a citation.
//!
//! A provisional judgment (`result` `provisional`) awaits a person's
//! review until one of two things happens: it is reversed, or a question
//! whose in-effect `escalate` names it in its `basis`, raised in any
//! session, is governed by a person's answer that reads `verified` on this
//! host (the question's answer in effect). Nothing else ends the wait: not
//! a session's answer, a settle or a withdrawal, an answer that doesn't
//! count or is outranked, a remote person answer, or a decision that cites
//! the judgment.
//!
//! A stored `state` or `result` that the write would have refused (a body
//! edited by hand) makes the decision unreadable to these reads.
//!
//! Every read here uses the local store alone, the way [`Index::scan`]
//! does with `remote` unset: no lock, no pull, no bucket request, no write.
//! `decisions show` takes a provisional judgment's review from the same
//! local reading.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::DecisionBody;
use super::chain;
use super::questions;
use super::read::{self, Entry};
use super::standing::{self, Index};
use super::FORMAT_VERSION;
use crate::session::{origin, Backend};

/// The values a judgment's `result` takes.
pub const RESULTS: [&str; 3] = ["decided", "undecided", "provisional"];

/// Whether `value` is `<state name>:<visit>`: a state name of the shape a
/// session name takes (an ASCII letter or digit, then letters, digits,
/// `-`, `.` or `_`, at most 255 bytes), a colon, and a visit written as a
/// whole number of 1 or more with no sign and no leading zero.
pub fn is_state_visit(value: &str) -> bool {
    let Some((state, visit)) = value.rsplit_once(':') else {
        return false;
    };
    let state_ok = crate::discover::validate_workflow_name(state).is_ok();
    let visit_ok = !visit.is_empty()
        && visit.len() <= 19
        && visit.bytes().all(|b| b.is_ascii_digit())
        && !visit.starts_with('0');
    state_ok && visit_ok
}

fn subject_str<'a>(body: &'a DecisionBody, field: &str) -> Option<&'a str> {
    body.subject.get(field).and_then(Value::as_str)
}

/// Why the stored `state` or `result` of a judgment or escalate would be
/// refused at write time, or `None` when both are absent or well formed.
/// A body edited by hand after it was written can carry either; the reads
/// here treat such a decision as unreadable rather than as an attempt.
fn malformed(body: &DecisionBody) -> Option<&'static str> {
    if body.kind != "judgment" && body.kind != "escalate" {
        return None;
    }
    if let Some(v) = body.subject.get("state") {
        if !v.as_str().is_some_and(is_state_visit) {
            return Some("stored subject field 'state' is not a state name, ':' and a visit");
        }
    }
    if body.kind == "judgment" {
        if let Some(v) = body.subject.get("result") {
            if !v.as_str().is_some_and(|r| RESULTS.contains(&r)) {
                return Some(
                    "stored subject field 'result' is not one of decided, undecided, provisional",
                );
            }
        }
    }
    None
}

/// Whether `body` is an attempt: a judgment carrying a well-formed
/// `result` (and, when present, a well-formed `state`).
pub fn is_attempt(body: &DecisionBody) -> bool {
    body.kind == "judgment" && subject_str(body, "result").is_some() && malformed(body).is_none()
}

/// Whether `body` is a provisional judgment.
pub fn is_provisional(body: &DecisionBody) -> bool {
    is_attempt(body) && subject_str(body, "result") == Some("provisional")
}

/// Whether sessions `a` and `b` are in one run: the same session, or one
/// continues the other through confirmed links.
fn same_run(
    backend: &Backend,
    preds: &mut BTreeMap<String, Vec<String>>,
    a: &str,
    b: &str,
) -> bool {
    if a == b {
        return true;
    }
    let mut of = |s: &str| -> Vec<String> {
        preds
            .entry(s.to_string())
            .or_insert_with(|| chain::run_predecessors(backend, s))
            .clone()
    };
    of(a).iter().any(|p| p == b) || of(b).iter().any(|p| p == a)
}

fn same(a: &DecisionBody, b: &DecisionBody) -> bool {
    a.session.id == b.session.id && a.id == b.id
}

/// The decision `session`'s directory holds under `id`, as the scan read it.
fn cited<'i>(index: &'i Index, session: &str, id: &str) -> Option<&'i DecisionBody> {
    match index.find(session, id)? {
        Entry::Decision(b) => Some(b),
        Entry::Unreadable { .. } => None,
    }
}

/// Every readable decision in the scan, each once by identity: a copy an
/// import carried into another session's directory is the same decision.
fn decisions(index: &Index) -> Vec<&DecisionBody> {
    let mut seen = BTreeSet::new();
    index
        .sessions()
        .values()
        .flatten()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
        .filter(|b| seen.insert((b.session.id.clone(), b.id.clone())))
        .collect()
}

/// The questions tied to each attempt, keyed by the attempt's (session id,
/// decision id), and for each provisional judgment the questions whose
/// `escalate` names it (only those can end its wait).
struct Ties<'i> {
    questions: BTreeMap<(String, String), Vec<&'i DecisionBody>>,
    reviews: BTreeMap<(String, String), Vec<&'i DecisionBody>>,
}

impl<'i> Ties<'i> {
    fn build(backend: &Backend, index: &'i Index) -> Self {
        let mut questions: BTreeMap<(String, String), Vec<&DecisionBody>> = BTreeMap::new();
        let mut reviews: BTreeMap<(String, String), Vec<&DecisionBody>> = BTreeMap::new();
        let mut preds = BTreeMap::new();
        let all = decisions(index);
        for d in &all {
            // A hand-edited `state` or `result` makes a decision unreadable
            // to these reads on either side of a tie: it ties to nothing and
            // ends no wait.
            if malformed(d).is_some() {
                continue;
            }
            for c in &d.basis {
                let Some(target) = cited(index, &c.session, &c.id) else {
                    continue;
                };
                if malformed(target).is_some() {
                    continue;
                }
                if target.time > d.time {
                    // A later decision under a reused name and id is never
                    // the one that was cited.
                    continue;
                }
                if questions::is_question(d) && is_attempt(target) {
                    // The escalate names the attempt. Any session may put
                    // a provisional judgment before a person, so the review
                    // counts every such question; only a person's verified
                    // answer ends the wait. The tie is narrower: a raiser
                    // speaks for its own run, and naming another session's
                    // attempt doesn't make that session have attempted the
                    // question.
                    let key = (target.session.id.clone(), target.id.clone());
                    if is_provisional(target) {
                        reviews.entry(key.clone()).or_default().push(*d);
                    }
                    if same_run(backend, &mut preds, &d.session.name, &target.session.name) {
                        questions.entry(key).or_default().push(*d);
                    }
                } else if is_attempt(d) && questions::is_question(target) {
                    // The attempt names the escalate.
                    questions
                        .entry((d.session.id.clone(), d.id.clone()))
                        .or_default()
                        .push(target);
                }
            }
        }
        for list in questions.values_mut().chain(reviews.values_mut()) {
            list.sort_by(|a, b| {
                (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id))
            });
            list.dedup_by(|a, b| same(a, b));
        }
        Ties { questions, reviews }
    }

    fn questions_of(&self, body: &DecisionBody) -> &[&'i DecisionBody] {
        self.questions
            .get(&(body.session.id.clone(), body.id.clone()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn reviews_of(&self, body: &DecisionBody) -> &[&'i DecisionBody] {
        self.reviews
            .get(&(body.session.id.clone(), body.id.clone()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// The answer that governs `question` (koto's answer in effect), when it
/// is a person's answer this host verified. An earlier verified answer the
/// governing one outranks, a settle, a withdrawal and an entry that doesn't
/// count never qualify.
fn governing_person_answer<'i>(
    backend: &Backend,
    index: &'i Index,
    runs: &questions::Runs<'_>,
    question: &DecisionBody,
) -> Option<&'i DecisionBody> {
    questions::answer_in_effect(index, runs, question)
        .filter(|a| questions::verified_person(index, backend, a))
}

/// `reversals` is the index whose `status` the response prints beside the
/// review, so the two always agree; `index` is the local reading the rest
/// of the review comes from.
fn review_with(
    backend: &Backend,
    reversals: &Index,
    index: &Index,
    ties: &Ties<'_>,
    body: &DecisionBody,
) -> Value {
    // `status` reads reversed only once the reversal wins; the winner is
    // what `reversed_by` reports.
    if let Some(w) = reversals.winner_of(&body.session.id, &body.id) {
        if reversals.status(body) == "reversed" {
            return json!({
                "state": "reversed",
                "reversed_by": standing::reversal_json(w),
            });
        }
    }
    let runs = questions::Runs::new(backend);
    let live: Vec<&DecisionBody> = ties
        .reviews_of(body)
        .iter()
        .copied()
        .filter(|q| index.status(q) == "in-effect")
        .collect();
    for q in &live {
        if let Some(a) = governing_person_answer(backend, index, &runs, q) {
            return json!({
                "state": "reviewed",
                "question": questions::identity(q),
                "answer": {
                    "session": a.session.name,
                    "id": a.id,
                    "text": subject_str(a, "text"),
                    "decider": a.decider,
                    "reading": super::prompt::VERIFIED,
                    "run_mode": a.authority.run_mode,
                    "time": a.time,
                },
            });
        }
    }
    // The question named is the earliest citing question still open, else
    // the earliest in effect: a later person answer can still govern a
    // question a session's answer closed.
    let waiting = live
        .iter()
        .find(|q| questions::is_open(index, &runs, q))
        .or(live.first());
    json!({
        "state": "awaiting",
        "question": waiting.map(|q| questions::identity(q)),
    })
}

/// A provisional judgment's review: `awaiting`, with the identity of the
/// earliest in-effect question whose `escalate` names it (or null);
/// `reviewed`, with that question and the verified person answer that
/// ended the wait; or `reversed`, with the winning reversal as `show`
/// reports it.
///
/// `reversals` decides whether it is reversed and must be the index whose
/// `status` the caller prints with it; `reading` is the local store the
/// rest of the review is read from.
pub fn review(backend: &Backend, reversals: &Index, reading: &Index, body: &DecisionBody) -> Value {
    review_with(
        backend,
        reversals,
        reading,
        &Ties::build(backend, reading),
        body,
    )
}

/// Where an attempts read looks.
pub enum Position<'a> {
    /// A session's run: it and the sessions it continues.
    In(&'a str),
    /// A position: the session or anchor with the sessions it continues,
    /// and every session below it, as `koto decisions open --at` reads it.
    At(&'a str),
}

/// The filters an attempts read takes; every one given must hold.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub result: Option<String>,
    pub state: Option<String>,
    /// A question identity, `<raiser's session id>/<decision id>`.
    pub question: Option<String>,
    pub awaiting_review: bool,
}

/// `koto decisions attempts`: every attempt at the position that passes
/// the filters, ordered by time, then the recording session's name, then
/// id.
pub fn attempts(backend: &Backend, position: &Position<'_>, filters: &Filters) -> Value {
    let index = Index::scan(backend, false);
    let (name, run): (&str, BTreeSet<String>) = match position {
        Position::In(s) | Position::At(s) => {
            let mut run = BTreeSet::new();
            run.insert(s.to_string());
            run.extend(chain::run_predecessors(backend, s));
            (s, run)
        }
    };
    let below = match position {
        Position::In(_) => BTreeSet::new(),
        Position::At(_) => {
            let local = match backend {
                Backend::Local(l) => l,
                Backend::Cloud(c) => c.local(),
            };
            origin::below(local, &read::local_sessions(backend), &run)
        }
    };
    let ties = Ties::build(backend, &index);
    let mut seen = BTreeSet::new();
    let mut found: Vec<(&DecisionBody, &'static str)> = Vec::new();
    let mut unreadable = Vec::new();
    for (session, entries) in index.sessions() {
        let relation = if run.contains(session) {
            "at"
        } else if below.contains(session) {
            "below"
        } else {
            continue;
        };
        for e in entries {
            let b = match e {
                Entry::Decision(b) => b,
                Entry::Unreadable { .. } => {
                    unreadable.push(e.to_json());
                    continue;
                }
            };
            if let Some(reason) = malformed(b) {
                if seen.insert((b.session.id.clone(), b.id.clone())) {
                    unreadable.push(json!({
                        "session": b.session.name,
                        "id": b.id,
                        "status": "unreadable",
                        "reason": reason,
                    }));
                }
                continue;
            }
            if !is_attempt(b) || !seen.insert((b.session.id.clone(), b.id.clone())) {
                continue;
            }
            found.push((b, relation));
        }
    }
    found.sort_by(|(a, _), (b, _)| {
        (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id))
    });
    let mut out = Vec::new();
    for (b, relation) in found {
        if filters
            .result
            .as_deref()
            .is_some_and(|r| subject_str(b, "result") != Some(r))
            || filters
                .state
                .as_deref()
                .is_some_and(|s| subject_str(b, "state") != Some(s))
        {
            continue;
        }
        let tied = ties.questions_of(b);
        if let Some(want) = filters.question.as_deref() {
            if !tied.iter().any(|q| questions::identity(q) == want) {
                continue;
            }
        }
        let review = is_provisional(b).then(|| review_with(backend, &index, &index, &ties, b));
        if filters.awaiting_review
            && review
                .as_ref()
                .is_none_or(|r| r["state"].as_str() != Some("awaiting"))
        {
            continue;
        }
        let mut e = json!({
            "identity": questions::identity(b),
            "session": b.session.name,
            "id": b.id,
            "what": subject_str(b, "what"),
            "state": subject_str(b, "state"),
            "result": subject_str(b, "result"),
            "decider": b.decider,
            "run_mode": b.authority.run_mode,
            "time": b.time,
            "status": index.status(b),
            "question": filters
                .question
                .clone()
                .or_else(|| tied.first().map(|q| questions::identity(q))),
            "questions": tied.iter().map(|q| questions::identity(q)).collect::<Vec<_>>(),
        });
        if matches!(position, Position::At(_)) {
            e["relation"] = json!(relation);
        }
        if let Some(r) = review {
            e["review"] = r;
        }
        out.push(e);
    }
    let mut doc = json!({ "v": FORMAT_VERSION });
    match position {
        Position::In(_) => doc["in"] = json!(name),
        Position::At(_) => doc["at"] = json!(name),
    }
    doc["attempts"] = Value::Array(out);
    doc["unreadable"] = Value::Array(unreadable);
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_is_a_name_a_colon_and_a_positive_visit() {
        for ok in [
            "implement:1",
            "a:12",
            "plan.v2_x-y:3",
            "s:9999999999999999999",
        ] {
            assert!(is_state_visit(ok), "{ok}");
        }
        for bad in [
            "implement",
            "implement:",
            ":1",
            "implement:0",
            "implement:01",
            "implement:-1",
            "implement:+1",
            "implement:1.5",
            "implement:x",
            "-x:1",
            "a b:1",
            "a~1:1",
            "s:99999999999999999999",
        ] {
            assert!(!is_state_visit(bad), "{bad}");
        }
    }
}
