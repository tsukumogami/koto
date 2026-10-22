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
}

impl LinkKind {
    /// Every kind koto knows.
    pub const ALL: [LinkKind; 1] = [LinkKind::DispatchedBy];

    /// The kind's key component, as it appears in `koto/links/<kind>/`.
    pub const fn as_str(self) -> &'static str {
        match self {
            LinkKind::DispatchedBy => "dispatched-by",
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkBody {
    pub v: u32,
    pub session: String,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
    pub at: String,
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
        at: crate::engine::types::now_iso8601(),
    };
    let content = serde_json::to_vec(&body)?;
    store.add_with_writer(session, &key, &content, WRITER_KOTO)?;
    append_to_session_best_effort(store, session, &added_event(&key, &content, WRITER_KOTO));
    Ok(LinkWrite::Written)
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
        assert_eq!(LinkKind::parse("held-by"), None);
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
    fn read_links_skips_unknown_kinds_and_unparseable_bodies() {
        let (_tmp, b) = backend();
        make_session(&b, "s", "sid", None);
        b.add_with_writer("s", "koto/links/held-by/anchor-1", b"{}", WRITER_KOTO)
            .unwrap();
        b.add_with_writer("s", "koto/links/dispatched-by/p", b"not json", WRITER_KOTO)
            .unwrap();
        let links = read_links(&b, "s").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].peer, "p");
        assert!(links[0].body.is_none());
    }
}
