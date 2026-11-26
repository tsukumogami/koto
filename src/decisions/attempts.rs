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
//! the question's `escalate` names the attempt, or the attempt names the
//! `escalate`. A basis entry names its session by name, which a later
//! session can reuse, so a citation counts only when the decision it
//! names, read now, is the one meant: the right kind, and recorded no
//! later than the decision citing it. That is how the `questions` report
//! on `koto next` resolves a citation.
//!
//! A provisional judgment (`result` `provisional`) awaits a person's
//! review until one of two things happens: it is reversed, or a question
//! whose in-effect `escalate` names it in its `basis` takes a person's
//! answer that reads `verified` on this host. Nothing else ends the wait:
//! not a session's answer, not a remote person answer, not a decision that
//! cites the judgment.
//!
//! Every read here uses the local store alone, the way [`Index::scan`]
//! does with `remote` unset: no lock, no pull, no bucket request, no write.

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

/// Whether `body` is an attempt: a judgment carrying `result`.
pub fn is_attempt(body: &DecisionBody) -> bool {
    body.kind == "judgment" && subject_str(body, "result").is_some()
}

/// Whether `body` is a provisional judgment.
pub fn is_provisional(body: &DecisionBody) -> bool {
    body.kind == "judgment" && subject_str(body, "result") == Some("provisional")
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

/// The questions tied to each attempt, keyed by the attempt's identity,
/// and for each provisional judgment the questions whose `escalate` names
/// it (only those can end its wait).
struct Ties<'i> {
    questions: BTreeMap<(String, String), Vec<&'i DecisionBody>>,
    reviews: BTreeMap<(String, String), Vec<&'i DecisionBody>>,
}

impl<'i> Ties<'i> {
    fn build(index: &'i Index) -> Self {
        let mut questions: BTreeMap<(String, String), Vec<&DecisionBody>> = BTreeMap::new();
        let mut reviews: BTreeMap<(String, String), Vec<&DecisionBody>> = BTreeMap::new();
        let all = decisions(index);
        for d in &all {
            for c in &d.basis {
                let Some(target) = cited(index, &c.session, &c.id) else {
                    continue;
                };
                if target.time > d.time {
                    // A later decision under a reused name and id is never
                    // the one that was cited.
                    continue;
                }
                if questions::is_question(d) && is_attempt(target) {
                    // The escalate names the attempt.
                    let key = (target.session.id.clone(), target.id.clone());
                    questions.entry(key.clone()).or_default().push(*d);
                    if is_provisional(target) {
                        reviews.entry(key).or_default().push(*d);
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

/// The first answer to `question` by a person that reads `verified` on
/// this host, in the order answers are kept (time, then the answering
/// session's name, then id).
fn verified_person_answer<'i>(
    backend: &Backend,
    index: &'i Index,
    question: &DecisionBody,
) -> Option<&'i DecisionBody> {
    index
        .answers_of(&question.session.id, &question.id)
        .into_iter()
        .filter(|a| a.prompt.is_some())
        .find(|a| {
            index
                .holders(a)
                .iter()
                .any(|h| standing::verified_in(backend, h, a))
        })
}

fn review_with(backend: &Backend, index: &Index, ties: &Ties<'_>, body: &DecisionBody) -> Value {
    if let Some(w) = index.winner_of(&body.session.id, &body.id) {
        if index.status(body) == "reversed" {
            return json!({
                "state": "reversed",
                "reversed_by": standing::reversal_json(w),
            });
        }
    }
    let live: Vec<&DecisionBody> = ties
        .reviews_of(body)
        .iter()
        .copied()
        .filter(|q| index.status(q) == "in-effect")
        .collect();
    for q in &live {
        if let Some(a) = verified_person_answer(backend, index, q) {
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
    json!({
        "state": "awaiting",
        "question": live.first().map(|q| questions::identity(q)),
    })
}

/// A provisional judgment's review: `awaiting`, with the identity of the
/// earliest in-effect question whose `escalate` names it (or null);
/// `reviewed`, with that question and the verified person answer that
/// ended the wait; or `reversed`, with the winning reversal as `show`
/// reports it.
pub fn review(backend: &Backend, index: &Index, body: &DecisionBody) -> Value {
    review_with(backend, index, &Ties::build(index), body)
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
    let ties = Ties::build(&index);
    let mut seen = BTreeSet::new();
    let mut found: Vec<(&DecisionBody, &'static str)> = Vec::new();
    for (session, entries) in index.sessions() {
        let relation = if run.contains(session) {
            "at"
        } else if below.contains(session) {
            "below"
        } else {
            continue;
        };
        for e in entries {
            let Entry::Decision(b) = e else { continue };
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
        let review = is_provisional(b).then(|| review_with(backend, &index, &ties, b));
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
            "question": tied.first().map(|q| questions::identity(q)),
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
