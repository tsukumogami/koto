//! One question's history, by its identity `<raiser's session id>/<id>`.
//!
//! The read gives the question as raised, where it stands now (the same
//! resolution every other read applies, [`questions::resolve`]), and an
//! ordered list of everything this machine's store holds about it: the
//! raise, every pass-up, every `answer` entry whatever its outcome, each
//! decision whose `basis` cites one of the question's answers, and each
//! file in the question's aimed area of any session that doesn't read.
//! An entry that doesn't count is listed and marked, never left out.
//!
//! The order is causal. The raise comes first. The rest sort by time, then
//! the recording session's name, then id, with two corrections a skewed
//! clock can't undo: a pass-up that moved the question never comes before
//! the pass-up it follows (the order of `addressees`), and a citing
//! decision never comes before the entry it cites. Unreadable files carry
//! no time and come last, by session name, then file name.
//!
//! The read looks only at the local store: no lock, no pull, no bucket
//! listing, no write.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::body::DecisionBody;
use super::questions::{self, Runs};
use super::read::Entry;
use super::standing::{self, Index};
use super::store::{self, Area};
use super::{chain, packets, FORMAT_VERSION};
use crate::session::Backend;

/// Why the history can't be read.
pub enum Refusal {
    /// No session in the local store holds a decision with that identity.
    NotFound,
    /// The decision isn't a question: not an `escalate`, or one with no
    /// typed recipient.
    NotQuestion(String),
}

/// The question with `identity`'s parts, read from the raiser's own
/// directory when the store holds it there, else from any copy.
fn find<'i>(index: &'i Index, sid: &str, id: &str) -> Option<&'i DecisionBody> {
    let mut copies = index
        .sessions()
        .iter()
        .flat_map(|(holder, entries)| entries.iter().map(move |e| (holder, e)))
        .filter_map(|(holder, e)| match e {
            Entry::Decision(b) if b.session.id == sid && b.id == id => Some((holder, &**b)),
            _ => None,
        });
    let first = copies.next()?;
    if first.0 == &first.1.session.name {
        return Some(first.1);
    }
    Some(
        copies
            .find(|(holder, b)| *holder == &b.session.name)
            .map_or(first.1, |(_, b)| b),
    )
}

/// The reading of a stored person answer, as `show` gives it, or `None`
/// for an entry without a prompt.
fn reading(index: &Index, backend: &Backend, body: &DecisionBody) -> Option<&'static str> {
    body.prompt.as_ref()?;
    let here = index
        .holders(body)
        .iter()
        .any(|h| standing::verified_in(backend, h, body));
    Some(if here {
        super::prompt::VERIFIED
    } else {
        super::prompt::REMOTE_READING
    })
}

struct Item {
    key: (String, String, String),
    /// Indexes into the item list of the entries this one must follow.
    /// The items for `aimed` are pushed first, one per entry and in its
    /// order, so an index into `aimed` is also an index into the item list;
    /// pass-up and citing constraints rely on that.
    after: Vec<usize>,
    json: Value,
}

