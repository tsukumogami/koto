//! A finished or failed session's `/workflows` entry, read back from the file
//! koto writes.
//!
//! Each test drives a real local session through the commit funnel
//! (`LocalBackend::append_event`, which materializes the entry) to a terminal
//! state or a cancellation, then reads the written `koto-<uuid>.json`.
//! Claude Code 2.1.314 counts a phase's `done` steps over all its steps and
//! draws a `progress` step on a run that isn't running as Stopped, so a
//! completed session has to write every step `done`, and a failed one has to
//! write its last step as an `error` that says why.

#[path = "support/host_session.rs"]
mod host_session;

use koto::engine::types::{Event, EventPayload, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use koto::workflows_surface::{publish_location, workflow_filename};

const WORKFLOW: &str = "legibility";
const SESSION_UUID: &str = "finished-uuid";

/// gather -> verify -> done (terminal), the three-phase shape of the
/// reproduction in koto#554, with `verify` able to end at a `failed`
/// terminal instead.
const TEMPLATE: &str = r#"{
    "format_version": 1,
    "name": "legibility-check",
    "version": "1.0",
    "initial_state": "gather",
    "states": {
        "gather": {
            "directive": "Collect the findings and submit them as findings evidence.",
            "transitions": [{"target": "verify"}]
        },
        "verify": {
            "directive": "Create ready.flag in the working directory.",
            "transitions": [{"target": "done"}, {"target": "failed"}],
            "gates": {"ready_flag": {"type": "command", "command": "test -f ready.flag"}}
        },
        "done": {
            "directive": "The check is complete.",
            "terminal": true
        },
        "failed": {
            "directive": "The check failed.",
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

/// A session on `template` with its `/workflows` location published, sitting
/// at the template's initial state.
fn session(base: &std::path::Path, wf_dir: &std::path::Path, template: &str) -> LocalBackend {
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
        template,
    )
    .unwrap();
    publish_location(&backend, &backend, WORKFLOW, wf_dir.to_str().unwrap()).unwrap();
    backend
}

fn append(backend: &LocalBackend, kind: &str, body: serde_json::Value) {
    backend
        .append_event(WORKFLOW, &payload(kind, body), "2026-01-01T00:01:00Z")
        .unwrap();
}

fn transition(backend: &LocalBackend, to: &str) {
    append(
        backend,
        "transitioned",
        serde_json::json!({"from": null, "to": to, "condition_type": "evidence"}),
    );
}

fn cancel(backend: &LocalBackend, state: &str) {
    append(
        backend,
        "workflow_cancelled",
        serde_json::json!({"state": state, "reason": "operator cancelled"}),
    );
}

/// Drive gather (with evidence) and verify (with a passing gate), landing on
/// `verify`.
fn drive_to_verify(backend: &LocalBackend) {
    append(
        backend,
        "evidence_submitted",
        serde_json::json!({"state": "gather", "fields": {"findings": "collected"}}),
    );
    transition(backend, "verify");
    append(
        backend,
        "gate_evaluated",
        serde_json::json!({
            "state": "verify",
            "gate": "ready_flag",
            "output": {"exit_code": 0},
            "outcome": "passed",
            "timestamp": "2026-01-01T00:02:00Z",
        }),
    );
}

fn read_entry(wf_dir: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(wf_dir.join(workflow_filename(SESSION_UUID)))
        .expect("the /workflows entry was written");
    serde_json::from_slice(&bytes).unwrap()
}

fn agent_steps(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v["workflowProgress"]
        .as_array()
        .expect("workflowProgress written")
        .iter()
        .filter(|n| n["type"] == "workflow_agent")
        .cloned()
        .collect()
}

/// The `workflow_agent` step of the phase titled `title`.
fn step(v: &serde_json::Value, title: &str) -> serde_json::Value {
    agent_steps(v)
        .into_iter()
        .find(|n| n["phaseTitle"] == title)
        .unwrap_or_else(|| panic!("a step for the {title} phase"))
}

/// The `detail` of the phase titled `title`.
fn phase_detail(v: &serde_json::Value, title: &str) -> String {
    v["phases"]
        .as_array()
        .expect("phases written")
        .iter()
        .find(|p| p["title"] == title)
        .unwrap_or_else(|| panic!("a {title} phase"))["detail"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// tsukumogami/koto#554: a session that reached its terminal state reads as
/// finished all the way down: every step `done`, none `progress`, and the
/// terminal phase not `in progress`.
#[test]
fn a_completed_session_writes_every_step_done() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session(base.path(), wf_dir.path(), TEMPLATE);
    drive_to_verify(&backend);
    transition(&backend, "done");

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "completed");
    assert_eq!(v["koto"]["status"], "completed");

    let steps = agent_steps(&v);
    assert_eq!(steps.len(), 3, "one step per entered phase: {steps:?}");
    let states: Vec<&str> = steps.iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(
        states,
        vec!["done", "done", "done"],
        "https://github.com/tsukumogami/koto/issues/554: a completed session's \
         terminal step must be written done, not left in progress (got {states:?})"
    );

    let terminal = step(&v, "Done");
    assert!(terminal.get("blocked").is_none(), "{terminal}");
    assert!(terminal.get("error").is_none(), "{terminal}");

    assert_eq!(phase_detail(&v, "Gather"), "evidence: findings");
    assert_eq!(phase_detail(&v, "Verify"), "gate ready_flag: PASS");
    assert_eq!(phase_detail(&v, "Done"), "done");
}

/// A session cancelled at a non-terminal state reads failed, its last step an
/// `error` naming the state it was cancelled at, not a Blocked one.
#[test]
fn a_cancelled_session_writes_its_step_failed_with_where_it_was_cancelled() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session(base.path(), wf_dir.path(), TEMPLATE);
    append(
        &backend,
        "evidence_submitted",
        serde_json::json!({"state": "gather", "fields": {"findings": "collected"}}),
    );
    transition(&backend, "verify");
    cancel(&backend, "verify");

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "failed");
    assert_eq!(v["koto"]["status"], "failed");
    let active = step(&v, "Verify");
    assert_eq!(active["state"], "error", "{active}");
    assert!(active.get("blocked").is_none(), "{active}");
    assert_eq!(active["error"], "cancelled at verify");
    assert_eq!(step(&v, "Gather")["state"], "done");
    // The phase has no gate or evidence line, so it shows the failure.
    assert_eq!(phase_detail(&v, "Verify"), "cancelled at verify");
}

