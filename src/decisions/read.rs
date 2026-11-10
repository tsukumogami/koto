//! The pure read path behind `koto decisions list` and `show` (PRD R14 to
//! R16, R44).
//!
//! It reads decision files and state-file headers straight from disk (and,
//! under the cloud backend, the bucket's decision objects), and never takes
//! a lock, pulls, appends an event or writes a key, a mirror, a spool, a
//! manifest or a bucket object, so the session read and every session
//! between it and the reader stay byte-identical.

use serde_json::{json, Value};

use super::body::DecisionBody;
use super::chain;
use super::store::{self, Area};
use super::{is_decision_id, FORMAT_VERSION};
use crate::session::{Backend, SessionBackend};

/// A decision's status (PRD Terms). This issue has no reversal yet, so a
/// readable decision is `in-effect` unless its kind is `refused`.
pub fn status_of(body: &DecisionBody) -> &'static str {
    if body.kind == "refused" {
        "refused"
    } else {
        "in-effect"
    }
}

/// One entry a read found: a readable decision, or something unreadable.
#[derive(Debug, Clone)]
pub enum Entry {
    Decision(Box<DecisionBody>),
    Unreadable {
        session: String,
        id: Option<String>,
        reason: String,
    },
}

impl Entry {
    pub fn to_json(&self) -> Value {
        match self {
            Entry::Decision(body) => json!({
                "session": body.session.name,
                "id": body.id,
                "status": status_of(body),
                "decision": body,
            }),
            Entry::Unreadable {
                session,
                id,
                reason,
            } => json!({
                "session": session,
                "id": id,
                "status": "unreadable",
                "reason": reason,
            }),
        }
    }
}

/// The session id the state file of `session` names, read from the file
/// without a lock or a pull; under the cloud backend with no local copy,
/// from the bucket's state file.
fn session_id(backend: &Backend, session: &str) -> Option<String> {
    let path = backend
        .session_dir(session)
        .join(crate::session::state_file_name(session));
    if let Some(id) = crate::session::links::LinkHints::from_state_file(&path).session_id {
        return Some(id);
    }
    match backend {
        Backend::Cloud(cloud) => cloud
            .remote_header(session)
            .ok()
            .flatten()
            .map(|h| h.session_id)
            .filter(|id| !id.is_empty()),
        Backend::Local(_) => None,
    }
}

/// Every decision in `session`'s `koto/decisions/` area, readable or not,
/// after the integrity checks: a file whose name isn't a decision id, an
/// id in both areas, a body that doesn't parse or whose own id, session
/// name or session id disagrees with where it was read, each make the
/// affected entries unreadable. Aimed decisions are checked but not
/// returned: they belong to the reversal a later command records.
pub fn read_session(backend: &Backend, session: &str) -> Vec<Entry> {
    let unreadable = |id: Option<String>, reason: String| Entry::Unreadable {
        session: session.to_string(),
        id,
        reason,
    };
    let listing = store::list(backend, session);
    if let Some(problem) = listing.problem {
        return vec![unreadable(None, problem)];
    }
    let own_id = session_id(backend, session);

    let mut seen_in: std::collections::BTreeMap<String, Vec<Area>> = Default::default();
    for f in &listing.found {
        seen_in
            .entry(f.name.clone())
            .or_default()
            .push(f.area.clone());
    }

    let mut out = Vec::new();
    for f in listing.found {
        if !is_decision_id(&f.name) {
            out.push(unreadable(
                None,
                format!(
                    "'{}' in {:?} isn't a decision id",
                    sanitize(&f.name),
                    f.area
                ),
            ));
            continue;
        }
        if seen_in.get(&f.name).is_some_and(|areas| areas.len() > 1) {
            if f.area == Area::Decisions {
                out.push(unreadable(
                    Some(f.name.clone()),
                    "the id appears in more than one area".to_string(),
                ));
            }
            continue;
        }
        let checked = f
            .bytes
            .and_then(|bytes| DecisionBody::parse(&bytes))
            .and_then(|body| {
                if body.id != f.name {
                    return Err("the body's id isn't its file name".to_string());
                }
                if body.session.name != session {
                    return Err("the body names another session".to_string());
                }
                if own_id.as_deref().is_some_and(|id| id != body.session.id) {
                    return Err("the body's session id isn't the session's".to_string());
                }
                Ok(body)
            });
        match (checked, &f.area) {
            (Ok(body), Area::Decisions) => out.push(Entry::Decision(Box::new(body))),
            (Ok(_), Area::Aimed(_)) => {}
            (Err(reason), _) => out.push(unreadable(Some(f.name.clone()), reason)),
        }
    }
    out
}

