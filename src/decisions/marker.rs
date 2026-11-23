//! The supersede marker (DESIGN-decision-record.md, "A write-once
//! supersede marker beside the fenced session").
//!
//! `koto init --supersede` writes `koto/superseded-by` in the predecessor
//! once its fence is in place and before the successor is created. It names
//! the successor by name and session id, carries the fence's actor and
//! time, and records `decisions_through`, the highest decision id the fence
//! saw in the predecessor, or `null` when it held none.
//!
//! The marker sits outside the context manifest, as the decision files do,
//! so one `stat` and one read find it under either backend. It is written
//! create-only, by the same create-rename the decision store publishes with
//! locally and with `If-None-Match: *` in the bucket, and no context-store
//! writer may change or remove it
//! ([`crate::session::links::check_decision_key`]).
//!
//! A reader trusts nothing it reads here: a marker that is a symlink, isn't
//! a regular file, is larger than [`MAX_MARKER_BYTES`] or doesn't parse to
//! a version-1 body with a valid successor makes the session unreadable,
//! and is never guessed past.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{Area, Listing};
use super::{is_decision_id, FORMAT_VERSION};
use crate::engine::persistence::SupersedeFence;
use crate::engine::types::SupersedeActor;

/// The context key of the supersede marker, in the fenced session.
pub const SUPERSEDED_BY_KEY: &str = "koto/superseded-by";

/// The largest marker a reader accepts. A real one is a few hundred bytes;
/// the cap keeps a hostile file from costing a reader more than that.
pub const MAX_MARKER_BYTES: usize = 4 * 1024;

/// The successor a marker names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Successor {
    /// The successor's session name.
    pub name: String,
    /// The successor's session id, as its header records it.
    pub id: String,
}

/// The body of `koto/superseded-by`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersedeMarker {
    /// The format version, [`FORMAT_VERSION`].
    pub v: u32,
    /// The session that took the fenced session's work over.
    pub successor: Successor,
    /// Who ran the command that fenced the session, as the fence records it.
    pub actor: SupersedeActor,
    /// When the fence was appended, as the fence records it.
    pub time: String,
    /// The highest decision id the fence saw in the fenced session, or
    /// `None` (`null`) when the session held no decision.
    pub decisions_through: Option<String>,
}

impl SupersedeMarker {
    /// The marker for `fence`, with the highest decision id it saw.
    pub fn new(fence: &SupersedeFence, decisions_through: Option<String>) -> Self {
        SupersedeMarker {
            v: FORMAT_VERSION,
            successor: Successor {
                name: fence.by.clone(),
                id: fence.by_session_id.clone(),
            },
            actor: fence.actor.clone(),
            time: fence.at.clone(),
            decisions_through,
        }
    }

    /// The bytes the marker is written as.
    pub fn to_bytes(&self) -> Vec<u8> {
        // A struct of strings and options always serializes.
        serde_json::to_vec(self).expect("a supersede marker serializes")
    }

    /// Parse and check a marker body: at most [`MAX_MARKER_BYTES`], version
    /// 1, no field it doesn't know, a successor name koto accepts, a
    /// session id that can name a directory, and a `decisions_through`
    /// that is `null` or a decision id.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MARKER_BYTES {
            return Err(format!("is larger than {MAX_MARKER_BYTES} bytes"));
        }
        let marker: SupersedeMarker =
            serde_json::from_slice(bytes).map_err(|e| format!("doesn't parse: {e}"))?;
        marker.validate()?;
        Ok(marker)
    }

    fn validate(&self) -> Result<(), String> {
        if self.v != FORMAT_VERSION {
            return Err(format!("has version {}, not {FORMAT_VERSION}", self.v));
        }
        crate::discover::validate_workflow_name(&self.successor.name)
            .map_err(|e| format!("names an invalid successor: {e}"))?;
        if !super::store::is_target_id(&self.successor.id) {
            return Err("names an invalid successor session id".to_string());
        }
        if self.time.is_empty() {
            return Err("records no time".to_string());
        }
        if let Some(through) = &self.decisions_through {
            if !is_decision_id(through) {
                return Err(format!("'{through}' isn't a decision id"));
            }
        }
        Ok(())
    }
}

/// Whether `key` is the marker key, or lies under it. Both components are
/// compared case-insensitively, as [`super::is_decision_area_key`] compares
/// them, so `KOTO/Superseded-By` can't reach the same file on a
/// case-insensitive filesystem, and a key under it can't put a directory
/// where the marker goes.
pub fn is_marker_key(key: &str) -> bool {
    let mut parts = key.split('/');
    let first = parts.next().unwrap_or("");
    let second = parts.next().unwrap_or("");
    first.eq_ignore_ascii_case("koto") && second.eq_ignore_ascii_case("superseded-by")
}

