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
//! - [`check_execution_dir`] and [`check_execution_anchor`] ask the
//!   same question of the *other* recorded directory a header carries:
//!   `execution_dir`, the session's execution anchor
//!   (DESIGN-koto-runs-commands.md Decisions 6 and 7). The anchor
//!   check adds the containment comparison `koto next` runs on every
//!   tick, but the "does this directory exist" half goes through
//!   [`check_template_source_path`] rather than duplicating it.
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

/// Header-accepting wrapper over [`check_template_source_path`] for the
/// session's execution anchor.
///
/// Same question, different recorded directory: does
/// `header.execution_dir` exist, and on whose machine. Kept here rather
/// than beside the anchor check in the CLI so both recorded directories
/// are answered by one implementation.
pub fn check_execution_dir(header: &StateFileHeader) -> Option<TemplateSourceStatus> {
    check_template_source_path(header.execution_dir.as_deref())
}

/// What a tick's working directory means for the session's recorded
/// execution anchor.
///
/// Produced by [`check_execution_anchor`] before `koto next` compiles a
/// template or builds a gate or action closure, so a refusal reaches
/// the caller with nothing executed and nothing appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionAnchorCheck {
    /// The session records no anchor -- it was created before
    /// anchoring existed. The tick adopts `anchor` (its own canonical
    /// working directory), records it, and says so once (R14).
    Adopt { anchor: PathBuf },

    /// The recorded anchor resolves and the working directory is the
    /// anchor or lies beneath it. The tick runs, with `anchor` as the
    /// working directory for every gate and action (Decision 7).
    Satisfied { anchor: PathBuf },

    /// The recorded anchor names nothing on this machine -- the
    /// checkout was deleted, or the session travelled to a machine
    /// where that path does not exist. Distinct from
    /// [`ExecutionAnchorCheck::Outside`] because the repair is
    /// different: rebind rather than change directory (R15).
    Unresolvable { status: TemplateSourceStatus },

    /// The recorded anchor resolves, but the working directory is
    /// neither it nor beneath it -- a different tree (R12).
    Outside { anchor: PathBuf, cwd: PathBuf },
}