/// The filters R14 names; every one given must hold (AND).
#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub kind: Option<String>,
    pub decider: Option<String>,
    /// `field=value`: the subject field equals the value, or for `path`
    /// the subject's `paths` holds it.
    pub subject: Vec<(String, String)>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub run_mode: Option<String>,
    pub defaulted: Option<bool>,
}

impl Filters {
    pub fn matches(&self, body: &DecisionBody) -> bool {
        self.kind.as_deref().is_none_or(|k| body.kind == k)
            && self
                .decider
                .as_deref()
                .is_none_or(|d| body.decider.kind() == d)
            && self.subject.iter().all(|(field, value)| {
                if field == "path" {
                    body.subject
                        .get("paths")
                        .and_then(Value::as_array)
                        .is_some_and(|ps| ps.iter().any(|p| p.as_str() == Some(value)))
                } else {
                    body.subject.get(field).and_then(Value::as_str) == Some(value.as_str())
                }
            })
            && self
                .since
                .as_deref()
                .is_none_or(|t| body.time.as_str() >= t)
            && self.until.as_deref().is_none_or(|t| body.time.as_str() < t)
            && self
                .run_mode
                .as_deref()
                .is_none_or(|m| body.authority.run_mode.value == m)
            && self.defaulted.is_none_or(|d| body.decider.defaulted() == d)
    }
}

/// Sort entries by time, then session, then id; unreadable entries,
/// which have no time, go last in session and id order.
fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        let key = |e: &Entry| match e {
            Entry::Decision(body) => (
                0,
                body.time.clone(),
                body.session.name.clone(),
                body.id.clone(),
            ),
            Entry::Unreadable { session, id, .. } => (
                1,
                String::new(),
                session.clone(),
                id.clone().unwrap_or_default(),
            ),
        };
        key(a).cmp(&key(b))
    });
}

/// The sessions in the local store whose chain, walked now, includes
/// `root`: every session below it, through any number of finished
/// sessions in between. Sorted by name.
pub fn sessions_below(backend: &Backend, root: &str) -> Vec<String> {
    let base = match backend {
        Backend::Local(l) => l.base_dir().to_path_buf(),
        Backend::Cloud(c) => c.local().base_dir().to_path_buf(),
    };
    let Ok(dir) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != root && chain::session_present(backend, n))
        .filter(|n| {
            chain::snapshot(backend, n)
                .links
                .iter()
                .any(|l| l.session == root)
        })
        .collect();
    names.sort();
    names
}

/// `koto decisions list <session> --format v1`, or with `below` the
/// decisions of every session below `session`.
pub fn list(backend: &Backend, session: &str, below: bool, filters: &Filters) -> Value {
    let sessions = if below {
        sessions_below(backend, session)
    } else {
        vec![session.to_string()]
    };
    let mut entries: Vec<Entry> = sessions
        .iter()
        .flat_map(|s| read_session(backend, s))
        .filter(|e| match e {
            Entry::Decision(body) => filters.matches(body),
            Entry::Unreadable { .. } => true,
        })
        .collect();
    sort(&mut entries);
    let mut out = json!({
        "v": FORMAT_VERSION,
        "session": session,
        "decisions": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
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
    let entries = read_session(backend, session);
    let found = entries.iter().find(|e| match e {
        Entry::Decision(body) => body.id == id,
        Entry::Unreadable { id: Some(i), .. } => i == id,
        Entry::Unreadable { id: None, .. } => false,
    });
    let whole_session_unreadable = entries
        .iter()
        .find(|e| matches!(e, Entry::Unreadable { id: None, .. }));
    let entry = found.or(whole_session_unreadable)?;
    let mut out = entry.to_json();
    out["v"] = json!(FORMAT_VERSION);
    out["id"] = json!(id);
    Some(out)
}

/// Strip control and terminal escape characters from text read from a
/// session, for an error message.
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}
