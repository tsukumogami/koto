//! Per-write epoch fence (PRD R43, Decision 2 sub-question 1).
//!
//! Every `koto next --with-data` write to a CHILD's log must present
//! its `dispatch_epoch` and have it validated against the value on the
//! child's [`StateFileHeader`] before any persistence call. On
//! mismatch the call returns
//! [`EngineError::EpochFenceViolation`] and exits 65 (`EX_DATAERR`);
//! the on-disk log is untouched.
//!
//! ## Why the fence exists
//!
//! When a child is redelegated (Issue 11 cases 3b/3c bump
//! `dispatch_epoch`), the ORIGINAL dispatched agent may still be alive
//! on the substrate and may attempt terminal writes against the
//! now-redelegated child. The fence rejects those writes — the
//! original agent presents the pre-bump epoch, the fence sees
//! `expected = N+1`, `presented = N`, and returns
//! `EpochFenceViolation` BEFORE any persistence call. The new
//! dispatched agent's write proceeds with the bumped epoch, the
//! protocol's exactly-one-winner property holds across the
//! redelegation boundary.
//!
//! The fence and the sidecar L3 unlink (Issue 11) are independent
//! mechanisms enforcing the same invariant from two directions:
//!
//! * L3 sidecar prevents a dead coordinator's CLAIM from blocking a
//!   new dispatch (claim-time mutual exclusion).
//! * The fence prevents a dead coordinator's dispatched SUBAGENT
//!   from corrupting the redelegated child's terminal state
//!   (write-time mutual exclusion).
//!
//! Removing either leaves a correctness hole the other cannot cover.
//!
//! ## Trust boundary
//!
//! The fence is server-side at the koto CLI boundary, NOT in the
//! `SubagentStop` hook itself. The hook is "dumb fire-and-write" by
//! design (Decision 4) — `child_session_id` and `dispatch_epoch` are
//! baked at spawn time. The koto CLI's validate-then-persist
//! discipline is the trust boundary; the hook does not re-read the
//! header for freshness. This is the same discipline as
//! [`crate::engine::types::ValidatedSessionId`]: trust at the
//! boundary, not at the source.
//!
//! ## Scope
//!
//! Only CHILD-log writes (`header.parent_workflow.is_some()`) are
//! under the fence. Coordinator-side writes against the parent
//! workflow's own log are NOT subject to validation (R43's wording:
//! "every writer to a CHILD'S log"). Coordinator logs are protected
//! by the single-writer-expected discipline; they cannot be
//! redelegated in the same sense.

use crate::engine::errors::EngineError;
use crate::engine::types::{StateFileHeader, ValidatedSessionId};

/// Validate a presented `dispatch_epoch` against the child's
/// [`StateFileHeader`].
///
/// Returns:
/// - `Ok(())` when `presented_epoch == header.dispatch_epoch`.
/// - `Err(EngineError::EpochFenceViolation { child_session_id,
///   expected, presented })` on any of:
///   - `presented_epoch` is `None` (missing-flag is implicit mismatch
///     per the design's wording: a writer that does not present an
///     epoch is rejected as if it had presented the wrong epoch).
///   - `presented_epoch == Some(n)` where `n != header.dispatch_epoch`
///     — strict equality, NOT lower-bound. Both a future-epoch (where
///     `presented` exceeds `expected`) and a stale-epoch (where
///     `presented` is less than `expected`) reject; the strict-
///     equality discipline catches a buggy spawner that bakes the
///     wrong epoch.
///
/// The validator is pure — it does NO I/O. The caller is responsible
/// for reading the header and ensuring the check fires BEFORE any
/// persistence write. Out-of-order callers risk leaving partial
/// state on disk on rejection.
pub fn validate_epoch(
    child_session_id: &ValidatedSessionId,
    header: &StateFileHeader,
    presented_epoch: Option<u32>,
) -> Result<(), EngineError> {
    let expected = header.dispatch_epoch;
    match presented_epoch {
        Some(p) if p == expected => Ok(()),
        Some(p) => Err(EngineError::EpochFenceViolation {
            child_session_id: child_session_id.as_str().to_string(),
            expected,
            presented: p,
        }),
        None => Err(EngineError::EpochFenceViolation {
            child_session_id: child_session_id.as_str().to_string(),
            expected,
            // A missing presentation reads as "presented 0" only for
            // error message purposes — header.dispatch_epoch == 0 is
            // a legitimate state (initial dispatch), so we need a
            // sentinel that's distinguishable. We use u32::MAX so the
            // error message reads "expected N, presented 4294967295"
            // — operators reading it see "this writer didn't present
            // any epoch at all" rather than the ambiguous "presented
            // 0 against a fresh-dispatch child (expected 0)".
            presented: u32::MAX,
        }),
    }
}

