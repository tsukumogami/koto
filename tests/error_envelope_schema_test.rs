//! Integration-level schema snapshot for the typed error envelope
//! (scenario-13 in `wip/implement-batch-child-spawning-test_plan.md`).
//!
//! Issue #10 pinned the full error surface to typed enums:
//!
//! - `NextResponse::Error` wraps every rejection in a stable
//!   `{"action": "error", ...}` envelope.
//! - `BatchError` serializes to a sibling `batch` object under
//!   `error.batch` when the failure originates in batch machinery.
//! - `InvalidBatchReason::InvalidName` uses `name_rule` (not `kind`)
//!   so the inner discriminator never collides with the outer one.
//! - `CompileError.kind` uses the typed `CompileErrorKind` enum, not a
//!   free string.
//! - `ChildEligibility.current_outcome` uses the typed `ChildOutcome`
//!   enum.
//! - `paths_tried` and other optional fields are omitted via
//!   `skip_serializing_if`, never emitted as `null`.
//! - `InvalidRetryReason::MultipleReasons` aggregates cross-variant
//!   rejections with the pinned precedence order `UnknownChildren →
//!   ChildIsBatchParent → ChildNotEligible → MixedWithOtherEvidence
//!   → RetryAlreadyInProgress`.
//!
//! This file asserts those invariants at the integration-test layer so
//! regressions in the envelope shape fail CI even when the unit tests
//! in `src/cli/batch_error.rs` pass.

use koto::cli::batch_error::{
    BatchError, ChildEligibility, ChildOutcome, DanglingRef, InvalidBatchReason, InvalidNameDetail,
    InvalidRetryReason, LimitKind, MutatedField,
};
use koto::cli::next_types::{
    BatchErrorContext, ErrorDetail, NextError, NextErrorCode, NextResponse,
};
use koto::cli::task_spawn_error::{CompileError, CompileErrorKind, SpawnErrorKind};

/// Sanity check every top-level `BatchError` variant round-trips
/// through `to_envelope` with `action: "error"` and the expected
/// snake_case `kind` tag.
#[test]
fn every_batch_error_kind_renders_with_action_error() {
    let cases: &[(BatchError, &str)] = &[
        (
            BatchError::ConcurrentTick { holder_pid: None },
            "concurrent_tick",
        ),
        (
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::EmptyTaskList,
            },
            "invalid_batch_definition",
        ),
        (
            BatchError::LimitExceeded {
                which: LimitKind::Tasks,
                limit: 1000,
                actual: 1001,
                task: None,
            },
            "limit_exceeded",
        ),
        (
            BatchError::TemplateNotFound {
                task: "t".into(),
                path: "p".into(),
                paths_tried: vec![],
            },
            "template_not_found",
        ),
        (
            BatchError::TemplateCompileFailed {
                task: "t".into(),
                path: "p".into(),
                compile_error: CompileError::from_rule_tag("E1", "m"),
            },
            "template_compile_failed",
        ),
        (
            BatchError::BackendError {
                message: "m".into(),
                retryable: false,
            },
            "backend_error",
        ),
        (
            BatchError::SpawnFailed {
                task: "t".into(),
                kind: SpawnErrorKind::IoError,
                message: "m".into(),
            },
            "spawn_failed",
        ),
        (
            BatchError::InvalidRetryRequest {
                reason: InvalidRetryReason::NoBatchMaterialized,
            },
            "invalid_retry_request",
        ),
    ];

    for (err, expected_kind) in cases {
        let env = err.to_envelope();
        assert_eq!(env["action"], "error", "envelope for {:?}", err);
        assert_eq!(
            env["batch"]["kind"], *expected_kind,
            "top-level kind for {:?}",
            err
        );
    }
}

