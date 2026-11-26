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

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::{DecisionBody, Recipient};
use super::chain;
use super::read::{self, Entry};
use super::standing::Index;
use super::FORMAT_VERSION;
use crate::session::Backend;

/// Set in a test to have `report` say on stderr when it scans the store.
pub const SCAN_PROBE_ENV: &str = "KOTO_TEST_QUESTIONS_SCAN_PROBE";

/// A question's identity: `<raiser's session id>/<decision id>`.
pub fn identity(body: &DecisionBody) -> String {
    format!("{}/{}", body.session.id, body.id)
}

fn subject_str<'a>(body: &'a DecisionBody, field: &str) -> Option<&'a str> {
    body.subject.get(field).and_then(Value::as_str)
}

fn recipient_of(body: &DecisionBody) -> Option<Recipient> {
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

/// `koto decisions open --person <handle>`: every in-effect escalation in
/// the local store whose recipient is `person:<handle>` and that no answer
/// has closed. It walks no chain: a question is found wherever it was
/// raised, whatever links its session has or lacks.
pub fn open(backend: &Backend, person: &str) -> Value {
    let index = Index::scan(backend, false);
    let mut found: Vec<&DecisionBody> = index
        .sessions()
        .values()
        .flatten()
        .filter_map(|e| match e {
            Entry::Decision(b) => Some(&**b),
            Entry::Unreadable { .. } => None,
        })
        .filter(|b| is_question(b) && index.status(b) == "in-effect")
        .filter(|b| recipient_of(b) == Some(Recipient::Person(person.to_string())))
        .filter(|b| index.answers_of(&b.session.id, &b.id).is_empty())
        .collect();
    found.sort_by(|a, b| (&a.time, &a.session.name, &a.id).cmp(&(&b.time, &b.session.name, &b.id)));
    json!({
        "v": FORMAT_VERSION,
        "person": person,
        "questions": found.iter().map(|b| json!({
            "identity": identity(b),
            "session": b.session.name,
            "id": b.id,
            "question": subject_str(b, "question"),
            "text": subject_str(b, "text"),
            "recipient": recipient_of(b).map(|r| r.to_json()),
            "time": b.time,
        })).collect::<Vec<_>>(),
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
        if index.status(q) != "in-effect" {
            continue;
        }
        let answers = index.answers_of(&q.session.id, &q.id);
        let Some(answer) = answers.first() else {
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
