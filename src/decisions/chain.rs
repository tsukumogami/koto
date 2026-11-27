//! The chain a decision records (PRD R12, R13).
//!
//! The chain is the ordered sessions above a deciding session: first those
//! reached by following dispatched-by links upward from it, then its anchor
//! (reached by its held-by link) and those reached upward from the anchor,
//! skipping any session already listed. Each entry carries how koto wrote
//! the link that reached it. A session a link names that isn't in the store
//! marks the chain `incomplete`; it is never guessed past.
//!
//! The walk reads link files straight from each session's directory, with
//! no lock, no pull and no write, so it changes no session it passes
//! (R16). It reads no workspace-manager file (R13).
//!
//! [`read`] is the same walk with a forward step (R45): from each chain
//! session it follows the supersede marker ([`super::marker`]) to the
//! successor, confirms the successor's session id from its header, and
//! keeps going while each hop holds. `koto decisions chain` prints it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::marker::{self, Successor, SupersedeMarker};
use crate::engine::types::SupersedeActor;
use crate::session::links::{
    LinkBody, LinkKind, LINKS_PREFIX, MAX_LINK_BODY_BYTES, MAX_SUPERSEDE_HOPS,
};
use crate::session::SessionBackend;

/// The most sessions a walk lists before it stops and marks the chain
/// incomplete, so a hostile link chain can't run a command past its limit.
pub const MAX_CHAIN: usize = 64;

/// How a link was written, as R12 names it.
pub const VIA_HELD_BY: &str = "held-by";
pub const VIA_IMPORT: &str = "import";
/// A dispatched-by link written before koto recorded how.
pub const VIA_UNKNOWN: &str = "unknown";

/// One session on a chain, and how the link that reached it was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLink {
    pub session: String,
    pub via: String,
}

/// A chain as a decision records it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSnapshot {
    pub links: Vec<ChainLink>,
    pub incomplete: bool,
}

/// The peers of `session`'s `kind` links, sorted by name, with their bodies
/// when readable. `Err` when the links directory can't be listed, or isn't
/// a directory. A link file that is a symlink or not a regular file is
/// skipped as no link.
fn peers(
    store: &dyn SessionBackend,
    session: &str,
    kind: LinkKind,
) -> Result<Vec<(String, Option<LinkBody>)>, ()> {
    let dir = store
        .session_dir(session)
        .join("ctx")
        .join(LINKS_PREFIX.trim_end_matches('/'))
        .join(kind.as_str());
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(()),
        Ok(meta) if !meta.is_dir() => return Err(()),
        Ok(_) => {}
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".lock") {
            continue;
        }
        if crate::discover::validate_workflow_name(&name).is_err() {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let body = read_small(&path).and_then(|b| serde_json::from_slice::<LinkBody>(&b).ok());
        out.push((name, body));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn read_small(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_LINK_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_LINK_BODY_BYTES).then_some(bytes)
}

/// Whether `name` is a session in the local store: its state file is
/// there. Read without a lock or a pull.
pub fn session_present(store: &dyn SessionBackend, name: &str) -> bool {
    crate::discover::validate_workflow_name(name).is_ok()
        && store
            .session_dir(name)
            .join(crate::session::state_file_name(name))
            .is_file()
}

/// How a dispatched-by link was written: its recorded `via`, `import` for
/// a body an import rewrote, else [`VIA_UNKNOWN`].
fn dispatched_via(body: Option<&LinkBody>) -> String {
    const KNOWN: [&str; 4] = ["leg", "init", "link", "anchor"];
    match body {
        Some(b) if b.via.as_deref().is_some_and(|v| KNOWN.contains(&v)) => {
            b.via.clone().unwrap_or_default()
        }
        Some(b) if b.was.is_some() => VIA_IMPORT.to_string(),
        _ => VIA_UNKNOWN.to_string(),
    }
}

/// The session `session`'s nearest dispatched-by link names, if any.
pub fn dispatcher(store: &dyn SessionBackend, session: &str) -> Option<String> {
    peers(store, session, LinkKind::DispatchedBy)
        .ok()
        .and_then(|p| p.into_iter().next().map(|(name, _)| name))
}

struct Walk<'a> {
    store: &'a dyn SessionBackend,
    seen: BTreeSet<String>,
    snapshot: ChainSnapshot,
    /// For each listed session, the session whose link reached it.
    reached_from: BTreeMap<String, String>,
}

