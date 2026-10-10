//! Integration tests for Issue 16:
//! `feat(respawn): F1 cold-restart re-priming + F3 fallback + respawn_generation_cap`.
//!
//! Covers the acceptance criteria from the issue body:
//! all-three-preconditions positive path, three precondition guards,
//! cap enforcement, four F3 cause classes, generation increment
//! across cycles, pre-request-store fixture compatibility, fixed-form
//! resume-context prompt snapshot, agent-membership invoked after
//! spawn, RequesterRespawn uses Issue 14's audit helper.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use koto::engine::audit::REQUESTER_RESPAWN;
use koto::engine::errors::EngineError;
use koto::engine::persistence::{append_header, read_events};
use koto::engine::respawn::{
    execute_respawn, render_resume_context_prompt, F3Cause, NoOpReason, RespawnExecuted,
    RespawnExecution, RespawnRequest, SubstrateRespawner, RESUME_CONTEXT_PROMPT,
};
use koto::engine::types::{EventPayload, StateFileHeader, ValidatedSessionId};
use koto::session::local::LocalBackend;
use koto::session::state_file_name;

#[path = "support/funnel_backend.rs"]
mod funnel_backend;
use funnel_backend::FunnelBackend;

// ----- Mock substrate respawner ------------------------------------------

#[derive(Default)]
struct RecordingRespawner {
    calls: Mutex<Vec<RespawnRequest>>,
    fail: Mutex<bool>,
}

impl RecordingRespawner {
    fn fail_next(&self) {
        *self.fail.lock().unwrap() = true;
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn last(&self) -> RespawnRequest {
        self.calls.lock().unwrap().last().unwrap().clone()
    }
}

impl SubstrateRespawner for RecordingRespawner {
    fn respawn(&self, request: &RespawnRequest) -> Result<(), EngineError> {
        if *self.fail.lock().unwrap() {
            return Err(EngineError::StateNotFound(
                request.session_id.as_str().to_string(),
            ));
        }
        self.calls.lock().unwrap().push(request.clone());
        Ok(())
    }
}

// ----- Helpers -----------------------------------------------------------

fn floor() -> Duration {
    Duration::from_secs(60 * 60 * 24 * 30) // 30 days
}

fn make_header(workflow: &str, role: Option<&str>) -> StateFileHeader {
    StateFileHeader {
        command_environment: None,
        schema_version: 1,
        workflow: workflow.into(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: None,
        template_source_dir: None,
        template_source_file: None,
        origin: None,
        execution_dir: None,
        session_id: workflow.into(),
        intent: None,
        template_name: Some("verdict".into()),
        needs_agent: Some(true),
        role: role.map(|s| s.to_string()),
        inputs: Some(serde_json::json!({"k": "v"})),
        coordinator_of_record: Some("coord".into()),
        requested_by: Some("coord".into()),
        assignment_claim: None,
        dispatch_epoch: 0,
        respawn_generation: None,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        root_session_id: None,
        parent_session_id: None,
    }
}

/// The session store the requester's log lives in: `dir` itself, as
/// [`write_requester_state`] lays it out.
fn store(dir: &std::path::Path) -> LocalBackend {
    LocalBackend::with_base_dir(dir.to_path_buf())
}

fn write_requester_state(
    dir: &std::path::Path,
    workflow: &str,
    header: &StateFileHeader,
) -> PathBuf {
    let session_dir = dir.join(workflow);
    std::fs::create_dir_all(&session_dir).unwrap();
    let path = session_dir.join(state_file_name(workflow));
    append_header(&path, header).unwrap();
    path
}

fn find_respawn_event(state_file: &std::path::Path) -> EventPayload {
    let (_, events) = read_events(state_file).unwrap();
    for e in events {
        if let EventPayload::EvidenceSubmitted { fields, .. } = &e.payload {
            if fields.get("kind").and_then(|v| v.as_str()) == Some(REQUESTER_RESPAWN) {
                return e.payload;
            }
        }
    }
    panic!("no RequesterRespawn event on log");
}

fn find_workflow_cancelled(state_file: &std::path::Path) -> Option<String> {
    let (_, events) = read_events(state_file).unwrap();
    for e in events {
        if let EventPayload::WorkflowCancelled { reason, .. } = e.payload {
            return Some(reason);
        }
    }
    None
}

// ----- AC: all-three-preconditions positive --------------------------------

#[test]
fn all_three_preconditions_positive_fires_f1() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    let state_file = write_requester_state(tmp.path(), "requester", &header);

    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60); // 60 days ago
    let last_activity = woken_at - Duration::from_secs(60); // before woken_at

    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();

