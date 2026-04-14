//! Path resolution for child-template paths submitted to the batch
//! scheduler.
//!
//! Decision 4 (refined by Decision 14) in
//! DESIGN-batch-child-spawning.md defines the resolution order:
//!
//! 1. **Absolute** paths pass through unchanged.
//! 2. **Base directory** — `template_source_dir` from the parent's
//!    state-file header (the directory the parent template was loaded
//!    from). When `Some`, relative paths are joined against it.
//! 3. **Submitter cwd** — `submitter_cwd` from the most recent
//!    `EvidenceSubmitted` event. When the base step misses, the
//!    relative path is joined against this directory.
//!
//! Each fallback step that triggers emits a [`SchedulerWarning`] so
//! agents can diagnose stale or missing path-resolution context after
//! cross-machine session migration.
//!
//! This module is intentionally pure: it never touches the filesystem
//! beyond `Path::exists`, never logs, and returns warnings in a
//! deterministic order so callers can plumb them straight onto
//! `SchedulerOutcome.warnings`.

use std::path::{Path, PathBuf};

use crate::engine::scheduler_warning::SchedulerWarning;

/// Outcome of resolving a single template path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathResolution {
    /// The path the resolver settled on. Always present, even if the
    /// file does not exist on disk — the caller decides whether a
    /// non-existent path is fatal (typically it produces a per-task
    /// `TemplateNotFound` per Decision 14).
    pub resolved: PathBuf,

    /// Warnings emitted during resolution. Empty in the happy paths
    /// (absolute path, or relative path resolved cleanly via the base
    /// directory). Populated when the resolver had to skip step (b)
    /// because `template_source_dir` was absent or stale.
    ///
    /// Caller-side dedup contract: `MissingTemplateSourceDir` is a
    /// per-tick condition (the parent header either has the field or
    /// it does not), so callers aggregating warnings across multiple
    /// task entries in a single scheduler tick should dedup it to at
    /// most one occurrence per tick. `StaleTemplateSourceDir` and
    /// `OmittedPriorTask` carry payload that distinguishes instances,
    /// and callers should preserve each occurrence.
    pub warnings: Vec<SchedulerWarning>,
}

/// Best-effort machine identifier surfaced on
/// [`SchedulerWarning::StaleTemplateSourceDir`].
///
/// Reads `/etc/machine-id` on Linux for a stable per-host string. Falls
/// back to the `HOSTNAME` environment variable. Returns `None` when no
/// usable identifier can be derived — a warning without a `machine_id`
/// is more honest than one carrying a fabricated `"unknown"` value, and
/// the design's `Option<String>` shape with `skip_serializing_if`
/// expresses this directly.
///
/// TODO: a future revision may swap this for the same identifier the
/// cloud-sync layer attaches to state files (Decision 12 Q5), so a
/// session migrated between machines surfaces matching IDs across the
/// `sync_status` and `StaleTemplateSourceDir` channels.
pub(crate) fn current_machine_id() -> Option<String> {
    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(host) = std::env::var("HOSTNAME") {
        if !host.is_empty() {
            return Some(host);
        }
    }
    None
}

