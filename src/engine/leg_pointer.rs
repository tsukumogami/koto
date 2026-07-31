//! The child-side leg pointer: how a delegate learns which request leg
//! it was dispatched to fulfil (DESIGN-request-lifecycle.md Decision 3).
//!
//! # Why this is a sidecar and not a header field
//!
//! An earlier draft of the design had `bind` rewrite the child's
//! state-file header in place. That is unsafe here. The existing atomic
//! rewrite ([`crate::engine::claim::rewrite_header_atomically`]) reads
//! the whole file and then writes header-plus-tail to a temp and
//! renames, with no lock. Its current callers all run when the child is
//! not ticking — at claim time and during stale-sidecar recovery — but
//! binding a leg to a child that is *already running* is explicitly
//! allowed, and a non-batch session holds no lock during `koto next`.
//! Any event the child appended between that read and the rename would
//! be lost, including a `Transitioned`, which would silently regress the
//! delegate.
//!
//! So the pointer is its own temp-and-rename file in the child's session
//! directory, following the claim sidecar's precedent
//! ([`crate::engine::claim::sidecar_path`]), and is read alongside the
//! header rather than out of it. The bind path never rewrites the
//! child's own log.
//!
//! # What it deliberately does not carry
//!
//! The pointer carries identity — `request_id` and `leg_name` — and not
//! authority. The child's `dispatch_epoch` is captured on the
//! `request.leg_bound` event instead, because a freshly-readable epoch
//! would let a displaced agent present the current value and defeat the
//! fence. Identity is readable; authority stays baked into the
//! delegate's dispatch.
//!
//! # Known denormalization
//!
//! The pointer can lag or lose against the `request.leg_bound` event,
//! mirroring how the claim record relates to the claim sidecar. A lost
//! write leaves a correctly-bound leg whose delegate reads no leg —
//! degraded capability, not corruption, and repairable from the event.
//! The bind path treats the event as authoritative and this write as
//! best-effort-with-warning.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Filename for the leg pointer inside a session directory.
///
/// Sits beside `claim.lock` and the state file. Named for what it points
/// at rather than for the mechanism, so an operator listing a session
/// directory can tell the two sidecars apart.
pub const POINTER_FILENAME: &str = "request-leg.toml";

/// On-disk shape of the leg pointer.
///
/// TOML, one field per line, matching the claim sidecar. `bound_at` is
/// recorded for operators reading the directory by hand; nothing in the
/// protocol compares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegPointer {
    /// The request whose leg this session fulfils.
    pub request_id: String,
    /// The leg within that request.
    pub leg_name: String,
    /// RFC 3339 UTC timestamp of the bind that wrote this pointer.
    pub bound_at: String,
}

impl LegPointer {
    /// Whether `other` names a different request-and-leg pair.
    ///
    /// This is the comparison `bind` makes to enforce "a child fulfils at
    /// most one leg". The lock is per-request, so two binds in
    /// *different* requests targeting one child do not serialize at all
    /// — the check has to live on the child side.
    pub fn names_a_different_leg(&self, request_id: &str, leg_name: &str) -> bool {
        self.request_id != request_id || self.leg_name != leg_name
    }
}

/// Compute the pointer path for a session directory.
pub fn pointer_path(session_dir: &Path) -> PathBuf {
    session_dir.join(POINTER_FILENAME)
}

/// Read the pointer at `<session_dir>/request-leg.toml` using
/// `O_NOFOLLOW`.
///
/// Returns `Ok(None)` when the session carries no pointer, which is the
/// overwhelmingly common case — every session that is not a bound
/// delegate. A present-but-malformed pointer is an error rather than a
/// `None`, so a bind cannot silently overwrite one it failed to parse.
pub fn read_pointer(session_dir: &Path) -> Result<Option<LegPointer>> {
    let path = pointer_path(session_dir);
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Refuse the open when the final component is a symlink, so a
        // foothold attacker cannot point a delegate at a file outside
        // its own session directory.
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    let file = match opts.open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow!(
                "leg pointer open refused at {}: {}",
                path.display(),
                e
            ))
        }
    };

    let mut buf = String::new();
    let mut reader = std::io::BufReader::new(file);
    use std::io::Read;
    reader
        .read_to_string(&mut buf)
        .with_context(|| format!("failed to read leg pointer {}", path.display()))?;
    let parsed: LegPointer = toml::from_str(&buf)
        .with_context(|| format!("failed to parse leg pointer {}", path.display()))?;
    Ok(Some(parsed))
}

