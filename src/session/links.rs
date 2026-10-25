//! Links: koto-owned records in a session's own context naming another
//! session (DESIGN-session-retention.md, Decision 1).
//!
//! Every key under the reserved [`RESERVED_PREFIX`] is koto's. A link is one
//! key per peer, `koto/links/<kind>/<peer>`, whose value is one JSON object
//! ([`LinkBody`]). Links live in the session they describe and nowhere else:
//! [`write_link`] writes into the named session only, and reading a peer's
//! header for the hints never writes to the peer.
//!
//! `koto context add` and `koto context remove` refuse any key under the
//! prefix. The refusal is [`check_writer_key`], called from the context
//! store's own write path for the agent writer, so it holds for every caller
//! of the store and not only for the two CLI arms. koto's own writer, and a
//! cloud pull replaying keys the remote already holds, are not affected.
//!
//! A template's own writes (`context_assignments`, `clear_on_entry`) are
//! refused when the template is compiled rather than here: a refusal at the
//! store would land mid-transition, after the `transitioned` event that
//! records the assignment, and every later read would retry the write. The
//! writer-less `ContextStore::add` and `remove` are store primitives for
//! koto's own callers and tests; agent-facing writes go through the
//! `*_with_writer` forms.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::session::context::ContextStore;
use crate::session::context_log::{
    added_event, append_to_session_best_effort, WRITER_AGENT, WRITER_KOTO,
};
use crate::session::SessionBackend;

/// The reserved area of a session's context. Only koto writes keys here.
pub const RESERVED_PREFIX: &str = "koto/";

/// The first component of every reserved key, compared case-insensitively.
const RESERVED_COMPONENT: &str = "koto";

/// Where a session's links live: `koto/links/<kind>/<peer>`.
pub const LINKS_PREFIX: &str = "koto/links/";

/// The `v` a link body carries today.
pub const LINK_BODY_VERSION: u32 = 1;

/// Largest link body [`read_links`] parses. A larger one reads as a link
/// with no body.
pub const MAX_LINK_BODY_BYTES: usize = 4 * 1024;

/// What a link says about its peer. One key layout serves every kind, so a
/// kind added later needs a new variant and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// The peer dispatched the session holding the link.
    DispatchedBy,
    /// The peer is the anchor of an agent session that opened or ticked
    /// the session holding the link.
    HeldBy,
    /// The peer is the anchor the session holding the link closes; only a
    /// close session holds one.
    Closes,
    /// The session holding the link continues the peer: it is the peer's
    /// successor (DESIGN-koto-session-continues.md, Decision 2). Unlike the
    /// other kinds it joins no hierarchy: retention reads it as a directed
    /// overlay that keeps the peer while the holder, or anything later, has
    /// standing. A session holds at most one.
    Continues,
}

impl LinkKind {
    /// Every kind koto knows.
    pub const ALL: [LinkKind; 4] = [
        LinkKind::DispatchedBy,
        LinkKind::HeldBy,
        LinkKind::Closes,
        LinkKind::Continues,
    ];

    /// The kind's key component, as it appears in `koto/links/<kind>/`.
    pub const fn as_str(self) -> &'static str {
        match self {
            LinkKind::DispatchedBy => "dispatched-by",
            LinkKind::HeldBy => "held-by",
            LinkKind::Closes => "closes",
            LinkKind::Continues => "continues",
        }
    }

    /// The kind a key component names, or `None` for one koto doesn't know.
    pub fn parse(component: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == component)
    }
}

impl std::fmt::Display for LinkKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The context key of a `kind` link to `peer`.
pub fn link_key(kind: LinkKind, peer: &str) -> String {
    format!("{LINKS_PREFIX}{}/{peer}", kind.as_str())
}

/// Whether `key` lies in the reserved area: its first component is `koto`,
/// compared case-insensitively so `KOTO/links/...` can't reach the same
/// directory on a case-insensitive filesystem. The bare key `koto` counts
/// too, since a file of that name would block the reserved directory.
pub fn is_reserved_key(key: &str) -> bool {
    let first = key.split('/').next().unwrap_or("");
    first.eq_ignore_ascii_case(RESERVED_COMPONENT)
}

/// A write or removal of a reserved key by a writer that may not touch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedKeyError {
    pub key: String,
}

