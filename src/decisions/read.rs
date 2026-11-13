//! The pure read path under `koto decisions list` and `show` (PRD R14 to
//! R16, R44): one session's decisions with their integrity checks.
//! The standing module, above this one, adds the statuses reversals give.
//! [`aimed_on_chain`] is the read `koto next` makes of what is aimed at a
//! live session from the sessions on its chain (R22, R23); the engine
//! decides from it whether the tick stops.
//!
//! It reads decision files and state-file headers straight from disk (and,
//! under the cloud backend, the bucket's decision objects), and never takes
//! a lock, pulls, appends an event or writes a key, a mirror, a spool, a
//! manifest or a bucket object, so the session read and every session
//! between it and the reader stay byte-identical.

use serde_json::{json, Value};

use super::body::{BasisRef, DecisionBody, Target};
use super::chain;
use super::is_decision_id;
use super::store::{self, Area};
use crate::session::{Backend, SessionBackend};

/// A decision's status as its body alone shows it, before any reversal
/// is applied.
fn status_of(body: &DecisionBody) -> &'static str {
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
fn session_id(backend: &Backend, session: &str, remote: bool) -> Option<String> {
    let path = backend
        .session_dir(session)
        .join(crate::session::state_file_name(session));
    if let Some(id) = crate::session::links::LinkHints::from_state_file(&path).session_id {
        return Some(id);
    }
    match backend {
        Backend::Cloud(_) if !remote => None,
        Backend::Cloud(cloud) => cloud
            .remote_header(session)
            .ok()
            .flatten()
            .map(|h| h.session_id)
            .filter(|id| !id.is_empty()),
        Backend::Local(_) => None,
    }
}

/// Every decision in `session`'s two areas, readable or not, after the
/// integrity checks: a file whose name isn't a decision id, an id in both
/// areas, a body that doesn't parse or whose own id, session name or
/// session id disagrees with where it was read, and an aimed body that
/// isn't a reversal or whose target id isn't its `koto/aimed/<t>/`
/// directory, each make the affected entries unreadable.
pub fn read_session(backend: &Backend, session: &str) -> Vec<Entry> {
    read_session_with(backend, session, true)
}

/// [`read_session`], reaching the bucket only when `remote` is set.
pub fn read_session_with(backend: &Backend, session: &str, remote: bool) -> Vec<Entry> {
    let unreadable = |id: Option<String>, reason: String| Entry::Unreadable {
        session: session.to_string(),
        id,
        reason,
    };
    let listing = store::list_with(backend, session, remote);
    if let Some(problem) = listing.problem {
        return vec![unreadable(None, problem)];
    }
    let own_id = session_id(backend, session, remote);

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
            if seen_in[&f.name].first() == Some(&f.area) {
                out.push(unreadable(
                    Some(f.name.clone()),
                    "the id appears in more than one area".to_string(),
                ));
            }
            continue;
        }
        let checked = f
            .bytes
            .and_then(|bytes| check_body(session, own_id.as_deref(), &f.area, &f.name, &bytes));
        match checked {
            Ok(body) => out.push(Entry::Decision(Box::new(body))),
            Err(reason) => out.push(unreadable(Some(f.name.clone()), reason)),
        }
    }
    out
}