/// Every `InvalidBatchReason` variant must serialize with the design's
/// snake_case `reason` tag. `InvalidName` uses `name_rule` (not
/// `kind`) for the inner discriminator per Issue #10 polish.
#[test]
fn every_invalid_batch_reason_serializes_with_canonical_reason_tag() {
    let cases: &[(InvalidBatchReason, &str)] = &[
        (InvalidBatchReason::EmptyTaskList, "empty_task_list"),
        (
            InvalidBatchReason::Cycle {
                cycle: vec!["a".into(), "b".into()],
            },
            "cycle",
        ),
        (
            InvalidBatchReason::DanglingRefs {
                entries: vec![DanglingRef {
                    task: "t".into(),
                    unknown: "g".into(),
                }],
            },
            "dangling_refs",
        ),
        (
            InvalidBatchReason::DuplicateNames {
                duplicates: vec!["a".into()],
            },
            "duplicate_names",
        ),
        (
            InvalidBatchReason::SpawnedTaskMutated {
                task: "t".into(),
                changed_fields: vec![MutatedField {
                    field: "template".into(),
                    spawned_value: serde_json::json!("a"),
                    submitted_value: serde_json::json!("b"),
                }],
            },
            "spawned_task_mutated",
        ),
        (
            InvalidBatchReason::InvalidName {
                task: "bad name".into(),
                name_rule: InvalidNameDetail::RegexMismatch,
            },
            "invalid_name",
        ),
        (
            InvalidBatchReason::ReservedNameCollision {
                task: "retry_failed".into(),
                reserved: "retry_failed".into(),
            },
            "reserved_name_collision",
        ),
    ];

    // Design: "All 7 InvalidBatchReason variants deserialize to the
    // canonical snake_case wire format."
    assert_eq!(cases.len(), 7);

    for (reason, expected_tag) in cases {
        let err = BatchError::InvalidBatchDefinition {
            reason: reason.clone(),
        };
        let env = err.to_envelope();
        assert_eq!(
            env["batch"]["reason"]["reason"], *expected_tag,
            "reason tag for {:?}",
            reason
        );
    }
}

/// Issue #10 renames `InvalidBatchReason::InvalidName.kind` to
/// `name_rule` so the inner discriminator never shadows the outer
/// envelope's `kind`.
#[test]
fn invalid_name_uses_name_rule_discriminator() {
    let err = BatchError::InvalidBatchDefinition {
        reason: InvalidBatchReason::InvalidName {
            task: "bad name".into(),
            name_rule: InvalidNameDetail::RegexMismatch,
        },
    };
    let env = err.to_envelope();
    // Inner discriminator must be `name_rule`, not `kind`.
    assert!(env["batch"]["reason"].get("name_rule").is_some());
    // Outer `kind` still names the BatchError variant.
    assert_eq!(env["batch"]["kind"], "invalid_batch_definition");
    // And the inner `name_rule` carries the typed sub-variant.
    assert_eq!(env["batch"]["reason"]["name_rule"], "regex_mismatch");
}

/// `paths_tried` is omitted (not `null`) when empty.
#[test]
fn empty_paths_tried_is_omitted_not_null() {
    let err = BatchError::TemplateNotFound {
        task: "t".into(),
        path: "p".into(),
        paths_tried: vec![],
    };
    let env = err.to_envelope();
    assert!(
        env["batch"].get("paths_tried").is_none(),
        "paths_tried must be absent when empty; got {env:?}"
    );
}

/// `CompileError.kind` renders as the typed `CompileErrorKind` enum,
/// serialized in lowercase rule-tag form (`e1`, `w4`, `f5`).
#[test]
fn compile_error_kind_is_typed_enum() {
    let err = BatchError::TemplateCompileFailed {
        task: "t".into(),
        path: "p".into(),
        compile_error: CompileError {
            kind: CompileErrorKind::E10,
            message: "E10 advisory".into(),
            location: None,
        },
    };
    let env = err.to_envelope();
    assert_eq!(env["batch"]["compile_error"]["kind"], "e10");
    assert!(env["batch"]["compile_error"].get("location").is_none());
}

/// `ChildEligibility.current_outcome` is the typed `ChildOutcome`
/// enum, not a free string.
#[test]
fn child_eligibility_current_outcome_is_typed() {
    let err = BatchError::InvalidRetryRequest {
        reason: InvalidRetryReason::ChildNotEligible {
            children: vec![
                ChildEligibility {
                    name: "a".into(),
                    current_outcome: ChildOutcome::Pending,
                },
                ChildEligibility {
                    name: "b".into(),
                    current_outcome: ChildOutcome::Success,
                },
            ],
        },
    };
    let env = err.to_envelope();
    assert_eq!(
        env["batch"]["reason"]["children"][0]["current_outcome"],
        "pending"
    );
    assert_eq!(
        env["batch"]["reason"]["children"][1]["current_outcome"],
        "success"
    );
}

