//! External-consumer compile-check fixture for koto's Stage 1
//! frozen public surface (Issue 20 / Decision 5).
//!
//! This crate exists to catch breaking changes to the API surface
//! bunki BK2 imports. The in-tree `tests/lib_reexports.rs` test
//! (Issue 19) verifies the same shape from inside the koto crate;
//! this crate verifies it again from the perspective of an
//! external consumer that depends on koto via `path = ".."` (or
//! `version = "0.10"` when published). The two checks together
//! form a defence-in-depth: a re-export that compiles in-tree but
//! breaks on an external import would fail this crate's build.
//!
//! ## Stage 1 frozen surface (per `docs/STABILITY.md`)
//!
//! - `koto::engine::types::StateFileHeader`
//! - `koto::engine::types::Event`
//! - `koto::engine::types::EventPayload`
//! - `koto::engine::types::SpawnEntrySnapshot`
//! - `koto::engine::types::ChildSnapshot`
//! - `koto::engine::types::AssignmentClaim`
//! - `koto::engine::types::derive_state_from_log`
//! - `koto::engine::types::CURRENT_SCHEMA_VERSION`
//! - `koto::error::Error` (re-exported `EngineError` alias)
//! - Four `SessionBackend` methods: `create`, `list`,
//!   `read_events`, `init_state_file` — exercised by the smoke
//!   test below via a trait object construction.
//!
//! ## What this crate is NOT
//!
//! - Not published to crates.io (`publish = false` in
//!   `Cargo.toml`).
//! - Not a behavior test — every assertion checks that an import
//!   resolves AND a default-construct works, not that the type's
//!   semantics are correct. Behavior tests live in the koto
//!   crate's `tests/` directory.
//! - Not run by `cargo test` from the koto crate alone; CI runs
//!   it explicitly via `cargo test -p koto-stability-tests`.

// The Stage 1 frozen import shape — exactly what bunki BK2 uses.
// Any rename or removal here is a compile-time regression. The
// `#[allow(unused_imports)]` allows these top-level `use`
// statements to document the contract surface even though the
// actual exercises live inside the `#[cfg(test)]` module below.
#[allow(unused_imports)]
use koto::engine::types::{
    derive_state_from_log, AssignmentClaim, ChildSnapshot, Event, EventPayload, SpawnEntrySnapshot,
    StateFileHeader, CURRENT_SCHEMA_VERSION,
};
#[allow(unused_imports)]
use koto::error::Error;

