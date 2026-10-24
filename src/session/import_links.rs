//! What `koto session import` does to the reserved `koto/` area of the copy
//! it builds (DESIGN-session-retention.md, Decision 6).
//!
//! The bucket is untrusted for the reserved area. Before any key is fetched,
//! [`drop_from_manifest`] takes out what a copy must never carry: the
//! observed marker (`koto/anchor/observed`), a close record
//! (`koto/anchor-close`), every `koto/links/closes/*` link, any
//! `koto/imported-from` the source carried, and any key whose first
//! component spells `koto` in another case. Once the kept keys are staged
//! and checked against the manifest, [`Settlement::apply`] validates every
//! link body (a malformed one is dropped and reported on stderr), settles
//! the copy's `koto/links/held-by/*` links, and writes `koto/imported-from`,
//! updating the manifest for every key it changes.
//!
//! A held-by link is settled by the anchor it names and the importing
//! command's `KOTO_ANCHOR`:
//!
//! - kept when the name resolves in the local store to an anchor that is
//!   open and not migrated. The link's `workspace` hint (the anchor's
//!   `execution_dir`) never decides it;
//! - otherwise rewritten to `koto/links/held-by/<importer's anchor>`, with
//!   `"was"` holding the old name;
//! - with no importer anchor (unset, or naming no anchor here), dropped.
//!
//! A pin (`koto/links/pin`) and its release record (`koto/pin-release`)
//! are carried verbatim, an unreadable pin included: the pin isn't a link,
//! so it is never validated as one.
//!
//! Nothing here reads or writes the source: it works on the staging
//! directory and reads the local store's sessions in place.

use std::collections::BTreeSet;
use std::path::Path;

use crate::engine::types::SessionKind;
use crate::session::anchor::{CLOSE_RECORD_KEY, OBSERVED_KEY};
use crate::session::context::{KeyMeta, Manifest};
use crate::session::context_log::WRITER_KOTO;
use crate::session::hierarchy::{Graph, Judge, RetentionSettings};
use crate::session::links::{
    is_reserved_key, link_key, validate_peer, LinkBody, LinkHints, LinkKind, LINKS_PREFIX,
    MAX_LINK_BODY_BYTES, RESERVED_PREFIX,
};

pub use crate::session::hierarchy::IMPORTED_FROM_KEY;

/// Whether an import leaves `key` behind whatever the source holds: the
/// reserved keys that describe the source's own host or agent session, and
/// the record of an earlier import, which this import writes afresh.
pub fn dropped_on_import(key: &str) -> bool {
    key == OBSERVED_KEY
        || key == CLOSE_RECORD_KEY
        || key == IMPORTED_FROM_KEY
        || key.starts_with(&link_key(LinkKind::Closes, ""))
        // A reserved component in another case is never one koto wrote; on
        // a case-insensitive filesystem it would land in koto's own area.
        || (is_reserved_key(key) && !key.starts_with(RESERVED_PREFIX))
}

/// Take every key [`dropped_on_import`] names out of `manifest`. Returns
/// whether any was there.
pub fn drop_from_manifest(manifest: &mut Manifest) -> bool {
    let before = manifest.keys.len();
    manifest.keys.retain(|key, _| !dropped_on_import(key));
    manifest.keys.len() != before
}

/// The kind and peer of a link key, when it has the shape
/// `koto/links/<known kind>/<peer>` and the peer is a session name.
fn link_of(key: &str) -> Result<(LinkKind, &str), String> {
    let rest = key
        .strip_prefix(LINKS_PREFIX)
        .ok_or_else(|| "it is not a link key".to_string())?;
    let (kind, peer) = rest
        .split_once('/')
        .ok_or_else(|| "it names no peer".to_string())?;
    let kind = LinkKind::parse(kind).ok_or_else(|| format!("its kind {kind:?} is unknown"))?;
    validate_peer(kind, peer).map_err(|e| e.to_string())?;
    Ok((kind, peer))
}

