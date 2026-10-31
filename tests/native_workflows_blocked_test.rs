//! A gate-blocked session's `/workflows` entry, read back from the file koto
//! writes.
//!
//! Each test drives a real local session through the commit funnel
//! (`LocalBackend::append_event`, which materializes the entry) into a state
//! whose gate was evaluated, then reads the written `koto-<uuid>.json`.
//! Claude Code 2.1.314 counts every top-level status other than `running` as
//! completed and draws a `progress` step on a run that isn't running as
//! Stopped, so a blocked session has to be written as `running` with an
//! `error` step flagged `blocked` and the failing gate as its text.

#[path = "support/host_session.rs"]
mod host_session;

use koto::engine::types::{Event, EventPayload, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use koto::workflows_surface::{publish_location, workflow_filename};

const WORKFLOW: &str = "impl-feature";
const SESSION_UUID: &str = "blocked-uuid";

/// gather_context -> implement -> verify -> review (terminal), with a command
/// gate on `verify`.
const TEMPLATE: &str = r#"{
    "format_version": 1,
    "name": "impl-feature",
    "version": "1.0",
    "initial_state": "gather_context",
    "states": {
        "gather_context": {
            "directive": "Read the issue and map the module.",
            "transitions": [{"target": "implement"}]
        },
        "implement": {
            "directive": "Implement the change per the gathered context.",
            "transitions": [{"target": "verify"}]
        },
        "verify": {
            "directive": "Run the test suite and submit the result.",
            "transitions": [{"target": "review"}],
            "gates": {"tests-pass": {"type": "command", "command": "cargo test"}}
        },
        "review": {
            "directive": "Review the change and finish.",
            "terminal": true
        }
    }
}"#;

fn header() -> StateFileHeader {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": WORKFLOW,
        "session_id": SESSION_UUID,
        "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn event(seq: u64, kind: &str, payload: serde_json::Value) -> Event {
    serde_json::from_value(serde_json::json!({
        "seq": seq,
        "timestamp": "2026-01-01T00:00:00Z",
        "type": kind,
        "payload": payload,
    }))
    .unwrap()
}

fn payload(kind: &str, payload: serde_json::Value) -> EventPayload {
    event(0, kind, payload).payload
}

/// A session with the template in place and its `/workflows` location
/// published, driven to `verify`.
fn session_at_verify(base: &std::path::Path, wf_dir: &std::path::Path) -> LocalBackend {
    let backend = LocalBackend::with_base_dir(base.to_path_buf());
    let init = event(
        1,
        "workflow_initialized",
        serde_json::json!({"template_path": "template.json", "variables": {}}),
    );
    backend
        .init_state_file(WORKFLOW, header(), vec![init])
        .unwrap();
    std::fs::write(
        backend.session_dir(WORKFLOW).join("template.json"),
        TEMPLATE,
    )
    .unwrap();
    publish_location(&backend, &backend, WORKFLOW, wf_dir.to_str().unwrap()).unwrap();
    for to in ["implement", "verify"] {
        backend
            .append_event(
                WORKFLOW,
                &payload(
                    "transitioned",
                    serde_json::json!({"from": null, "to": to, "condition_type": "evidence"}),
                ),
                "2026-01-01T00:01:00Z",
            )
            .unwrap();
    }
    backend
}

fn evaluate_gate(backend: &LocalBackend, gate: &str, outcome: &str) {
    backend
        .append_event(
            WORKFLOW,
            &payload(
                "gate_evaluated",
                serde_json::json!({
                    "state": "verify",
                    "gate": gate,
                    "output": {"exit_code": 1, "error": "captured gate output"},
                    "outcome": outcome,
                    "timestamp": "2026-01-01T00:02:00Z",
                }),
            ),
            "2026-01-01T00:02:00Z",
        )
        .unwrap();
}

