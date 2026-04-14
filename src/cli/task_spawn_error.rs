//! Per-task spawn error envelope.
//!
//! When the batch scheduler (Issue #12) attempts to materialize a child
//! workflow for a parent's task entry, a variety of failure modes can
//! surface per task without aborting the whole tick: the template can't
//! be found, compilation fails, the state file already exists on disk,
//! the backend refuses the write, and so on. Decision 11 Q4 commits to
//! "siblings keep spawning" semantics, so these errors are accumulated
//! per task into `SchedulerOutcome::Scheduled.errored`.
//!
//! This module introduces the typed envelope used for that accumulation.
//! Issue #3 extracts the child-spawn helper from `handle_init` and
//! returns `Result<(), TaskSpawnError>` so the future scheduler can
//! collect per-task outcomes. Callers that today want an anyhow error
//! instead simply keep calling `handle_init`.
//!
//! # Shape
//!
//! ```ignore
//! TaskSpawnError {
//!   task: "issue-1",
//!   kind: SpawnErrorKind::Collision,
//!   message: "child workflow 'parent.issue-1' already exists",
//!   paths_tried: None,
//!   template_source: None,
//!   compile_error: None,
//! }
//! ```
//!
//! `paths_tried`, `template_source`, and `compile_error` are reserved
//! for the scheduler's path-resolution and compile-cache machinery
//! (Issues #5, #8, #12). They are `Option<_>` so Issue #3 can leave
//! them `None` without a breaking change when later issues populate
//! them.

use serde::{Deserialize, Serialize};

/// Discriminator for the reason a single task's spawn failed.
///
/// Mirrors the Key Interfaces definition in
/// `docs/designs/DESIGN-batch-child-spawning.md` (Decision 12). The
/// string representation is `snake_case` so the JSON surface agents see
/// matches the design doc verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnErrorKind {
    /// Template path didn't resolve against any configured search base.
    TemplateNotFound,
    /// Template was found and read, but compilation failed.
    TemplateCompileFailed,
    /// A state file already exists at the target child path.
    ///
    /// Surfaces from `SessionError::Collision` (the atomic
    /// `init_state_file` rename saw `EEXIST`).
    Collision,
    /// The session backend couldn't be reached at all (cloud outage,
    /// remote unreachable). Separate from `IoError` so agents can
    /// distinguish "retry once storage comes back" from "retry the task
    /// — the filesystem has a specific complaint".
    BackendUnavailable,
    /// The kernel refused the write with `EACCES` / `EPERM`. Carved
    /// out of `IoError` because the remediation ("fix your directory
    /// permissions") is different from generic I/O trouble.
    PermissionDenied,
    /// Catch-all for I/O errors that don't match the variants above.
    IoError,
}

/// Indicates whether the template path used for a task came from the
/// agent-supplied `template` override or from the hook's
/// `default_template`. Populated by the scheduler when it knows;
/// `None` means the caller didn't provide the context (e.g., the
/// direct-init path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSource {
    /// Task entry carried an explicit `template` field.
    Override,
    /// Task entry inherited `default_template` from the hook.
    Default,
}

/// Typed compile-error detail shared with `BatchError::TemplateCompileFailed`.
///
/// This struct duplicates the shape documented in the design doc's Key
/// Interfaces section so the serialized JSON matches byte-for-byte.
/// Populating it is the scheduler's job; Issue #3 leaves
/// `TaskSpawnError.compile_error` as `None` and stuffs the compile
/// message into the top-level `message` field instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileError {
    /// Short machine-parseable discriminator (e.g. `yaml_parse`,
    /// `missing_field`, `state_reference`).
    pub kind: String,
    /// Human-readable message from the compiler.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<CompileErrorLocation>,
}

/// Optional source-location detail on a compile error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileErrorLocation {
    pub line: u32,
    pub column: u32,
}

/// Per-task spawn error. Collected per-tick by the batch scheduler
/// (Issue #12) and surfaced via `SchedulerOutcome::Scheduled.errored`.
///
/// Issue #3 introduces it as the return type of
/// [`crate::cli::init_child_from_parent`] so the helper can already be
/// a scheduler-ready building block even though no scheduler exists
/// yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpawnError {
    /// Short task name as the caller (or scheduler) knows it. For
    /// `init_child_from_parent` this is the child's short name (the
    /// one appended to the parent when composing the full child name).
    pub task: String,

    /// Discriminator — see [`SpawnErrorKind`].
    pub kind: SpawnErrorKind,

    /// Human-readable message. Always populated, even when richer
    /// structured detail lives in one of the optional fields below.
    pub message: String,

    /// Absolute paths the scheduler probed during template resolution,
    /// canonicalized. `None` on the direct-init path where resolution
    /// is a single lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths_tried: Option<Vec<String>>,

    /// Whether the template path came from an agent override or the
    /// hook's default. Populated by the scheduler when it knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_source: Option<TemplateSource>,

    /// Typed compile-error detail when `kind == TemplateCompileFailed`.
    /// Populated by the scheduler; Issue #3's direct-init helper leaves
    /// it `None` and relies on `message` for compile failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compile_error: Option<CompileError>,
}

impl TaskSpawnError {
    /// Convenience constructor for a minimal error (`kind + message`).
    ///
    /// Callers that only have a message and a kind — which is the
    /// common case on the direct-init path — don't need to spell out
    /// every optional field.
    pub fn new(task: impl Into<String>, kind: SpawnErrorKind, message: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            kind,
            message: message.into(),
            paths_tried: None,
            template_source: None,
            compile_error: None,
        }
    }
}

impl std::fmt::Display for TaskSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task {:?}: {}", self.task, self.message)
    }
}

impl std::error::Error for TaskSpawnError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_kind_serialization() {
        let cases = [
            (SpawnErrorKind::TemplateNotFound, "template_not_found"),
            (
                SpawnErrorKind::TemplateCompileFailed,
                "template_compile_failed",
            ),
            (SpawnErrorKind::Collision, "collision"),
            (SpawnErrorKind::BackendUnavailable, "backend_unavailable"),
            (SpawnErrorKind::PermissionDenied, "permission_denied"),
            (SpawnErrorKind::IoError, "io_error"),
        ];
        for (kind, expected) in cases {
            let v = serde_json::to_value(kind.clone()).unwrap();
            assert_eq!(
                v,
                serde_json::Value::String(expected.into()),
                "kind={:?}",
                kind
            );
        }
    }

    #[test]
    fn optional_fields_omitted_when_none() {
        let err = TaskSpawnError::new("issue-1", SpawnErrorKind::Collision, "already exists");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "task": "issue-1",
                "kind": "collision",
                "message": "already exists",
            })
        );
    }
}
