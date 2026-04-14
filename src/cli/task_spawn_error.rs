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

impl CompileError {
    /// Build a `CompileError` from a rule-tag prefix and message.
    ///
    /// The v1 `validate_materialize_children_errors` path returns
    /// `Result<(), String>` where each error string is prefixed with its
    /// rule tag (for example `"E1: state \"plan\": ..."`). Callers that
    /// want a structured envelope can split the prefix off and feed both
    /// halves to this helper rather than re-encoding the convention.
    ///
    /// `rule_tag` should be a short identifier such as `"E1"`, `"E10"`,
    /// or `"W4"`. It is stored verbatim in `CompileError.kind` so
    /// downstream consumers (scenario-11 in the functional tests, the
    /// scheduler envelope in Issue #12) can assert against the rule
    /// identifier without re-parsing the message.
    ///
    /// Issue #10 will introduce the full typed error envelope; this
    /// helper is the minimum wiring needed so the rule tag round-trips
    /// through `CompileError.kind` until then.
    pub fn from_rule_tag(rule_tag: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: rule_tag.into(),
            message: message.into(),
            location: None,
        }
    }
}

/// Extract a compile-rule tag (e.g., `"E1"`, `"W4"`) from the prefix of
/// an error or warning message produced by
/// `validate_materialize_children_errors` or
/// `collect_materialize_children_warnings`.
///
/// Both surfaces format messages as `"<TAG>: <rest>"` where `<TAG>` is
/// `E1`..`E10` for compile errors and `W1`..`W5` / `F5` for warnings.
/// Returns `Some("E1")` / `Some("W4")` / etc. when the message begins
/// with that shape, otherwise `None`.
///
/// This is the v1 bridge between the string-based validator output and
/// the structured `CompileError.kind` field. Issue #10 replaces it with
/// a typed enum pipeline end-to-end; until then scenario-11 uses this
/// helper to assert rule identity without coupling to message wording.
pub fn parse_compile_rule_tag(msg: &str) -> Option<&str> {
    let colon = msg.find(':')?;
    let tag = &msg[..colon];
    if tag.is_empty() {
        return None;
    }
    let mut chars = tag.chars();
    let first = chars.next()?;
    // Tag must start with E/W/F and have at least one digit after.
    if !matches!(first, 'E' | 'W' | 'F') {
        return None;
    }
    let rest_is_digits = chars.clone().count() > 0 && chars.all(|c| c.is_ascii_digit());
    if rest_is_digits {
        Some(tag)
    } else {
        None
    }
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
    ///
    /// Populated by Issue #5 (template path resolution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths_tried: Option<Vec<String>>,

    /// Whether the template path came from an agent override or the
    /// hook's default. Populated by the scheduler when it knows.
    ///
    /// Populated by Issue #5 (template path resolution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_source: Option<TemplateSource>,

    /// Typed compile-error detail when `kind == TemplateCompileFailed`.
    /// Populated by the scheduler; Issue #3's direct-init helper leaves
    /// it `None` and relies on `message` for compile failures.
    ///
    /// Populated by Issue #8 (typed compile-error plumbing).
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
    fn parse_compile_rule_tag_accepts_documented_shapes() {
        assert_eq!(
            parse_compile_rule_tag("E1: state \"plan\": ..."),
            Some("E1")
        );
        assert_eq!(
            parse_compile_rule_tag("E10: state \"plan\": x"),
            Some("E10")
        );
        assert_eq!(parse_compile_rule_tag("W4: state \"plan\": x"), Some("W4"));
        assert_eq!(parse_compile_rule_tag("F5: child 'x': ..."), Some("F5"));
    }

    #[test]
    fn parse_compile_rule_tag_rejects_non_rule_prefixes() {
        assert_eq!(parse_compile_rule_tag("state \"plan\": foo"), None);
        assert_eq!(parse_compile_rule_tag("warning: something"), None);
        assert_eq!(parse_compile_rule_tag("E: missing digits"), None);
        assert_eq!(parse_compile_rule_tag("E1"), None); // no colon at all
        assert_eq!(parse_compile_rule_tag(""), None);
        assert_eq!(parse_compile_rule_tag("Ex1: not a digit"), None);
    }

    #[test]
    fn compile_error_from_rule_tag_populates_kind() {
        let err = CompileError::from_rule_tag("E1", "state \"plan\": from_field must not be empty");
        assert_eq!(err.kind, "E1");
        assert!(err.message.starts_with("state "));
        assert!(err.location.is_none());
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
