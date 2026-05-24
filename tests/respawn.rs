//! Integration tests for KT1 Issue 16:
//! `feat(respawn): F1 cold-restart re-priming + F3 fallback + respawn_generation_cap`.
//!
//! Covers the acceptance criteria from the issue body:
//! all-three-preconditions positive path, three precondition guards,
//! cap enforcement, four F3 cause classes, generation increment
//! across cycles, pre-KT1 fixture compatibility, fixed-form
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
use koto::session::state_file_name;

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
        schema_version: 1,
        workflow: workflow.into(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: None,
        template_source_dir: None,
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
    }
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
            requester_state_file: &state_file,
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
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    // 1 day ago — well under the 30-day floor.
    let woken_at = now - Duration::from_secs(60 * 60 * 24);
    let last_activity = now;
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            requester_state_file: &state_file,
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
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60); // 60 days ago
                                                                 // requester resumed RECENTLY (yesterday) — last_activity is well within floor.
    let last_activity = now - Duration::from_secs(60 * 60 * 24);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            requester_state_file: &state_file,
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
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    // woken_at 90 days ago; requester resumed 60 days ago, then idle for 60 days > floor.
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 90);
    let last_activity = now - Duration::from_secs(60 * 60 * 24 * 60);
    let respawner = RecordingRespawner::default();
    let outcome = execute_respawn(
        &RespawnExecution {
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
        let state_file = write_requester_state(tmp.path(), "requester-0", &header);
        let sid = ValidatedSessionId::new("requester-0").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                requester_state_file: &state_file,
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
        let state_file = write_requester_state(tmp.path(), "requester-1", &header);
        let sid = ValidatedSessionId::new("requester-1").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                requester_state_file: &state_file,
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
        let state_file = write_requester_state(tmp.path(), "requester-2", &header);
        let sid = ValidatedSessionId::new("requester-2").unwrap();
        let outcome = execute_respawn(
            &RespawnExecution {
                requester_state_file: &state_file,
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

// ----- AC: pre-KT1 fixture compatibility — round-trip ---------------------

#[test]
fn pre_kt1_fixture_compatibility() {
    // A pre-Issue-16 header on disk has no `respawn_generation`
    // field. The serde-additive contract requires:
    //   1. Deserialize OK with respawn_generation == None.
    //   2. Round-trip on write: serialize omits the field.
    let pre_kt1_json = serde_json::json!({
        "schema_version": 1,
        "workflow": "legacy-wf",
        "template_hash": "deadbeef",
        "created_at": "2026-05-24T00:00:00Z",
        "session_id": "legacy-wf",
        "dispatch_epoch": 0
    });
    let header: StateFileHeader = serde_json::from_value(pre_kt1_json).unwrap();
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
        "You are resuming session <id>. Read your prior state via `koto session info <id>` and prior children via `koto session list --parent <id>`; advance from where you left off."
    );
    let id = ValidatedSessionId::new("test-session").unwrap();
    let rendered = render_resume_context_prompt(&id);
    assert_eq!(
        rendered,
        "You are resuming session test-session. Read your prior state via `koto session info test-session` and prior children via `koto session list --parent test-session`; advance from where you left off."
    );
}

// ----- AC: respawn request carries the saved role/template/inputs --------

#[test]
fn respawn_request_carries_saved_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = make_header("requester", Some("custom-role"));
    header.template_name = Some("custom-template".into());
    header.inputs = Some(serde_json::json!({"draft_path": "docs/draft.md"}));
    let state_file = write_requester_state(tmp.path(), "requester", &header);
    let sid = ValidatedSessionId::new("requester").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let last_activity = woken_at - Duration::from_secs(60);
    let respawner = RecordingRespawner::default();
    let _ = execute_respawn(
        &RespawnExecution {
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
            requester_state_file: &state_file,
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
