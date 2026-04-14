//! Batch-workflow error envelope.
//!
//! This module defines the typed error machinery for batch-scoped ticks
//! and the JSON envelope they emit. Issue #2 introduced the
//! `ConcurrentTick` variant surfaced when `handle_next` fails to acquire
//! the advisory flock on a batch-scoped parent's state file. Issue #9
//! added `InvalidBatchDefinition` and `LimitExceeded`. Issue #10
//! completes the envelope with the full set of typed variants required
//! by Decision 11 and pins the wire format via snapshot tests.
//!
//! # Envelope shape
//!
//! The serialized form is always the top-level `{"action": "error",
//! "batch": ...}` object agents already parse. Individual variants
//! expand under `batch`; `action` never changes.
//!
//! ```json
//! {
//!   "action": "error",
//!   "batch": {
//!     "kind": "concurrent_tick",
//!     "holder_pid": 1234
//!   }
//! }
//! ```
//!
//! `holder_pid` is `null` when the flock path cannot determine the
//! holder PID (the `EWOULDBLOCK` signal does not carry peer identity).
//!
//! A richer wrapper (`NextResponse::Error`) lives in `next_types.rs`
//! and re-emits the same `batch` payload as a sibling key on the typed
//! `NextError` envelope. See [`BatchError::to_error_context`].

use serde::{Deserialize, Serialize};

use crate::cli::task_spawn_error::{CompileError, SpawnErrorKind};

/// Errors raised by batch-workflow machinery.
///
/// Every variant maps to a snake_case `kind` in the JSON envelope and
/// carries the typed context the design commits to. Issue #10 promoted
/// `TemplateNotFound`, `TemplateCompileFailed`, `BackendError`,
/// `SpawnFailed`, and `InvalidRetryRequest` from informal strings to
/// first-class variants so agents can branch without string parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchError {
    /// A concurrent `koto next` invocation holds the advisory flock on
    /// this batch parent's state file. The current tick aborts rather
    /// than block so an agent (or CI runner) receives an immediate,
    /// structured response instead of a hang.
    ///
    /// `holder_pid` is best-effort; today it is always `None`. The
    /// field is reserved so a future `F_OFD_GETLK` probe can populate
    /// it without changing the envelope.
    ConcurrentTick { holder_pid: Option<u32> },

    /// A pre-append runtime rule (R0/R3/R4/R5/R8/R9) rejected the
    /// submission. Carries the typed [`InvalidBatchReason`] so the
    /// error envelope stays machine-parseable.
    InvalidBatchDefinition { reason: InvalidBatchReason },

    /// A pre-append limit (R6) was violated. `which` identifies which
    /// hard limit tripped; `actual` reports the submitted magnitude;
    /// `task` names the offending task when the limit is per-task
    /// (e.g. `waits_on.len() > 10`), and is `None` for whole-
    /// submission limits (`tasks.len() > 1000`, DAG depth > 50).
    LimitExceeded {
        which: LimitKind,
        limit: u32,
        actual: u32,
        task: Option<String>,
    },

    /// Template path did not resolve against any configured search
    /// base. Decision 14 split this out of the former
    /// `TemplateResolveFailed`.
    TemplateNotFound {
        task: String,
        path: String,
        paths_tried: Vec<String>,
    },

    /// Template was found and read, but compilation failed. Carries
    /// the typed [`CompileError`] so agents render one shape for
    /// compile failures regardless of surface.
    TemplateCompileFailed {
        task: String,
        path: String,
        compile_error: CompileError,
    },

    /// Backend list/read failed during classification. Tick-wide.
    BackendError { message: String, retryable: bool },

    /// `backend.create` / `init_state_file` failed for a specific
    /// task. Promoted to a typed variant so per-task spawn errors
    /// surface through the same envelope as the tick-wide ones.
    ///
    /// Serialized as `spawn_kind` (not `kind`) to avoid collision with
    /// the outer envelope's `kind` discriminator. Mirrors the
    /// `name_rule` rename on `InvalidName`'s inner kind.
    SpawnFailed {
        task: String,
        kind: SpawnErrorKind,
        message: String,
    },

    /// Retry submission failed validation. Decision 9, Decision 11 Q11.
    InvalidRetryRequest { reason: InvalidRetryReason },
}