/// Resolve `target` against the configured base directories.
///
/// Resolution order:
///
/// 1. Absolute `target` → returned unchanged, no warnings.
/// 2. Relative `target` joined against `template_source_dir` (when
///    `Some` and the directory exists). If the joined path exists,
///    returned as-is.
/// 3. Relative `target` joined against `submitter_cwd` (when `Some`).
///    Returned regardless of whether the file exists — the caller
///    classifies a missing file as a per-task error.
///
/// Warnings:
///
/// - [`SchedulerWarning::MissingTemplateSourceDir`] — emitted when
///   `template_source_dir` is `None` and the resolver had to skip
///   step (b) entirely. Carries the relative `target` path so agents
///   can attribute the warning back to a task entry.
/// - [`SchedulerWarning::StaleTemplateSourceDir`] — emitted when
///   `template_source_dir` is `Some` but the directory does not exist
///   on the current machine, or when the joined path doesn't exist.
///   Carries the recorded path, the current machine identifier, and
///   the directory the resolver fell back to.
///
/// When neither `template_source_dir` nor `submitter_cwd` is set, the
/// relative path is returned unchanged (no warnings beyond
/// `MissingTemplateSourceDir`). Real callers always supply at least
/// one of the two — the helper tolerates the empty case for ease of
/// testing.
pub fn resolve_template_path(
    target: &str,
    template_source_dir: Option<&Path>,
    submitter_cwd: Option<&Path>,
) -> PathResolution {
    let target_path = Path::new(target);

    // Step (a): absolute paths bypass the resolver entirely.
    if target_path.is_absolute() {
        return PathResolution {
            resolved: target_path.to_path_buf(),
            warnings: Vec::new(),
        };
    }

    let mut warnings = Vec::new();

    // Step (b): try the template_source_dir base.
    match template_source_dir {
        Some(base) => {
            if !base.exists() {
                // The recorded base directory is gone (typically a
                // cross-machine migration). Emit StaleTemplateSourceDir
                // and fall through to submitter_cwd.
                //
                // F3: when there is no `submitter_cwd` to fall back to,
                // there is nothing meaningful to point `falling_back_to`
                // at. Use the original (relative) target path so the
                // warning records the actual resolved value rather than
                // pretending we fell back to the stale base.
                let fallback = submitter_cwd
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| target_path.to_path_buf());
                warnings.push(SchedulerWarning::StaleTemplateSourceDir {
                    path: base.to_string_lossy().into_owned(),
                    machine_id: current_machine_id(),
                    falling_back_to: fallback,
                });
            } else {
                let candidate = base.join(target_path);
                if candidate.exists() {
                    return PathResolution {
                        resolved: candidate,
                        warnings,
                    };
                }
                // Base exists but the file isn't there. Per Decision 14
                // we fall through to submitter_cwd silently — the
                // typical cause is a moved template, not stale base.
                // (StaleTemplateSourceDir is reserved for the
                // base-doesn't-exist case.)
            }
        }
        None => {
            // No base configured (pre-feature state file). Skip step
            // (b) and emit MissingTemplateSourceDir. The variant is a
            // unit value; the affected task is identifiable from the
            // surrounding scheduler context.
            warnings.push(SchedulerWarning::MissingTemplateSourceDir);
        }
    }

    // Step (c): try submitter_cwd.
    if let Some(cwd) = submitter_cwd {
        return PathResolution {
            resolved: cwd.join(target_path),
            warnings,
        };
    }

    // Neither base nor cwd available; return the path as-is and let
    // the caller classify the failure.
    PathResolution {
        resolved: target_path.to_path_buf(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn absolute_path_returns_unchanged_no_warnings() {
        let abs = if cfg!(windows) {
            r"C:\tmp\template.md"
        } else {
            "/tmp/template.md"
        };
        let res = resolve_template_path(abs, None, None);
        assert_eq!(res.resolved, PathBuf::from(abs));
        assert!(res.warnings.is_empty());
    }

    #[test]
    fn absolute_path_ignores_base_and_cwd() {
        // Even with both base and cwd set, an absolute target must
        // pass through verbatim — Decision 14 step (a).
        let base = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let abs = if cfg!(windows) {
            r"C:\absolute\path.md".to_string()
        } else {
            "/absolute/path.md".to_string()
        };
        let res = resolve_template_path(&abs, Some(base.path()), Some(cwd.path()));
        assert_eq!(res.resolved, PathBuf::from(&abs));
        assert!(res.warnings.is_empty());
    }

    #[test]
    fn relative_resolves_via_base_when_file_exists() {
        let base = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        // Place template only under the base dir.
        let file = base.path().join("child.md");
        std::fs::write(&file, "x").unwrap();

        let res = resolve_template_path("child.md", Some(base.path()), Some(cwd.path()));
        assert_eq!(res.resolved, file);
        assert!(
            res.warnings.is_empty(),
            "no warnings expected on happy path, got {:?}",
            res.warnings
        );
    }

    #[test]
    fn relative_falls_through_to_cwd_when_base_misses_file() {
        // base exists but does not contain the file; fall through to
        // cwd silently (no warning) per Decision 14.
        let base = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let file = cwd.path().join("child.md");
        std::fs::write(&file, "x").unwrap();

        let res = resolve_template_path("child.md", Some(base.path()), Some(cwd.path()));
        assert_eq!(res.resolved, cwd.path().join("child.md"));
        assert!(
            res.warnings.is_empty(),
            "fallback through existing-but-missing-file base should not warn"
        );
    }

    #[test]
    fn missing_base_emits_warning_and_falls_back_to_cwd() {
        let cwd = TempDir::new().unwrap();
        let res = resolve_template_path("child.md", None, Some(cwd.path()));
        assert_eq!(res.resolved, cwd.path().join("child.md"));
        assert_eq!(res.warnings.len(), 1);
        assert!(matches!(
            res.warnings[0],
            SchedulerWarning::MissingTemplateSourceDir
        ));
    }

    #[test]
    fn stale_base_emits_warning_with_machine_id_and_fallback() {
        // Base path that doesn't exist on disk simulates a migrated
        // session whose recorded base is from another machine.
        let cwd = TempDir::new().unwrap();
        let stale = PathBuf::from("/definitely/does/not/exist/anywhere/koto-test");
        assert!(
            !stale.exists(),
            "test precondition: stale path must not exist"
        );

        let res = resolve_template_path("child.md", Some(&stale), Some(cwd.path()));
        assert_eq!(res.resolved, cwd.path().join("child.md"));
        assert_eq!(res.warnings.len(), 1);
        match &res.warnings[0] {
            SchedulerWarning::StaleTemplateSourceDir {
                path,
                machine_id,
                falling_back_to,
            } => {
                assert_eq!(path, &stale.to_string_lossy().into_owned());
                // machine_id is best-effort: Some when /etc/machine-id
                // or HOSTNAME is available, None otherwise. We only
                // check that, when populated, it is non-empty.
                if let Some(id) = machine_id {
                    assert!(!id.is_empty(), "machine_id, when Some, must be non-empty");
                }
                assert_eq!(falling_back_to, &cwd.path().to_path_buf());
            }
            other => panic!("expected StaleTemplateSourceDir, got {:?}", other),
        }
    }

    #[test]
    fn stale_base_without_cwd_falls_back_to_target_path() {
        // F3: when both base is stale and there is no submitter_cwd,
        // `falling_back_to` should record the original (relative)
        // target rather than the stale base — we did not actually fall
        // back to the stale base.
        let stale = PathBuf::from("/definitely/does/not/exist/anywhere/koto-test");
        assert!(
            !stale.exists(),
            "test precondition: stale path must not exist"
        );

        let res = resolve_template_path("child.md", Some(&stale), None);
        assert_eq!(res.resolved, PathBuf::from("child.md"));
        assert_eq!(res.warnings.len(), 1);
        match &res.warnings[0] {
            SchedulerWarning::StaleTemplateSourceDir {
                path,
                falling_back_to,
                ..
            } => {
                assert_eq!(path, &stale.to_string_lossy().into_owned());
                assert_eq!(
                    falling_back_to,
                    &PathBuf::from("child.md"),
                    "without cwd, falling_back_to should record the original target"
                );
            }
            other => panic!("expected StaleTemplateSourceDir, got {:?}", other),
        }
    }

    #[test]
    fn no_base_no_cwd_returns_relative_path_with_missing_warning() {
        let res = resolve_template_path("child.md", None, None);
        assert_eq!(res.resolved, PathBuf::from("child.md"));
        assert_eq!(res.warnings.len(), 1);
        assert!(matches!(
            res.warnings[0],
            SchedulerWarning::MissingTemplateSourceDir
        ));
    }

    #[test]
    fn current_machine_id_returns_some_or_none_consistently() {
        // The value depends on the environment (CI vs. dev box); we
        // only check that the call doesn't panic and, when Some, the
        // string is non-empty.
        if let Some(id) = current_machine_id() {
            assert!(!id.is_empty());
        }
    }
}