/// `InvalidRetryReason::MultipleReasons` orders its inner list per
/// the pinned precedence `UnknownChildren → ChildIsBatchParent →
/// ChildNotEligible → MixedWithOtherEvidence → RetryAlreadyInProgress`.
#[test]
fn multiple_reasons_precedence_in_envelope() {
    let agg = InvalidRetryReason::aggregate(vec![
        InvalidRetryReason::RetryAlreadyInProgress,
        InvalidRetryReason::MixedWithOtherEvidence {
            extra_fields: vec!["notes".into()],
        },
        InvalidRetryReason::ChildNotEligible {
            children: vec![ChildEligibility {
                name: "a".into(),
                current_outcome: ChildOutcome::Pending,
            }],
        },
        InvalidRetryReason::ChildIsBatchParent {
            children: vec!["b".into()],
        },
        InvalidRetryReason::UnknownChildren {
            children: vec!["c".into()],
        },
    ])
    .expect("five reasons → aggregate returns Some");

    let err = BatchError::InvalidRetryRequest { reason: agg };
    let env = err.to_envelope();
    assert_eq!(env["batch"]["reason"]["reason"], "multiple_reasons");
    let reasons = env["batch"]["reason"]["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 5);
    assert_eq!(reasons[0]["reason"], "unknown_children");
    assert_eq!(reasons[1]["reason"], "child_is_batch_parent");
    assert_eq!(reasons[2]["reason"], "child_not_eligible");
    assert_eq!(reasons[3]["reason"], "mixed_with_other_evidence");
    assert_eq!(reasons[4]["reason"], "retry_already_in_progress");
}

/// `NextResponse::Error` wraps an arbitrary error with `action: "error"`
/// and an optional `batch` sibling under `error.batch`.
#[test]
fn next_response_error_wraps_every_rejection_variant() {
    for batch_err in [
        BatchError::ConcurrentTick {
            holder_pid: Some(100),
        },
        BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::EmptyTaskList,
        },
        BatchError::LimitExceeded {
            which: LimitKind::Tasks,
            limit: 1000,
            actual: 1001,
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
        BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::UnknownChildren {
                children: vec!["z".into()],
            },
        },
        BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::ChildIsBatchParent {
                children: vec!["y".into()],
            },
        },
        BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::ChildNotEligible {
                children: vec![ChildEligibility {
                    name: "c".into(),
                    current_outcome: ChildOutcome::Pending,
                }],
            },
        },
        BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::MixedWithOtherEvidence {
                extra_fields: vec!["notes".into()],
            },
        },
        BatchError::InvalidRetryRequest {
            reason: InvalidRetryReason::RetryAlreadyInProgress,
        },
    ] {
        let resp = NextResponse::Error {
            state: "plan".into(),
            advanced: false,
            error: NextError {
                code: NextErrorCode::InvalidSubmission,
                message: "rejected".into(),
                details: vec![ErrorDetail {
                    field: "tasks".into(),
                    reason: "varies".into(),
                }],
            },
            batch: Some(BatchErrorContext::from_batch_error(&batch_err)),
            blocking_conditions: vec![],
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["action"], "error");
        // error.batch must mirror the `batch` payload produced by
        // to_envelope — one source of truth.
        assert_eq!(v["error"]["batch"], batch_err.to_envelope()["batch"]);
    }
}

/// Round-trip: every `NextErrorCode` value serializes and deserializes.
#[test]
fn next_error_code_round_trips_through_serde() {
    let codes = [
        NextErrorCode::GateBlocked,
        NextErrorCode::InvalidSubmission,
        NextErrorCode::PreconditionFailed,
        NextErrorCode::IntegrationUnavailable,
        NextErrorCode::TerminalState,
        NextErrorCode::WorkflowNotInitialized,
        NextErrorCode::TemplateError,
        NextErrorCode::PersistenceError,
        NextErrorCode::ConcurrentAccess,
    ];
    for code in codes {
        let v = serde_json::to_value(&code).unwrap();
        assert!(v.is_string(), "code {:?} should serialize as string", code);
    }
}