/// `koto decisions history <session id>/<decision id>`.
pub fn history(backend: &Backend, sid: &str, id: &str) -> Result<Value, Refusal> {
    let index = Index::scan(backend, false);
    let runs = Runs::new(backend);
    let Some(q) = find(&index, sid, id) else {
        return Err(Refusal::NotFound);
    };
    if !questions::is_question(q) {
        return Err(Refusal::NotQuestion(q.kind.clone()));
    }
    let resolution = questions::resolve(&index, &runs, q);
    let identity = questions::identity(q);
    let aimed_at = |b: &DecisionBody| {
        b.target
            .as_ref()
            .is_some_and(|t| t.id == q.session.id && t.decision == q.id)
    };

    // Everything aimed at the question, once each, fenced entries included
    // so they are listed as not counting.
    let mut seen = BTreeSet::new();
    let aimed: Vec<&DecisionBody> = questions::decisions(&index)
        .filter(|b| (b.kind == "answer" || b.kind == "readdress") && aimed_at(b))
        .filter(|b| seen.insert((b.session.id.clone(), b.id.clone())))
        .collect();

    let mut items: Vec<Item> = Vec::new();
    for b in &aimed {
        let judged = questions::judge_entry(&index, backend, b);
        let mut e = json!({
            "entry": if b.kind == "answer" { "answer" } else { "pass-up" },
            "session": b.session.name,
            "session_id": b.session.id,
            "id": b.id,
            "time": b.time,
            "kind": b.kind,
            "outcome": b.outcome,
            "decider": b.decider,
            "counted": judged.counted,
        });
        if let Some(r) = judged.reason {
            e["reason"] = json!(r);
        }
        if b.kind == "answer" {
            e["text"] = json!(questions::subject_str(b, "text"));
            e["governs"] = json!(judged.governs.unwrap_or(false));
            if let Some(r) = reading(&index, backend, b) {
                e["reading"] = json!(r);
            }
        } else {
            e["recipient"] = json!(questions::recipient_of(b).map(|r| r.to_json()));
        }
        items.push(Item {
            key: (b.time.clone(), b.session.name.clone(), b.id.clone()),
            after: Vec::new(),
            json: e,
        });
    }
    // A pass-up that moved the question follows the one before it. Every
    // applied pass-up comes from the same index and is aimed at the
    // question, so it is in `aimed`; one that somehow isn't only loses its
    // ordering constraint rather than failing the read.
    let position = |b: &DecisionBody| aimed.iter().position(|a| questions::same(a, b));
    for pair in resolution.applied.windows(2) {
        if let (Some(before), Some(next)) = (position(pair[0]), position(pair[1])) {
            items[next].after.push(before);
        }
    }

    // Decisions citing one of the question's answers.
    let answers: Vec<usize> = (0..aimed.len())
        .filter(|&i| aimed[i].kind == "answer")
        .collect();
    let mut seen_citing = BTreeSet::new();
    for d in questions::decisions(&index) {
        if aimed_at(d) && (d.kind == "answer" || d.kind == "readdress") {
            continue;
        }
        let mut cites: Vec<usize> = Vec::new();
        for c in &d.basis {
            if let Some(Entry::Decision(a)) = index.find(&c.session, &c.id) {
                if a.time <= d.time {
                    if let Some(&i) = answers.iter().find(|&&i| questions::same(aimed[i], a)) {
                        if !cites.contains(&i) {
                            cites.push(i);
                        }
                    }
                }
            }
        }
        if cites.is_empty() || !seen_citing.insert((d.session.id.clone(), d.id.clone())) {
            continue;
        }
        let named: Vec<Value> = cites
            .iter()
            .map(|&i| json!({"session": aimed[i].session.name, "id": aimed[i].id}))
            .collect();
        items.push(Item {
            key: (d.time.clone(), d.session.name.clone(), d.id.clone()),
            after: cites,
            json: json!({
                "entry": "citing",
                "session": d.session.name,
                "session_id": d.session.id,
                "id": d.id,
                "time": d.time,
                "kind": d.kind,
                "outcome": d.outcome,
                "decider": d.decider,
                "cites": named,
            }),
        });
    }

    let mut entries = vec![json!({
        "entry": "raise",
        "session": q.session.name,
        "session_id": q.session.id,
        "id": q.id,
        "time": q.time,
        "kind": q.kind,
        "decider": q.decider,
        "recipient": questions::recipient_of(q).map(|r| r.to_json()),
    })];
    entries.extend(causal(items));
    entries.extend(unreadable(&index, backend, &q.session.id));

    let in_effect = resolution.in_effect.map(|a| {
        let mut out = json!({
            "session": a.session.name,
            "id": a.id,
            "outcome": a.outcome,
            "text": questions::subject_str(a, "text"),
            "decider": a.decider,
            "run_mode": a.authority.run_mode,
            "time": a.time,
        });
        if let Some(r) = reading(&index, backend, a) {
            out["reading"] = json!(r);
        }
        out
    });
    let packet = questions::subject_str(q, "packet");
    let content_stored =
        packet.is_some() && matches!(packets::read(backend, &q.session.name, &q.id), Ok(Some(_)));
    let status = index.status(q);
    let open = status == "in-effect" && resolution.in_effect.is_none();
    Ok(json!({
        "v": FORMAT_VERSION,
        "identity": identity,
        "question": {
            "id": q.id,
            "question": questions::subject_str(q, "question"),
            "text": questions::subject_str(q, "text"),
            "label": questions::label_of(q),
            "need": questions::need_of(q),
            "state": questions::subject_str(q, "state"),
            "packet": packet,
            "content_stored": content_stored,
            "recipient": questions::recipient_of(q).map(|r| r.to_json()),
            "time": q.time,
        },
        "raiser": {"name": q.session.name, "id": q.session.id},
        "run_mode": q.authority.run_mode,
        "successor": chain::run_successors(backend, &q.session.name).into_iter().next(),
        "status": status,
        "open": open,
        "addressee": resolution.current().map(|r| r.to_json()),
        "addressees": resolution.history.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
        "answer": in_effect,
        "entries": entries,
    }))
}

/// `items` sorted by key, each held back until every entry it must follow
/// is out.
fn causal(mut items: Vec<Item>) -> Vec<Value> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].key.cmp(&items[b].key));
    let mut out_done = vec![false; items.len()];
    let mut out = Vec::with_capacity(items.len());
    let mut pending: Vec<usize> = Vec::new();
    let ready = |i: usize, done: &[bool], items: &[Item]| items[i].after.iter().all(|&d| done[d]);
    for i in order {
        pending.push(i);
        while let Some(pos) = pending.iter().position(|&p| ready(p, &out_done, &items)) {
            let p = pending.remove(pos);
            out_done[p] = true;
            out.push(p);
        }
    }
    // A dependency the list doesn't hold can't be waited on.
    out.extend(pending);
    out.into_iter()
        .map(|i| std::mem::take(&mut items[i].json))
        .collect()
}

/// The files in any session's aimed area for the raiser's session id that
/// don't read, each with its session.
fn unreadable(index: &Index, backend: &Backend, raiser_id: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (holder, entries) in index.sessions() {
        let bad: Vec<(&str, &str)> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Unreadable {
                    id: Some(i),
                    reason,
                    ..
                } => Some((i.as_str(), reason.as_str())),
                _ => None,
            })
            .collect();
        if bad.is_empty() {
            continue;
        }
        let listing = store::list_with(backend, holder, false);
        for (id, reason) in bad {
            let here = listing
                .found
                .iter()
                .any(|f| f.name == id && f.area == Area::Aimed(raiser_id.to_string()));
            if here {
                out.push(json!({
                    "entry": "unreadable",
                    "session": holder,
                    "id": id,
                    "counted": false,
                    "reason": "unreadable",
                    "detail": reason,
                }));
            }
        }
    }
    out
}
