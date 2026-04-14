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
/// Only `ConcurrentTick` is implemented in Issue #2. Additional
/// variants will be added in Issue #10 (invalid batch definitions,
/// per-parent limits exceeded, scheduler failures, ...). When those
/// land, extend this enum -- the JSON envelope produced by
/// [`BatchError::to_envelope`] must remain a top-level object of the
/// form `{"action": "error", "batch": {"kind": <snake_case>, ...}}`.
// TODO(#10): extend with InvalidBatchDefinition, LimitExceeded, etc.
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
}

/// Serializable "batch" payload embedded inside the error envelope.
///
/// Rendered under the `batch` key of the outer `{"action": "error",
/// "batch": ...}` object so the two top-level keys remain stable while
/// individual variants evolve.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BatchErrorPayload {
    ConcurrentTick { holder_pid: Option<u32> },
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
        };
        serde_json::to_value(BatchErrorEnvelope {
            action: "error",
            batch: payload,
        })
        .expect("BatchErrorEnvelope is always serializable")
    }

    /// Process exit code for this error.
    ///
    /// Aligned with `NextErrorCode::ConcurrentAccess` (exit 1,
    /// transient) so retry heuristics in callers stay uniform across
    /// the two error envelopes while the batch machinery is wired up.
    pub fn exit_code(&self) -> i32 {
        match self {
            BatchError::ConcurrentTick { .. } => 1,
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
}