/// Structural rejection reasons for a batch submission.
///
/// One variant per runtime rule class. R6 limits are carried by the
/// sibling [`BatchError::LimitExceeded`] variant (not nested here) so
/// callers can branch on "limit" vs. "structural rule" without peeking
/// inside a sub-enum.
#[derive(Debug, Clone, PartialEq)]
pub enum InvalidBatchReason {
    /// R0: `tasks.len() >= 1` failed.
    EmptyTaskList,
    /// R3: `waits_on` contains a cycle. `cycle` lists the task names
    /// in the order they close the loop (first element repeats at the
    /// end implicitly).
    Cycle { cycle: Vec<String> },
    /// R4: one or more `waits_on` entries name tasks absent from the
    /// submission. Aggregated into a single list so agents get every
    /// offender at once.
    DanglingRefs { entries: Vec<DanglingRef> },
    /// R5: two or more submitted tasks share a name.
    DuplicateNames { duplicates: Vec<String> },
    /// R8: the submitted entry for an already-spawned task disagrees
    /// with the `spawn_entry` snapshot. `changed_fields` enumerates
    /// every field whose value differs.
    SpawnedTaskMutated {
        task: String,
        changed_fields: Vec<MutatedField>,
    },
    /// R9 (regex/length band): the task name fails the regex, is
    /// outside the 1..=64 length band, or similar. `reserved` is a
    /// sibling variant so reserved-name collisions never nest under
    /// `InvalidName`.
    InvalidName {
        task: String,
        name_rule: InvalidNameDetail,
    },
    /// R9 (reserved list): the task name is on the reserved list
    /// (e.g. `retry_failed`, `cancel_tasks`).
    ReservedNameCollision { task: String, reserved: String },
}

/// One `waits_on` edge that pointed at an absent task name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DanglingRef {
    pub task: String,
    pub unknown: String,
}

/// One field whose submitted value disagreed with the recorded
/// `spawn_entry` snapshot. `field` names the disagreeing field using
/// the same dotted path the design's R8 check uses (`template`,
/// `vars`, `waits_on`, `vars.<key>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutatedField {
    pub field: String,
    pub spawned_value: serde_json::Value,
    pub submitted_value: serde_json::Value,
}

/// Which hard limit (R6) was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    /// `tasks.len() > 1000`.
    Tasks,
    /// Some task's `waits_on.len() > 10`.
    WaitsOn,
    /// DAG depth, measured as node count along the longest root-to-
    /// leaf path, exceeded 50.
    Depth,
    /// The inbound payload exceeded the 1MB budget.
    PayloadBytes,
}

/// Why R9 rejected a task name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidNameDetail {
    /// Did not match `^[A-Za-z0-9_-]+$`.
    RegexMismatch,
    /// Length was outside the 1..=64 band. Carries the observed length.
    LengthOutOfRange(usize),
}

/// Rejection reasons for `retry_failed` submissions. Decision 9
/// and Decision 11 Q11 pin the precedence order `UnknownChildren →
/// ChildIsBatchParent → ChildNotEligible → MixedWithOtherEvidence →
/// RetryAlreadyInProgress` so `MultipleReasons` aggregation is stable.
#[derive(Debug, Clone, PartialEq)]
pub enum InvalidRetryReason {
    /// Parent has no materialized children yet (premature retry).
    NoBatchMaterialized,
    /// `retry_failed.children` was empty.
    EmptyChildList,
    /// Named children exist on disk but are not in a retryable state.
    ChildNotEligible { children: Vec<ChildEligibility> },
    /// Named children do not exist on disk for this parent.
    UnknownChildren { children: Vec<String> },
    /// Named children are themselves batch parents (carry a
    /// `materialize_children` hook). v1 rejects cross-level retry.
    ChildIsBatchParent { children: Vec<String> },
    /// Reserved for non-flocked futures. Under Decision 12's advisory
    /// flock, a concurrent retry submission surfaces as
    /// [`BatchError::ConcurrentTick`] at a lower layer.
    RetryAlreadyInProgress,
    /// `retry_failed` was submitted alongside other evidence keys;
    /// `extra_fields` names the offending keys.
    MixedWithOtherEvidence { extra_fields: Vec<String> },
    /// Aggregation wrapper for submissions that violate more than one
    /// rule in the same tick. `reasons` is ordered per the pinned
    /// precedence: UnknownChildren → ChildIsBatchParent →
    /// ChildNotEligible → MixedWithOtherEvidence →
    /// RetryAlreadyInProgress. `NoBatchMaterialized` and
    /// `EmptyChildList` short-circuit before aggregation.
    MultipleReasons { reasons: Vec<InvalidRetryReason> },
}