/// The checks every reader applies to one decision file: the body parses,
/// its own id, session name and session id agree with where it was read, a
/// reversal sits in `koto/aimed/<t>/` with target `t`, and anything else
/// there is a replacement (a decision whose `basis` names a reversal) with
/// target `t`.
fn check_body(
    session: &str,
    own_id: Option<&str>,
    area: &Area,
    name: &str,
    bytes: &[u8],
) -> Result<DecisionBody, String> {
    let body = DecisionBody::parse(bytes)?;
    if body.id != name {
        return Err("the body's id isn't its file name".to_string());
    }
    if body.session.name != session {
        return Err("the body names another session".to_string());
    }
    if own_id.is_some_and(|id| id != body.session.id) {
        return Err("the body's session id isn't the session's".to_string());
    }
    let filed_under = |target: &str, what: &str| {
        if body.target.as_ref().is_none_or(|t| t.id != target) {
            Err(format!(
                "the {what}'s target isn't the session it is filed under"
            ))
        } else {
            Ok(())
        }
    };
    match area {
        Area::Aimed(target) if body.kind == "reverse" => filed_under(target, "reversal")?,
        Area::Aimed(target) if is_replacement(&body) => filed_under(target, "replacement")?,
        Area::Aimed(_) => return Err("an aimed decision isn't a reversal".to_string()),
        Area::Decisions if body.kind == "reverse" => {
            return Err("a reversal sits outside koto/aimed/".to_string());
        }
        Area::Decisions => {}
    }
    Ok(body)
}

/// Whether `body` is shaped as a replacement: neither a reversal nor a
/// refusal, aimed at a decision, with a `basis`.
fn is_replacement(body: &DecisionBody) -> bool {
    body.kind != "reverse"
        && body.kind != "refused"
        && body.target.is_some()
        && !body.basis.is_empty()
}

/// The decision a new decision with `basis` replaces (PRD R22's clearing
/// path): when an entry of `basis` names a `reverse`, the decision that
/// reversal is aimed at. A replacement is recorded under
/// `koto/aimed/<target id>/` with this target, where the reversal read at
/// `koto next` finds it. Reads local copies only.
pub fn replacement_target(backend: &Backend, basis: &[BasisRef]) -> Option<Target> {
    basis.iter().find_map(|b| {
        read_session_with(backend, &b.session, false)
            .into_iter()
            .find_map(|e| match e {
                Entry::Decision(d) if d.id == b.id && d.kind == "reverse" => d.target,
                _ => None,
            })
    })
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
pub fn sort(entries: &mut [Entry]) {
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
    local_sessions(backend)
        .into_iter()
        .filter(|n| n != root)
        .filter(|n| {
            chain::snapshot(backend, n)
                .links
                .iter()
                .any(|l| l.session == root)
        })
        .collect()
}

/// Every session in the local store, sorted by name.
pub fn local_sessions(backend: &Backend) -> Vec<String> {
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
        .filter(|n| chain::session_present(backend, n))
        .collect();
    names.sort();
    names
}

/// Strip control and terminal escape characters from text read from a
/// session, for an error message.
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}

// ===== The reversal read at `koto next` (PRD R22, R23) =====

/// What [`aimed_on_chain`] read, counted so a test can tell a skipped read
/// from a cheap one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reads {
    /// Link directories listed by the chain walk.
    pub links_listed: usize,
    /// `koto/aimed/<aim-id>/` listings, local or bucket, across the chain.
    pub aimed_listed: usize,
    /// Decision files read.
    pub files_read: usize,
}

/// A chain session the read couldn't trust, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableSession {
    pub session: String,
    pub reason: String,
}

/// Everything aimed at a session from the sessions on its chain, itself
/// first.
#[derive(Debug, Clone, Default)]
pub struct ChainRead {
    /// Whether the session's own areas held nothing, so nothing else was
    /// read.
    pub skipped: bool,
    /// The session's header id and every id in `koto/imported-from`.
    pub aim_ids: Vec<String>,
    /// The sessions read, the session itself first, in R12's order.
    pub sessions: Vec<String>,
    /// Every readable aimed decision found, with the session holding it.
    pub found: Vec<(String, DecisionBody)>,
    pub unreadable: Vec<UnreadableSession>,
    /// Whether a link named a session that isn't there.
    pub incomplete: bool,
    pub reads: Reads,
}

/// The largest `koto/imported-from` read.
const MAX_IMPORTED_FROM_BYTES: u64 = 64 * 1024;

fn ctx_dir(backend: &Backend, session: &str) -> std::path::PathBuf {
    match backend {
        Backend::Local(l) => l.ctx_path(session),
        Backend::Cloud(c) => c.local().ctx_path(session),
    }
}

