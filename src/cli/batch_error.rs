//! Batch-workflow error envelope.
//!
//! This module introduces the minimum slice of the batch-error type
//! required by Issue #2 -- a single `ConcurrentTick` variant surfaced
//! when `handle_next` fails to acquire the advisory flock on a
//! batch-scoped parent's state file.
//!
//! Issue #10 will extend [`BatchError`] with additional variants
//! (`InvalidBatchDefinition`, `LimitExceeded`, ...). The serialization
//! contract defined here (a top-level `{"action": "error", "batch":
//! {"kind": ..., ...}}` envelope) is intentionally stable so those
//! future variants can plug in without reshaping the JSON shape agents
//! already parse.
//!
//! # Envelope shape
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
//! A future `F_OFD_GETLK`-based probe may populate it without breaking
//! the envelope shape.

use serde::Serialize;

/// Errors raised by batch-workflow machinery.
///
/// `ConcurrentTick` was introduced in Issue #2; Issue #9 adds the
/// `InvalidBatchDefinition` and `LimitExceeded` families that carry
/// runtime-rule (R0-R9) rejections. Issue #10 will complete the
/// envelope with the remaining typed variants. When those land,
/// extend this enum -- the JSON envelope produced by
/// [`BatchError::to_envelope`] must remain a top-level object of the
/// form `{"action": "error", "batch": {"kind": <snake_case>, ...}}`.
// TODO(#10): extend with TemplateCompileFailed, etc., and wire the
// richer envelope shape (error.batch.reason, error.batch.limits, ...).
#[derive(Debug, Clone, PartialEq)]
pub enum BatchError {
    /// A concurrent `koto next` invocation holds the advisory flock on
    /// this batch parent's state file. The current tick aborts rather
    /// than block so an agent (or CI runner) receives an immediate,
    /// structured response instead of a hang.
    ///
    /// `holder_pid` is best-effort. Today it is always `None`; the
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
    ///
    /// Promoted to a top-level variant instead of an
    /// `InvalidBatchReason` member so callers (and the Issue #10
    /// envelope) can branch on "limit" vs. "structural rule" without
    /// peeking inside a sub-enum.
    LimitExceeded {
        which: LimitKind,
        limit: u32,
        actual: u32,
        task: Option<String>,
    },
}

/// Structural rejection reasons for a batch submission.
///
/// One variant per runtime rule (R0, R3, R4, R5, R8, R9). R6 is the
/// [`BatchError::LimitExceeded`] variant on the outer enum. R1, R2,
/// and R7 are not pre-append rules (they run at spawn time or are
/// enforced by the session layer) so they have no entry here.
///
/// TODO(#10): expand with the remaining variants named in the
/// Decision 11 table and pin the wire format via a schema test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidBatchReason {
    /// R0: `tasks.len() >= 1` failed.
    EmptyTaskList,
    /// R3: `waits_on` contains a cycle. `cycle` lists the task names
    /// in the order they close the loop (first element repeats at the
    /// end implicitly).
    Cycle { cycle: Vec<String> },
    /// R4: a `waits_on` entry names a task absent from the submission.
    UnknownWaitsOn { task: String, unknown: String },
    /// R5: two submitted tasks share a name.
    DuplicateTaskName { task: String },
    /// R8: the submitted entry for an already-spawned task disagrees
    /// with the `spawn_entry` snapshot. `diff` is a human-readable
    /// summary of the differing fields; richer structure is deferred
    /// to Issue #10.
    SpawnedTaskMutated { task: String, diff: String },
    /// R9: a task name fails the `^[A-Za-z0-9_-]+$` regex, is outside
    /// the 1..=64 length band, or is on the reserved list.
    InvalidName {
        task: String,
        kind: InvalidNameDetail,
    },
}

/// Which hard limit (R6) was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    /// `tasks.len() > 1000`.
    Tasks,
    /// Some task's `waits_on.len() > 10`.
    WaitsOn,
    /// DAG depth, measured as node count along the longest root-to-
    /// leaf path, exceeded 50.
    Depth,
}

/// Why R9 rejected a task name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidNameDetail {
    /// Did not match `^[A-Za-z0-9_-]+$`.
    RegexMismatch,
    /// Length was outside the 1..=64 band. Carries the observed length.
    LengthOutOfRange(usize),
    /// Name was on the reserved list (e.g. `retry_failed`,
    /// `cancel_tasks`). Carries the offending name for logging.
    Reserved(String),
}

/// Serializable "batch" payload embedded inside the error envelope.
///
/// Rendered under the `batch` key of the outer `{"action": "error",
/// "batch": ...}` object so the two top-level keys remain stable while
/// individual variants evolve.
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
}

/// Wire payload for [`InvalidBatchReason`].
///
/// Kept as a sibling type so Issue #10 can reshape the outer envelope
/// (e.g. add `error.batch.reason_detail`) without touching every
/// validator call site. The `rename` on `SpawnedTaskMutated` matches
/// the design document's `InvalidBatchReason::SpawnedTaskMutated`
/// wire identifier.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InvalidBatchReasonPayload {
    EmptyTaskList,
    Cycle {
        cycle: Vec<String>,
    },
    UnknownWaitsOn {
        task: String,
        unknown: String,
    },
    DuplicateTaskName {
        task: String,
    },
    SpawnedTaskMutated {
        task: String,
        diff: String,
    },
    InvalidName {
        task: String,
        #[serde(flatten)]
        detail: InvalidNameDetailPayload,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "name_rule", rename_all = "snake_case")]
