//! Non-fatal warnings the batch scheduler emits during a tick.
//!
//! Decision 14 in DESIGN-batch-child-spawning.md surfaces these warnings
//! through `SchedulerOutcome.warnings` so agents can distinguish
//! "your scheduler tick produced output but with caveats" from
//! "your scheduler tick failed."
//!
//! Three variants live here today:
//!
//! - [`SchedulerWarning::MissingTemplateSourceDir`] — the parent's
//!   state-file header has no `template_source_dir`, typically because
//!   the workflow was created before the field existed. The scheduler
//!   skips the base-directory step of path resolution and falls back
//!   to `submitter_cwd`. This is a unit variant: the affected task is
//!   identifiable from the surrounding scheduler context, and callers
//!   are expected to dedup so at most one MissingTemplateSourceDir
//!   appears per tick.
//!
//! - [`SchedulerWarning::StaleTemplateSourceDir`] — the parent's
//!   state-file header records a `template_source_dir`, but that
//!   directory does not exist on the current machine (typically
//!   following a cross-machine session migration). The scheduler
//!   falls back to `submitter_cwd` and reports the original path,
//!   the machine identifier the failure was observed on (when known),
//!   and the directory it actually used.
//!
//! - [`SchedulerWarning::OmittedPriorTask`] — emitted when a submission
//!   omits a task name that appeared in a prior submission for this
//!   parent. Informational only (omission is not a cancellation signal
//!   per Decision 10). No emission logic exists yet; Issues #10/#21
//!   will wire it up.
//!
//! The `#[serde(tag = "kind", rename_all = "snake_case")]` attribute
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
/// {"kind": "missing_template_source_dir"}
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
///
/// or
///
/// ```json
/// {"kind": "omitted_prior_task", "task": "build"}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerWarning {
    /// The parent header carries no `template_source_dir`. Step (b)
    /// of path resolution is skipped; resolution falls through to
    /// `submitter_cwd`. Unit variant — callers are expected to dedup
    /// so at most one of these surfaces per scheduler tick.
    MissingTemplateSourceDir,

    /// The parent header records a `template_source_dir`, but the
    /// directory does not exist on the current machine. `path` is
    /// the value recorded in the header (serialized as a string for
    /// stable JSON across platforms); `machine_id` is a stable
    /// identifier for the machine the warning was observed on, when
    /// one can be determined (omitted from JSON when `None`);
    /// `falling_back_to` is the directory the scheduler actually
    /// used (typically `submitter_cwd`).
    StaleTemplateSourceDir {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        machine_id: Option<String>,
        falling_back_to: PathBuf,
    },

    /// A submission omitted a task name that appeared in a prior
    /// submission for this parent. Informational: per Decision 10,
    /// omission is not a cancellation signal, but agents are told
    /// rather than left to infer silently.
    OmittedPriorTask { task: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_serializes_with_kind_discriminator() {
        let w = SchedulerWarning::MissingTemplateSourceDir;
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, "{\"kind\":\"missing_template_source_dir\"}");
    }

    #[test]
    fn stale_serializes_with_kind_discriminator() {
        let w = SchedulerWarning::StaleTemplateSourceDir {
            path: "/host-a/work".to_string(),
            machine_id: Some("host-b".to_string()),
            falling_back_to: PathBuf::from("/host-b/cwd"),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(
            json.contains("\"kind\":\"stale_template_source_dir\""),
            "got {}",
            json
        );
        assert!(json.contains("\"path\":\"/host-a/work\""), "got {}", json);
        assert!(json.contains("\"machine_id\":\"host-b\""), "got {}", json);
        assert!(
            json.contains("\"falling_back_to\":\"/host-b/cwd\""),
            "got {}",
            json
        );
    }

    #[test]
    fn stale_omits_machine_id_when_none() {
        let w = SchedulerWarning::StaleTemplateSourceDir {
            path: "/host-a/work".to_string(),
            machine_id: None,
            falling_back_to: PathBuf::from("/host-b/cwd"),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(
            !json.contains("machine_id"),
            "machine_id should be omitted when None, got {}",
            json
        );
    }

    #[test]
    fn omitted_prior_task_serializes_with_kind_discriminator() {
        let w = SchedulerWarning::OmittedPriorTask {
            task: "build".to_string(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, "{\"kind\":\"omitted_prior_task\",\"task\":\"build\"}");
    }

    #[test]
    fn round_trip() {
        let cases = vec![
            SchedulerWarning::MissingTemplateSourceDir,
            SchedulerWarning::StaleTemplateSourceDir {
                path: "/x".to_string(),
                machine_id: Some("m".to_string()),
                falling_back_to: PathBuf::from("/y"),
            },
            SchedulerWarning::StaleTemplateSourceDir {
                path: "/x".to_string(),
                machine_id: None,
                falling_back_to: PathBuf::from("/y"),
            },
            SchedulerWarning::OmittedPriorTask {
                task: "deploy".to_string(),
            },
        ];
        for w in cases {
            let json = serde_json::to_string(&w).unwrap();
            let parsed: SchedulerWarning = serde_json::from_str(&json).unwrap();
            assert_eq!(w, parsed);
        }
    }
}