/// `lstat` `path`: `Ok(false)` when absent, `Ok(true)` for a real
/// directory, `Err` for anything else there or a failed `lstat`.
fn is_real_dir(path: &std::path::Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{} can't be read: {e}", path.display())),
        Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a symlink", path.display())),
        Ok(m) if !m.is_dir() => Err(format!("{} isn't a directory", path.display())),
        Ok(_) => Ok(true),
    }
}

/// Whether `session`'s own two areas hold any entry, without reading one.
/// An area that can't be listed counts as holding something.
///
/// This looks at the local copy only, under the cloud backend too, and
/// that is enough because of an ordering this relies on: only the
/// session's own commands write its own areas, and they write the local
/// copy before the bucket's; and `handle_next` reads the session's log
/// (`Backend::read_events`) before it computes the block, which under the
/// cloud backend pulls the session first, decision keys included, and
/// strictly when its home is another workspace. A caller that reaches this
/// without that pull would need to list the bucket's areas as well.
fn holds_decisions(backend: &Backend, session: &str) -> bool {
    let ctx = ctx_dir(backend, session);
    [super::DECISIONS_AREA, super::AIMED_AREA]
        .iter()
        .any(|area| {
            let dir = area.split('/').fold(ctx.clone(), |p, c| p.join(c));
            match is_real_dir(&dir) {
                Ok(false) => false,
                Ok(true) => std::fs::read_dir(&dir).map_or(true, |mut d| d.next().is_some()),
                Err(_) => true,
            }
        })
}

