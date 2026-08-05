//! Shared check for whether a session's recorded `template_source_dir`
//! still resolves, and on whose machine.
//!
//! `StateFileHeader::template_source_dir` (the directory a session's
//! source template was loaded from at `koto init` time) has exactly one
//! consumer today: the batch scheduler's path resolver
//! (`path_resolution.rs`), which checks `Path::exists()` inline and
//! reports a [`crate::engine::scheduler_warning::SchedulerWarning::StaleTemplateSourceDir`]
//! when the directory is gone -- typically because the working tree it
//! lived in (a reaped ephemeral sandbox, a removed git worktree, a
//! container teardown) no longer exists.
//!
//! DESIGN-orphaned-session-detection.md extends that same check to three
//! more call sites -- `koto status`, `koto init`'s "already exists"
//! collision path, and `koto session list` -- and extracts it into this
//! module so there is exactly one place in the codebase that answers
//! "does this recorded directory exist, and on whose machine":
//!
//! - [`check_template_source_path`] is the core function, used directly
//!   by the batch scheduler's per-tick probe (`batch.rs`), which only
//!   ever has a raw `Option<&Path>` in scope, not a header.
//! - [`check_template_source_dir`] is a thin wrapper over the core,
//!   used by the three new call sites, which all have a
//!   [`StateFileHeader`] in hand.
//! - [`format_stale_template_source_note`] is the shared wording helper
//!   for surfacing a stale result to a human, softened for
//!   cloud-synced sessions where a missing directory may simply mean
//!   "resumed on another machine" rather than "deleted."
//!
//! This module is purely additive: it introduces the shared core but
//! does not wire it into any existing call site. Later issues in the
//! same plan route the scheduler's existing construction site and the
//! three new consumers through it.

use std::path::{Path, PathBuf};

use crate::engine::path_resolution::current_machine_id;
use crate::engine::types::StateFileHeader;

/// Result of checking whether a recorded `template_source_dir` still
/// resolves on the current machine.
///
/// `machine_id` is a best-effort, non-authoritative label (see
/// [`current_machine_id`]) identifying which machine performed the
/// check -- it is not compared against any stored "creator machine"
/// value, since none exists in [`StateFileHeader`] today.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TemplateSourceStatus {
    /// The recorded `template_source_dir` path, unchanged from the
    /// header (or from whatever `Option<&Path>` the caller supplied).
    pub path: PathBuf,

    /// Whether `path` exists on this machine, per `Path::exists()`.
    ///
    /// This is a bare existence check, not an "is this a directory"
    /// check: a `template_source_dir` that resolves to a regular file
    /// still reports `true` here. Stronger validation is out of scope
    /// for this read-only, informational signal -- see
    /// DESIGN-orphaned-session-detection.md's Decisions Already Made.
    pub exists: bool,

    /// Best-effort identifier for the machine the check ran on. `None`
    /// when no usable identifier could be derived. See
    /// [`current_machine_id`] for how this is computed.
    pub machine_id: Option<String>,
}

/// Core check: does `path` exist, and on whose machine.
///
/// Returns `None` when `path` is `None` -- there is nothing to check.
/// Otherwise runs `Path::exists()` once and attaches the current
/// machine's best-effort identifier.
///
/// Used directly by callers that hold a raw `Option<&Path>` rather than
/// a full [`StateFileHeader`] -- today, the batch scheduler's per-tick
/// probe in `batch.rs`. Callers with a header in hand should use
/// [`check_template_source_dir`] instead.
pub fn check_template_source_path(path: Option<&Path>) -> Option<TemplateSourceStatus> {
    let path = path?.to_path_buf();
    let exists = path.exists();
    Some(TemplateSourceStatus {
        path,
        exists,
        machine_id: current_machine_id(),
    })
}