fn read_entry(wf_dir: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(wf_dir.join(workflow_filename(SESSION_UUID)))
        .expect("the /workflows entry was written");
    serde_json::from_slice(&bytes).unwrap()
}

/// The active phase's `workflow_agent` step: the one for `Verify`.
fn verify_step(v: &serde_json::Value) -> serde_json::Value {
    v["workflowProgress"]
        .as_array()
        .expect("workflowProgress written")
        .iter()
        .find(|n| n["type"] == "workflow_agent" && n["phaseTitle"] == "Verify")
        .expect("a step for the active Verify phase")
        .clone()
}

/// tsukumogami/koto#553: a session stopped on a failing gate reads as
/// running with a Blocked step naming the gate, and koto's own
/// classification is in `koto.status`.
#[test]
fn a_gate_blocked_session_reads_blocked_in_the_running_bucket() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "failed");

    let v = read_entry(wf_dir.path());

    assert_eq!(
        v["status"], "running",
        "https://github.com/tsukumogami/koto/issues/553: a gate-blocked entry \
         must be written top-level running, not counted completed (got {})",
        v["status"]
    );
    assert_eq!(v["koto"]["status"], "blocked");
    assert_eq!(v["koto"]["contractVersion"], 5);

    let step = verify_step(&v);
    assert_eq!(step["state"], "error");
    assert_eq!(step["blocked"], true);
    assert_eq!(step["error"], "gate tests-pass: FAIL");

    let summary = v["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("koto \u{b7} impl-feature \u{b7} verify"),
        "{summary}"
    );
    assert!(summary.contains("blocked"), "{summary}");
    assert!(
        summary.ends_with(" \u{b7} blocked on gate tests-pass"),
        "{summary}"
    );

    // The gate's captured output reaches neither the step nor the summary.
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("captured gate output"), "{text}");
}

/// A passing gate leaves the running entry as it was: a `progress` step with
/// no blocked or error field and the plain summary.
#[test]
fn a_passing_gate_keeps_the_running_shape() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "passed");

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "running");
    assert_eq!(v["koto"]["status"], "running");
    let step = verify_step(&v);
    assert_eq!(step["state"], "progress");
    assert!(step.get("blocked").is_none(), "{step}");
    assert!(step.get("error").is_none(), "{step}");
    assert_eq!(v["summary"], "koto \u{b7} impl-feature \u{b7} verify");
}

/// A gate name carrying a control character reaches the step's `error` and
/// the summary as a visible escape, never raw.
#[test]
fn a_control_character_in_the_gate_name_is_escaped() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "evil\u{1B}[2Jgate", "failed");

    let v = read_entry(wf_dir.path());
    let step = verify_step(&v);
    let error = step["error"].as_str().unwrap();
    let summary = v["summary"].as_str().unwrap();
    for text in [error, summary] {
        assert!(!text.contains('\u{1B}'), "raw escape character in {text:?}");
        assert!(text.contains("evil\\u{1B}[2Jgate"), "{text:?}");
    }
    assert_eq!(error, "gate evil\\u{1B}[2Jgate: FAIL");
    assert!(
        summary.ends_with(" \u{b7} blocked on gate evil\\u{1B}[2Jgate"),
        "{summary}"
    );
}

// ---------------------------------------------------------------------------
// Unblocking and edge cases
// ---------------------------------------------------------------------------

/// Append a state-changing event of `kind` (`transitioned` or `rewound`)
/// moving the session from `from` to `to`.
fn move_to(backend: &LocalBackend, kind: &str, from: &str, to: &str) {
    let body = match kind {
        "rewound" => serde_json::json!({"from": from, "to": to}),
        _ => serde_json::json!({"from": from, "to": to, "condition_type": "evidence"}),
    };
    backend
        .append_event(WORKFLOW, &payload(kind, body), "2026-01-01T00:03:00Z")
        .unwrap();
}

