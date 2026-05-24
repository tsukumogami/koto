//! Request-store reserved audit-event kinds and typed payload-shape
//! helpers.
//!
//! Decision 6 (DESIGN-koto-request-store) commits to **reusing**
//! [`EventPayload::EvidenceSubmitted`] rather than adding new
//! `EventPayload` variants for the request-store audit family — PRD
//! D10 requires zero new variants. The audit family is therefore keyed
//! off a reserved `fields.kind` discriminator on every
//! `EvidenceSubmitted` event.
//!
//! Four canonical reserved kinds are exposed here, plus the
//! `request_store.` prefix reservation. Template authors cannot use
//! any of these as a `fields.kind` value on `koto next --with-data`
//! submissions — [`is_reserved_kind`] is consumed by the CLI parser
//! to reject collisions at write time with
//! [`crate::engine::errors::EngineError::ReservedKindCollision`].
//!
//! [`wake_payload_summary`] takes
//! `&[crate::engine::types::ValidatedSessionId]` rather than
//! `&[String]` as a structural mitigation for security touch-up #5:
//! unvalidated session ids literally cannot flow into the
//! human-readable narrative, pre-paying the future-template-registry
//! hardening called out in the design's Security Considerations
//! carve-outs.
//!
//! See DESIGN-koto-request-store.md Decision 6 (lines 801-806 for
//! the wire-shape table) and Security Considerations >
//! `wake_payload_summary` content discipline (lines 2011-2025).

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::engine::types::ValidatedSessionId;

/// `fields.kind` value for the audit event emitted when a coordinator
/// dispatches a child to fulfill a request.
pub const CHILD_DISPATCHED: &str = "ChildDispatched";

/// `fields.kind` value for the audit event emitted when a previously
/// dispatched child is re-dispatched after its claim expires or its
/// agent fails (PRD R29 redelegation cap counts these).
pub const CHILD_REDELEGATED: &str = "ChildRedelegated";

/// `fields.kind` value for the audit event emitted when a parent
/// session is "woken" because at least one of its children reached
/// a terminal state (PRD R30 wake-candidates).
pub const REQUESTER_WOKEN: &str = "RequesterWoken";

/// `fields.kind` value for the audit event emitted when a child
/// session is respawned to retry the requester's work after a
/// terminal child failure (PRD R31 respawn).
pub const REQUESTER_RESPAWN: &str = "RequesterRespawn";

/// Prefix reserved for request-store audit-event kinds.
///
/// Template authors cannot submit a `fields.kind` whose value starts
/// with this prefix; the reservation gives koto headroom to add
/// future audit kinds without shadowing template-author code.
pub const REQUEST_STORE_PREFIX: &str = "request_store.";

/// Canonical list of reserved literal kinds.
///
/// Used by [`is_reserved_kind`] and by table-driven tests; kept
/// `const` so it stays in sync with the four `CHILD_*` /
/// `REQUESTER_*` constants above.
pub const RESERVED_KINDS: &[&str] = &[
    CHILD_DISPATCHED,
    CHILD_REDELEGATED,
    REQUESTER_WOKEN,
    REQUESTER_RESPAWN,
];

/// Return `true` when `kind` collides with the request-store audit
/// family.
///
/// A collision means any of:
/// 1. exact match against one of the four reserved literal names; OR
/// 2. starts with the `request_store.` prefix.
///
/// The CLI parser-rejection hook (`validate_with_data_payload` in
/// `src/cli/mod.rs`) consumes this predicate to reject offending
/// `koto next --with-data` payloads before any disk write.
pub fn is_reserved_kind(kind: &str) -> bool {
    if kind.starts_with(REQUEST_STORE_PREFIX) {
        return true;
    }
    RESERVED_KINDS.contains(&kind)
}