impl std::fmt::Display for ReservedKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "context key '{}' is under the reserved prefix '{}': only koto writes keys there, \
             so `koto context add` and `koto context remove` refuse them",
            self.key, RESERVED_PREFIX
        )
    }
}

impl std::error::Error for ReservedKeyError {}

/// Refuse a write or removal of a reserved key by the agent writer.
///
/// Called from the context store's write path (the local and cloud stores'
/// writes and [`ContextStore::remove_with_writer`]), so every caller of the
/// store meets it. Every other writer passes: koto's own, a transition's,
/// and a cloud pull's.
pub fn check_writer_key(key: &str, writer: &str) -> Result<(), ReservedKeyError> {
    if writer == WRITER_AGENT && is_reserved_key(key) {
        return Err(ReservedKeyError {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// A peer name a link can't carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPeerError {
    pub peer: String,
    pub reason: String,
}

impl std::fmt::Display for InvalidPeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let preview: String = self.peer.chars().take(64).collect();
        write!(
            f,
            "invalid peer session name '{}': {}",
            preview, self.reason
        )
    }
}

impl std::error::Error for InvalidPeerError {}

/// Check `peer` against the session-name rule and, as the last component of
/// its link key, the context-key grammar. `~`, which only internal epoch
/// branches use, fails both.
pub fn validate_peer(kind: LinkKind, peer: &str) -> Result<(), InvalidPeerError> {
    let invalid = |reason: String| InvalidPeerError {
        peer: peer.to_string(),
        reason,
    };
    crate::discover::validate_workflow_name(peer).map_err(invalid)?;
    crate::session::validate::validate_context_key(&link_key(kind, peer))
        .map_err(|e| invalid(e.to_string()))?;
    Ok(())
}

/// What a link records about its peer beside its name. Both are hints: a
/// reader goes by the name, and uses these only to tell a same-named peer
/// apart or to find it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkHints {
    /// The peer's session id, when the peer's session exists.
    pub session_id: Option<String>,
    /// The peer's workspace path (its execution directory), when known.
    pub workspace: Option<String>,
}

impl LinkHints {
    /// The hints for `peer` from its header in the local store, read
    /// straight from the file: no pull, no lock and no write, so the peer
    /// stays byte-identical. A peer that is absent or unreadable gives none,
    /// and so does a name that isn't a session name, which never reaches a
    /// path.
    pub fn for_peer(backend: &dyn SessionBackend, peer: &str) -> Self {
        if crate::discover::validate_workflow_name(peer).is_err() {
            return Self::default();
        }
        let path = backend
            .session_dir(peer)
            .join(crate::session::state_file_name(peer));
        Self::from_state_file(&path)
    }

    fn from_state_file(path: &Path) -> Self {
        let Ok(header) = crate::engine::persistence::read_header(path) else {
            return Self::default();
        };
        let session_id = Some(header.session_id).filter(|id| !id.is_empty());
        let workspace = header
            .execution_dir
            .or_else(|| header.origin.map(|o| o.anchor))
            .map(|p| p.to_string_lossy().into_owned());
        Self {
            session_id,
            workspace,
        }
    }
}

/// A link's value. `session_id` and `workspace` are `null` when unknown.
/// `was` is present only on a held-by link `koto session import` rewrote to
/// the importing agent session's anchor: the anchor it named before.
///
/// A continues link also carries `prefix`, the predecessor's bucket prefix
/// under the cloud backend (`null` otherwise), and `holder_session_id`, the
/// successor's own session id, which lets a reader of the bucket match the
/// relation without reading the successor's state. Both are absent from
/// every other kind's body, and `v` stays 1: an older reader ignores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkBody {
    pub v: u32,
    pub session: String,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
    /// The predecessor's bucket prefix: written on continues links only,
    /// and there always, `null` under the local backend.
    #[serde(default, skip_serializing_if = "Prefix::is_absent")]
    pub prefix: Prefix,
    /// The holder's own session id: continues links only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_session_id: Option<String>,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
}

/// A link body's `prefix`: absent from every kind but continues, `null`
/// when the predecessor has no bucket prefix (the local backend), or the
/// prefix's 16 lowercase hex digits. Kept apart from `Option` so a
/// continues body always writes the field and every other body never does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Prefix {
    #[default]
    Absent,
    Null,
    Value(String),
}