impl Walk<'_> {
    fn full(&self) -> bool {
        self.snapshot.links.len() >= MAX_CHAIN
    }

    /// List `peer`, reached by a link written `via`, unless it is already
    /// listed or missing. Returns whether it was listed.
    fn visit(&mut self, from: &str, peer: &str, via: String) -> bool {
        if self.seen.contains(peer) {
            return false;
        }
        if self.full() {
            self.snapshot.incomplete = true;
            return false;
        }
        if !session_present(self.store, peer) {
            self.snapshot.incomplete = true;
            return false;
        }
        self.seen.insert(peer.to_string());
        self.reached_from.insert(peer.to_string(), from.to_string());
        self.snapshot.links.push(ChainLink {
            session: peer.to_string(),
            via,
        });
        true
    }

    /// Follow dispatched-by links upward from `start`, breadth first, each
    /// session's links in name order.
    fn up(&mut self, start: &str) {
        let mut queue = VecDeque::from([start.to_string()]);
        while let Some(current) = queue.pop_front() {
            let Ok(found) = peers(self.store, &current, LinkKind::DispatchedBy) else {
                self.snapshot.incomplete = true;
                continue;
            };
            for (peer, body) in found {
                if self.visit(&current, &peer, dispatched_via(body.as_ref())) {
                    queue.push_back(peer);
                }
            }
        }
    }
}

/// The chain above `session`, as it stands now.
pub fn snapshot(store: &dyn SessionBackend, session: &str) -> ChainSnapshot {
    walk(store, session).snapshot
}

/// How a supersede hop is named in a standing path (PRD R41).
pub const VIA_SUPERSEDE: &str = "supersede";

/// One step of the path a reversal's standing came through: `to` is
/// reached from `session` by a link written `via`.
///
/// A supersede hop (`via` is [`VIA_SUPERSEDE`]) goes from the fenced
/// session to its successor, and also names the successor's session id
/// and the fence's actor and time, as the fenced session's marker records
/// them (R41). A link step carries none of the three, and a body written
/// before hops were recorded still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStep {
    pub session: String,
    pub via: String,
    pub to: String,
    /// The successor's session id, for a supersede hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_id: Option<String>,
    /// Who ran the command that fenced `session`, for a supersede hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<SupersedeActor>,
    /// When the fence was appended, for a supersede hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

impl PathStep {
    /// A step reached by a link written `via`.
    pub fn link(session: &str, via: &str, to: &str) -> Self {
        PathStep {
            session: session.to_string(),
            via: via.to_string(),
            to: to.to_string(),
            successor_id: None,
            actor: None,
            time: None,
        }
    }

    /// The supersede hop from `fenced` its marker records.
    pub fn hop(fenced: &str, marker: &SupersedeMarker) -> Self {
        PathStep {
            session: fenced.to_string(),
            via: VIA_SUPERSEDE.to_string(),
            to: marker.successor.name.clone(),
            successor_id: Some(marker.successor.id.clone()),
            actor: Some(marker.actor.clone()),
            time: Some(marker.time.clone()),
        }
    }

    /// Whether this step is a supersede hop.
    pub fn is_hop(&self) -> bool {
        self.via == VIA_SUPERSEDE
    }
}

/// The steps the chain walk from `from` takes to reach `to`, as the links
/// stand now, or `None` when the walk doesn't reach it.
pub fn path(store: &dyn SessionBackend, from: &str, to: &str) -> Option<Vec<PathStep>> {
    let walk = walk(store, from);
    let via_of = |s: &str| {
        walk.snapshot
            .links
            .iter()
            .find(|l| l.session == s)
            .map(|l| l.via.clone())
    };
    let mut steps = Vec::new();
    let mut current = to.to_string();
    while current != from {
        let parent = walk.reached_from.get(&current)?.clone();
        steps.push(PathStep::link(&parent, &via_of(&current)?, &current));
        current = parent;
        if steps.len() > MAX_CHAIN {
            return None;
        }
    }
    steps.reverse();
    Some(steps)
}