/// Build the `fields` map for a `ChildDispatched` audit event.
///
/// Wire shape (Decision 6 / design lines 801-806):
/// ```json
/// {
///   "kind": "ChildDispatched",
///   "child_session_id": "<child>",
///   "coord_id": "<coordinator>",
///   "dispatch_epoch": <n>
/// }
/// ```
pub fn child_dispatched_fields(
    child_session_id: &ValidatedSessionId,
    coord_id: &str,
    dispatch_epoch: u32,
) -> HashMap<String, Value> {
    let mut fields = HashMap::with_capacity(4);
    fields.insert("kind".to_string(), json!(CHILD_DISPATCHED));
    fields.insert(
        "child_session_id".to_string(),
        json!(child_session_id.as_str()),
    );
    fields.insert("coord_id".to_string(), json!(coord_id));
    fields.insert("dispatch_epoch".to_string(), json!(dispatch_epoch));
    fields
}

/// Build the `fields` map for a `ChildRedelegated` audit event.
///
/// Wire shape (Decision 6):
/// ```json
/// {
///   "kind": "ChildRedelegated",
///   "child_session_id": "<child>",
///   "coord_id": "<coordinator>",
///   "dispatch_epoch": <n>,
///   "respawn_generation": <g>
/// }
/// ```
pub fn child_redelegated_fields(
    child_session_id: &ValidatedSessionId,
    coord_id: &str,
    dispatch_epoch: u32,
    respawn_generation: u32,
) -> HashMap<String, Value> {
    let mut fields = HashMap::with_capacity(5);
    fields.insert("kind".to_string(), json!(CHILD_REDELEGATED));
    fields.insert(
        "child_session_id".to_string(),
        json!(child_session_id.as_str()),
    );
    fields.insert("coord_id".to_string(), json!(coord_id));
    fields.insert("dispatch_epoch".to_string(), json!(dispatch_epoch));
    fields.insert("respawn_generation".to_string(), json!(respawn_generation));
    fields
}

/// Build the `fields` map for a `RequesterWoken` audit event.
///
/// Emitted on the coordinator's own log when one or more dispatched
/// children reach a terminal state, signaling the wake-candidates pass
/// (PRD R30). Wire shape (Decision 6 / design line 805):
/// ```json
/// {
///   "kind": "RequesterWoken",
///   "summary": "N children completed",
///   "child_count": <n>,
///   "child_session_ids": ["<child-1>", "<child-2>"],
///   "child_dispatch_epochs": [<e1>, <e2>],
///   "requested_by": "<parent-session-id>"
/// }
/// ```
///
/// Per design line 2024 the canonical machine-readable wake payload
/// is `child_session_ids`; `summary` is purely human-readable
/// scaffolding. The wake-candidates pass (Issue 15) consumes
/// `child_session_ids` to determine which `ChildDispatched` events
/// have already been woken; `child_dispatch_epochs` is the parallel
/// array of epochs so a re-dispatched child (same id, higher epoch)
/// gets a fresh wake instead of being silently filtered by the bare-id
/// match against an earlier wake. The helper signature takes
/// `&[ValidatedSessionId]` so unvalidated ids cannot flow into either
/// the array or the summary string. `epochs.len()` must equal
/// `child_ids.len()`.
pub fn requester_woken_fields(
    child_ids: &[ValidatedSessionId],
    epochs: &[u32],
    requested_by: &str,
) -> HashMap<String, Value> {
    debug_assert_eq!(
        child_ids.len(),
        epochs.len(),
        "requester_woken_fields: epochs.len() must equal child_ids.len()"
    );
    let mut fields = HashMap::with_capacity(6);
    fields.insert("kind".to_string(), json!(REQUESTER_WOKEN));
    fields.insert(
        "summary".to_string(),
        json!(wake_payload_summary(child_ids)),
    );
    fields.insert("child_count".to_string(), json!(child_ids.len()));
    fields.insert(
        "child_session_ids".to_string(),
        json!(child_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>()),
    );
    fields.insert("child_dispatch_epochs".to_string(), json!(epochs));
    fields.insert("requested_by".to_string(), json!(requested_by));
    fields
}