/// The aim ids of `session`: its header id and every source id its
/// `koto/imported-from` names, in today's single-source form
/// (`{"from_session_id": ...}`) or as a list of those. `Err` when the key
/// is there but can't be read or parsed.
fn aim_ids(backend: &Backend, session: &str, session_id: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    if !session_id.is_empty() {
        ids.push(session_id.to_string());
    }
    let path = ctx_dir(backend, session).join("koto").join("imported-from");
    let bytes = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
        Err(e) => return Err(format!("koto/imported-from can't be read: {e}")),
        Ok(m) if !m.is_file() => return Err("koto/imported-from isn't a file".to_string()),
        Ok(m) if m.len() > MAX_IMPORTED_FROM_BYTES => {
            return Err("koto/imported-from is too large".to_string())
        }
        Ok(_) => {
            std::fs::read(&path).map_err(|e| format!("koto/imported-from can't be read: {e}"))?
        }
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("koto/imported-from doesn't parse: {e}"))?;
    let records = match value {
        Value::Array(list) => list,
        one => vec![one],
    };
    for record in records {
        let id = record
            .get("from_session_id")
            .and_then(Value::as_str)
            .filter(|id| store::is_target_id(id))
            .ok_or_else(|| "koto/imported-from names no valid session id".to_string())?;
        if !ids.iter().any(|i| i == id) {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// The files in `ctx_dir`'s `koto/aimed/<aim>/`, read without following a
/// symlink. An absent directory holds nothing; `Err` when one of the
/// directories on the way is a symlink or not a directory, or can't be
/// listed.
fn aimed_local(ctx_dir: &std::path::Path, aim: &str) -> Result<Vec<store::Found>, String> {
    let koto = ctx_dir.join("koto");
    let aimed = koto.join("aimed");
    let target = aimed.join(aim);
    for dir in [&koto, &aimed, &target] {
        if !is_real_dir(dir)? {
            return Ok(Vec::new());
        }
    }
    let mut found = Vec::new();
    let listing = std::fs::read_dir(&target)
        .map_err(|e| format!("{} can't be listed: {e}", target.display()))?;
    for entry in listing {
        let entry = entry.map_err(|e| format!("{} can't be listed: {e}", target.display()))?;
        let path = entry.path();
        let bytes = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => Err("is a symlink".to_string()),
            Ok(m) if !m.is_file() => Err("isn't a regular file".to_string()),
            Ok(_) => store::read_file(&path),
            Err(e) => Err(format!("can't be read: {e}")),
        };
        found.push(store::Found {
            area: Area::Aimed(aim.to_string()),
            name: entry.file_name().to_string_lossy().into_owned(),
            bytes,
        });
        if found.len() > super::MAX_AREA_ENTRIES {
            return Err(format!(
                "{} holds more than {} entries",
                target.display(),
                super::MAX_AREA_ENTRIES
            ));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// The strict walk the reversal read takes: the chain in R12's order, the
/// session itself first, where a link that can't be listed, a session held
/// only in the bucket, or a chain past [`chain::MAX_CHAIN`] sessions makes
/// a session unreadable instead of ending the walk.
struct StrictWalk<'a> {
    backend: &'a Backend,
    seen: std::collections::BTreeSet<String>,
    out: ChainRead,
}

impl StrictWalk<'_> {
    fn unreadable(&mut self, session: &str, reason: impl Into<String>) {
        self.out.unreadable.push(UnreadableSession {
            session: session.to_string(),
            reason: reason.into(),
        });
    }

    /// The session `peer` names now: under the cloud backend, the session
    /// it was imported as when its source carries a migration marker.
    fn resolve(&self, peer: &str) -> String {
        if let Err(e) = self.backend.check_not_migrated(peer) {
            if let Some(moved) = e.downcast_ref::<crate::session::SessionMigrated>() {
                if crate::discover::validate_workflow_name(&moved.target).is_ok() {
                    return moved.target.clone();
                }
            }
        }
        peer.to_string()
    }

    /// Add `peer` to the chain unless it is already on it. Returns the
    /// session the walk goes on upward from, if any.
    fn visit(&mut self, peer: &str) -> Option<String> {
        let peer = self.resolve(peer);
        if !self.seen.insert(peer.clone()) {
            return None;
        }
        // The session itself is the first entry, so the chain above it
        // holds one fewer.
        if self.out.sessions.len() > chain::MAX_CHAIN {
            self.unreadable(
                &peer,
                format!(
                    "the chain is longer than {} sessions here",
                    chain::MAX_CHAIN
                ),
            );
            return None;
        }
        if !chain::session_present(self.backend, &peer) {
            if let Backend::Cloud(cloud) = self.backend {
                match cloud.remote_header(&peer) {
                    Ok(None) => {}
                    Ok(Some(_)) => {
                        self.unreadable(&peer, "it is held only in the bucket; pull it to read it");
                        return None;
                    }
                    Err(e) => {
                        self.unreadable(&peer, format!("the bucket can't be read: {e:#}"));
                        return None;
                    }
                }
            }
            self.out.incomplete = true;
            return None;
        }
        self.out.sessions.push(peer.clone());
        Some(peer)
    }

    fn links(&mut self, session: &str, kind: crate::session::links::LinkKind) -> Vec<String> {
        self.out.reads.links_listed += 1;
        match chain::peers(self.backend, session, kind) {
            Ok(found) => found.into_iter().map(|(peer, _)| peer).collect(),
            Err(()) => {
                self.unreadable(
                    session,
                    format!("its {} links can't be listed", kind.as_str()),
                );
                Vec::new()
            }
        }
    }

    /// Follow dispatched-by links upward from `start`, breadth first.
    fn up(&mut self, start: &str) {
        let mut queue = std::collections::VecDeque::from([start.to_string()]);
        while let Some(current) = queue.pop_front() {
            for peer in self.links(&current, crate::session::links::LinkKind::DispatchedBy) {
                if let Some(next) = self.visit(&peer) {
                    queue.push_back(next);
                }
            }
        }
    }
}

/// Read what is aimed at `session`'s decisions from every session on its
/// chain, itself first (PRD R22, R23). Writes nothing anywhere: link
/// directories and decision files are read straight from disk and, under
/// the cloud backend, each chain session's bucket decision objects are
/// listed fresh.
///
/// When `session`'s own areas hold nothing the read stops there and reads
/// no other session. A session with no directory and no bucket object ends
/// its branch of the chain (`incomplete`); one whose links or aimed area
/// can't be read, or whose aimed file fails its checks, is unreadable.
pub fn aimed_on_chain(backend: &Backend, session: &str, header_id: &str) -> ChainRead {
    if !holds_decisions(backend, session) {
        return ChainRead {
            skipped: true,
            ..ChainRead::default()
        };
    }
    let mut walk = StrictWalk {
        backend,
        seen: std::collections::BTreeSet::from([session.to_string()]),
        out: ChainRead {
            sessions: vec![session.to_string()],
            ..ChainRead::default()
        },
    };
    match aim_ids(backend, session, header_id) {
        Ok(ids) => walk.out.aim_ids = ids,
        Err(reason) => {
            walk.unreadable(session, reason);
            return walk.out;
        }
    }
    walk.up(session);
    for anchor in walk.links(session, crate::session::links::LinkKind::HeldBy) {
        if let Some(next) = walk.visit(&anchor) {
            walk.up(&next);
        }
    }

    let mut out = walk.out;
    let sessions = out.sessions.clone();
    let aims = out.aim_ids.clone();
    for holder in &sessions {
        let mut listing = store::Listing::default();
        let ctx = ctx_dir(backend, holder);
        for aim in &aims {
            out.reads.aimed_listed += 1;
            match aimed_local(&ctx, aim) {
                Ok(found) => listing.found.extend(found),
                Err(e) => listing.problem = Some(e),
            }
        }
        if let Backend::Cloud(cloud) = backend {
            out.reads.aimed_listed += 1;
            match cloud.decision_objects(holder) {
                Ok(objects) => {
                    let wanted: Vec<store::RemoteObject> = objects
                        .into_iter()
                        .filter(|(key, _)| {
                            aims.iter().any(|aim| {
                                key.starts_with(&format!("{}/{aim}/", super::AIMED_AREA))
                            })
                        })
                        .collect();
                    store::merge_remote(&mut listing, wanted);
                }
                Err(e) => {
                    listing.problem.get_or_insert_with(|| {
                        format!("the bucket's decisions can't be listed: {e:#}")
                    });
                }
            }
        }
        if let Some(problem) = listing.problem {
            out.unreadable.push(UnreadableSession {
                session: holder.clone(),
                reason: problem,
            });
            continue;
        }
        let holder_id = session_id(backend, holder, false);
        for f in listing.found {
            out.reads.files_read += 1;
            let checked = if is_decision_id(&f.name) {
                f.bytes.and_then(|bytes| {
                    check_body(holder, holder_id.as_deref(), &f.area, &f.name, &bytes)
                })
            } else {
                Err("the file name isn't a decision id".to_string())
            };
            match checked {
                Ok(body) => out.found.push((holder.clone(), body)),
                Err(reason) => out.unreadable.push(UnreadableSession {
                    session: holder.clone(),
                    reason: format!("{}: {reason}", sanitize(&f.name)),
                }),
            }
        }
    }
    out
}

/// The decision `id` in `session`'s own `koto/decisions/`, read without a
/// lock. `Ok(None)` when it isn't there; `Err` when it is there but can't
/// be read or fails its checks. Its session id may be any of `aim_ids`, so
/// a decision an import carried over still reads.
pub fn own_decision(
    backend: &Backend,
    session: &str,
    aim_ids: &[String],
    id: &str,
) -> Result<Option<DecisionBody>, String> {
    if !is_decision_id(id) {
        return Ok(None);
    }
    let path = ctx_dir(backend, session)
        .join("koto")
        .join("decisions")
        .join(id);
    if let Err(e) = std::fs::symlink_metadata(&path) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
    }
    let bytes = store::read_file(&path)?;
    let body = check_body(session, None, &Area::Decisions, id, &bytes)?;
    if !aim_ids.contains(&body.session.id) {
        return Err("the body's session id isn't the session's".to_string());
    }
    Ok(Some(body))
}