// The Stage 1 `SessionBackend` trait + the canonical local impl
// used by the smoke test as the trait-object construction proof.
#[allow(unused_imports)]
use koto::session::local::LocalBackend;
#[allow(unused_imports)]
use koto::session::SessionBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ----- Per-type compile-check tests -------------------------------

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
        assert!(h.template_source_dir.is_none());
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
    fn event_payload_unknown_resolves() {
        // Unknown is the forward-compat catch-all. External consumers
        // (bunki BK2) rely on this variant when koto adds a new
        // EventPayload variant in a minor release; existing builds
        // continue to deserialize via this fallthrough.
        let p = EventPayload::Unknown {
            type_name: "future_variant".into(),
            raw_payload: serde_json::Value::Null,
        };
        match p {
            EventPayload::Unknown { type_name, .. } => {
                assert_eq!(type_name, "future_variant");
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
        // into engine::types — Decision 5 line "the canonical export
        // path is decoupled from the implementation module".
        let events: Vec<Event> = Vec::new();
        let out = derive_state_from_log(&events);
        assert!(out.is_none());
    }

    #[test]
    fn current_schema_version_pins_to_one() {
        // Pin the current value so an accidental bump on this
        // external crate's build catches the regression. The bump
        // protocol is documented in docs/STABILITY.md (Issue 19).
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
    }

    #[test]
    fn koto_error_alias_resolves() {
        // koto::error::Error is the re-exported EngineError alias.
        let e: Error = Error::EpochFenceViolation {
            child_session_id: "child-a".into(),
            expected: 1,
            presented: 0,
        };
        let msg = format!("{}", e);
        assert!(msg.contains("epoch fence"));
        assert_eq!(e.exit_code(), 65);
    }

    #[test]
    fn koto_error_carries_additive_variants() {
        // Variants added by Phases 1-6 must all be reachable via
        // the public alias. Additive evolution proof point.
        let _ = Error::RedelegationCapExceeded {
            child_session_id: "child-b".into(),
            cap: 10,
        };
        let _ = Error::ReservedKindCollision {
            offending_kind: "request_store.child_dispatched".into(),
        };
        let _ = Error::ConcurrentSubmissionConflict {
            session_id: "child-c".into(),
            state_name: "work".into(),
        };
    }

    #[test]
    fn path_buf_used_for_template_source_dir() {
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

    // ----- SessionBackend smoke test ---------------------------------
    //
    // Construct a Box<dyn SessionBackend> against a tempdir, then
    // call each of the four frozen methods (create, list,
    // read_events, init_state_file). The dyn-compatibility proof
    // is implicit in `Box<dyn SessionBackend>` succeeding — if
    // Issue 19's lockdown accidentally added a non-dyn-compatible
    // method (e.g. `Self: Sized` bound), this would fail to
    // compile.
    //
    // The smoke test asserts call shapes compile and return
    // sensible values. It does NOT assert behavior correctness —
    // that's covered by koto's own integration tests.

    #[test]
    fn session_backend_dyn_compatibility_smoke() {
        let tmp = tempfile::tempdir().unwrap();
        // Construct via the public `with_base_dir` constructor.
        // The resulting type erases through Box<dyn SessionBackend>
        // proving dyn-compatibility.
        let backend: Box<dyn SessionBackend> =
            Box::new(LocalBackend::with_base_dir(tmp.path().to_path_buf()));

        // ----- list() — Stage 1 frozen -------------------------------
        // An empty sessions root must list cleanly (Vec::new()).
        let listed = backend.list().expect("list against empty tempdir succeeds");
        assert!(listed.is_empty());

        // ----- create() — Stage 1 frozen -----------------------------
        let session_id = "test-session";
        let dir = backend
            .create(session_id)
            .expect("create against empty tempdir succeeds");
        assert!(dir.exists());

        // ----- init_state_file() — Stage 1 frozen --------------------
        // Build a minimal header + initial event, then atomically
        // initialize the session's state file.
        let header = StateFileHeader {
            schema_version: CURRENT_SCHEMA_VERSION,
            workflow: session_id.into(),
            template_hash: "deadbeef".into(),
            created_at: "2026-05-24T00:00:00Z".into(),
            parent_workflow: None,
            template_source_dir: None,
            session_id: session_id.into(),
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
        let initial_events = vec![Event {
            seq: 1,
            timestamp: "2026-05-24T00:00:00Z".into(),
            event_type: "workflow_initialized".into(),
            payload: EventPayload::WorkflowInitialized {
                template_path: String::new(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];
        // Re-use a fresh id so create() above doesn't collide with
        // init_state_file's fail-if-exists semantics.
        let init_id = "test-init";
        backend.create(init_id).expect("create init session dir");
        let init_header = StateFileHeader {
            workflow: init_id.into(),
            session_id: init_id.into(),
            ..header
        };
        backend
            .init_state_file(init_id, init_header, initial_events)
            .expect("init_state_file succeeds against an empty session dir");

        // ----- read_events() — Stage 1 frozen ------------------------
        let (read_header, read_events) = backend
            .read_events(init_id)
            .expect("read_events against the just-initialized session succeeds");
        assert_eq!(read_header.workflow, init_id);
        assert_eq!(read_events.len(), 1);
        assert_eq!(read_events[0].seq, 1);

        // Round-trip confirmation: listing the workspace now shows
        // the fully-initialized session. The earlier `create()` call
        // only made the session directory; `list()` requires a
        // state file (which only `init_state_file` writes) to count
        // a directory as a session.
        let listed_after = backend.list().expect("list after init_state_file succeeds");
        assert!(listed_after.iter().any(|s| s.id == init_id));
    }
}