impl Prefix {
    fn is_absent(&self) -> bool {
        matches!(self, Prefix::Absent)
    }

    /// The prefix, when the body records one.
    pub fn value(&self) -> Option<&str> {
        match self {
            Prefix::Value(p) => Some(p),
            _ => None,
        }
    }
}

impl Serialize for Prefix {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Prefix::Value(p) => serializer.serialize_str(p),
            _ => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for Prefix {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(p) => Prefix::Value(p),
            None => Prefix::Null,
        })
    }
}

/// One link a session holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: LinkKind,
    /// The peer's name, from the key.
    pub peer: String,
    /// The parsed body, or `None` when it can't be read, is too large or is
    /// not a link body.
    pub body: Option<LinkBody>,
}

/// Whether [`write_link`] wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkWrite {
    /// The link was new and is now recorded.
    Written,
    /// The session already held this link; nothing changed.
    Unchanged,
}

impl LinkWrite {
    pub fn written(self) -> bool {
        self == LinkWrite::Written
    }
}

/// Record a `kind` link to `peer` in `session`, and only in `session`.
///
/// A peer that fails [`validate_peer`], or that is the session itself, is
/// refused with an [`InvalidPeerError`] before anything is read or written,
/// and a session that doesn't exist is refused too. A self-link says nothing
/// (a session is always in its own hierarchy) and would give every reader
/// that follows links an edge back to where it started. A link that is
/// already there is left alone, body included, so recording it again leaves
/// every file of the session byte-identical. Otherwise the key is written
/// with writer `koto`, and a `context_added` event naming that writer is
/// appended to the session's log, best-effort, as for koto's other own keys.
///
/// The presence check and the write aren't one atomic step: two writers
/// racing on the same new link both write it, the later body (and its `at`)
/// wins and the log gets two `context_added` events. Either body names the
/// same peer, so the link means the same thing.
pub fn write_link<S>(
    store: &S,
    session: &str,
    kind: LinkKind,
    peer: &str,
    hints: &LinkHints,
) -> anyhow::Result<LinkWrite>
where
    S: ContextStore + SessionBackend,
{
    validate_peer(kind, peer)?;
    if peer == session {
        return Err(InvalidPeerError {
            peer: peer.to_string(),
            reason: "a session can't link to itself".to_string(),
        }
        .into());
    }
    if !store.exists(session) {
        anyhow::bail!("session '{}' does not exist", session);
    }

    let key = link_key(kind, peer);
    if store.ctx_exists(session, &key) {
        return Ok(LinkWrite::Unchanged);
    }

    let body = LinkBody {
        v: LINK_BODY_VERSION,
        session: peer.to_string(),
        session_id: hints.session_id.clone(),
        workspace: hints.workspace.clone(),
        prefix: Prefix::Absent,
        holder_session_id: None,
        at: crate::engine::types::now_iso8601(),
        was: None,
    };
    let content = serde_json::to_vec(&body)?;
    store.add_with_writer(session, &key, &content, WRITER_KOTO)?;
    append_to_session_best_effort(store, session, &added_event(&key, &content, WRITER_KOTO));
    Ok(LinkWrite::Written)
}

/// Where a session's continues link lives: `koto/links/continues/`.
pub fn continues_prefix() -> String {
    format!("{LINKS_PREFIX}{}/", LinkKind::Continues.as_str())
}

/// The predecessors `session` names in a continues link, by the key's
/// peer, in key order. A session holds at most one; more is a store
/// written by hand. A key whose shape isn't `continues/<peer>` is skipped.
pub fn continues_of(store: &dyn ContextStore, session: &str) -> anyhow::Result<Vec<String>> {
    let prefix = continues_prefix();
    Ok(store
        .list_keys(session, Some(&prefix))?
        .into_iter()
        .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
        .filter(|peer| !peer.is_empty() && !peer.contains('/'))
        .collect())
}

/// What a continues link records beside the predecessor's name (R2), and
/// what [`write_continues`] checks a same-name predecessor against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuesHints {
    /// The predecessor's session id and workspace path.
    pub peer: LinkHints,
    /// The predecessor's bucket prefix; `None` under the local backend.
    pub prefix: Option<String>,
    /// The successor's own session id.
    pub holder_session_id: Option<String>,
    /// The successor's own bucket prefix, when it has one. A predecessor
    /// with the successor's own name is accepted only when `prefix` is set
    /// and differs from this.
    pub holder_prefix: Option<String>,
}