/// Retryability classification for a child named in `retry_failed`.
///
/// `current_outcome` is a typed enum so the JSON surface carries the
/// same snake_case tokens used elsewhere (`TaskOutcome`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildEligibility {
    pub name: String,
    pub current_outcome: ChildOutcome,
}

/// Subset of `TaskOutcome` that a retryable child can carry. There is
/// deliberately no `Unknown` sentinel — unknown names surface through
/// [`InvalidRetryReason::UnknownChildren`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildOutcome {
    Failure,
    Skipped,
    SpawnFailed,
    Pending,
    Success,
    Blocked,
}

// --------- Serialization payloads ------------------------------------

/// Wire payload under the `batch` key of the outer `{"action": "error",
/// "batch": ...}` object. A named struct with `#[serde(tag = "kind")]`
/// keeps the two top-level keys stable while individual variants
/// evolve.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BatchErrorPayload {
    ConcurrentTick {
        holder_pid: Option<u32>,
    },
    InvalidBatchDefinition {
        reason: InvalidBatchReasonPayload,
    },
    LimitExceeded {
        which: LimitKind,
        limit: u32,
        actual: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        task: Option<String>,
    },
    TemplateNotFound {
        task: String,
        path: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        paths_tried: Vec<String>,
    },
    TemplateCompileFailed {
        task: String,
        path: String,
        compile_error: CompileError,
    },
    BackendError {
        message: String,
        retryable: bool,
    },
    SpawnFailed {
        task: String,
        spawn_kind: SpawnErrorKind,
        message: String,
    },
    InvalidRetryRequest {
        reason: InvalidRetryReasonPayload,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
enum InvalidBatchReasonPayload {
    EmptyTaskList,
    Cycle {
        cycle: Vec<String>,
    },
    DanglingRefs {
        entries: Vec<DanglingRef>,
    },
    DuplicateNames {
        duplicates: Vec<String>,
    },
    SpawnedTaskMutated {
        task: String,
        changed_fields: Vec<MutatedField>,
    },
    InvalidName {
        task: String,
        #[serde(flatten)]
        detail: InvalidNameDetailPayload,
    },
    ReservedNameCollision {
        task: String,
        reserved: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "name_rule", rename_all = "snake_case")]
enum InvalidNameDetailPayload {
    RegexMismatch,
    LengthOutOfRange { length: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
enum InvalidRetryReasonPayload {
    NoBatchMaterialized,
    EmptyChildList,
    ChildNotEligible {
        children: Vec<ChildEligibility>,
    },
    UnknownChildren {
        children: Vec<String>,
    },
    ChildIsBatchParent {
        children: Vec<String>,
    },
    RetryAlreadyInProgress,
    MixedWithOtherEvidence {
        extra_fields: Vec<String>,
    },
    MultipleReasons {
        reasons: Vec<InvalidRetryReasonPayload>,
    },
}

/// Full envelope written to stdout when a batch-scoped tick fails.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct BatchErrorEnvelope {
    action: &'static str,
    batch: BatchErrorPayload,
}

impl BatchError {
    /// Return the JSON envelope for this error, ready to serialize with
    /// `serde_json::to_string`.
    pub fn to_envelope(&self) -> serde_json::Value {
        let payload = self.to_payload();
        serde_json::to_value(BatchErrorEnvelope {
            action: "error",
            batch: payload,
        })
        .expect("BatchErrorEnvelope is always serializable")
    }

    /// Return the batch-specific JSON payload alone — the value that
    /// lives under `{"action": "error", "batch": ...}`. Used by
    /// `NextResponse::Error` to embed the same payload alongside the
    /// typed `error` key without duplicating serialization logic.
    pub fn to_batch_payload(&self) -> serde_json::Value {
        serde_json::to_value(self.to_payload()).expect("BatchErrorPayload is always serializable")
    }

    fn to_payload(&self) -> BatchErrorPayload {
        match self {
            BatchError::ConcurrentTick { holder_pid } => BatchErrorPayload::ConcurrentTick {
                holder_pid: *holder_pid,
            },
            BatchError::InvalidBatchDefinition { reason } => {
                BatchErrorPayload::InvalidBatchDefinition {
                    reason: reason_to_payload(reason),
                }
            }
            BatchError::LimitExceeded {
                which,
                limit,
                actual,
                task,
            } => BatchErrorPayload::LimitExceeded {
                which: *which,
                limit: *limit,
                actual: *actual,
                task: task.clone(),
            },
            BatchError::TemplateNotFound {
                task,
                path,
                paths_tried,
            } => BatchErrorPayload::TemplateNotFound {
                task: task.clone(),
                path: path.clone(),
                paths_tried: paths_tried.clone(),
            },
            BatchError::TemplateCompileFailed {
                task,
                path,
                compile_error,
            } => BatchErrorPayload::TemplateCompileFailed {
                task: task.clone(),
                path: path.clone(),
                compile_error: compile_error.clone(),
            },
            BatchError::BackendError { message, retryable } => BatchErrorPayload::BackendError {
                message: message.clone(),
                retryable: *retryable,
            },
            BatchError::SpawnFailed {
                task,
                kind,
                message,
            } => BatchErrorPayload::SpawnFailed {
                task: task.clone(),
                spawn_kind: kind.clone(),
                message: message.clone(),
            },
            BatchError::InvalidRetryRequest { reason } => BatchErrorPayload::InvalidRetryRequest {
                reason: retry_reason_to_payload(reason),
            },
        }
    }

    /// Process exit code for this error.
    ///
    /// Transient failures (`ConcurrentTick`, `BackendError { retryable:
    /// true }`) return 1 so agents retry. Caller errors (definition
    /// and retry-submission failures, limits, template problems)
    /// return 2. `SpawnFailed` is a per-task condition that the
    /// scheduler usually accumulates per tick; when surfaced directly
    /// it's an operational failure — exit 3.
    pub fn exit_code(&self) -> i32 {
        match self {
            BatchError::ConcurrentTick { .. } => 1,
            BatchError::BackendError {
                retryable: true, ..
            } => 1,
            BatchError::InvalidBatchDefinition { .. }
            | BatchError::LimitExceeded { .. }
            | BatchError::InvalidRetryRequest { .. }
            | BatchError::TemplateNotFound { .. }
            | BatchError::TemplateCompileFailed { .. } => 2,
            BatchError::BackendError { .. } | BatchError::SpawnFailed { .. } => 3,
        }
    }
}

fn reason_to_payload(reason: &InvalidBatchReason) -> InvalidBatchReasonPayload {
    match reason {
        InvalidBatchReason::EmptyTaskList => InvalidBatchReasonPayload::EmptyTaskList,
        InvalidBatchReason::Cycle { cycle } => InvalidBatchReasonPayload::Cycle {
            cycle: cycle.clone(),
        },
        InvalidBatchReason::DanglingRefs { entries } => InvalidBatchReasonPayload::DanglingRefs {
            entries: entries.clone(),
        },
        InvalidBatchReason::DuplicateNames { duplicates } => {
            InvalidBatchReasonPayload::DuplicateNames {
                duplicates: duplicates.clone(),
            }
        }
        InvalidBatchReason::SpawnedTaskMutated {
            task,
            changed_fields,
        } => InvalidBatchReasonPayload::SpawnedTaskMutated {
            task: task.clone(),
            changed_fields: changed_fields.clone(),
        },
        InvalidBatchReason::InvalidName { task, name_rule } => {
            let detail = match name_rule {
                InvalidNameDetail::RegexMismatch => InvalidNameDetailPayload::RegexMismatch,
                InvalidNameDetail::LengthOutOfRange(len) => {
                    InvalidNameDetailPayload::LengthOutOfRange { length: *len }
                }
            };
            InvalidBatchReasonPayload::InvalidName {
                task: task.clone(),
                detail,
            }
        }
        InvalidBatchReason::ReservedNameCollision { task, reserved } => {
            InvalidBatchReasonPayload::ReservedNameCollision {
                task: task.clone(),
                reserved: reserved.clone(),
            }
        }
    }
}

fn retry_reason_to_payload(reason: &InvalidRetryReason) -> InvalidRetryReasonPayload {
    match reason {
        InvalidRetryReason::NoBatchMaterialized => InvalidRetryReasonPayload::NoBatchMaterialized,
        InvalidRetryReason::EmptyChildList => InvalidRetryReasonPayload::EmptyChildList,
        InvalidRetryReason::ChildNotEligible { children } => {
            InvalidRetryReasonPayload::ChildNotEligible {
                children: children.clone(),
            }
        }
        InvalidRetryReason::UnknownChildren { children } => {
            InvalidRetryReasonPayload::UnknownChildren {
                children: children.clone(),
            }
        }
        InvalidRetryReason::ChildIsBatchParent { children } => {
            InvalidRetryReasonPayload::ChildIsBatchParent {
                children: children.clone(),
            }
        }
        InvalidRetryReason::RetryAlreadyInProgress => {
            InvalidRetryReasonPayload::RetryAlreadyInProgress
        }
        InvalidRetryReason::MixedWithOtherEvidence { extra_fields } => {
            InvalidRetryReasonPayload::MixedWithOtherEvidence {
                extra_fields: extra_fields.clone(),
            }
        }
        InvalidRetryReason::MultipleReasons { reasons } => {
            InvalidRetryReasonPayload::MultipleReasons {
                reasons: reasons.iter().map(retry_reason_to_payload).collect(),
            }
        }
    }
}

/// Pinned precedence order used by [`InvalidRetryReason::aggregate`].
/// Higher numbers are later in the ordering. `NoBatchMaterialized` and
/// `EmptyChildList` short-circuit (they both render the retry set
/// meaningless) so they aren't ranked here.
fn precedence_rank(reason: &InvalidRetryReason) -> u8 {
    match reason {
        InvalidRetryReason::UnknownChildren { .. } => 0,
        InvalidRetryReason::ChildIsBatchParent { .. } => 1,
        InvalidRetryReason::ChildNotEligible { .. } => 2,
        InvalidRetryReason::MixedWithOtherEvidence { .. } => 3,
        InvalidRetryReason::RetryAlreadyInProgress => 4,
        // These are short-circuits; if they ever land in an aggregate
        // they should sort to the front.
        InvalidRetryReason::NoBatchMaterialized => 100,
        InvalidRetryReason::EmptyChildList => 101,
        InvalidRetryReason::MultipleReasons { .. } => 200,
    }
}

impl InvalidRetryReason {
    /// Aggregate a set of violations into a single reason, respecting
    /// the Decision 9 precedence. Returns the lone reason when only
    /// one is supplied (so single-violation submissions never wrap in
    /// `MultipleReasons`), and `None` when the input is empty.
    pub fn aggregate(mut reasons: Vec<InvalidRetryReason>) -> Option<InvalidRetryReason> {
        if reasons.is_empty() {
            return None;
        }
        if reasons.len() == 1 {
            return Some(reasons.pop().expect("len == 1"));
        }
        reasons.sort_by_key(precedence_rank);
        Some(InvalidRetryReason::MultipleReasons { reasons })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ConcurrentTick ----------------------------------------------

    #[test]
    fn concurrent_tick_serializes_with_holder_pid() {
        let err = BatchError::ConcurrentTick {
            holder_pid: Some(4321),
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "concurrent_tick",
                    "holder_pid": 4321,
                }
            })
        );
    }

    #[test]
    fn concurrent_tick_serializes_without_holder_pid() {
        let err = BatchError::ConcurrentTick { holder_pid: None };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "concurrent_tick",
                    "holder_pid": null,
                }
            })
        );
    }

    #[test]
    fn concurrent_tick_exit_code_is_transient() {
        assert_eq!(
            BatchError::ConcurrentTick { holder_pid: None }.exit_code(),
            1
        );
    }

    // --- InvalidBatchDefinition --------------------------------------

    #[test]
    fn invalid_batch_definition_empty_task_list_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::EmptyTaskList,
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "empty_task_list",
                    }
                }
            })
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn invalid_batch_definition_cycle_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::Cycle {
                cycle: vec!["a".to_string(), "b".to_string(), "a".to_string()],
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "cycle",
                        "cycle": ["a", "b", "a"],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_dangling_refs_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::DanglingRefs {
                entries: vec![DanglingRef {
                    task: "b".into(),
                    unknown: "a".into(),
                }],
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "dangling_refs",
                        "entries": [{"task": "b", "unknown": "a"}],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_duplicate_names_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::DuplicateNames {
                duplicates: vec!["a".into(), "a".into()],
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "duplicate_names",
                        "duplicates": ["a", "a"],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_spawned_task_mutated_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::SpawnedTaskMutated {
                task: "t".into(),
                changed_fields: vec![MutatedField {
                    field: "template".into(),
                    spawned_value: serde_json::json!("a.md"),
                    submitted_value: serde_json::json!("b.md"),
                }],
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "spawned_task_mutated",
                        "task": "t",
                        "changed_fields": [{
                            "field": "template",
                            "spawned_value": "a.md",
                            "submitted_value": "b.md",
                        }],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_invalid_name_envelope_uses_name_rule_key() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::InvalidName {
                task: "bad name".to_string(),
                name_rule: InvalidNameDetail::RegexMismatch,
            },
        };
        let env = err.to_envelope();
        // Inner `name_rule` key must not collide with outer envelope `kind`.
        assert_eq!(env["batch"]["kind"], "invalid_batch_definition");
        assert_eq!(env["batch"]["reason"]["reason"], "invalid_name");
        assert_eq!(env["batch"]["reason"]["task"], "bad name");
        assert_eq!(env["batch"]["reason"]["name_rule"], "regex_mismatch");
    }

    #[test]
    fn invalid_batch_definition_invalid_name_length_out_of_range_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::InvalidName {
                task: "a".repeat(65),
                name_rule: InvalidNameDetail::LengthOutOfRange(65),
            },
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["name_rule"], "length_out_of_range");
        assert_eq!(env["batch"]["reason"]["length"], 65);
    }

    #[test]
    fn invalid_batch_definition_reserved_name_collision_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::ReservedNameCollision {
                task: "retry_failed".into(),
                reserved: "retry_failed".into(),
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "reason": "reserved_name_collision",
                        "task": "retry_failed",
                        "reserved": "retry_failed",
                    }
                }
            })
        );
    }

    // --- LimitExceeded -----------------------------------------------

    #[test]
    fn limit_exceeded_tasks_envelope_omits_task_field() {
        let err = BatchError::LimitExceeded {
            which: LimitKind::Tasks,
            limit: 1000,
            actual: 1001,
            task: None,
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "limit_exceeded",
                    "which": "tasks",
                    "limit": 1000,
                    "actual": 1001,
                }
            })
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn limit_exceeded_waits_on_envelope_carries_task() {
        let err = BatchError::LimitExceeded {
            which: LimitKind::WaitsOn,
            limit: 10,
            actual: 11,
            task: Some("head".to_string()),
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "limit_exceeded",
                    "which": "waits_on",
                    "limit": 10,
                    "actual": 11,
                    "task": "head",
                }
            })
        );
    }

    #[test]
    fn limit_exceeded_payload_bytes_variant() {
        let err = BatchError::LimitExceeded {
            which: LimitKind::PayloadBytes,
            limit: 1_048_576,
            actual: 2_000_000,
            task: None,
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["which"], "payload_bytes");
    }

    // --- Template errors ---------------------------------------------

    #[test]
    fn template_not_found_envelope_round_trips() {
        let err = BatchError::TemplateNotFound {
            task: "t".into(),
            path: "child.md".into(),
            paths_tried: vec!["/a/child.md".into(), "/b/child.md".into()],
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "template_not_found",
                    "task": "t",
                    "path": "child.md",
                    "paths_tried": ["/a/child.md", "/b/child.md"],
                }
            })
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn template_not_found_omits_empty_paths_tried() {
        let err = BatchError::TemplateNotFound {
            task: "t".into(),
            path: "child.md".into(),
            paths_tried: vec![],
        };
        let env = err.to_envelope();
        assert!(
            env["batch"].get("paths_tried").is_none(),
            "paths_tried must be omitted when empty, not null; got {env:?}"
        );
    }

    #[test]
    fn template_compile_failed_envelope_uses_typed_compile_error() {
        let err = BatchError::TemplateCompileFailed {
            task: "t".into(),
            path: "child.md".into(),
            compile_error: CompileError::from_rule_tag("E1", "from_field must not be empty"),
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["kind"], "template_compile_failed");
        assert_eq!(env["batch"]["task"], "t");
        assert_eq!(env["batch"]["compile_error"]["kind"], "e1");
        assert_eq!(
            env["batch"]["compile_error"]["message"],
            "from_field must not be empty"
        );
        // No location supplied → field must be omitted.
        assert!(env["batch"]["compile_error"].get("location").is_none());
    }

    // --- BackendError + SpawnFailed -----------------------------------

    #[test]
    fn backend_error_retryable_envelope() {
        let err = BatchError::BackendError {
            message: "list failed".into(),
            retryable: true,
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "backend_error",
                    "message": "list failed",
                    "retryable": true,
                }
            })
        );
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn backend_error_nonretryable_exit_code_is_three() {
        let err = BatchError::BackendError {
            message: "corrupt".into(),
            retryable: false,
        };
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn spawn_failed_envelope_round_trips() {
        let err = BatchError::SpawnFailed {
            task: "a".into(),
            kind: SpawnErrorKind::Collision,
            message: "exists".into(),
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["kind"], "spawn_failed");
        assert_eq!(env["batch"]["task"], "a");
        assert_eq!(env["batch"]["spawn_kind"], "collision");
    }

    // --- InvalidRetryRequest -----------------------------------------

    #[test]
    fn invalid_retry_unknown_children_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::UnknownChildren {
                children: vec!["ghost".into()],
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_retry_request",
                    "reason": {
                        "reason": "unknown_children",
                        "children": ["ghost"],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_retry_child_is_batch_parent_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::ChildIsBatchParent {
                children: vec!["nested".into()],
            },
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["reason"], "child_is_batch_parent");
        assert_eq!(
            env["batch"]["reason"]["children"],
            serde_json::json!(["nested"])
        );
    }

    #[test]
    fn invalid_retry_child_not_eligible_uses_typed_current_outcome() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::ChildNotEligible {
                children: vec![ChildEligibility {
                    name: "alpha".into(),
                    current_outcome: ChildOutcome::Pending,
                }],
            },
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["reason"], "child_not_eligible");
        assert_eq!(
            env["batch"]["reason"]["children"][0]["current_outcome"],
            "pending"
        );
    }

    #[test]
    fn invalid_retry_mixed_with_other_evidence_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::MixedWithOtherEvidence {
                extra_fields: vec!["notes".into()],
            },
        };
        let env = err.to_envelope();
        assert_eq!(
            env["batch"]["reason"]["reason"],
            "mixed_with_other_evidence"
        );
    }

    #[test]
    fn invalid_retry_retry_already_in_progress_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::RetryAlreadyInProgress,
        };
        let env = err.to_envelope();
        assert_eq!(
            env["batch"]["reason"]["reason"],
            "retry_already_in_progress"
        );
    }

    #[test]
    fn invalid_retry_no_batch_materialized_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::NoBatchMaterialized,
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["reason"], "no_batch_materialized");
    }

    #[test]
    fn invalid_retry_empty_child_list_envelope() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::EmptyChildList,
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["reason"], "empty_child_list");
    }

    // --- Precedence & MultipleReasons --------------------------------

    #[test]
    fn aggregate_single_reason_returns_unwrapped() {
        let one = InvalidRetryReason::RetryAlreadyInProgress;
        let agg = InvalidRetryReason::aggregate(vec![one.clone()]).unwrap();
        assert_eq!(agg, one);
    }

    #[test]
    fn aggregate_empty_returns_none() {
        assert!(InvalidRetryReason::aggregate(vec![]).is_none());
    }

    #[test]
    fn aggregate_preserves_pinned_precedence_order() {
        // Submit out-of-order; aggregate must sort to
        // UnknownChildren → ChildIsBatchParent → ChildNotEligible →
        // MixedWithOtherEvidence → RetryAlreadyInProgress.
        let agg = InvalidRetryReason::aggregate(vec![
            InvalidRetryReason::RetryAlreadyInProgress,
            InvalidRetryReason::MixedWithOtherEvidence {
                extra_fields: vec!["notes".into()],
            },
            InvalidRetryReason::ChildNotEligible {
                children: vec![ChildEligibility {
                    name: "x".into(),
                    current_outcome: ChildOutcome::Pending,
                }],
            },
            InvalidRetryReason::ChildIsBatchParent {
                children: vec!["y".into()],
            },
            InvalidRetryReason::UnknownChildren {
                children: vec!["z".into()],
            },
        ])
        .unwrap();

        let ordered = match agg {
            InvalidRetryReason::MultipleReasons { reasons } => reasons,
            other => panic!("expected MultipleReasons, got {:?}", other),
        };
        assert!(matches!(
            ordered[0],
            InvalidRetryReason::UnknownChildren { .. }
        ));
        assert!(matches!(
            ordered[1],
            InvalidRetryReason::ChildIsBatchParent { .. }
        ));
        assert!(matches!(
            ordered[2],
            InvalidRetryReason::ChildNotEligible { .. }
        ));
        assert!(matches!(
            ordered[3],
            InvalidRetryReason::MixedWithOtherEvidence { .. }
        ));
        assert!(matches!(
            ordered[4],
            InvalidRetryReason::RetryAlreadyInProgress
        ));
    }

    #[test]
    fn multiple_reasons_envelope_serializes_nested_list() {
        let err = BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::MultipleReasons {
                reasons: vec![
                    InvalidRetryReason::UnknownChildren {
                        children: vec!["z".into()],
                    },
                    InvalidRetryReason::ChildIsBatchParent {
                        children: vec!["y".into()],
                    },
                ],
            },
        };
        let env = err.to_envelope();
        assert_eq!(env["batch"]["reason"]["reason"], "multiple_reasons");
        let reasons = &env["batch"]["reason"]["reasons"];
        assert_eq!(reasons[0]["reason"], "unknown_children");
        assert_eq!(reasons[1]["reason"], "child_is_batch_parent");
    }

    // --- Snapshot --------------------------------------------------------

    /// Schema snapshot: every top-level `kind` variant must render
    /// with the shape agents depend on. Keeping one test in the
    /// batch_error module (instead of only the integration snapshot in
    /// `tests/`) ensures the check runs with unit-test filters too.
    #[test]
    fn schema_snapshot_all_top_level_kinds() {
        let kinds: Vec<&str> = vec![
            "concurrent_tick",
            "invalid_batch_definition",
            "limit_exceeded",
            "template_not_found",
            "template_compile_failed",
            "backend_error",
            "spawn_failed",
            "invalid_retry_request",
        ];
        let samples: Vec<BatchError> = vec![
            BatchError::ConcurrentTick { holder_pid: None },
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::EmptyTaskList,
            },
            BatchError::LimitExceeded {
                which: LimitKind::Tasks,
                limit: 1,
                actual: 2,
                task: None,
            },
            BatchError::TemplateNotFound {
                task: "t".into(),
                path: "p".into(),
                paths_tried: vec![],
            },
            BatchError::TemplateCompileFailed {
                task: "t".into(),
                path: "p".into(),
                compile_error: CompileError::from_rule_tag("E1", "m"),
            },
            BatchError::BackendError {
                message: "m".into(),
                retryable: true,
            },
            BatchError::SpawnFailed {
                task: "t".into(),
                kind: SpawnErrorKind::IoError,
                message: "m".into(),
            },
            BatchError::InvalidRetryRequest {
                reason: InvalidRetryReason::NoBatchMaterialized,
            },
        ];
        assert_eq!(kinds.len(), samples.len());
        for (expected_kind, sample) in kinds.iter().zip(samples.iter()) {
            let env = sample.to_envelope();
            assert_eq!(env["action"], "error");
            assert_eq!(
                env["batch"]["kind"], *expected_kind,
                "sample {:?} did not emit kind={}",
                sample, expected_kind
            );
        }
    }
}
