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
    std::fs::write(backend.session_dir(WORKFLOW).join("template.json"), TEMPLATE).unwrap();
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