    assert_eq!(outcome, RespawnExecuted::Respawned { new_generation: 1 });
    assert_eq!(respawner.count(), 1);
    let req = respawner.last();
    assert_eq!(req.role, "scrutineer");
    assert_eq!(req.template_name, "verdict");
    assert_eq!(req.new_respawn_generation, 1);
    assert!(req.resume_prompt.contains("requester"));

    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(fields["kind"], serde_json::json!("RequesterRespawn"));
        assert_eq!(fields["reason"], serde_json::json!("transcript_expired"));
        assert_eq!(fields["respawn_generation"], serde_json::json!(1));
        assert_eq!(
            fields["prior_coordinator_of_record"],
            serde_json::json!("coord")
        );
        assert_eq!(
            fields["new_coordinator_of_record"],
            serde_json::json!("coord")
        );
    }
}

// ----- AC: precondition #1 guard — woken too recent -----------------------

#[test]
fn precondition_1_guard_woken_younger_than_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    // 1 day ago — well under the 30-day floor.
    let woken_at = now - Duration::from_secs(60 * 60 * 24);
    let last_activity = now;
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::NoOp {
            reason: NoOpReason::WokenYoungerThanFloor
        }
    );
    assert_eq!(respawner.count(), 0);
}

// ----- AC: precondition #2 guard — requester resumed and is active -------

#[test]
fn precondition_2_guard_requester_resumed_and_active() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60); // 60 days ago
                                                                 // requester resumed RECENTLY (yesterday) — last_activity is well within floor.
    let last_activity = now - Duration::from_secs(60 * 60 * 24);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::NoOp {
            reason: NoOpReason::RequesterRecentlyActive
        }
    );
    assert_eq!(respawner.count(), 0);
}

// ----- AC: precondition #3 — resumed-then-idle past floor FIRES F1 --------

#[test]
fn precondition_3_resumed_then_idle_past_floor_fires() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    // woken_at 90 days ago; requester resumed 60 days ago, then idle for 60 days > floor.
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 90);
    let last_activity = now - Duration::from_secs(60 * 60 * 24 * 60);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(outcome, RespawnExecuted::Respawned { new_generation: 1 });
    assert_eq!(respawner.count(), 1);
}

// ----- AC: cap enforcement — cap=2 means gen>=2 triggers F3 --------------

#[test]
fn cap_exceeded_yields_f3_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = make_header("requester", Some("scrutineer"));
    header.respawn_generation = Some(2); // cap == 2
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::Abandoned {
            cause: F3Cause::RespawnGenerationCapExceeded
        }
    );
    // No substrate call.
    assert_eq!(respawner.count(), 0);
    // RequesterRespawn event present with cap-exceeded reason.
    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(
            fields["reason"],
            serde_json::json!("respawn_failed: respawn_generation_cap_exceeded")
        );
    }
    // WorkflowCancelled landed.
    let cancel_reason = find_workflow_cancelled(&state_file).unwrap();
    assert_eq!(
        cancel_reason,
        "respawn_failed: respawn_generation_cap_exceeded"
    );
}

// ----- AC: F3 missing role -----------------------------------------------

#[test]
fn f3_missing_role_yields_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", None); // role == None
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::Abandoned {
            cause: F3Cause::MissingRole
        }
    );
    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(
            fields["reason"],
            serde_json::json!("respawn_failed: missing_role")
        );
    }
}

// ----- AC: F3 template_not_found -----------------------------------------

#[test]
fn f3_template_missing_yields_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: false,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::Abandoned {
            cause: F3Cause::TemplateNotFound
        }
    );
    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(
            fields["reason"],
            serde_json::json!("respawn_failed: template_not_found")
        );
    }
}

// ----- AC: F3 substrate_refused — substrate primitive returns error ------

#[test]
fn f3_substrate_refused_yields_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    respawner.fail_next();
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    assert_eq!(
        outcome,
        RespawnExecuted::Abandoned {
            cause: F3Cause::SubstrateRefused
        }
    );
    // The respawner errored; nothing was recorded.
    assert_eq!(respawner.count(), 0);
    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(
            fields["reason"],
            serde_json::json!("respawn_failed: substrate_refused")
        );
    }
    let cancel_reason = find_workflow_cancelled(&state_file).unwrap();
    assert_eq!(cancel_reason, "respawn_failed: substrate_refused");
}

