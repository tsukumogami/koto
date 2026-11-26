//! The precedent read: the answers a person gave that are in effect in a
//! session's chain, narrowed to a question label.
//!
//! A session's chain is every session and anchor connected to it through
//! the links `koto decisions open --at` follows ([`origin::above`]), in
//! either direction, together with the sessions each of those continues,
//! and those continuing it, through a confirmed succession, and the
//! sessions connected to those in turn. An answer belongs to the chain
//! when the session it was given in is in the chain, or when the session
//! that raised its question is still in the store and is in the chain. A raiser is matched by session id, so
//! a later session reusing a removed raiser's name is not that raiser.
//!
//! Only verified person answers in effect come back: a session's answer, a
//! settle and a withdrawal are never precedent, and a person answer that
//! reads as a remote session's (an import, a pull) isn't either. The label
//! is the question's when the store still holds it, else the one koto
//! copied onto the answer when it was recorded; it is compared byte for
//! byte.
//!
//! The read looks only at the local store, the way [`Index::scan`] does
//! with `remote` unset: no lock, no pull, no bucket listing, no write.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::{DecisionBody, Target};
use super::chain;
use super::questions::{self, Runs};
use super::read;
use super::standing::Index;
use super::FORMAT_VERSION;
use crate::session::{origin, Backend};

fn local(backend: &Backend) -> &crate::session::local::LocalBackend {
    match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    }
}

/// The chain of `name`: every session and anchor in the local store
/// connected to it through [`origin::above`] links, in either direction,
/// with the sessions each continues ([`chain::run_predecessors`]) and those
/// that continue it ([`chain::run_successors`]), and whatever those connect
/// to in turn. `name` is always in it.
pub fn chain_of(backend: &Backend, name: &str) -> BTreeSet<String> {
    let sessions = read::local_sessions(backend);
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in &sessions {
        for parent in origin::above(local(backend), s) {
            edges.entry(s.clone()).or_default().insert(parent.clone());
            edges.entry(parent).or_default().insert(s.clone());
        }
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue = vec![name.to_string()];
    while let Some(at) = queue.pop() {
        if !seen.insert(at.clone()) {
            continue;
        }
        for next in edges.get(&at).into_iter().flatten() {
            if !seen.contains(next) {
                queue.push(next.clone());
            }
        }
        // A succession joins the chain in both directions: a successor that
        // has no dispatched-by link of its own still shares its
        // predecessor's chain, and the predecessor shares the successor's.
        for p in chain::run_predecessors(backend, &at)
            .into_iter()
            .chain(chain::run_successors(backend, &at))
        {
            if !seen.contains(&p) {
                queue.push(p);
            }
        }
    }
    seen
}

/// One precedent: the verified person answer in effect on a question, with
/// the question's identity and label.
pub struct Precedent<'i> {
    pub identity: String,
    pub label: Option<String>,
    pub answer: &'i DecisionBody,
    /// The raising session's name, while the store still holds the
    /// question.
    pub raiser: Option<String>,
}

/// Every verified person answer in effect in the local store, once per
/// question, in no particular order.
pub fn in_effect<'i>(index: &'i Index, backend: &Backend) -> Vec<Precedent<'i>> {
    let runs = Runs::new(backend);
    let mut targets: BTreeMap<(String, String), &Target> = BTreeMap::new();
    for b in questions::decisions(index) {
        if b.kind != "answer" {
            continue;
        }
        if let Some(t) = &b.target {
            targets
                .entry((t.id.clone(), t.decision.clone()))
                .or_insert(t);
        }
    }
    let mut out = Vec::new();
    for ((sid, did), target) in targets {
        let held = questions::question_of(index, target).filter(|q| questions::is_question(q));
        let winner = match held {
            Some(q) => {
                if index.status(q) != "in-effect" {
                    continue;
                }
                match questions::answer_in_effect(index, &runs, q) {
                    Some(w) if questions::verified_person(index, backend, w) => w,
                    _ => continue,
                }
            }
            // The question is gone with its raiser: what is left says which
            // answer is in effect, and whether a reversal or a replacing
            // answer withdrew it.
            None => match questions::gone_question_answer(index, backend, &sid, &did) {
                Some(w) => w,
                None => continue,
            },
        };
        let label = held
            .and_then(questions::label_of)
            .or_else(|| questions::subject_str(winner, "label"))
            .map(str::to_string);
        out.push(Precedent {
            identity: format!("{sid}/{did}"),
            label,
            answer: winner,
            raiser: held.map(|q| q.session.name.clone()),
        });
    }
    out
}

/// `koto decisions precedent --person <handle> --chain <session-or-anchor>
/// [--label <text>]`.
pub fn precedent(backend: &Backend, person: &str, at: &str, label: Option<&str>) -> Value {
    let index = Index::scan(backend, false);
    let chain_set = chain_of(backend, at);

    // The run of the named session, for `cited` and `below`.
    let mut run: BTreeSet<String> = BTreeSet::new();
    run.insert(at.to_string());
    run.extend(chain::run_predecessors(backend, at));
    let below = origin::below(local(backend), &read::local_sessions(backend), &run);
    let run_decisions: Vec<&DecisionBody> = run
        .iter()
        .filter_map(|s| index.sessions().get(s))
        .flatten()
        .filter_map(|e| match e {
            read::Entry::Decision(b) => Some(&**b),
            read::Entry::Unreadable { .. } => None,
        })
        .collect();
    let cited = questions::cited(&index, &run_decisions);

    let mut found: Vec<Precedent> = in_effect(&index, backend)
        .into_iter()
        .filter(|p| questions::person_handle(p.answer) == Some(person))
        .filter(|p| label.is_none_or(|l| p.label.as_deref() == Some(l)))
        .filter(|p| {
            chain_set.contains(&p.answer.session.name)
                || p.raiser.as_ref().is_some_and(|r| chain_set.contains(r))
        })
        .collect();
    found.sort_by(|a, b| questions::order_key(a.answer).cmp(&questions::order_key(b.answer)));
    let answers: Vec<Value> = found
        .iter()
        .map(|p| {
            let a = p.answer;
            json!({
                "identity": p.identity,
                "label": p.label,
                "session": a.session.name,
                "id": a.id,
                "text": questions::subject_str(a, "text"),
                "time": a.time,
                "decider": a.decider,
                "reading": super::prompt::VERIFIED,
                "cited": cited.contains(&(a.session.id.clone(), a.id.clone())),
                "below": run.contains(&a.session.name) || below.contains(&a.session.name),
            })
        })
        .collect();
    json!({
        "v": FORMAT_VERSION,
        "person": person,
        "chain": at,
        "label": label,
        "answers": answers,
    })
}