enum InvalidNameDetailPayload {
    RegexMismatch,
    LengthOutOfRange { length: usize },
    Reserved { name: String },
}

/// Full envelope written to stdout when a batch-scoped tick fails.
///
/// A flat `serde_json::Value` would work equally well at the call site,
/// but a named struct documents the contract and keeps the two top-
/// level keys in one place for Issue #10 to extend.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct BatchErrorEnvelope {
    action: &'static str,
    batch: BatchErrorPayload,
}

impl BatchError {
    /// Return the JSON envelope for this error, ready to serialize with
    /// `serde_json::to_string`.
    ///
    /// Callers that need to write to stdout should go through this
    /// method rather than hand-rolling a JSON object: it keeps the
    /// envelope shape centralized for Issue #10.
    pub fn to_envelope(&self) -> serde_json::Value {
        let payload = match self {
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
        };
        serde_json::to_value(BatchErrorEnvelope {
            action: "error",
            batch: payload,
        })
        .expect("BatchErrorEnvelope is always serializable")
    }

    /// Process exit code for this error.
    ///
    /// `ConcurrentTick` is transient (exit 1). Pre-append rejections
    /// (`InvalidBatchDefinition`, `LimitExceeded`) are caller errors
    /// (exit 2) so agents can distinguish "retry later" from "fix the
    /// submission and resend".
    pub fn exit_code(&self) -> i32 {
        match self {
            BatchError::ConcurrentTick { .. } => 1,
            BatchError::InvalidBatchDefinition { .. } | BatchError::LimitExceeded { .. } => 2,
        }
    }
}

fn reason_to_payload(reason: &InvalidBatchReason) -> InvalidBatchReasonPayload {
    match reason {
        InvalidBatchReason::EmptyTaskList => InvalidBatchReasonPayload::EmptyTaskList,
        InvalidBatchReason::Cycle { cycle } => InvalidBatchReasonPayload::Cycle {
            cycle: cycle.clone(),
        },
        InvalidBatchReason::UnknownWaitsOn { task, unknown } => {
            InvalidBatchReasonPayload::UnknownWaitsOn {
                task: task.clone(),
                unknown: unknown.clone(),
            }
        }
        InvalidBatchReason::DuplicateTaskName { task } => {
            InvalidBatchReasonPayload::DuplicateTaskName { task: task.clone() }
        }
        InvalidBatchReason::SpawnedTaskMutated { task, diff } => {
            InvalidBatchReasonPayload::SpawnedTaskMutated {
                task: task.clone(),
                diff: diff.clone(),
            }
        }
        InvalidBatchReason::InvalidName { task, kind } => {
            let detail = match kind {
                InvalidNameDetail::RegexMismatch => InvalidNameDetailPayload::RegexMismatch,
                InvalidNameDetail::LengthOutOfRange(len) => {
                    InvalidNameDetailPayload::LengthOutOfRange { length: *len }
                }
                InvalidNameDetail::Reserved(name) => {
                    InvalidNameDetailPayload::Reserved { name: name.clone() }
                }
            };
            InvalidBatchReasonPayload::InvalidName {
                task: task.clone(),
                detail,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_tick_serializes_with_holder_pid() {
        let err = BatchError::ConcurrentTick {
            holder_pid: Some(4321),
        };
        let envelope = err.to_envelope();
        assert_eq!(
            envelope,
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
        let envelope = err.to_envelope();
        assert_eq!(
            envelope,
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
                        "kind": "empty_task_list",
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
                        "kind": "cycle",
                        "cycle": ["a", "b", "a"],
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_invalid_name_envelope_flattens_detail() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::InvalidName {
                task: "bad name".to_string(),
                kind: InvalidNameDetail::RegexMismatch,
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "kind": "invalid_name",
                        "task": "bad name",
                        "name_rule": "regex_mismatch",
                    }
                }
            })
        );
    }

    #[test]
    fn invalid_batch_definition_invalid_name_reserved_envelope() {
        let err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::InvalidName {
                task: "retry_failed".to_string(),
                kind: InvalidNameDetail::Reserved("retry_failed".to_string()),
            },
        };
        assert_eq!(
            err.to_envelope(),
            serde_json::json!({
                "action": "error",
                "batch": {
                    "kind": "invalid_batch_definition",
                    "reason": {
                        "kind": "invalid_name",
                        "task": "retry_failed",
                        "name_rule": "reserved",
                        "name": "retry_failed",
                    }
                }
            })
        );
    }

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
    fn limit_exceeded_depth_envelope() {
        let err = BatchError::LimitExceeded {
            which: LimitKind::Depth,
            limit: 50,
            actual: 51,
            task: None,
        };
        let envelope = err.to_envelope();
        assert_eq!(envelope["batch"]["kind"], "limit_exceeded");
        assert_eq!(envelope["batch"]["which"], "depth");
        assert_eq!(envelope["batch"]["limit"], 50);
        assert_eq!(envelope["batch"]["actual"], 51);
    }
}