// ----- AC: respawn_generation increments across cycles -------------------

#[test]
fn respawn_generation_increments_across_cycles() {
    let tmp = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();

    // gen=0 → F1 fires → gen=1
    {
        let header = make_header("requester-0", Some("scrutineer"));
        write_requester_state(tmp.path(), "requester-0", &header);
        let sid = ValidatedSessionId::new("requester-0").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                backend: &store(tmp.path()),
                header: &header,
                coord_id: "coord",
                requester_session_id: &sid,
                woken_at: Some(woken_at),
                last_log_activity: last_activity,
                now,
                retention_floor: floor(),
                cap: 2,
                template_exists: true,
            },
            &respawner,
        )
        .unwrap();
        assert_eq!(outcome, RespawnExecuted::Respawned { new_generation: 1 });
    }

    // gen=1 → F1 fires → gen=2
    {
        let mut header = make_header("requester-1", Some("scrutineer"));
        header.respawn_generation = Some(1);
        write_requester_state(tmp.path(), "requester-1", &header);
        let sid = ValidatedSessionId::new("requester-1").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                backend: &store(tmp.path()),
                header: &header,
                coord_id: "coord",
                requester_session_id: &sid,
                woken_at: Some(woken_at),
                last_log_activity: last_activity,
                now,
                retention_floor: floor(),
                cap: 2,
                template_exists: true,
            },
            &respawner,
        )
        .unwrap();
        assert_eq!(outcome, RespawnExecuted::Respawned { new_generation: 2 });
    }

    // gen=2 (cap met) → F1 refuses → F3 cap_exceeded
    {
        let mut header = make_header("requester-2", Some("scrutineer"));
        header.respawn_generation = Some(2);
        write_requester_state(tmp.path(), "requester-2", &header);
        let sid = ValidatedSessionId::new("requester-2").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                backend: &store(tmp.path()),
                header: &header,
                coord_id: "coord",
                requester_session_id: &sid,
                woken_at: Some(woken_at),
                last_log_activity: last_activity,
                now,
                retention_floor: floor(),
                cap: 2,
                template_exists: true,
            },
            &respawner,
        )
        .unwrap();
        assert_eq!(
            outcome,
            RespawnExecuted::Abandoned {
                cause: F3Cause::RespawnGenerationCapExceeded
            }
        );
    }
}

// ----- AC: pre-request-store fixture compatibility — round-trip ---------------------

#[test]
fn pre_request_store_fixture_compatibility() {
    // A pre-Issue-16 header on disk has no `respawn_generation`
    // field. The serde-additive contract requires:
    //   1. Deserialize OK with respawn_generation == None.
    //   2. Round-trip on write: serialize omits the field.
    let pre_request_store_json = serde_json::json!({
        "schema_version": 1,
        "workflow": "legacy-wf",
        "template_hash": "deadbeef",
        "created_at": "2026-05-24T00:00:00Z",
        "session_id": "legacy-wf",
        "dispatch_epoch": 0
    });
    let header: StateFileHeader = serde_json::from_value(pre_request_store_json).unwrap();
    assert_eq!(header.respawn_generation, None);

    // Round-trip: serialize and confirm the field is absent.
    let s = serde_json::to_string(&header).unwrap();
    assert!(!s.contains("respawn_generation"));
}

// ----- AC: resume-context prompt is fixed-form ---------------------------

#[test]
fn resume_context_prompt_is_fixed_form_snapshot() {
    // Snapshot test: any drift from the committed template is a
    // deliberate breaking change requiring a test update.
    assert_eq!(
        RESUME_CONTEXT_PROMPT,
        "You are resuming session <id>. Read your prior state via `koto status <id>` and prior children via `koto workflows --children <id>`; advance from where you left off."
    );
    let id = ValidatedSessionId::new("test-session").unwrap();
    let rendered = render_resume_context_prompt(&id);
    assert_eq!(
        rendered,
        "You are resuming session test-session. Read your prior state via `koto status test-session` and prior children via `koto workflows --children test-session`; advance from where you left off."
    );
}

// ----- AC: respawn request carries the saved role/template/inputs --------

#[test]
fn respawn_request_carries_saved_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = make_header("requester", Some("custom-role"));
    header.template_name = Some("custom-template".into());
    header.inputs = Some(serde_json::json!({"draft_path": "docs/draft.md"}));
    write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let _ = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    let req = respawner.last();
    assert_eq!(req.role, "custom-role");
    assert_eq!(req.template_name, "custom-template");
    assert_eq!(
        req.inputs,
        Some(serde_json::json!({"draft_path": "docs/draft.md"}))
    );
    assert_eq!(req.coord_id, "coord");
}

