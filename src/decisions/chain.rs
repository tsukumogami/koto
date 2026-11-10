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

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::session::links::{LinkBody, LinkKind, LINKS_PREFIX, MAX_LINK_BODY_BYTES};
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
}

impl Walk<'_> {
    fn full(&self) -> bool {
        self.snapshot.links.len() >= MAX_CHAIN
    }

    /// List `peer`, reached by a link written `via`, unless it is already
    /// listed or missing. Returns whether it was listed.
    fn visit(&mut self, peer: &str, via: String) -> bool {
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
                if self.visit(&peer, dispatched_via(body.as_ref())) {
                    queue.push_back(peer);
                }
            }
        }
    }
}

/// The chain above `session`, as it stands now.
pub fn snapshot(store: &dyn SessionBackend, session: &str) -> ChainSnapshot {
    let mut walk = Walk {
        store,
        seen: BTreeSet::from([session.to_string()]),
        snapshot: ChainSnapshot::default(),
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
                if walk.visit(&anchor, via.to_string()) {
                    walk.up(&anchor);
                }
            }
        }
        Err(()) => walk.snapshot.incomplete = true,
    }
    walk.snapshot
}