/// Check one link key and its body: a known kind and a valid peer in the
/// key, a body of at most 4 KiB that parses as a link body, whose
/// `session` is the peer the key names. Returns the parsed body.
pub fn validate_link(key: &str, body: &[u8]) -> Result<LinkBody, String> {
    let (_, peer) = link_of(key)?;
    if body.len() > MAX_LINK_BODY_BYTES {
        return Err(format!(
            "its body is {} bytes, over the {MAX_LINK_BODY_BYTES}-byte limit",
            body.len()
        ));
    }
    let parsed: LinkBody =
        serde_json::from_slice(body).map_err(|e| format!("its body doesn't parse: {e}"))?;
    if parsed.v == 0 {
        return Err("its body has version 0".to_string());
    }
    if parsed.session != peer {
        return Err(format!(
            "its body names session {:?}, not the {:?} its key names",
            parsed.session, peer
        ));
    }
    Ok(parsed)
}

/// What [`Settlement::apply`] did, in the order it did it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Settled {
    /// Held-by links kept as they were, by the anchor they name.
    pub kept: Vec<String>,
    /// Held-by links rewritten to the importer's anchor: (old anchor, new).
    pub rewritten: Vec<(String, String)>,
    /// Held-by links dropped because nothing here could hold the copy.
    pub dropped: Vec<String>,
    /// Link keys dropped because their key or body was malformed, with why.
    pub malformed: Vec<(String, String)>,
}

/// The import that is settling a copy: where it settles anchors from, who
/// is importing, and what it copied.
#[derive(Debug, Clone, Copy)]
pub struct Settlement<'a> {
    /// The importing store's sessions directory, where held-by anchors are
    /// resolved by name.
    pub local_base: &'a Path,
    /// The importing command's `KOTO_ANCHOR`, when it is set.
    pub importer_anchor: Option<&'a str>,
    /// The copy's name.
    pub target: &'a str,
    /// The source session's name and header id, for `koto/imported-from`.
    pub from_session: &'a str,
    pub from_session_id: &'a str,
}

/// The local anchors among `names` that are open and not migrated, and
/// whether `importer` is an anchor here that is open and not migrated too. A store that can't be scanned
/// can't say which anchors are open, so every name counts as open and no
/// held-by link is changed.
fn resolve_anchors(
    base: &Path,
    names: &BTreeSet<String>,
    importer: Option<&str>,
) -> (BTreeSet<String>, bool) {
    let graph = match Graph::scan(base) {
        Ok(graph) => graph,
        Err(e) => {
            eprintln!(
                "warning: koto session import: the local sessions directory can't be read \
                 ({e:#}), so the copy's held-by links are kept as they are"
            );
            return (names.clone(), false);
        }
    };
    let is_anchor = |name: &str| {
        graph
            .facts(name)
            .is_some_and(|f| f.kind == SessionKind::Anchor)
    };
    let settings = RetentionSettings::load().unwrap_or_else(|e| {
        eprintln!(
            "warning: {e:#}; koto session import judges the held-by anchors with the default \
             settings"
        );
        let day = std::time::Duration::from_secs(86_400);
        RetentionSettings::from_durations(7 * day, 7 * day, 30 * day)
    });
    let mut judge = Judge::new(&graph, settings, std::time::SystemTime::now());
    let mut usable = |n: &str| is_anchor(n) && !graph.is_migrated(n) && judge.is_alive(n);
    let open = names.iter().filter(|n| usable(n)).cloned().collect();
    // The importer's own anchor passes the same test as a held-by anchor: a
    // closed or migrated anchor can't take over the copy's links.
    let importer_is_anchor = importer.is_some_and(&mut usable);
    (open, importer_is_anchor)
}