/// A session that ended at a terminal state named `failed` reads failed, its
/// last step an `error` naming the state it ended at.
#[test]
fn a_session_ended_at_a_failure_terminal_writes_where_it_ended() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session(base.path(), wf_dir.path(), TEMPLATE);
    drive_to_verify(&backend);
    transition(&backend, "failed");

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "failed");
    assert_eq!(v["koto"]["status"], "failed");
    let active = step(&v, "Failed");
    assert_eq!(active["state"], "error", "{active}");
    assert!(active.get("blocked").is_none(), "{active}");
    assert_eq!(active["error"], "ended at failed");
    assert_eq!(phase_detail(&v, "Failed"), "ended at failed");
    // The phases it passed through keep their own outcome lines.
    assert_eq!(phase_detail(&v, "Verify"), "gate ready_flag: PASS");
}

/// A session cancelled while it sat at a failure terminal is classified by
/// the state it ended at, so it gets the `ended at` line.
#[test]
fn a_cancelled_session_at_a_failure_terminal_writes_where_it_ended() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session(base.path(), wf_dir.path(), TEMPLATE);
    drive_to_verify(&backend);
    transition(&backend, "failed");
    cancel(&backend, "failed");

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "failed");
    assert_eq!(step(&v, "Failed")["error"], "ended at failed");
}

/// The state name inside a failed step's `error` is escaped, so a control
/// character in it reaches the file as a visible escape, never raw.
#[test]
fn a_control_character_in_a_failed_state_name_is_escaped() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let template = TEMPLATE.replace("\"verify\"", "\"ver\\u001bify\"");
    let backend = session(base.path(), wf_dir.path(), &template);
    transition(&backend, "ver\u{1B}ify");
    cancel(&backend, "ver\u{1B}ify");

    let v = read_entry(wf_dir.path());
    let error = agent_steps(&v)
        .into_iter()
        .find(|s| s["state"] == "error")
        .expect("a failed step")["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        !error.contains('\u{1B}'),
        "raw escape character in {error:?}"
    );
    assert_eq!(error, "cancelled at ver\\u{1B}ify");
}

/// A running session's entry is untouched: a `progress` step with no blocked
/// or error field and the plain summary.
#[test]
fn a_running_session_keeps_the_progress_step() {
    let base = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let backend = session(base.path(), wf_dir.path(), TEMPLATE);
    drive_to_verify(&backend);

    let v = read_entry(wf_dir.path());

    assert_eq!(v["status"], "running");
    assert_eq!(v["koto"]["status"], "running");
    let active = step(&v, "Verify");
    assert_eq!(active["state"], "progress");
    assert!(active.get("blocked").is_none(), "{active}");
    assert!(active.get("error").is_none(), "{active}");
    assert_eq!(v["summary"], "koto \u{b7} legibility \u{b7} verify");
}