/// No `workflow_agent` step is flagged blocked and the summary says nothing
/// about being blocked.
fn assert_not_blocked(v: &serde_json::Value) {
    assert_ne!(v["koto"]["status"], "blocked", "{v}");
    let steps = v["workflowProgress"].as_array().expect("workflowProgress");
    for step in steps.iter().filter(|n| n["type"] == "workflow_agent") {
        assert!(step.get("blocked").is_none(), "a blocked step: {step}");
    }
    let summary = v["summary"].as_str().unwrap();
    assert!(!summary.contains("blocked"), "{summary}");
}

/// A passing evaluation of the gate that blocked the session clears the
/// reading on the next write.
#[test]
fn a_passing_reevaluation_clears_the_blocked_reading() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "failed");
    assert_eq!(read_entry(wf_dir.path())["koto"]["status"], "blocked");

    evaluate_gate(&backend, "tests-pass", "passed");

    let v = read_entry(wf_dir.path());
    assert_not_blocked(&v);
    assert_eq!(v["koto"]["status"], "running");
    assert_eq!(verify_step(&v)["state"], "progress");
}

/// Advancing out of the blocked state clears the reading.
#[test]
fn an_advance_clears_the_blocked_reading() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "failed");
    assert_eq!(read_entry(wf_dir.path())["koto"]["status"], "blocked");

    move_to(&backend, "transitioned", "verify", "review");

    let v = read_entry(wf_dir.path());
    assert_not_blocked(&v);
    assert_eq!(v["koto"]["currentState"], "review", "{v}");
}

/// Rewinding to an earlier state clears the reading.
#[test]
fn a_rewind_clears_the_blocked_reading() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "failed");
    assert_eq!(read_entry(wf_dir.path())["koto"]["status"], "blocked");

    move_to(&backend, "rewound", "verify", "implement");

    let v = read_entry(wf_dir.path());
    assert_not_blocked(&v);
    assert_eq!(v["status"], "running");
    assert_eq!(v["koto"]["status"], "running");
    assert_eq!(v["koto"]["currentState"], "implement", "{v}");
}

/// Leaving the blocked state and entering it again starts a new epoch: with
/// no gate evaluated since, the session isn't blocked and the re-entered
/// step carries no `error`.
#[test]
fn reentering_without_a_new_evaluation_clears_the_blocked_reading() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    evaluate_gate(&backend, "tests-pass", "failed");
    assert_eq!(read_entry(wf_dir.path())["koto"]["status"], "blocked");

    move_to(&backend, "transitioned", "verify", "implement");
    move_to(&backend, "transitioned", "implement", "verify");

    let v = read_entry(wf_dir.path());
    assert_not_blocked(&v);
    assert_eq!(v["koto"]["status"], "running");
    let step = verify_step(&v);
    assert_eq!(step["state"], "progress");
    assert!(step.get("error").is_none(), "{step}");
    assert_eq!(v["summary"], "koto \u{b7} impl-feature \u{b7} verify");
}

/// With two gates in one state the last one evaluated decides, and it is
/// the one named.
#[test]
fn the_last_evaluated_of_two_gates_decides_and_is_named() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());

    evaluate_gate(&backend, "gate-a", "failed");
    evaluate_gate(&backend, "gate-b", "passed");
    let v = read_entry(wf_dir.path());
    assert_not_blocked(&v);
    assert_eq!(v["koto"]["status"], "running");

    evaluate_gate(&backend, "gate-a", "passed");
    evaluate_gate(&backend, "gate-b", "failed");
    let v = read_entry(wf_dir.path());
    assert_eq!(v["koto"]["status"], "blocked");
    let step = verify_step(&v);
    assert_eq!(step["blocked"], true);
    assert_eq!(step["error"], "gate gate-b: FAIL");
    let summary = v["summary"].as_str().unwrap();
    assert!(
        summary.ends_with(" \u{b7} blocked on gate gate-b"),
        "{summary}"
    );
    assert!(!summary.contains("gate-a"), "{summary}");
}