fn walk<'a>(store: &'a dyn SessionBackend, session: &str) -> Walk<'a> {
    let mut walk = Walk {
        store,
        seen: BTreeSet::from([session.to_string()]),
        snapshot: ChainSnapshot::default(),
        reached_from: BTreeMap::new(),
    };
    walk.up(session);
    match peers(store, session, LinkKind::HeldBy) {
        Ok(anchors) => {
            for (anchor, body) in anchors {
                let via = if body.as_ref().is_some_and(|b| b.was.is_some()) {
                    VIA_IMPORT
                } else {
                    VIA_HELD_BY
                };
                if walk.visit(session, &anchor, via.to_string()) {
                    walk.up(&anchor);
                }
            }
        }
        Err(()) => walk.snapshot.incomplete = true,
    }
    walk
}

// ===== The chain read: the walk with each supersede hop followed forward =====

/// One supersede hop followed forward from a session: the successor its
/// marker names, whether koto confirmed it, and the fence's actor and time
/// as the marker records them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    /// The successor the marker names.
    pub successor: Successor,
    /// Whether the successor is in the store and its header's session id
    /// is the one the marker names.
    pub confirmed: bool,
    /// Who ran the command that fenced the session.
    pub actor: SupersedeActor,
    /// When the fence was appended.
    pub time: String,
}

/// One session on a chain read: how the link that reached it was written,
/// and the supersede hops followed forward from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadLink {
    /// The chain session.
    pub session: String,
    /// How the link that reached it was written.
    pub via: String,
    /// The supersede hops from it, in order; empty when it has no marker.
    pub hops: Vec<Hop>,
    /// Why the forward step stopped at a marker it couldn't read, naming
    /// the session whose marker it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
}

/// A session's chain as koto would walk it now (PRD R45): each link and
/// its kind, each supersede hop and whether it was confirmed, the
/// session's own supersede marker, and whether the walk is incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainRead {
    /// The format version, [`crate::decisions::FORMAT_VERSION`].
    pub v: u32,
    /// The session read.
    pub session: String,
    /// The session's own supersede marker, or `None` (`null`) when it has
    /// none or it can't be read.
    pub marker: Option<SupersedeMarker>,
    /// Why the session's own marker can't be read, when it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker_unreadable: Option<String>,
    /// The chain sessions, in the walk's order.
    pub links: Vec<ReadLink>,
    /// Whether the walk, or a forward step, stopped short of what the
    /// links and markers name.
    pub incomplete: bool,
}

/// The session id in `name`'s header, read from the local state file with
/// no lock and no pull: the cloud backend's own header read pulls first,
/// and this read writes nothing.
fn header_session_id(store: &dyn SessionBackend, name: &str) -> Option<String> {
    let path = store
        .session_dir(name)
        .join(crate::session::state_file_name(name));
    crate::engine::persistence::read_header(&path)
        .ok()
        .map(|h| h.session_id)
}

/// The hops followed forward from `start` through supersede markers, why
/// the step stopped at a marker it couldn't read, and whether it stopped
/// short. A missing marker ends the step; an unconfirmed hop is recorded
/// and ends it; a successor already met on this step (or `start` itself),
/// or more than [`MAX_SUPERSEDE_HOPS`] hops, end it and mark it short.
fn forward(store: &dyn SessionBackend, start: &str) -> (Vec<Hop>, Option<String>, bool) {
    let mut hops = Vec::new();
    let mut seen = BTreeSet::from([start.to_string()]);
    let mut at = start.to_string();
    loop {
        let marker = match marker::read(&store.session_dir(&at).join("ctx")) {
            Ok(None) => return (hops, None, false),
            Ok(Some(m)) => m,
            Err(reason) => return (hops, Some(format!("'{at}': {reason}")), true),
        };
        if hops.len() >= MAX_SUPERSEDE_HOPS || seen.contains(&marker.successor.name) {
            return (hops, None, true);
        }
        let name = marker.successor.name.clone();
        let confirmed = session_present(store, &name)
            && header_session_id(store, &name).as_deref() == Some(marker.successor.id.as_str());
        hops.push(Hop {
            successor: marker.successor,
            confirmed,
            actor: marker.actor,
            time: marker.time,
        });
        if !confirmed {
            return (hops, None, false);
        }
        seen.insert(name.clone());
        at = name;
    }
}

