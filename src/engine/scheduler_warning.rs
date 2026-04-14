//! Non-fatal warnings the batch scheduler emits during a tick.
//!
//! Decision 14 in DESIGN-batch-child-spawning.md surfaces these warnings
//! through `SchedulerOutcome.warnings` so agents can distinguish
//! "your scheduler tick produced output but with caveats" from
//! "your scheduler tick failed."
//!
//! Two variants live here today:
//!
//! - [`SchedulerWarning::MissingTemplateSourceDir`] — the parent's
//!   state-file header has no `template_source_dir`, typically because
//!   the workflow was created before the field existed. The scheduler
//!   skips the base-directory step of path resolution and falls back
//!   to `submitter_cwd`.
//!
//! - [`SchedulerWarning::StaleTemplateSourceDir`] — the parent's
//!   state-file header records a `template_source_dir`, but that
//!   directory does not exist on the current machine (typically
//!   following a cross-machine session migration). The scheduler
//!   falls back to `submitter_cwd` and reports the original path,
//!   the machine identifier the failure was observed on, and the
//!   directory it actually used.
//!
//! TODO(#16/#21): Issues #16 and #21 will extend this enum with
//! additional variants (e.g., `OmittedPriorTask`). The
//! `#[serde(tag = "kind", rename_all = "snake_case")]` attribute
//! keeps the external JSON shape stable so adding variants is
//! additive — existing consumers that switch on `kind` continue to
//! work, and unknown variants surface as a discriminator they can
//! ignore.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Non-fatal warning produced during a scheduler tick.
///
/// Serialized with an external `kind` discriminator so the JSON shape
/// is stable across feature additions:
///
/// ```json
/// {"kind": "missing_template_source_dir", "path": "/tmp/x"}
/// ```
///
/// or
///
/// ```json
/// {
///   "kind": "stale_template_source_dir",
///   "path": "/host-a/work",
///   "machine_id": "host-b",
///   "falling_back_to": "/host-b/cwd"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerWarning {
    /// The parent header carries no `template_source_dir`. Step (b)
    /// of path resolution is skipped; resolution falls through to
    /// `submitter_cwd`. `path` is the relative template path that
    /// triggered the warning so the agent can attribute it back to
    /// a specific task entry.
    MissingTemplateSourceDir { path: PathBuf },

    /// The parent header records a `template_source_dir`, but the
    /// directory does not exist on the current machine. `path` is
    /// the value recorded in the header; `machine_id` is a stable
    /// identifier for the machine the warning was observed on;
    /// `falling_back_to` is the directory the scheduler actually
    /// used (typically `submitter_cwd`).
    StaleTemplateSourceDir {
        path: PathBuf,
        machine_id: String,
        falling_back_to: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_serializes_with_kind_discriminator() {
        let w = SchedulerWarning::MissingTemplateSourceDir {
            path: PathBuf::from("relative/template.md"),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(
            json.contains("\"kind\":\"missing_template_source_dir\""),
            "got {}",
            json
        );
        assert!(
            json.contains("\"path\":\"relative/template.md\""),
            "got {}",
            json
        );
    }

    #[test]
    fn stale_serializes_with_kind_discriminator() {
        let w = SchedulerWarning::StaleTemplateSourceDir {
            path: PathBuf::from("/host-a/work"),
            machine_id: "host-b".to_string(),
            falling_back_to: PathBuf::from("/host-b/cwd"),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(
            json.contains("\"kind\":\"stale_template_source_dir\""),
            "got {}",
            json
        );
        assert!(json.contains("\"machine_id\":\"host-b\""), "got {}", json);
        assert!(
            json.contains("\"falling_back_to\":\"/host-b/cwd\""),
            "got {}",
            json
        );
    }

    #[test]
    fn round_trip() {
        let cases = vec![
            SchedulerWarning::MissingTemplateSourceDir {
                path: PathBuf::from("a/b.md"),
            },
            SchedulerWarning::StaleTemplateSourceDir {
                path: PathBuf::from("/x"),
                machine_id: "m".to_string(),
                falling_back_to: PathBuf::from("/y"),
            },
        ];
        for w in cases {
            let json = serde_json::to_string(&w).unwrap();
            let parsed: SchedulerWarning = serde_json::from_str(&json).unwrap();
            assert_eq!(w, parsed);
        }
    }
}