/// Why [`write_continues`] refused. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuesRefusal {
    /// The predecessor name fails the session-name rule or the key grammar,
    /// or is the successor's own name in its own workspace.
    InvalidPeer(InvalidPeerError),
    /// The successor already continues another session (R3, R4).
    OtherPredecessor { existing: String },
    /// The successor doesn't exist.
    NoSuchSession,
}

impl std::fmt::Display for ContinuesRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContinuesRefusal::InvalidPeer(e) => e.fmt(f),
            ContinuesRefusal::OtherPredecessor { existing } => write!(
                f,
                "the session already continues '{existing}', and a session continues at most \
                 one predecessor"
            ),
            ContinuesRefusal::NoSuchSession => f.write_str("the session does not exist"),
        }
    }
}

impl std::error::Error for ContinuesRefusal {}

/// Record in `session`, and only there, that it continues `pred`: the
/// continues link `koto/links/continues/<pred>` (Decision 2).
///
/// Refuses with a [`ContinuesRefusal`], writing nothing: a name
/// [`validate_peer`] refuses; the session's own name, unless `hints` give
/// the predecessor a bucket prefix that differs from the holder's (a
/// same-host import's copy naming its source in another workspace); a
/// session that doesn't exist; and a session that already continues
/// another predecessor. A relation the session already holds is left
/// alone, body included ([`LinkWrite::Unchanged`]). Otherwise the body
/// records `pred`'s name, session id, workspace and prefix and the holder's
/// own session id, written with writer `koto` and a `context_added` event,
/// as [`write_link`] writes. Nothing here reads an anchor, an authority or
/// a request leg (R1, R20).
pub fn write_continues<S>(
    store: &S,
    session: &str,
    pred: &str,
    hints: &ContinuesHints,
) -> anyhow::Result<LinkWrite>
where
    S: ContextStore + SessionBackend,
{
    let kind = LinkKind::Continues;
    validate_peer(kind, pred).map_err(ContinuesRefusal::InvalidPeer)?;
    let other_prefix = matches!(
        (&hints.prefix, &hints.holder_prefix),
        (Some(p), Some(h)) if p != h
    );
    if pred == session && !other_prefix {
        return Err(ContinuesRefusal::InvalidPeer(InvalidPeerError {
            peer: pred.to_string(),
            reason: "a session can't continue itself".to_string(),
        })
        .into());
    }
    if !store.exists(session) {
        return Err(ContinuesRefusal::NoSuchSession.into());
    }
    let held = continues_of(store, session)?;
    if held.iter().any(|p| p == pred) {
        return Ok(LinkWrite::Unchanged);
    }
    if let Some(existing) = held.into_iter().next() {
        return Err(ContinuesRefusal::OtherPredecessor { existing }.into());
    }

    let body = LinkBody {
        v: LINK_BODY_VERSION,
        session: pred.to_string(),
        session_id: hints.peer.session_id.clone(),
        workspace: hints.peer.workspace.clone(),
        prefix: match &hints.prefix {
            Some(p) => Prefix::Value(p.clone()),
            None => Prefix::Null,
        },
        holder_session_id: hints.holder_session_id.clone(),
        at: crate::engine::types::now_iso8601(),
        was: None,
    };
    let key = link_key(kind, pred);
    let content = serde_json::to_vec(&body)?;
    store.add_with_writer(session, &key, &content, WRITER_KOTO)?;
    append_to_session_best_effort(store, session, &added_event(&key, &content, WRITER_KOTO));
    Ok(LinkWrite::Written)
}

/// The standalone reserved records [`write_reserved_record`] and
/// [`remove_reserved_record`] may touch: a pin and its release record
/// (DESIGN-session-pin.md, Decision 3). Every other reserved key goes
/// through the context store.
fn is_standalone_record(key: &str) -> bool {
    key == crate::session::pin::PIN_KEY || key == crate::session::pin::RELEASE_KEY
}