/// Read the marker in the session whose context directory is `ctx_dir`,
/// with no lock and no write. `Ok(None)` when there is none. A marker that
/// is a symlink (or sits under a symlinked `ctx/koto`), isn't a regular
/// file, is too large or doesn't parse and check is `Err` with the reason:
/// the session then reads as unreadable.
pub fn read(ctx_dir: &Path) -> Result<Option<SupersedeMarker>, String> {
    let path = ctx_dir.join(SUPERSEDED_BY_KEY);
    let meta = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("its supersede marker can't be read: {e}")),
        Ok(meta) => meta,
    };
    if meta.file_type().is_symlink() {
        return Err("its supersede marker is a symlink".to_string());
    }
    if !meta.is_file() {
        return Err("its supersede marker isn't a regular file".to_string());
    }
    // The lookup above follows a symlink at `ctx/koto`; a marker reached
    // through one isn't the session's own.
    if std::fs::symlink_metadata(ctx_dir.join("koto")).map_or(true, |m| !m.is_dir()) {
        return Err("its ctx/koto directory is a symlink or not a directory".to_string());
    }
    if meta.len() > MAX_MARKER_BYTES as u64 {
        return Err(format!(
            "its supersede marker is larger than {MAX_MARKER_BYTES} bytes"
        ));
    }
    let bytes = read_bounded(&path).map_err(|e| format!("its supersede marker {e}"))?;
    SupersedeMarker::parse(&bytes)
        .map(Some)
        .map_err(|e| format!("its supersede marker {e}"))
}

/// Read at most one byte past [`MAX_MARKER_BYTES`] from `path`, refusing a
/// symlink swapped in since the `stat` and anything but a regular file on
/// the open descriptor.
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("can't be opened as a regular file: {e}"))?;
    if !file
        .metadata()
        .map_err(|e| format!("can't be read: {e}"))?
        .is_file()
    {
        return Err("isn't a regular file".to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MARKER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("can't be read: {e}"))?;
    if bytes.len() > MAX_MARKER_BYTES {
        return Err(format!("is larger than {MAX_MARKER_BYTES} bytes"));
    }
    Ok(bytes)
}

/// The highest decision id in `listing`, across both areas, or `None` when
/// it holds none. A listing with a problem is `Err`: the fence then can't
/// say which decisions came before it, and writing a number it can't vouch
/// for would let a later reader take a decision for one the fence saw.
pub fn decisions_through(listing: &Listing) -> Result<Option<String>, String> {
    if let Some(problem) = &listing.problem {
        return Err(problem.clone());
    }
    Ok(listing.ids().into_iter().max())
}

/// The highest decision id among bucket context keys (each relative to the
/// session's `ctx/`), or `None` when none of them is a decision. A key in
/// a decision area whose name isn't a decision id is skipped, as the
/// allocator skips it.
pub fn decisions_through_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    keys.into_iter()
        .filter_map(Area::of_key)
        .map(|(_, name)| name)
        .filter(|name| is_decision_id(name))
        .max()
}

