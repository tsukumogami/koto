//! In-tree compile-check for the KT1 Stage 1 frozen public surface
//! (Issue 19 / Decision 5).
//!
//! Verifies that every type bunki BK2 imports resolves at its
//! canonical `koto::engine::types::*` path AND that
//! `koto::error::Error` is exposed. A regression here — a re-export
//! that got dropped, a type that moved without an alias, or the
//! `Error` re-export breaking — is caught at `cargo test` time
//! BEFORE the external `koto-stability-tests` crate (Issue 20) runs.
//!
//! The test bodies do not exercise behavior; they construct default
//! or trivial instances of each type to prove the symbol resolves
//! and the constructor signature has not changed unexpectedly. The
//! full downstream-import contract — Cargo.toml `[dependencies]`
//! resolution, MSRV, feature flags — lands in Issue 20's external
//! crate fixture.

use std::collections::HashMap;
use std::path::PathBuf;

// The canonical Stage 1 import shape. The eight types below all
// resolve via `koto::engine::types::*` per Decision 5.
use koto::engine::types::{
    derive_state_from_log, AssignmentClaim, ChildSnapshot, Event, EventPayload, SpawnEntrySnapshot,
    StateFileHeader, CURRENT_SCHEMA_VERSION,
};

// `koto::error::Error` is the re-exported `EngineError` alias per
// Decision 5. The alias path is the canonical name bunki BK2
// imports.
use koto::error::Error;

#[test]
fn state_file_header_resolves_and_constructs() {
    let h = StateFileHeader {
        schema_version: CURRENT_SCHEMA_VERSION,
        workflow: "test".into(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: "test".into(),
        intent: None,
        template_name: None,
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        respawn_generation: None,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
    };
    assert_eq!(h.schema_version, CURRENT_SCHEMA_VERSION);
}

#[test]
fn event_resolves_and_constructs() {
    let e = Event {
        seq: 1,
        timestamp: "2026-05-24T00:00:00Z".into(),
        event_type: "workflow_initialized".into(),
        payload: EventPayload::WorkflowInitialized {
            template_path: String::new(),
            variables: HashMap::new(),
            spawn_entry: None,
        },
        idempotency_hash: None,
    };
    assert_eq!(e.seq, 1);
}

#[test]
fn event_payload_resolves_with_unknown_variant() {
    // The Unknown catch-all is part of the Stage 1 surface — the
    // forward-compat hook documented in docs/STABILITY.md.
    let p = EventPayload::Unknown {
        type_name: "future_variant_not_yet_known".into(),
        raw_payload: serde_json::Value::Null,
    };
    match p {
        EventPayload::Unknown { type_name, .. } => {
            assert_eq!(type_name, "future_variant_not_yet_known");
        }
        _ => unreachable!(),
    }
}

#[test]
fn spawn_entry_snapshot_resolves_and_constructs() {
    let s = SpawnEntrySnapshot::new("child-template.md".into(), Default::default(), Vec::new());
    assert_eq!(s.template, "child-template.md");
    assert!(s.vars.is_empty());
    assert!(s.waits_on.is_empty());
}

#[test]
fn child_snapshot_resolves_and_constructs() {
    let c = ChildSnapshot {
        current_state: "work".into(),
        terminal: false,
        failure: false,
        skipped_marker: false,
        spawn_entry: None,
    };
    assert_eq!(c.current_state, "work");
    assert!(!c.terminal);
}

#[test]
fn assignment_claim_resolves_and_constructs() {
    let a = AssignmentClaim {
        coord_id: "team-lead".into(),
        claimed_at: "2026-05-24T14:35:01.000Z".into(),
    };
    assert_eq!(a.coord_id, "team-lead");
}

#[test]
fn derive_state_from_log_resolves_at_engine_types_path() {
    // The alias re-exports the function from engine::persistence
    // into engine::types. The test confirms the path resolves AND
    // the function is callable with an empty event slice.
    let events: Vec<Event> = Vec::new();
    // derive_state_from_log returns Option<String> (the current
    // state name) — None on an empty log because there are no
    // transitions to derive from.
    let out = derive_state_from_log(&events);
    assert!(out.is_none(), "empty event log must produce None");
}

#[test]
fn current_schema_version_resolves_as_constant() {
    // CURRENT_SCHEMA_VERSION is a const u32 at the Stage 1 path.
    // Pin the current value as a regression guard against accidental
    // bumps without going through the STABILITY.md protocol.
    assert_eq!(CURRENT_SCHEMA_VERSION, 1);
}

#[test]
fn koto_error_alias_resolves_and_carries_variants() {
    // koto::error::Error is the re-exported EngineError alias. The
    // alias must carry all the typed variants Decision 5 pins.
    let e: Error = Error::EpochFenceViolation {
        child_session_id: "child-a".into(),
        expected: 1,
        presented: 0,
    };
    // The Display impl matches the engine's typed format string.
    let msg = format!("{}", e);
    assert!(msg.contains("epoch fence violation"));
    assert!(msg.contains("child-a"));
    // Exit-code mapping is part of the contract.
    assert_eq!(e.exit_code(), 65);
}

#[test]
fn koto_error_alias_carries_additive_evolution_variants() {
    // Variants added across Phases 1-6 must all be reachable via the
    // alias. This is the additive-evolution proof point — minor
    // releases can add new variants to Error without breaking
    // downstream consumers.
    let _ = Error::RedelegationCapExceeded {
        child_session_id: "child-b".into(),
        cap: 10,
    };
    let _ = Error::RecursionCapExceeded {
        dimension: "depth".into(),
        threshold: 10,
        observed: 11,
    };
    let _ = Error::ReservedKindCollision {
        offending_kind: "kt1.child_dispatched".into(),
    };
    let _ = Error::ConcurrentSubmissionConflict {
        session_id: "child-c".into(),
        state_name: "work".into(),
    };
}

#[test]
fn path_buf_used_for_template_source_dir_field() {
    // template_source_dir is a public field of type Option<PathBuf>.
    // This test just confirms the field exists in the canonical
    // shape after the lockdown (paranoid additive-evolution check
    // for an existing field).
    let h = StateFileHeader {
        schema_version: CURRENT_SCHEMA_VERSION,
        workflow: "x".into(),
        template_hash: "x".into(),
        created_at: "x".into(),
        parent_workflow: None,
        template_source_dir: Some(PathBuf::from("/x")),
        session_id: "x".into(),
        intent: None,
        template_name: None,
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        respawn_generation: None,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
    };
    assert!(h.template_source_dir.is_some());
}