/// Write `content` as reserved record `key` of local session `session`: the
/// file by temp file and rename, then its manifest entry with writer `koto`,
/// under the manifest lock. Unlike a context write it logs no event,
/// journals nothing and takes no per-key lock file, so the session's log
/// and every clock read from it stay as they were. Only the pin and the
/// release record are written this way, and only by
/// [`crate::session::pin`], which holds the session's lock around it.
///
/// With `indexed` false the record is a bare file instead: the copy a cloud
/// host keeps of an object the bucket holds, which the sync must not track
/// (R20a). No manifest entry is written, and one an earlier local-backend
/// write left is dropped; a manifest that lists no such key isn't touched.
pub(crate) fn write_reserved_record(
    store: &crate::session::local::LocalBackend,
    session: &str,
    key: &str,
    content: &[u8],
    indexed: bool,
) -> anyhow::Result<()> {
    use std::io::Write;
    anyhow::ensure!(
        is_standalone_record(key),
        "'{key}' isn't a standalone reserved record"
    );
    crate::session::validate::validate_context_key(key)?;
    let path = store.session_dir(session).join("ctx").join(key);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("'{key}' has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".koto-record-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| {
            anyhow::anyhow!("failed to create a temp file in {}: {e}", parent.display())
        })?;
    tmp.write_all(content)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| anyhow::anyhow!("failed to write {key}: {e}"))?;
    tmp.persist(&path)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {}", path.display(), e.error))?;
    if !indexed {
        let listed = store
            .read_manifest(session)
            .is_ok_and(|m| m.keys.contains_key(key));
        return if listed {
            store.update_manifest_locked(session, |m| {
                m.keys.remove(key);
            })
        } else {
            Ok(())
        };
    }
    let meta = crate::session::context::KeyMeta {
        created_at: crate::engine::types::now_iso8601(),
        size: content.len() as u64,
        hash: crate::cache::sha256_hex(content),
        writer: Some(WRITER_KOTO.to_string()),
    };
    store.update_manifest_locked(session, |m| {
        m.keys.insert(key.to_string(), meta);
    })
}

/// Remove reserved record `key` of local session `session`: its manifest
/// entry, then its file. Logs no event. A record already gone is not an
/// error. A directory at the record's path (an unreadable record) goes
/// whole, with every manifest entry under it. The counterpart of
/// [`write_reserved_record`], with the same limits.
pub(crate) fn remove_reserved_record(
    store: &crate::session::local::LocalBackend,
    session: &str,
    key: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        is_standalone_record(key),
        "'{key}' isn't a standalone reserved record"
    );
    crate::session::validate::validate_context_key(key)?;
    let under = format!("{key}/");
    let listed = |k: &String| k == key || k.starts_with(&under);
    // A manifest that lists none of it is left alone: a cloud host's bare
    // copy (R20a) has no entry to drop.
    if store
        .read_manifest(session)
        .map_or(true, |m| m.keys.keys().any(listed))
    {
        store.update_manifest_locked(session, |m| {
            m.keys.retain(|k, _| !listed(k));
        })?;
    }
    let path = store.session_dir(session).join("ctx").join(key);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
        Ok(_) => std::fs::remove_file(&path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
    .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", path.display()))
}