// ----- AC: RequesterRespawn uses the audit helper (kind constant from audit.rs) --

#[test]
fn requester_respawn_uses_audit_helper_kind_constant() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let _ = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    // The event's `kind` value must equal the REQUESTER_RESPAWN
    // constant from audit.rs (not a hand-typed string).
    let payload = find_respawn_event(&state_file);
    if let EventPayload::EvidenceSubmitted { fields, .. } = payload {
        assert_eq!(fields["kind"], serde_json::json!(REQUESTER_RESPAWN));
    }
}

// ----- AC: no_op outcomes do NOT emit any events -------------------------

#[test]
fn no_op_outcomes_emit_no_events() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", Some("scrutineer"));
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24); // 1 day — under floor
    let respawner = RecordingRespawner::default();
    let _ = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: now,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    // Confirm no RequesterRespawn or WorkflowCancelled appended.
    let (_, events) = read_events(&state_file).unwrap();
    for e in &events {
        match &e.payload {
            EventPayload::EvidenceSubmitted { fields, .. } => {
                assert_ne!(
                    fields.get("kind").and_then(|v| v.as_str()),
                    Some(REQUESTER_RESPAWN),
                    "no_op must not emit RequesterRespawn"
                );
            }
            EventPayload::WorkflowCancelled { .. } => {
                panic!("no_op must not emit WorkflowCancelled");
            }
            _ => {}
        }
    }
}

// ----- AC: WorkflowCancelled is emitted alongside F3 RequesterRespawn ---

#[test]
fn f3_paths_emit_workflow_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let header = make_header("requester", None); // missing role → F3
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let _ = execute_respawn(
        &RespawnExecution {
            backend: &store(tmp.path()),
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: last_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        },
        &respawner,
    )
    .unwrap();
    let cancel_reason = find_workflow_cancelled(&state_file).unwrap();
    assert!(cancel_reason.starts_with("respawn_failed: "));
}

// ----- Writes go through the session backend's commit funnel ---------------
//
// The executor appends to the requester's log through
// `SessionBackend::append_event`, so the store's post-commit hooks see a
// respawn-fallback cancel exactly as they see `koto cancel`: the run
// journal (`<base>/_run_journal.jsonl` for a store on an explicit base)
// records it as `cancelled`. The header is never written.

/// The respawn paths, one per outcome that writes events.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RespawnPath {
    Respawned,
    MissingRole,
    CapExceeded,
    TemplateNotFound,
    SubstrateRefused,
}

const FALLBACKS: [RespawnPath; 4] = [
    RespawnPath::MissingRole,
    RespawnPath::CapExceeded,
    RespawnPath::TemplateNotFound,
    RespawnPath::SubstrateRefused,
];

/// One respawn run against a fresh store in its own temporary directory.
struct Run {
    tmp: tempfile::TempDir,
    state_file: PathBuf,
    header_line_before: String,
    result: anyhow::Result<RespawnExecuted>,
    appended: Vec<(String, String)>,
}

fn header_line(state_file: &std::path::Path) -> String {
    let text = std::fs::read_to_string(state_file).unwrap();
    text.lines().next().unwrap().to_string()
}

/// Event types on the requester's log, in order.
fn event_types(state_file: &std::path::Path) -> Vec<String> {
    let (_, events) = read_events(state_file).unwrap();
    events
        .iter()
        .map(|e| e.payload.type_name().to_string())
        .collect()
}