/// Write `content` as key `key` under `ctx_dir`, and record it in
/// `manifest` with writer `koto`.
fn write_key(
    ctx_dir: &Path,
    manifest: &mut Manifest,
    key: &str,
    content: &[u8],
) -> std::io::Result<()> {
    let path = ctx_dir.join(key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    manifest.keys.insert(
        key.to_string(),
        KeyMeta {
            created_at: crate::engine::types::now_iso8601(),
            size: content.len() as u64,
            hash: crate::cache::sha256_hex(content),
            writer: Some(WRITER_KOTO.to_string()),
        },
    );
    Ok(())
}

/// Remove key `key` from `ctx_dir` and `manifest`.
fn remove_key(ctx_dir: &Path, manifest: &mut Manifest, key: &str) -> std::io::Result<()> {
    manifest.keys.remove(key);
    match std::fs::remove_file(ctx_dir.join(key)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

impl Settlement<'_> {
    /// Settle the staged copy whose context is in `ctx_dir`, listed by
    /// `manifest`: validate every link body, settle the held-by links and
    /// write `koto/imported-from`, changing `manifest` to match. Each
    /// malformed link is reported on stderr. Fails only on a write to the
    /// staging directory.
    pub fn apply(&self, ctx_dir: &Path, manifest: &mut Manifest) -> std::io::Result<Settled> {
        let mut settled = Settled::default();

        // Every link body the copy keeps is validated. The pin isn't a link:
        // it travels verbatim, an unreadable one included, so the copy is
        // kept exactly as its source was.
        let link_keys: Vec<String> = manifest
            .keys
            .keys()
            .filter(|k| k.starts_with(LINKS_PREFIX) && !crate::session::pin::is_pin_key(k))
            .cloned()
            .collect();
        let mut held_by = BTreeSet::new();
        for key in link_keys {
            let body = std::fs::read(ctx_dir.join(&key)).unwrap_or_default();
            match validate_link(&key, &body) {
                Ok(link) => {
                    if key == link_key(LinkKind::HeldBy, &link.session) {
                        held_by.insert(link.session);
                    }
                }
                Err(why) => {
                    eprintln!(
                        "warning: koto session import: dropped context key {key:?} from the \
                         copy: {why}"
                    );
                    remove_key(ctx_dir, manifest, &key)?;
                    settled.malformed.push((key, why));
                }
            }
        }

        // The held-by links, by the anchor each names.
        let importer = self.importer_anchor.filter(|a| *a != self.target);
        let (open, importer_is_anchor) = if held_by.is_empty() {
            (BTreeSet::new(), false)
        } else {
            resolve_anchors(self.local_base, &held_by, importer)
        };
        let importer = importer.filter(|_| importer_is_anchor);
        for anchor in &held_by {
            if open.contains(anchor) || importer == Some(anchor.as_str()) {
                settled.kept.push(anchor.clone());
                continue;
            }
            remove_key(ctx_dir, manifest, &link_key(LinkKind::HeldBy, anchor))?;
            match importer {
                Some(to) => {
                    let key = link_key(LinkKind::HeldBy, to);
                    if !manifest.keys.contains_key(&key) {
                        let hints = LinkHints::for_peer(
                            &crate::session::local::LocalBackend::with_base_dir(
                                self.local_base.to_path_buf(),
                            ),
                            to,
                        );
                        let body = LinkBody {
                            v: crate::session::links::LINK_BODY_VERSION,
                            session: to.to_string(),
                            session_id: hints.session_id,
                            workspace: hints.workspace,
                            at: crate::engine::types::now_iso8601(),
                            was: Some(anchor.clone()),
                        };
                        let content =
                            serde_json::to_vec(&body).expect("LinkBody serialize is infallible");
                        write_key(ctx_dir, manifest, &key, &content)?;
                    }
                    settled.rewritten.push((anchor.clone(), to.to_string()));
                }
                None => settled.dropped.push(anchor.clone()),
            }
        }

        // Every import records what it copied, replacing anything the
        // source carried under the key.
        let record = serde_json::json!({
            "from_session": self.from_session,
            "from_session_id": self.from_session_id,
        });
        let content = serde_json::to_vec(&record).expect("JSON serialize is infallible");
        write_key(ctx_dir, manifest, IMPORTED_FROM_KEY, &content)?;
        Ok(settled)
    }
}

#[cfg(test)]
mod tests;