/// A gate-blocked session whose compiled template can't be read has no phase
/// list, and still reads blocked through its top-level status, `koto.status`
/// and summary.
#[test]
fn a_blocked_session_without_a_readable_template_still_reads_blocked() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session_at_verify(base.path(), wf_dir.path());
    std::fs::remove_file(backend.session_dir(WORKFLOW).join("template.json")).unwrap();

    evaluate_gate(&backend, "tests-pass", "failed");

    let v = read_entry(wf_dir.path());
    assert_eq!(v["status"], "running");
    assert_eq!(v["koto"]["status"], "blocked");
    let summary = v["summary"].as_str().unwrap();
    assert!(summary.contains("blocked"), "{summary}");
    assert!(
        summary.ends_with(" \u{b7} blocked on gate tests-pass"),
        "{summary}"
    );
    assert!(v.get("workflowProgress").is_none(), "{v}");
}

/// A published target that can't be written (a regular file sits where its
/// directory should be) never fails the commit: every append succeeds and
/// the session's state moves as it would with nothing rendered.
#[test]
fn an_unwritable_target_never_fails_the_commit() {
    let base = tempfile::TempDir::new().unwrap();
    let parent = tempfile::TempDir::new().unwrap();
    let blocker = parent.path().join("not-a-dir");
    std::fs::write(&blocker, b"a regular file").unwrap();
    let target = blocker.join("workflows");

    let backend = session_at_verify(base.path(), &target);
    evaluate_gate(&backend, "tests-pass", "failed");
    move_to(&backend, "transitioned", "verify", "review");

    assert!(blocker.is_file(), "the regular file is left as it was");
    let (_, events) = backend.read_events(WORKFLOW).unwrap();
    assert_eq!(
        koto::engine::types::derive_state_from_log(&events).as_deref(),
        Some("review")
    );
    assert!(
        events.iter().any(|e| matches!(
            &e.payload,
            EventPayload::GateEvaluated { gate, outcome, .. }
                if gate == "tests-pass" && outcome == "failed"
        )),
        "the gate evaluation was committed"
    );
}

/// A session cancelled before it entered any state has no current state; no
/// step's `error`, phase detail or summary ends in `at ` with an empty name.
#[test]
fn a_failed_session_with_no_current_state_writes_no_empty_state_name() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
    let init = event(
        1,
        "workflow_initialized",
        serde_json::json!({"template_path": "template.json", "variables": {}}),
    );
    backend
        .init_state_file(WORKFLOW, header(), vec![init])
        .unwrap();
    std::fs::write(
        backend.session_dir(WORKFLOW).join("template.json"),
        TEMPLATE,
    )
    .unwrap();
    publish_location(
        &backend,
        &backend,
        WORKFLOW,
        wf_dir.path().to_str().unwrap(),
    )
    .unwrap();
    backend
        .append_event(
            WORKFLOW,
            &payload(
                "workflow_cancelled",
                serde_json::json!({"state": "", "reason": "abandoned"}),
            ),
            "2026-01-01T00:01:00Z",
        )
        .unwrap();

    let v = read_entry(wf_dir.path());
    assert_eq!(v["status"], "failed");
    assert_eq!(v["koto"]["status"], "failed");
    assert!(v["koto"]["currentState"].is_null(), "{v}");
    for step in v["workflowProgress"].as_array().into_iter().flatten() {
        if let Some(error) = step.get("error").and_then(|e| e.as_str()) {
            assert!(!error.is_empty(), "{step}");
            assert!(!error.trim_end().ends_with(" at"), "{step}");
        }
    }
    for phase in v["phases"].as_array().into_iter().flatten() {
        let detail = phase["detail"].as_str().unwrap_or("");
        assert!(!detail.trim_end().ends_with(" at"), "{phase}");
    }
    let summary = v["summary"].as_str().unwrap();
    assert!(!summary.trim_end().ends_with(" at"), "{summary}");
}
