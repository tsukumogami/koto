//! Integration coverage for the KT1 reserved-kind audit family
//! (Decision 6 in DESIGN-koto-request-store).
//!
//! Exercises the four reserved-kind constants, the parser-rejection
//! contract (template authors cannot submit a reserved `fields.kind`
//! via `koto next --with-data`), the `kt1.` prefix gate, the
//! `wake_payload_summary` count-only narrative, and a bunki-shaped
//! consumer that omits `EvidenceSubmitted` and falls through to an
//! `Unknown` catch-all on the wire payload.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use koto::engine::audit::{
    child_dispatched_fields, child_redelegated_fields, is_reserved_kind, requester_respawn_fields,
    requester_woken_fields, wake_payload_summary, CHILD_DISPATCHED, CHILD_REDELEGATED, KT1_PREFIX,
    REQUESTER_RESPAWN, REQUESTER_WOKEN, RESERVED_KINDS,
};
use koto::engine::errors::EngineError;
use koto::engine::types::{Event, EventPayload, ValidatedSessionId};

fn sid(s: &str) -> ValidatedSessionId {
    ValidatedSessionId::new(s).expect("test id must validate")
}

/// Build an `EvidenceSubmitted` Event carrying `fields` and a fixed
/// state for round-trip tests.
fn evidence_event(seq: u64, fields: HashMap<String, serde_json::Value>) -> Event {
    let payload = EventPayload::EvidenceSubmitted {
        state: "dispatch".to_string(),
        fields,
        submitter_cwd: None,
    };
    Event {
        seq,
        timestamp: "2026-05-24T00:00:00Z".to_string(),
        event_type: payload.type_name().to_string(),
        payload,
        idempotency_hash: None,
    }
}

// ----- Reserved-kind constants -----

#[test]
fn reserved_constants_match_design_table() {
    assert_eq!(CHILD_DISPATCHED, "ChildDispatched");
    assert_eq!(CHILD_REDELEGATED, "ChildRedelegated");
    assert_eq!(REQUESTER_WOKEN, "RequesterWoken");
    assert_eq!(REQUESTER_RESPAWN, "RequesterRespawn");
    assert_eq!(KT1_PREFIX, "kt1.");
    assert_eq!(RESERVED_KINDS.len(), 4);
}

#[test]
fn reserved_kinds_table_test_all_four() {
    for kind in RESERVED_KINDS {
        assert!(
            is_reserved_kind(kind),
            "literal {:?} must be reported as reserved",
            kind
        );
    }
}

#[test]
fn kt1_prefix_table_test() {
    for kind in ["kt1.", "kt1.foo", "kt1.respawn", "kt1.deeply.nested.kind"] {
        assert!(is_reserved_kind(kind));
    }
}

#[test]
fn template_author_kinds_are_accepted() {
    for ok in ["verdict", "review", "scrutineer", "kt", "kt1", "kt1foo"] {
        assert!(
            !is_reserved_kind(ok),
            "{:?} must not be treated as reserved",
            ok
        );
    }
}

// ----- Typed builder + round-trip -----