/// Journal records for `session` in the store at `base`.
fn journal_records(base: &std::path::Path, session: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(base.join("_run_journal.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|r| r["session"] == session)
        .collect()
}

fn run_path(path: RespawnPath, refuse: Option<&'static str>) -> Run {
    // The run journal reads the driving session from this process's
    // environment on every commit, and an in-process run has no child
    // process to clear it on. Clear it here, so a suite run inside a Claude
    // Code session records no driver and journals only what each test
    // expects.
    std::env::remove_var("CLAUDE_CODE_SESSION_ID");
    let tmp = tempfile::tempdir().unwrap();
    let role = (path != RespawnPath::MissingRole).then_some("scrutineer");
    let mut header = make_header("requester", role);
    if path == RespawnPath::CapExceeded {
        header.respawn_generation = Some(2);
    }
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let header_line_before = header_line(&state_file);
    let backend = match refuse {
        Some(event_type) => FunnelBackend::refusing(tmp.path(), event_type),
        None => FunnelBackend::new(tmp.path()),
    };
    let respawner = RecordingRespawner::default();
    if path == RespawnPath::SubstrateRefused {
        respawner.fail_next();
    }
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let result = execute_respawn(
        &RespawnExecution {
            backend: &backend,
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: woken_at - Duration::from_secs(60),
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: path != RespawnPath::TemplateNotFound,
        },
        &respawner,
    );
    let appended = backend.appended();
    Run {
        tmp,
        state_file,
        header_line_before,
        result,
        appended,
    }
}

#[test]
fn every_respawn_write_goes_through_the_backend_and_leaves_the_header_untouched() {
    for path in [RespawnPath::Respawned]
        .into_iter()
        .chain(FALLBACKS.iter().copied())
    {
        let run = run_path(path, None);
        run.result.as_ref().unwrap();
        let expected: Vec<&str> = if path == RespawnPath::Respawned {
            vec!["evidence_submitted"]
        } else {
            vec!["evidence_submitted", "workflow_cancelled"]
        };
        // The same events in the same order as before the switch.
        assert_eq!(event_types(&run.state_file), expected, "{path:?}");
        // Each one appended through the backend, to the requester's log.
        let through_backend: Vec<(String, String)> = expected
            .iter()
            .map(|t| ("requester".to_string(), t.to_string()))
            .collect();
        assert_eq!(run.appended, through_backend, "{path:?}");
        // The header line is byte-identical.
        assert_eq!(
            header_line(&run.state_file),
            run.header_line_before,
            "{path:?}"
        );
    }
}

#[test]
fn a_respawn_fallback_cancel_journals_one_cancelled_record_after_the_event_commits() {
    for path in FALLBACKS {
        let run = run_path(path, None);
        assert!(
            matches!(run.result, Ok(RespawnExecuted::Abandoned { .. })),
            "{path:?}: {:?}",
            run.result
        );
        let records = journal_records(run.tmp.path(), "requester");
        let kinds: Vec<&str> = records
            .iter()
            .map(|r| r["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec!["cancelled"],
            "{path:?}: one record, no terminal"
        );
        assert_eq!(records[0]["koto.session.id"], "requester", "{path:?}");

        // The cancel event is committed on the log, and the journal file
        // was last written no earlier than the log. Timestamps can tie, so
        // this alone doesn't order the two writes: the test below adds the
        // other half, that a cancel whose append fails writes no record.
        assert_eq!(
            event_types(&run.state_file).last().map(String::as_str),
            Some("workflow_cancelled"),
            "{path:?}"
        );
        let log_written = std::fs::metadata(&run.state_file)
            .unwrap()
            .modified()
            .unwrap();
        let journal_written = std::fs::metadata(run.tmp.path().join("_run_journal.jsonl"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(journal_written >= log_written, "{path:?}");
    }

    // A successful respawn cancels nothing and journals nothing.
    let run = run_path(RespawnPath::Respawned, None);
    assert_eq!(
        run.result.unwrap(),
        RespawnExecuted::Respawned { new_generation: 1 }
    );
    assert!(journal_records(run.tmp.path(), "requester").is_empty());
}

#[test]
fn a_failed_cancel_append_writes_no_cancelled_record_and_fails_as_before() {
    for path in FALLBACKS {
        let run = run_path(path, Some("workflow_cancelled"));
        // The error the executor returned when it wrote the state file
        // directly: the append's own error under
        // `append WorkflowCancelled to <state file>`.
        let err = run.result.expect_err("the cancel append was refused");
        assert_eq!(
            format!("{err:#}"),
            format!(
                "append WorkflowCancelled to {}: {}",
                run.state_file.display(),
                funnel_backend::INJECTED_FAILURE
            ),
            "{path:?}"
        );
        // The session is left as a failed cancel left it: the respawn
        // evidence committed, no cancel, the header untouched.
        assert_eq!(
            event_types(&run.state_file),
            vec!["evidence_submitted"],
            "{path:?}"
        );
        assert_eq!(
            header_line(&run.state_file),
            run.header_line_before,
            "{path:?}"
        );
        // And no `cancelled` record.
        assert!(
            journal_records(run.tmp.path(), "requester").is_empty(),
            "{path:?}"
        );
    }
}