/// Read the pointer, treating every failure as "no pointer".
///
/// The read paths — `koto next`, `koto status`, the terminal tick's
/// promotion — all want a bound delegate's identity if it is legible and
/// want to carry on regardless if it is not. A malformed pointer warns
/// once here rather than failing a tick that has nothing to do with the
/// request store.
pub fn read_pointer_best_effort(session_dir: &Path) -> Option<LegPointer> {
    match read_pointer(session_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: ignoring an unreadable leg pointer: {e}");
            None
        }
    }
}

/// Write the pointer via temp-and-rename.
///
/// The rename is what makes a concurrent reader see either the old
/// pointer or the new one and never a half-written file. It also
/// replaces anything already at the path, including a planted symlink.
pub fn write_pointer(session_dir: &Path, pointer: &LegPointer) -> Result<()> {
    let path = pointer_path(session_dir);
    let body = toml::to_string(pointer).context("failed to serialize the leg pointer")?;
    let tmp = session_dir.join(format!(".{}.tmp.{}", POINTER_FILENAME, std::process::id()));

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("failed to open temp {}", tmp.display()))?;
        file.write_all(body.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    }

    fs::rename(&tmp, &path).with_context(|| {
        // Leave no temp behind when the rename is the thing that failed.
        let _ = fs::remove_file(&tmp);
        format!("failed to rename {} to {}", tmp.display(), path.display())
    })?;
    Ok(())
}

/// The pointer's modification time, if it has one.
///
/// Used as the baseline for the abandonment check's mtime
/// short-circuit: the pointer is written immediately after the bind
/// event, so a request log no younger than the pointer cannot have
/// gained an abandonment since (DESIGN-request-lifecycle.md Decision 4).
pub fn pointer_mtime(session_dir: &Path) -> Option<SystemTime> {
    fs::metadata(pointer_path(session_dir))
        .ok()
        .and_then(|md| md.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LegPointer {
        LegPointer {
            request_id: "req-abcdef".to_string(),
            leg_name: "reviewer-a".to_string(),
            bound_at: "2026-07-29T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn a_session_with_no_pointer_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), None);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write_pointer(dir.path(), &sample()).unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), Some(sample()));
    }

    #[test]
    fn a_second_write_replaces_the_first_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        write_pointer(dir.path(), &sample()).unwrap();
        let mut second = sample();
        second.leg_name = "reviewer-b".to_string();
        write_pointer(dir.path(), &second).unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), Some(second));

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != POINTER_FILENAME)
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp-and-rename must leave nothing behind, found {leftovers:?}"
        );
    }

    #[test]
    fn the_pointer_carries_no_dispatch_epoch() {
        let body = toml::to_string(&sample()).unwrap();
        assert!(
            !body.contains("dispatch_epoch") && !body.contains("epoch"),
            "a readable epoch would let a displaced agent present the current value \
             and defeat the fence: {body}"
        );
    }

    #[test]
    fn a_malformed_pointer_is_an_error_not_a_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(pointer_path(dir.path()), "this is not toml = = =").unwrap();
        assert!(read_pointer(dir.path()).is_err());
        assert_eq!(
            read_pointer_best_effort(dir.path()),
            None,
            "the best-effort read still degrades to no-pointer"
        );
    }

    #[test]
    fn a_different_request_or_leg_is_recognized_as_different() {
        let p = sample();
        assert!(!p.names_a_different_leg("req-abcdef", "reviewer-a"));
        assert!(p.names_a_different_leg("req-abcdef", "reviewer-b"));
        assert!(p.names_a_different_leg("req-999999", "reviewer-a"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_pointer_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.toml");
        std::fs::write(&target, toml::to_string(&sample()).unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, pointer_path(dir.path())).unwrap();
        assert!(
            read_pointer(dir.path()).is_err(),
            "O_NOFOLLOW must refuse a planted symlink"
        );
    }
}