/// The links `session` holds, in key order. A key in the links area whose
/// kind koto doesn't know, or whose shape isn't `<kind>/<peer>`, is skipped.
///
/// This reads through the context store and trusts what it finds, which
/// suits showing a session's links. It is not the reader a retention
/// decision needs: that one must refuse symlinks, keep unknown kinds and
/// versions as edges, and treat anything unreadable as a reason to keep.
pub fn read_links(store: &dyn ContextStore, session: &str) -> anyhow::Result<Vec<Link>> {
    let mut links = Vec::new();
    for key in store.list_keys(session, Some(LINKS_PREFIX))? {
        let Some((kind, peer)) = key
            .strip_prefix(LINKS_PREFIX)
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let Some(kind) = LinkKind::parse(kind) else {
            continue;
        };
        if peer.is_empty() || peer.contains('/') {
            continue;
        }
        let body = store
            .get(session, &key)
            .ok()
            .filter(|bytes| bytes.len() <= MAX_LINK_BODY_BYTES)
            .and_then(|bytes| serde_json::from_slice::<LinkBody>(&bytes).ok());
        links.push(Link {
            kind,
            peer: peer.to_string(),
            body,
        });
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{EventPayload, StateFileHeader};
    use crate::session::context_log::WRITER_SYNC;
    use crate::session::local::LocalBackend;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn make_session(backend: &LocalBackend, name: &str, session_id: &str, exec: Option<&str>) {
        let mut header: StateFileHeader = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "workflow": name,
            "template_hash": "abc",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        header.session_id = session_id.to_string();
        header.execution_dir = exec.map(PathBuf::from);
        backend.create(name).unwrap();
        let path = backend
            .session_dir(name)
            .join(crate::session::state_file_name(name));
        crate::engine::persistence::append_header(&path, &header).unwrap();
    }

    /// Every file under `dir` with its bytes, by relative path.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let bytes = std::fs::read(&p).unwrap();
                    out.insert(p.strip_prefix(dir).unwrap().to_path_buf(), bytes);
                }
            }
        }
        out
    }

    fn backend() -> (tempfile::TempDir, LocalBackend) {
        let tmp = tempfile::TempDir::new().unwrap();
        let b = LocalBackend::with_base_dir(tmp.path().join("sessions"));
        (tmp, b)
    }

    #[test]
    fn key_layout_names_the_kind_and_the_peer() {
        assert_eq!(
            link_key(LinkKind::DispatchedBy, "deliver-a"),
            "koto/links/dispatched-by/deliver-a"
        );
        assert_eq!(
            LinkKind::parse("dispatched-by"),
            Some(LinkKind::DispatchedBy)
        );
        assert_eq!(LinkKind::parse("held-by"), Some(LinkKind::HeldBy));
        assert_eq!(LinkKind::parse("closes"), Some(LinkKind::Closes));
        assert_eq!(
            link_key(LinkKind::HeldBy, "anchor-1"),
            "koto/links/held-by/anchor-1"
        );
        assert_eq!(LinkKind::parse("read-by"), None);
    }

    #[test]
    fn reserved_keys_match_the_first_component_case_insensitively() {
        for key in ["koto/links/x", "KOTO/links/x", "Koto/a", "koto"] {
            assert!(is_reserved_key(key), "{key}");
        }
        for key in ["kotox/a", "notes/koto/a", "workflows/publish-location", "k"] {
            assert!(!is_reserved_key(key), "{key}");
        }
    }

    #[test]
    fn only_the_agent_writer_is_refused() {
        let err = check_writer_key("KOTO/links/dispatched-by/a", WRITER_AGENT).unwrap_err();
        assert!(err.to_string().contains("'koto/'"), "{err}");
        assert!(check_writer_key("koto/links/dispatched-by/a", WRITER_KOTO).is_ok());
        assert!(check_writer_key("koto/x", WRITER_SYNC).is_ok());
        assert!(check_writer_key("notes", WRITER_AGENT).is_ok());
    }

    #[test]
    fn peers_failing_either_grammar_are_refused() {
        for bad in [
            "",
            "a~1",
            "parent~1.task-a",
            "_x",
            "-x",
            ".x",
            "a/b",
            "a b",
            "..",
        ] {
            assert!(
                validate_peer(LinkKind::DispatchedBy, bad).is_err(),
                "{bad:?}"
            );
        }
        // Passes the session-name rule but makes a key over the length cap.
        let long = "a".repeat(240);
        assert!(validate_peer(LinkKind::DispatchedBy, &long).is_err());
        assert!(validate_peer(LinkKind::DispatchedBy, "deliver-a.1_b").is_ok());
    }

    #[test]
    fn the_local_store_refuses_agent_writes_and_removals_of_reserved_keys() {
        let (_tmp, b) = backend();
        make_session(&b, "s", "sid", None);
        let before = snapshot(&b.session_dir("s"));
        for key in [
            "koto/links/dispatched-by/a",
            "KOTO/links/dispatched-by/a",
            "koto",
        ] {
            let err = b.add_with_writer("s", key, b"x", WRITER_AGENT).unwrap_err();
            assert!(err.downcast_ref::<ReservedKeyError>().is_some(), "{err}");
            let err = b.remove_with_writer("s", key, WRITER_AGENT).unwrap_err();
            assert!(err.downcast_ref::<ReservedKeyError>().is_some(), "{err}");
        }
        assert_eq!(snapshot(&b.session_dir("s")), before);

        // koto's own writer is not affected, and the agent can't remove
        // what koto wrote.
        b.add_with_writer("s", "koto/links/dispatched-by/a", b"{}", WRITER_KOTO)
            .unwrap();
        assert!(b
            .remove_with_writer("s", "koto/links/dispatched-by/a", WRITER_AGENT)
            .is_err());
        assert!(b.ctx_exists("s", "koto/links/dispatched-by/a"));
    }

    #[test]
    fn write_link_records_the_body_and_a_context_added_event() {
        let (_tmp, b) = backend();
        make_session(&b, "child", "child-id", None);
        make_session(&b, "parent", "parent-id", Some("/work/space"));

        let hints = LinkHints::for_peer(&b, "parent");
        assert_eq!(
            hints,
            LinkHints {
                session_id: Some("parent-id".to_string()),
                workspace: Some("/work/space".to_string()),
            }
        );
        let parent_before = snapshot(&b.session_dir("parent"));
        let outcome = write_link(&b, "child", LinkKind::DispatchedBy, "parent", &hints).unwrap();
        assert_eq!(outcome, LinkWrite::Written);
        assert_eq!(snapshot(&b.session_dir("parent")), parent_before);

        let key = "koto/links/dispatched-by/parent";
        let raw: serde_json::Value = serde_json::from_slice(&b.get("child", key).unwrap()).unwrap();
        let mut fields: Vec<&String> = raw.as_object().unwrap().keys().collect();
        fields.sort();
        assert_eq!(fields, ["at", "session", "session_id", "v", "workspace"]);
        assert_eq!(raw["v"], 1);
        assert_eq!(raw["session"], "parent");
        assert_eq!(raw["session_id"], "parent-id");
        assert_eq!(raw["workspace"], "/work/space");
        assert!(raw["at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(
            b.meta("child", key).unwrap().writer.as_deref(),
            Some("koto")
        );

        let (_, events) = b.read_events("child").unwrap();
        assert!(events.iter().any(|e| matches!(
            &e.payload,
            EventPayload::ContextAdded { key: k, writer: Some(w), .. } if k == key && w == "koto"
        )));

        let links = read_links(&b, "child").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::DispatchedBy);
        assert_eq!(links[0].peer, "parent");
        assert_eq!(
            links[0].body.as_ref().unwrap().session_id.as_deref(),
            Some("parent-id")
        );
    }

    #[test]
    fn an_absent_peer_gets_null_hints() {
        let (_tmp, b) = backend();
        make_session(&b, "child", "child-id", None);
        let hints = LinkHints::for_peer(&b, "gone");
        assert_eq!(hints, LinkHints::default());
        write_link(&b, "child", LinkKind::DispatchedBy, "gone", &hints).unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&b.get("child", "koto/links/dispatched-by/gone").unwrap())
                .unwrap();
        assert!(raw["session_id"].is_null());
        assert!(raw["workspace"].is_null());
    }

    #[test]
    fn writing_an_existing_link_changes_nothing() {
        let (_tmp, b) = backend();
        make_session(&b, "child", "child-id", None);
        let first = write_link(
            &b,
            "child",
            LinkKind::DispatchedBy,
            "p",
            &LinkHints::default(),
        )
        .unwrap();
        assert_eq!(first, LinkWrite::Written);
        let before = snapshot(&b.session_dir("child"));
        let hints = LinkHints {
            session_id: Some("different".to_string()),
            workspace: None,
        };
        let again = write_link(&b, "child", LinkKind::DispatchedBy, "p", &hints).unwrap();
        assert_eq!(again, LinkWrite::Unchanged);
        assert_eq!(snapshot(&b.session_dir("child")), before);
    }

    #[test]
    fn invalid_peers_self_links_and_missing_sessions_write_nothing() {
        let (tmp, b) = backend();
        make_session(&b, "child", "child-id", None);
        let before = snapshot(tmp.path());
        for peer in ["a~1", "_x", "child"] {
            let err = write_link(
                &b,
                "child",
                LinkKind::DispatchedBy,
                peer,
                &LinkHints::default(),
            )
            .unwrap_err();
            assert!(
                err.downcast_ref::<InvalidPeerError>().is_some(),
                "{peer}: {err}"
            );
        }
        assert!(write_link(
            &b,
            "nope",
            LinkKind::DispatchedBy,
            "p",
            &LinkHints::default()
        )
        .is_err());
        assert_eq!(snapshot(tmp.path()), before);
    }

    #[test]
    fn write_continues_records_the_relation_once_and_refuses_a_second() {
        let (tmp, b) = backend();
        make_session(&b, "v1", "id-v1", Some("/work/one"));
        make_session(&b, "v2", "id-v2", None);
        make_session(&b, "other", "id-other", None);
        assert_eq!(LinkKind::parse("continues"), Some(LinkKind::Continues));
        let hints = ContinuesHints {
            peer: LinkHints::for_peer(&b, "v1"),
            prefix: None,
            holder_session_id: Some("id-v2".to_string()),
            holder_prefix: None,
        };
        let v1_before = snapshot(&b.session_dir("v1"));
        assert_eq!(
            write_continues(&b, "v2", "v1", &hints).unwrap(),
            LinkWrite::Written
        );
        assert_eq!(snapshot(&b.session_dir("v1")), v1_before);
        let key = "koto/links/continues/v1";
        let raw: serde_json::Value = serde_json::from_slice(&b.get("v2", key).unwrap()).unwrap();
        let mut fields: Vec<&String> = raw.as_object().unwrap().keys().collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                "at",
                "holder_session_id",
                "prefix",
                "session",
                "session_id",
                "v",
                "workspace"
            ]
        );
        assert_eq!(raw["session_id"], "id-v1");
        assert_eq!(raw["workspace"], "/work/one");
        assert!(raw["prefix"].is_null());
        assert_eq!(raw["holder_session_id"], "id-v2");
        assert_eq!(b.meta("v2", key).unwrap().writer.as_deref(), Some("koto"));
        assert_eq!(continues_of(&b, "v2").unwrap(), vec!["v1".to_string()]);
        // The body round-trips, and other kinds never carry the new fields.
        let body: LinkBody = serde_json::from_value(raw).unwrap();
        assert_eq!(body.prefix, Prefix::Null);
        let plain = serde_json::to_value(LinkBody {
            v: 1,
            session: "p".to_string(),
            session_id: None,
            workspace: None,
            prefix: Prefix::Absent,
            holder_session_id: None,
            at: "x".to_string(),
            was: None,
        })
        .unwrap();
        assert!(plain.get("prefix").is_none() && plain.get("holder_session_id").is_none());

        // Again: nothing changes. Another predecessor, the session itself
        // and a session that doesn't exist: refused, nothing written.
        let before = snapshot(tmp.path());
        assert_eq!(
            write_continues(&b, "v2", "v1", &hints).unwrap(),
            LinkWrite::Unchanged
        );
        for (session, pred) in [("v2", "other"), ("v2", "v2"), ("nope", "v1"), ("v2", "a~1")] {
            let err = write_continues(&b, session, pred, &hints).unwrap_err();
            assert!(
                err.downcast_ref::<ContinuesRefusal>().is_some(),
                "{session} {pred}: {err}"
            );
        }
        assert_eq!(snapshot(tmp.path()), before);
    }

    #[test]
    fn a_same_name_predecessor_needs_another_prefix() {
        let (_tmp, b) = backend();
        make_session(&b, "s", "id-s", None);
        let mut hints = ContinuesHints {
            prefix: Some("0123456789abcdef".to_string()),
            holder_prefix: Some("0123456789abcdef".to_string()),
            ..Default::default()
        };
        assert!(write_continues(&b, "s", "s", &hints).is_err());
        hints.holder_prefix = Some("fedcba9876543210".to_string());
        assert_eq!(
            write_continues(&b, "s", "s", &hints).unwrap(),
            LinkWrite::Written
        );
        let raw: serde_json::Value =
            serde_json::from_slice(&b.get("s", "koto/links/continues/s").unwrap()).unwrap();
        assert_eq!(raw["prefix"], "0123456789abcdef");
    }

    #[test]
    fn read_links_skips_unknown_kinds_and_unparseable_bodies() {
        let (_tmp, b) = backend();
        make_session(&b, "s", "sid", None);
        b.add_with_writer("s", "koto/links/read-by/anchor-1", b"{}", WRITER_KOTO)
            .unwrap();
        b.add_with_writer("s", "koto/links/dispatched-by/p", b"not json", WRITER_KOTO)
            .unwrap();
        let links = read_links(&b, "s").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].peer, "p");
        assert!(links[0].body.is_none());
    }
}