/// Returns `true` when the child-log fence applies to a write
/// against `header`.
///
/// The fence is scoped specifically to request-store dispatched
/// children — those whose `needs_agent == Some(true)`. Two header
/// shapes are NOT under the fence:
///
/// 1. Top-level workflows (`parent_workflow.is_none()`). The
///    coordinator's own log is protected by the
///    single-writer-expected discipline.
/// 2. Batch-spawned children of a `materialize_children` parent that
///    have no request-store semantics (`needs_agent.is_none()` or
///    `Some(false)`). These children are not subject to redelegation
///    in the request-store sense; the fence would reject legitimate
///    in-process writes.
///
/// R43's wording is "every writer to a CHILD'S log" but the design's
/// scope is the request-store dispatched-agent protocol. Batch
/// children predate the request-store and have their own single-writer discipline
/// (the dispatched agent is the same process as the spawning batch
/// scheduler).
pub fn fence_applies_to(header: &StateFileHeader) -> bool {
    header.parent_workflow.is_some() && header.needs_agent == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_child_header(epoch: u32) -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: "child-a".into(),
            template_hash: "deadbeef".into(),
            created_at: "2026-05-24T00:00:00Z".into(),
            parent_workflow: Some("parent-coord".into()),
            template_source_dir: None,
            session_id: "child-a".into(),
            intent: None,
            template_name: Some("verdict".into()),
            needs_agent: Some(true),
            role: Some("scrutineer".into()),
            inputs: None,
            coordinator_of_record: Some("coord-a".into()),
            requested_by: Some("requester-1".into()),
            assignment_claim: None,
            dispatch_epoch: epoch,
            respawn_generation: None,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
        }
    }

    fn make_top_level_header(epoch: u32) -> StateFileHeader {
        let mut h = make_child_header(epoch);
        h.parent_workflow = None;
        h
    }

    fn child_sid() -> ValidatedSessionId {
        ValidatedSessionId::new("child-a").unwrap()
    }

    // ----- matching-epoch happy path -----

    #[test]
    fn validate_epoch_accepts_matching_value_zero() {
        let h = make_child_header(0);
        validate_epoch(&child_sid(), &h, Some(0)).expect("epoch 0 vs 0 must succeed");
    }

    #[test]
    fn validate_epoch_accepts_matching_value_nonzero() {
        let h = make_child_header(7);
        validate_epoch(&child_sid(), &h, Some(7)).expect("epoch 7 vs 7 must succeed");
    }

    // ----- stale-epoch rejection (the load-bearing redelegation case) -----

    #[test]
    fn validate_epoch_rejects_stale_presented_epoch() {
        let h = make_child_header(1);
        let err = validate_epoch(&child_sid(), &h, Some(0)).expect_err("stale epoch must reject");
        match err {
            EngineError::EpochFenceViolation {
                child_session_id,
                expected,
                presented,
            } => {
                assert_eq!(child_session_id, "child-a");
                assert_eq!(expected, 1);
                assert_eq!(presented, 0);
            }
            other => panic!("expected EpochFenceViolation, got {:?}", other),
        }
    }

    // ----- future-epoch rejection (defensive against buggy spawner) -----

    #[test]
    fn validate_epoch_rejects_future_presented_epoch() {
        let h = make_child_header(0);
        let err = validate_epoch(&child_sid(), &h, Some(5))
            .expect_err("future epoch must reject (strict equality, not lower-bound)");
        match err {
            EngineError::EpochFenceViolation {
                child_session_id,
                expected,
                presented,
            } => {
                assert_eq!(child_session_id, "child-a");
                assert_eq!(expected, 0);
                assert_eq!(presented, 5);
            }
            other => panic!("expected EpochFenceViolation, got {:?}", other),
        }
    }

    // ----- missing-flag is implicit mismatch -----

    #[test]
    fn validate_epoch_rejects_missing_presentation() {
        let h = make_child_header(0);
        let err = validate_epoch(&child_sid(), &h, None)
            .expect_err("missing --dispatch-epoch must reject");
        match err {
            EngineError::EpochFenceViolation {
                child_session_id,
                expected,
                presented,
            } => {
                assert_eq!(child_session_id, "child-a");
                assert_eq!(expected, 0);
                // u32::MAX sentinel distinguishes "didn't present"
                // from "presented 0 against fresh-dispatch child".
                assert_eq!(presented, u32::MAX);
            }
            other => panic!("expected EpochFenceViolation, got {:?}", other),
        }
    }

    // ----- exit-code mapping (the Issue 3 contract) -----

    #[test]
    fn epoch_fence_violation_exit_code_is_65() {
        let h = make_child_header(1);
        let err = validate_epoch(&child_sid(), &h, Some(0)).unwrap_err();
        assert_eq!(err.exit_code(), 65);
    }

    // ----- error message clarity -----

    #[test]
    fn epoch_fence_violation_display_names_all_three_fields() {
        let h = make_child_header(7);
        let err = validate_epoch(&child_sid(), &h, Some(3)).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("child-a"),
            "error message must name child_session_id, got {}",
            msg
        );
        assert!(
            msg.contains("expected dispatch_epoch=7"),
            "error message must name expected epoch, got {}",
            msg
        );
        assert!(
            msg.contains("presented=3"),
            "error message must name presented epoch, got {}",
            msg
        );
    }

    // ----- fence_applies_to scope rule -----

    #[test]
    fn fence_applies_to_request_store_child_only() {
        // Request-store child (needs_agent=true + parent) — fence
        // applies.
        assert!(fence_applies_to(&make_child_header(0)));
    }

    #[test]
    fn fence_does_not_apply_to_top_level_workflow() {
        assert!(!fence_applies_to(&make_top_level_header(0)));
    }

    #[test]
    fn fence_does_not_apply_to_batch_spawned_child_without_needs_agent() {
        // A `--parent`-init child without request-store semantics is
        // a batch-spawned child of a `materialize_children` parent.
        // Such children are not under the fence (their writes come
        // from the spawning batch scheduler's same process; no
        // redelegation race).
        let mut h = make_child_header(0);
        h.needs_agent = None;
        assert!(!fence_applies_to(&h));
        h.needs_agent = Some(false);
        assert!(!fence_applies_to(&h));
    }

    // PathBuf is referenced in scope for fixture clarity (no fixture
    // currently uses it but the parent-workflow shape pulls it in).
    #[allow(dead_code)]
    fn _path_buf_in_scope() {
        let _ = PathBuf::new();
    }
}