/// Decide what `cwd` means for a session whose header records
/// `recorded` as its execution anchor.
///
/// Both paths are compared in canonical form: `fs::canonicalize`
/// resolves `.`, `..`, and symlinks and strips trailing slashes, so a
/// symlinked path and a trailing-slash variant of the anchor both
/// satisfy it without special handling. Canonicalization does not
/// case-fold, so a path differing only in case names a different
/// directory and does not satisfy the anchor, on every platform.
///
/// "Satisfies" is containment, not equality: standing in a
/// subdirectory of the anchor is an ordinary thing to do and is not
/// the hazard the check exists to close, which is ticking a session
/// from a *different* tree. Containment is compared component-wise
/// (`Path::starts_with`), so `/repo-2` is not beneath `/repo`.
///
/// Existence is answered by [`check_template_source_path`], the
/// module's one implementation of that question, so the refusal can
/// carry the same machine label the other recorded-directory surfaces
/// report.
pub fn check_execution_anchor(recorded: Option<&Path>, cwd: &Path) -> ExecutionAnchorCheck {
    // A working directory that cannot be canonicalized is used as
    // given; the containment comparison below then fails closed rather
    // than accepting a tick it cannot vouch for.
    let cwd_canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());

    let recorded = match recorded {
        None => {
            return ExecutionAnchorCheck::Adopt {
                anchor: cwd_canonical,
            }
        }
        Some(path) => path,
    };

    let status = match check_template_source_path(Some(recorded)) {
        Some(status) => status,
        // Unreachable: `recorded` is `Some` here.
        None => {
            return ExecutionAnchorCheck::Adopt {
                anchor: cwd_canonical,
            }
        }
    };
    if !status.exists {
        return ExecutionAnchorCheck::Unresolvable { status };
    }

    // Exists but unreadable (a permission change on an ancestor, say)
    // reads the same way to a caller as gone: the tick cannot run
    // there, and rebinding is the repair.
    let anchor = match std::fs::canonicalize(recorded) {
        Ok(anchor) => anchor,
        Err(_) => return ExecutionAnchorCheck::Unresolvable { status },
    };

    if cwd_canonical.starts_with(&anchor) {
        ExecutionAnchorCheck::Satisfied { anchor }
    } else {
        ExecutionAnchorCheck::Outside {
            anchor,
            cwd: cwd_canonical,
        }
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
            execution_dir: None,
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

    // ===== Execution anchor (Decisions 6 and 7) =====

    #[test]
    fn anchor_absent_adopts_the_working_directory() {
        let dir = TempDir::new().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            check_execution_anchor(None, dir.path()),
            ExecutionAnchorCheck::Adopt { anchor: canonical }
        );
    }

    #[test]
    fn header_without_execution_dir_reports_no_status() {
        let header = header_with_template_source_dir(None);
        assert!(check_execution_dir(&header).is_none());
    }

    #[test]
    fn header_with_execution_dir_reports_status() {
        let dir = TempDir::new().unwrap();
        let mut header = header_with_template_source_dir(None);
        header.execution_dir = Some(dir.path().to_path_buf());
        let status = check_execution_dir(&header).unwrap();
        assert_eq!(status.path, dir.path());
        assert!(status.exists);
    }

    #[test]
    fn working_directory_equal_to_the_anchor_satisfies_it() {
        let dir = TempDir::new().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            check_execution_anchor(Some(dir.path()), dir.path()),
            ExecutionAnchorCheck::Satisfied {
                anchor: canonical.clone()
            }
        );
    }

    #[test]
    fn working_directory_beneath_the_anchor_satisfies_it() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("src").join("engine");
        std::fs::create_dir_all(&sub).unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            check_execution_anchor(Some(dir.path()), &sub),
            ExecutionAnchorCheck::Satisfied { anchor: canonical },
            "standing in a subdirectory is ordinary and is not the hazard the check closes"
        );
    }

    #[test]
    fn a_different_tree_does_not_satisfy_the_anchor() {
        let anchor_dir = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();
        let anchor = std::fs::canonicalize(anchor_dir.path()).unwrap();
        let cwd = std::fs::canonicalize(other_dir.path()).unwrap();
        assert_eq!(
            check_execution_anchor(Some(anchor_dir.path()), other_dir.path()),
            ExecutionAnchorCheck::Outside { anchor, cwd }
        );
    }

    #[test]
    fn a_sibling_sharing_a_name_prefix_does_not_satisfy_the_anchor() {
        // Containment is compared component-wise, so `repo-2` is not
        // "beneath" `repo` even though its path string starts with it.
        let base = TempDir::new().unwrap();
        let anchor_dir = base.path().join("repo");
        let sibling = base.path().join("repo-2");
        std::fs::create_dir_all(&anchor_dir).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(matches!(
            check_execution_anchor(Some(&anchor_dir), &sibling),
            ExecutionAnchorCheck::Outside { .. }
        ));
    }

    #[test]
    fn a_trailing_slash_variant_of_the_anchor_satisfies_it() {
        // PRD case 2: canonicalization strips the trailing separator,
        // so this needs no special handling.
        let dir = TempDir::new().unwrap();
        let with_slash = PathBuf::from(format!("{}/", dir.path().display()));
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            check_execution_anchor(Some(&with_slash), dir.path()),
            ExecutionAnchorCheck::Satisfied {
                anchor: canonical.clone()
            }
        );
        assert_eq!(
            check_execution_anchor(Some(dir.path()), &with_slash),
            ExecutionAnchorCheck::Satisfied { anchor: canonical }
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_to_the_anchor_satisfies_it() {
        // PRD case 1: canonicalization resolves the link, so ticking
        // through a symlinked path is the same directory.
        let base = TempDir::new().unwrap();
        let real = base.path().join("real-checkout");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.path().join("link-to-checkout");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let canonical = std::fs::canonicalize(&real).unwrap();

        assert_eq!(
            check_execution_anchor(Some(&real), &link),
            ExecutionAnchorCheck::Satisfied {
                anchor: canonical.clone()
            }
        );
        assert_eq!(
            check_execution_anchor(Some(&link), &real),
            ExecutionAnchorCheck::Satisfied { anchor: canonical },
            "the anchor is recorded canonically, so a symlinked recording resolves too"
        );
    }

    #[test]
    fn a_path_differing_only_in_case_does_not_satisfy_the_anchor() {
        // PRD case 3: canonicalization does not case-fold, so `Repo`
        // and `repo` are different directories. On a case-insensitive
        // filesystem the second create fails because it IS the first
        // directory -- there is no distinct case-differing path to
        // compare, so there is nothing to assert.
        let base = TempDir::new().unwrap();
        let upper = base.path().join("Repo");
        let lower = base.path().join("repo");
        std::fs::create_dir(&upper).unwrap();
        if std::fs::create_dir(&lower).is_err() {
            return;
        }
        assert!(matches!(
            check_execution_anchor(Some(&upper), &lower),
            ExecutionAnchorCheck::Outside { .. }
        ));
    }

    #[test]
    fn an_anchor_that_names_nothing_is_unresolvable() {
        let missing = PathBuf::from("/definitely/does/not/exist/anywhere/koto-anchor");
        let cwd = TempDir::new().unwrap();
        match check_execution_anchor(Some(&missing), cwd.path()) {
            ExecutionAnchorCheck::Unresolvable { status } => {
                assert_eq!(status.path, missing);
                assert!(!status.exists);
            }
            other => panic!("expected Unresolvable, got {:?}", other),
        }
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