/// Publish `marker` in the local session whose directory is `session_dir`,
/// create-only. `Ok(true)` when this call wrote it, `Ok(false)` when
/// something is already at the marker's path, which is left as it is.
pub(crate) fn write_local(
    session_dir: &Path,
    ctx_dir: &Path,
    marker: &SupersedeMarker,
) -> anyhow::Result<bool> {
    super::store::publish_create_only(session_dir, ctx_dir, SUPERSEDED_BY_KEY, &marker.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence() -> SupersedeFence {
        SupersedeFence {
            by: "succ".to_string(),
            by_session_id: "1a304ea0-f1ad-4779-acef-105b4dface95".to_string(),
            actor: SupersedeActor {
                claude_session: None,
                host: Some("h".to_string()),
                user: Some("u".to_string()),
            },
            at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn session_ctx() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = dir.path().join("ctx");
        std::fs::create_dir_all(ctx.join("koto")).unwrap();
        (dir, ctx)
    }

    #[test]
    fn a_missing_marker_reads_as_none() {
        let (_dir, ctx) = session_ctx();
        assert_eq!(read(&ctx), Ok(None));
        // No ctx/koto at all is no marker either.
        let empty = tempfile::TempDir::new().unwrap();
        assert_eq!(read(&empty.path().join("ctx")), Ok(None));
    }

    #[test]
    fn a_written_marker_reads_back_and_is_never_replaced() {
        let (dir, ctx) = session_ctx();
        let marker = SupersedeMarker::new(&fence(), Some("000000000042".to_string()));
        assert!(write_local(dir.path(), &ctx, &marker).unwrap());
        assert_eq!(read(&ctx), Ok(Some(marker.clone())));
        let bytes = std::fs::read(ctx.join(SUPERSEDED_BY_KEY)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["successor"]["name"], "succ");
        assert_eq!(value["time"], "2026-01-01T00:00:00Z");
        assert_eq!(value["decisions_through"], "000000000042");

        let other = SupersedeMarker::new(&fence(), None);
        assert!(!write_local(dir.path(), &ctx, &other).unwrap());
        assert_eq!(std::fs::read(ctx.join(SUPERSEDED_BY_KEY)).unwrap(), bytes);
    }

    #[test]
    fn no_decisions_is_null() {
        let marker = SupersedeMarker::new(&fence(), None);
        let value: serde_json::Value = serde_json::from_slice(&marker.to_bytes()).unwrap();
        assert!(value["decisions_through"].is_null());
        assert!(value.as_object().unwrap().contains_key("decisions_through"));
    }

    #[test]
    fn a_malformed_marker_is_unreadable() {
        let (_dir, ctx) = session_ctx();
        let path = ctx.join(SUPERSEDED_BY_KEY);
        for body in [
            &b"not json"[..],
            br#"{"v":1,"successor":{"name":"../x","id":"a"},"actor":{},"time":"t","decisions_through":null}"#,
            br#"{"v":1,"successor":{"name":"s","id":""},"actor":{},"time":"t","decisions_through":null}"#,
            br#"{"v":1,"successor":{"name":"s","id":"a"},"actor":{},"time":"t","decisions_through":"42"}"#,
            br#"{"v":1,"successor":{"name":"s","id":"a"},"actor":{},"time":"t","decisions_through":null,"x":1}"#,
        ] {
            std::fs::write(&path, body).unwrap();
            assert!(read(&ctx).is_err(), "{}", String::from_utf8_lossy(body));
        }
        std::fs::write(&path, vec![b' '; MAX_MARKER_BYTES + 1]).unwrap();
        assert!(read(&ctx).unwrap_err().contains("larger"));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read(&ctx).is_err());
    }

    #[test]
    fn an_unknown_version_is_unreadable() {
        let (_dir, ctx) = session_ctx();
        std::fs::write(
            ctx.join(SUPERSEDED_BY_KEY),
            br#"{"v":2,"successor":{"name":"s","id":"a"},"actor":{},"time":"t","decisions_through":null}"#,
        )
        .unwrap();
        assert!(read(&ctx).unwrap_err().contains("version 2"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_marker_is_unreadable() {
        let (dir, ctx) = session_ctx();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, SupersedeMarker::new(&fence(), None).to_bytes()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ctx.join(SUPERSEDED_BY_KEY)).unwrap();
        assert!(read(&ctx).unwrap_err().contains("symlink"));

        // A marker reached through a symlinked ctx/koto isn't the session's.
        let (other, ctx2) = session_ctx();
        std::fs::remove_dir(ctx2.join("koto")).unwrap();
        let real = other.path().join("real-koto");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(
            real.join("superseded-by"),
            SupersedeMarker::new(&fence(), None).to_bytes(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&real, ctx2.join("koto")).unwrap();
        assert!(read(&ctx2).is_err());
    }

    #[test]
    fn marker_keys_are_recognised_whatever_their_case() {
        assert!(is_marker_key("koto/superseded-by"));
        assert!(is_marker_key("KOTO/Superseded-By"));
        assert!(is_marker_key("koto/superseded-by/x"));
        assert!(!is_marker_key("koto/superseded-carry-complete"));
        assert!(!is_marker_key("superseded-by"));
    }

    #[test]
    fn the_highest_id_spans_both_areas_and_a_problem_refuses() {
        let keys = [
            "koto/decisions/000000000002",
            "koto/aimed/abc/000000000007",
            "koto/decisions/not-an-id",
            "manifest.json",
        ];
        assert_eq!(
            decisions_through_keys(keys),
            Some("000000000007".to_string())
        );
        assert_eq!(decisions_through_keys(["manifest.json"]), None);
        let listing = Listing {
            found: Vec::new(),
            problem: Some("bad".to_string()),
        };
        assert!(decisions_through(&listing).is_err());
        assert_eq!(decisions_through(&Listing::default()), Ok(None));
    }
}