/// Read `session`'s chain as koto would walk it now: the walk
/// [`snapshot`] takes, upward from the session and its anchors only, then
/// from each chain session the supersede hops its marker leads to. The
/// successors' own links aren't followed. Writes nothing, and fails
/// closed: a marker that can't be read marks the read incomplete and is
/// never guessed past.
pub fn read(store: &dyn SessionBackend, session: &str) -> ChainRead {
    let walk = walk(store, session);
    let mut incomplete = walk.snapshot.incomplete;
    let (marker, marker_unreadable) = match marker::read(&store.session_dir(session).join("ctx")) {
        Ok(m) => (m, None),
        Err(reason) => {
            incomplete = true;
            (None, Some(reason))
        }
    };
    let links = walk
        .snapshot
        .links
        .into_iter()
        .map(|link| {
            let (hops, unreadable, short) = forward(store, &link.session);
            incomplete |= short;
            ReadLink {
                session: link.session,
                via: link.via,
                hops,
                unreadable,
            }
        })
        .collect();
    ChainRead {
        v: crate::decisions::FORMAT_VERSION,
        session: session.to_string(),
        marker,
        marker_unreadable,
        links,
        incomplete,
    }
}

// ===== Supersede hops for standing and reads (R39, R40) =====

/// How the supersede hops followed forward from one session reach another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Reached with every hop confirmed: one [`PathStep::hop`] per hop, in
    /// order.
    Confirmed(Vec<PathStep>),
    /// Reached only through a hop koto can't confirm (R40): a marker
    /// naming a session id other than the successor's own, a fence with no
    /// marker or with one that can't be read, or more than
    /// [`MAX_SUPERSEDE_HOPS`] hops.
    Unconfirmed,
    /// The forward walk doesn't reach it.
    No,
}

/// Follow supersede hops forward from `from` and say how they reach `to`.
///
/// Each hop is confirmed as [`read`] confirms it: the marker names a
/// successor in the store whose header carries the marker's session id.
/// Where a session has no marker, or one that can't be read, its log's
/// fence still names the successor, so the walk goes on by that name, but
/// nothing past it is confirmed. So does a walk past
/// [`MAX_SUPERSEDE_HOPS`] hops, so a reach beyond the cap is refused as
/// unconfirmed rather than read as off the chain. A name that isn't a
/// session name, or one already met, ends the walk. Reads only; no lock,
/// no pull, no write.
pub fn hops_to(store: &dyn SessionBackend, from: &str, to: &str) -> Reach {
    let mut steps = Vec::new();
    let mut confirmed = true;
    let mut seen = BTreeSet::from([from.to_string()]);
    let mut at = from.to_string();
    for n in 0..2 * MAX_SUPERSEDE_HOPS {
        if n >= MAX_SUPERSEDE_HOPS {
            confirmed = false;
        }
        let next = match marker::read(&store.session_dir(&at).join("ctx")) {
            Ok(Some(m)) => {
                let name = m.successor.name.clone();
                let holds = session_present(store, &name)
                    && header_session_id(store, &name).as_deref() == Some(m.successor.id.as_str());
                confirmed &= holds;
                if confirmed {
                    steps.push(PathStep::hop(&at, &m));
                }
                name
            }
            Ok(None) | Err(_) => {
                let Some(fence) = crate::session::links::supersede_fence(store, &at) else {
                    return Reach::No;
                };
                confirmed = false;
                fence.by
            }
        };
        if crate::discover::validate_workflow_name(&next).is_err() || !seen.insert(next.clone()) {
            return Reach::No;
        }
        if next == to {
            return if confirmed {
                Reach::Confirmed(steps)
            } else {
                Reach::Unconfirmed
            };
        }
        at = next;
    }
    Reach::No
}

/// The earlier members of `session`'s run: the predecessors its continues
/// link names, then theirs, each kept only while the supersede hops from it
/// reach `session` with every hop confirmed. Read from the link files and
/// markers on disk, so it takes no lock, makes no network call and writes
/// nothing; a session with no continues link costs one directory stat.
pub fn run_predecessors(store: &dyn SessionBackend, session: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut at = session.to_string();
    for _ in 0..MAX_SUPERSEDE_HOPS {
        let Ok(peers) = peers(store, &at, LinkKind::Continues) else {
            break;
        };
        let Some((pred, _)) = peers.into_iter().next() else {
            break;
        };
        if pred == session || out.contains(&pred) {
            break;
        }
        if !matches!(hops_to(store, &pred, session), Reach::Confirmed(_)) {
            break;
        }
        out.push(pred.clone());
        at = pred;
    }
    out
}