/// Build the `fields` map for a `RequesterRespawn` audit event.
///
/// Emitted on the requester's own log when F1 cold-restart re-priming
/// (Issue 16 / Decision 5) spawns a fresh subagent to replace one
/// whose substrate transcript has expired.
///
/// Wire shape (Decision 6 / design line 806):
/// ```json
/// {
///   "kind": "RequesterRespawn",
///   "child_session_id": "<child>",
///   "respawn_generation": <g>,
///   "reason": "transcript_expired",
///   "prior_coordinator_of_record": "<prior-coord>",
///   "new_coordinator_of_record": "<new-coord>",
///   "respawned_at": "<rfc3339-millis>"
/// }
/// ```
///
/// `reason` carries the F1 trigger label (`transcript_expired`) on a
/// successful respawn, OR an F3 fallback reason
/// (`respawn_failed: <cause>`) when F1 refuses to fire. The
/// `prior_coordinator_of_record` and `new_coordinator_of_record`
/// fields are typically the same value in the single-coordinator
/// model but design line 806 reserves them as distinct so a future
/// hand-off can record both sides.
pub fn requester_respawn_fields(
    child_session_id: &ValidatedSessionId,
    respawn_generation: u32,
    reason: &str,
    prior_coordinator_of_record: &str,
    new_coordinator_of_record: &str,
    respawned_at: &str,
) -> HashMap<String, Value> {
    let mut fields = HashMap::with_capacity(7);
    fields.insert("kind".to_string(), json!(REQUESTER_RESPAWN));
    fields.insert(
        "child_session_id".to_string(),
        json!(child_session_id.as_str()),
    );
    fields.insert("respawn_generation".to_string(), json!(respawn_generation));
    fields.insert("reason".to_string(), json!(reason));
    fields.insert(
        "prior_coordinator_of_record".to_string(),
        json!(prior_coordinator_of_record),
    );
    fields.insert(
        "new_coordinator_of_record".to_string(),
        json!(new_coordinator_of_record),
    );
    fields.insert("respawned_at".to_string(), json!(respawned_at));
    fields
}