#[test]
fn child_dispatched_fields_round_trip() {
    let child = sid("parent.task-a");
    let fields = child_dispatched_fields(&child, "coord-7", 3);
    let event = evidence_event(1, fields);
    let s = serde_json::to_string(&event).unwrap();
    assert!(s.contains("\"kind\":\"ChildDispatched\""));
    assert!(s.contains("\"child_session_id\":\"parent.task-a\""));
    let parsed: Event = serde_json::from_str(&s).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn child_redelegated_fields_round_trip() {
    let child = sid("parent.task-b");
    let fields = child_redelegated_fields(&child, "coord-7", 5, 2);
    let event = evidence_event(2, fields);
    let s = serde_json::to_string(&event).unwrap();
    assert!(s.contains("\"kind\":\"ChildRedelegated\""));
    assert!(s.contains("\"respawn_generation\":2"));
    let parsed: Event = serde_json::from_str(&s).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn requester_woken_fields_round_trip() {
    let kids = [
        sid("parent.task-a"),
        sid("parent.task-b"),
        sid("parent.task-c"),
    ];
    let fields = requester_woken_fields(&kids);
    let event = evidence_event(3, fields);
    let s = serde_json::to_string(&event).unwrap();
    assert!(s.contains("\"kind\":\"RequesterWoken\""));
    assert!(s.contains("\"summary\":\"3 children completed\""));
    assert!(s.contains("\"child_count\":3"));
    let parsed: Event = serde_json::from_str(&s).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn requester_respawn_fields_round_trip() {
    let child = sid("parent.task-a");
    let fields = requester_respawn_fields(&child, 7);
    let event = evidence_event(4, fields);
    let s = serde_json::to_string(&event).unwrap();
    assert!(s.contains("\"kind\":\"RequesterRespawn\""));
    assert!(s.contains("\"respawn_generation\":7"));
    let parsed: Event = serde_json::from_str(&s).unwrap();
    assert_eq!(event, parsed);
}

// ----- ReservedKindCollision error -----

#[test]
fn reserved_kind_collision_display_names_offending_kind() {
    let err = EngineError::ReservedKindCollision {
        offending_kind: "ChildDispatched".to_string(),
    };
    let display = format!("{}", err);
    assert_eq!(display, "reserved audit-event kind: ChildDispatched");
}

#[test]
fn reserved_kind_collision_exit_code_defaults_to_one() {
    // The collision is operator-controllable (they pick the kind);
    // the design doesn't pin a sysexit value, so it falls through
    // to the generic-error default.
    let err = EngineError::ReservedKindCollision {
        offending_kind: "kt1.foo".to_string(),
    };
    assert_eq!(err.exit_code(), 1);
}

// ----- wake_payload_summary content discipline -----

#[test]
fn wake_payload_summary_singular_plural_and_empty() {
    assert_eq!(wake_payload_summary(&[]), "no children completed");
    assert_eq!(wake_payload_summary(&[sid("a")]), "1 child completed");
    assert_eq!(
        wake_payload_summary(&[sid("a"), sid("b")]),
        "2 children completed"
    );
}

#[test]
fn wake_payload_summary_never_quotes_session_ids() {
    let kids = [sid("looks-like-a-malicious-string"), sid("another-id")];
    let summary = wake_payload_summary(&kids);
    assert!(!summary.contains("looks-like-a-malicious-string"));
    assert!(!summary.contains("another-id"));
    assert_eq!(summary, "2 children completed");
}

// ----- Bunki-shaped consumer (Unknown catch-all) -----

/// Stripped-down EventPayload mirror with NO `EvidenceSubmitted`
/// variant, plus an `#[serde(other)] Unknown` catch-all. Bunki BK2
/// is intentionally smaller than koto; this fixture mimics the
/// "external-crate consumer" shape so we can prove that an audit
/// event written by koto deserializes as `Unknown` (not as a panic
/// or hard error) on a consumer that doesn't model `EvidenceSubmitted`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BunkiPayload {
    Transitioned {
        from: Option<String>,
        to: String,
    },
    #[serde(other)]
    Unknown,
}

#[test]
fn bunki_shaped_consumer_treats_audit_events_as_unknown() {
    // Direct payload form (not the full Event envelope) so we can
    // model a consumer that uses serde's tagged-enum dispatch and
    // an Unknown catch-all.
    for kind in RESERVED_KINDS {
        let payload_json = json!({
            "type": "evidence_submitted",
            "state": "dispatch",
            "fields": { "kind": kind }
        });
        let parsed: BunkiPayload = serde_json::from_value(payload_json).expect(
            "bunki-shaped consumer must deserialize reserved kinds as Unknown without panic",
        );
        assert_eq!(parsed, BunkiPayload::Unknown);
    }
}

// ----- End-to-end check on the CLI parser-rejection contract -----
//
// `validate_with_data_payload` is private to koto::cli, so this
// section exercises the visible behavior via the public audit
// helper that the CLI consults. The acceptance criteria for the
// CLI rejection are also covered by the CLI module's own unit
// tests under `src/cli/mod.rs`.

#[test]
fn reserved_literal_kinds_fail_audit_predicate() {
    for kind in RESERVED_KINDS {
        assert!(is_reserved_kind(kind));
    }
}

#[test]
fn kt1_prefix_kinds_fail_audit_predicate() {
    for kind in ["kt1.foo", "kt1.respawn", "kt1.x.y.z"] {
        assert!(is_reserved_kind(kind));
    }
}

#[test]
fn template_author_authored_kinds_pass_audit_predicate() {
    for ok in ["verdict", "scrutineer", "review", "decision"] {
        assert!(!is_reserved_kind(ok));
    }
}