/// The later members of `session`'s run: the successor its supersede
/// marker names, then that one's, each kept only while the hop is
/// confirmed (the successor is present under the id the marker records)
/// and the successor's continues link names the session it took over, so
/// a session lists `session` among its [`run_predecessors`] exactly when
/// this lists it. Reads only; no lock, no pull, no write.
pub fn run_successors(store: &dyn SessionBackend, session: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut at = session.to_string();
    for _ in 0..MAX_SUPERSEDE_HOPS {
        let Ok(Some(m)) = marker::read(&store.session_dir(&at).join("ctx")) else {
            break;
        };
        let next = m.successor.name.clone();
        if crate::discover::validate_workflow_name(&next).is_err()
            || next == session
            || out.contains(&next)
            || !session_present(store, &next)
            || header_session_id(store, &next).as_deref() != Some(m.successor.id.as_str())
        {
            break;
        }
        let continues = peers(store, &next, LinkKind::Continues)
            .ok()
            .and_then(|p| p.into_iter().next())
            .map(|(pred, _)| pred);
        if continues.as_deref() != Some(at.as_str()) {
            break;
        }
        out.push(next.clone());
        at = next;
    }
    out
}

/// The sessions among `sessions` whose confirmed supersede hops reach
/// `session`: the fenced predecessors it stands in for (R39). Only a
/// session with a readable marker can be one, so a session with none
/// costs one `stat`.
pub fn predecessors(store: &dyn SessionBackend, sessions: &[String], session: &str) -> Vec<String> {
    sessions
        .iter()
        .filter(|s| s.as_str() != session)
        .filter(|s| matches!(marker::read(&store.session_dir(s).join("ctx")), Ok(Some(_))))
        .filter(|s| matches!(hops_to(store, s, session), Reach::Confirmed(_)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::persistence::SupersedeFence;
    use crate::engine::types::StateFileHeader;
    use crate::session::local::LocalBackend;

    fn backend() -> (tempfile::TempDir, LocalBackend) {
        let tmp = tempfile::TempDir::new().unwrap();
        let b = LocalBackend::with_base_dir(tmp.path().join("sessions"));
        (tmp, b)
    }

    /// Session `name`, its header recording `id-<name>`.
    fn make_session(b: &LocalBackend, name: &str) {
        let mut header: StateFileHeader = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "workflow": name,
            "template_hash": "abc",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        header.session_id = format!("id-{name}");
        b.create(name).unwrap();
        let path = b
            .session_dir(name)
            .join(crate::session::state_file_name(name));
        crate::engine::persistence::append_header(&path, &header).unwrap();
    }

    /// Fence `session` for `by`, with no marker beside it: `replaced` as a
    /// replace writes it, else as a supersede stopped before its marker.
    fn fence(b: &LocalBackend, session: &str, by: &str, replaced: bool) {
        let path = b
            .session_dir(session)
            .join(crate::session::state_file_name(session));
        let fence = SupersedeFence {
            by: by.to_string(),
            by_session_id: format!("id-{by}"),
            actor: SupersedeActor::default(),
            at: "2026-01-01T00:00:00Z".to_string(),
            replaced,
        };
        crate::engine::persistence::append_supersede_fence(
            &path,
            &fence,
            "2026-01-01T00:00:00Z",
            crate::engine::persistence::FENCE_LOCK_WAIT,
        )
        .unwrap();
    }

    #[test]
    fn the_decision_chain_stops_at_a_replaced_fence() {
        // A plain fence with no marker reaches its successor, unconfirmed.
        let (_tmp, b) = backend();
        for name in ["a", "b"] {
            make_session(&b, name);
        }
        fence(&b, "a", "b", false);
        assert_eq!(hops_to(&b, "a", "b"), Reach::Unconfirmed);

        // A replaced fence reaches nothing: the run it names isn't a
        // successor, so a decision it makes isn't read as one's.
        let (_tmp, b) = backend();
        for name in ["a", "b"] {
            make_session(&b, name);
        }
        fence(&b, "a", "b", true);
        assert_eq!(hops_to(&b, "a", "b"), Reach::No);

        // Nor does a walk through one further on.
        let (_tmp, b) = backend();
        for name in ["a", "b", "c"] {
            make_session(&b, name);
        }
        fence(&b, "a", "b", false);
        fence(&b, "b", "c", true);
        assert_eq!(hops_to(&b, "a", "b"), Reach::Unconfirmed);
        assert_eq!(hops_to(&b, "a", "c"), Reach::No);
        assert_eq!(hops_to(&b, "b", "c"), Reach::No);
    }
}