/// Build the human-readable `summary` string carried on a
/// `RequesterWoken` audit event.
///
/// **Signature is load-bearing.** Takes
/// `&[ValidatedSessionId]` (not `&[String]` or `&[&str]`) so a
/// caller cannot pass an unvalidated id — closing the
/// future-template-registry risk surface called out in Security
/// Considerations carve-outs. The function never quotes raw ids; it
/// emits a count-only narrative.
///
/// Output shape:
/// - 0 children → `"no children completed"`
/// - 1 child   → `"1 child completed"`
/// - N > 1     → `"N children completed"`
pub fn wake_payload_summary(child_ids: &[ValidatedSessionId]) -> String {
    match child_ids.len() {
        0 => "no children completed".to_string(),
        1 => "1 child completed".to_string(),
        n => format!("{} children completed", n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{EventPayload, ValidatedSessionId};

    fn sid(s: &str) -> ValidatedSessionId {
        ValidatedSessionId::new(s).expect("test id must be valid")
    }

    // ---- is_reserved_kind ----

    #[test]
    fn reserved_literals_collide() {
        for kind in RESERVED_KINDS {
            assert!(
                is_reserved_kind(kind),
                "literal {:?} must be flagged as reserved",
                kind
            );
        }
    }

    #[test]
    fn request_store_prefix_collides() {
        assert!(is_reserved_kind("request_store.foo"));
        assert!(is_reserved_kind("request_store."));
        assert!(is_reserved_kind("request_store.anything.with.dots"));
    }

    #[test]
    fn template_author_kinds_do_not_collide() {
        for ok in [
            "verdict",
            "review",
            "scrutineer",
            "child-completed",
            "request",
            "request_stor",
        ] {
            assert!(
                !is_reserved_kind(ok),
                "{:?} must NOT be flagged as reserved",
                ok
            );
        }
    }

    #[test]
    fn case_sensitive_reservation() {
        // Reserved-name matching is case-sensitive — "childdispatched"
        // (all-lowercase) is NOT a collision. Template authors can
        // still author lowercase analogues if they want to.
        assert!(!is_reserved_kind("childdispatched"));
        assert!(!is_reserved_kind("CHILD_DISPATCHED"));
    }

    // ---- typed builders + round-trip ----

    #[test]
    fn child_dispatched_round_trips_through_evidence_submitted() {
        let child = sid("parent.task-a");
        let fields = child_dispatched_fields(&child, "coord-7", 3);
        assert_eq!(fields["kind"], json!("ChildDispatched"));
        assert_eq!(fields["child_session_id"], json!("parent.task-a"));
        assert_eq!(fields["coord_id"], json!("coord-7"));
        assert_eq!(fields["dispatch_epoch"], json!(3));

        let payload = EventPayload::EvidenceSubmitted {
            state: "dispatch".to_string(),
            fields,
            submitter_cwd: None,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let parsed: EventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn child_redelegated_round_trips_through_evidence_submitted() {
        let child = sid("parent.task-a");
        let fields = child_redelegated_fields(&child, "coord-7", 4, 2);
        assert_eq!(fields["kind"], json!("ChildRedelegated"));
        assert_eq!(fields["respawn_generation"], json!(2));
        let payload = EventPayload::EvidenceSubmitted {
            state: "dispatch".to_string(),
            fields,
            submitter_cwd: None,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let parsed: EventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn requester_woken_round_trips_through_evidence_submitted() {
        let kids = [sid("parent.task-a"), sid("parent.task-b")];
        let epochs = [0u32, 1u32];
        let fields = requester_woken_fields(&kids, &epochs, "parent");
        assert_eq!(fields["kind"], json!("RequesterWoken"));
        assert_eq!(fields["child_count"], json!(2));
        assert_eq!(fields["summary"], json!("2 children completed"));
        assert_eq!(
            fields["child_session_ids"],
            json!(["parent.task-a", "parent.task-b"])
        );
        assert_eq!(fields["child_dispatch_epochs"], json!([0, 1]));
        assert_eq!(fields["requested_by"], json!("parent"));
        let payload = EventPayload::EvidenceSubmitted {
            state: "wake".to_string(),
            fields,
            submitter_cwd: None,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let parsed: EventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn requester_respawn_round_trips_through_evidence_submitted() {
        let child = sid("parent.task-a");
        let fields = requester_respawn_fields(
            &child,
            5,
            "transcript_expired",
            "old-coord",
            "new-coord",
            "2026-05-24T00:00:00.000Z",
        );
        assert_eq!(fields["kind"], json!("RequesterRespawn"));
        assert_eq!(fields["respawn_generation"], json!(5));
        assert_eq!(fields["reason"], json!("transcript_expired"));
        assert_eq!(fields["prior_coordinator_of_record"], json!("old-coord"));
        assert_eq!(fields["new_coordinator_of_record"], json!("new-coord"));
        assert_eq!(fields["respawned_at"], json!("2026-05-24T00:00:00.000Z"));
        let payload = EventPayload::EvidenceSubmitted {
            state: "respawn".to_string(),
            fields,
            submitter_cwd: None,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let parsed: EventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, parsed);
    }

    // ---- wake_payload_summary ----

    #[test]
    fn wake_payload_summary_zero_children() {
        let kids: Vec<ValidatedSessionId> = vec![];
        assert_eq!(wake_payload_summary(&kids), "no children completed");
    }

    #[test]
    fn wake_payload_summary_one_child() {
        let kids = [sid("scrutineer-a")];
        assert_eq!(wake_payload_summary(&kids), "1 child completed");
    }

    #[test]
    fn wake_payload_summary_many_children() {
        let kids = [
            sid("scrutineer-a"),
            sid("scrutineer-b"),
            sid("scrutineer-c"),
        ];
        assert_eq!(wake_payload_summary(&kids), "3 children completed");
    }

    #[test]
    fn wake_payload_summary_never_quotes_ids() {
        let kids = [sid("dangerous-id-1"), sid("dangerous-id-2")];
        let summary = wake_payload_summary(&kids);
        // Structural guarantee from the signature: the function can
        // only see the ids through ValidatedSessionId, but it also
        // chooses never to interpolate them. Verify both halves.
        assert!(!summary.contains("dangerous-id-1"));
        assert!(!summary.contains("dangerous-id-2"));
        assert_eq!(summary, "2 children completed");
    }
}