/// Header-accepting wrapper over [`check_template_source_path`].
///
/// Extracts `header.template_source_dir` and delegates to the core
/// function. Used by the three call sites that already have a
/// [`StateFileHeader`] in scope: `koto status`, `koto init`'s collision
/// path, and `koto session list`.
pub fn check_template_source_dir(header: &StateFileHeader) -> Option<TemplateSourceStatus> {
    check_template_source_path(header.template_source_dir.as_deref())
}

/// Human-readable note describing a stale `template_source_dir`,
/// worded differently depending on whether the session's backend
/// supports cross-machine cloud sync.
///
/// `CloudBackend` sessions can legitimately be resumed on a machine
/// other than the one that created them (`docs/guides/cloud-sync-setup.md`),
/// so a missing `template_source_dir` there may simply reflect that --
/// not deletion. The wording softens accordingly rather than asserting
/// deletion outright. `LocalBackend` sessions have no such explanation
/// available (a session can't be resumed on another machine without
/// cloud sync), so the wording stays direct.
///
/// This function only changes the *words*; the underlying
/// `Path::exists()` computation performed by [`check_template_source_path`]
/// / [`check_template_source_dir`] never differs by backend.
pub fn format_stale_template_source_note(is_cloud: bool) -> &'static str {
    if is_cloud {
        "template source directory not found (if this session was synced from another machine, this may be expected)"
    } else {
        "template source directory no longer exists"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn header_with_template_source_dir(dir: Option<PathBuf>) -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: "test-workflow".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: dir,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            respawn_generation: None,
        }
    }

    #[test]
    fn path_present_and_existing() {
        let dir = TempDir::new().unwrap();
        let status = check_template_source_path(Some(dir.path())).unwrap();
        assert_eq!(status.path, dir.path());
        assert!(status.exists);
    }

    #[test]
    fn path_present_and_missing() {
        let missing = PathBuf::from("/definitely/does/not/exist/anywhere/koto-test");
        assert!(!missing.exists(), "test precondition violated");
        let status = check_template_source_path(Some(&missing)).unwrap();
        assert_eq!(status.path, missing);
        assert!(!status.exists);
    }

    #[test]
    fn path_absent_returns_none() {
        assert!(check_template_source_path(None).is_none());
    }

    #[test]
    fn dir_present_and_existing() {
        let dir = TempDir::new().unwrap();
        let header = header_with_template_source_dir(Some(dir.path().to_path_buf()));
        let status = check_template_source_dir(&header).unwrap();
        assert_eq!(status.path, dir.path());
        assert!(status.exists);
    }

    #[test]
    fn dir_present_and_missing() {
        let missing = PathBuf::from("/definitely/does/not/exist/anywhere/koto-test-2");
        let header = header_with_template_source_dir(Some(missing.clone()));
        let status = check_template_source_dir(&header).unwrap();
        assert_eq!(status.path, missing);
        assert!(!status.exists);
    }

    #[test]
    fn dir_absent_returns_none() {
        let header = header_with_template_source_dir(None);
        assert!(check_template_source_dir(&header).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn dangling_symlink_reports_exists_false_not_none() {
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("dangling-link");
        let target = dir.path().join("does-not-exist");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let status = check_template_source_path(Some(&link)).unwrap();
        assert_eq!(status.path, link);
        assert!(
            !status.exists,
            "a dangling symlink must report exists: false, not None"
        );
    }

    #[test]
    fn regular_file_instead_of_directory_reports_exists_true() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, b"content").unwrap();

        let status = check_template_source_path(Some(&file)).unwrap();
        assert_eq!(status.path, file);
        assert!(
            status.exists,
            "existence-only check must report exists: true for a regular file, not treat it as invalid"
        );
    }

    #[test]
    fn format_note_direct_for_local() {
        let note = format_stale_template_source_note(false);
        assert_eq!(note, "template source directory no longer exists");
    }

    #[test]
    fn format_note_softened_for_cloud() {
        let note = format_stale_template_source_note(true);
        assert!(note.contains("synced from another machine"));
        assert_ne!(note, "template source directory no longer exists");
    }
}
