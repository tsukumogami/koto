//! A parent that routes out of its batching state on the tick a failed batch
//! completes is still offered `reserved_actions` in the state it reaches, as
//! long as that state routes a retry (Issue #277).
//!
//! The scheduler, which `reserved_actions` is normally built from, runs only
//! in a state that declares `materialize_children`. The koto-author example
//! coordinator routes to `analyze_failures` on `all_complete` +
//! `needs_attention`, so the tick that saw the batch fail stopped where the
//! scheduler never runs, and the retry invocation its directive tells the
//! agent to copy was missing.
//!
//! These tests drive the shipped example templates as written:
//! `plugins/koto-skills/skills/koto-author/references/examples/`
//! `batch-coordinator.md` and `batch-worker.md`.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

const EXAMPLES: &str = "plugins/koto-skills/skills/koto-author/references/examples";

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

/// Run koto and return `(success, last stdout line as JSON, stderr)`.
fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().last().unwrap_or("");
    let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (ok, json, stderr) = run_koto(dir, args);
    assert!(ok, "koto {args:?} failed: stderr={stderr} json={json}");
    json
}

/// Copy the example pair into `dir`, init `coord` from the coordinator, and
/// submit one task, `task-1`.
fn start_example_batch(dir: &Path) {
    let tasks = serde_json::json!({
        "tasks": [{"name": "task-1", "waits_on": [], "vars": {"ISSUE_NUMBER": "1"}}]
    });
    start_example_batch_with(dir, &tasks);
}

/// Copy the example pair into `dir`, init `coord`, and submit `tasks`.
fn start_example_batch_with(dir: &Path, tasks: &serde_json::Value) {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join(EXAMPLES);
    for name in ["batch-coordinator.md", "batch-worker.md"] {
        std::fs::copy(examples.join(name), dir.join(name)).unwrap();
    }
    std::fs::write(dir.join("plan.md"), "# Plan\n").unwrap();
    run_ok(
        dir,
        &[
            "init",
            "coord",
            "--template",
            dir.join("batch-coordinator.md").to_str().unwrap(),
            "--var",
            "plan_path=plan.md",
        ],
    );
    // The gate blocks while the children run, which exits non-zero.
    run_koto(dir, &["next", "coord", "--with-data", &tasks.to_string()]);
}

fn drive(dir: &Path, child: &str, data: serde_json::Value) {
    run_ok(
        dir,
        &[
            "next",
            child,
            "--no-cleanup",
            "--with-data",
            &data.to_string(),
        ],
    );
}

fn fail_task_1(dir: &Path) {
    let data = serde_json::json!({"status": "blocked", "failure_reason": "API quota exhausted"});
    run_ok(
        dir,
        &[
            "next",
            "coord.task-1",
            "--no-cleanup",
            "--with-data",
            &data.to_string(),
        ],
    );
}

/// Run `invocation` through a shell exactly as an agent would copy it, with
/// `koto` resolving to the binary under test.
fn run_invocation(dir: &Path, invocation: &str) -> serde_json::Value {
    let bin = assert_cmd::cargo::cargo_bin("koto");
    let bin_dir = bin.parent().unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(invocation)
        .current_dir(dir)
        .env("PATH", path)
        .env("KOTO_SESSIONS_BASE", sessions_base(dir))
        .env("HOME", dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "invocation {invocation:?} failed: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(stdout.lines().last().unwrap_or("")).unwrap()
}

fn retry_action(json: &serde_json::Value) -> &serde_json::Value {
    let actions = json["reserved_actions"]
        .as_array()
        .unwrap_or_else(|| panic!("no reserved_actions in {json}"));
    assert_eq!(actions.len(), 1, "{json}");
    assert_eq!(actions[0]["action"], "retry_failed", "{json}");
    &actions[0]
}

#[test]
fn the_example_coordinator_can_retry_from_analyze_failures() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_example_batch(dir);
    fail_task_1(dir);

    // The tick that sees the batch fail leaves `plan_and_await` for
    // `analyze_failures`, and still offers the retry.
    let json = run_ok(dir, &["next", "coord"]);
    assert_eq!(json["state"], "analyze_failures", "{json}");
    let action = retry_action(&json);
    assert_eq!(
        action["applies_to"],
        serde_json::json!(["task-1"]),
        "{json}"
    );

    // A later tick in the same state offers it again: an agent that
    // re-ticks or resumes is not left without it.
    let again = run_ok(dir, &["next", "coord"]);
    assert_eq!(again["state"], "analyze_failures", "{again}");
    assert_eq!(retry_action(&again), action, "the same action on a re-tick");

    // Running the invocation as written routes back to the batching state
    // and restarts the child.
    let invocation = action["invocation"].as_str().unwrap().to_string();
    let retried = run_invocation(dir, &invocation);
    assert_eq!(retried["state"], "plan_and_await", "{retried}");
    let dispatched = retried["retry_dispatched"]
        .as_array()
        .unwrap_or_else(|| panic!("no retry_dispatched in {retried}"));
    assert_eq!(dispatched[0]["task"], "task-1", "{retried}");

    // The retried child succeeds; the batch completes clean and the parent
    // reaches `summarize` with nothing left to retry.
    run_ok(
        dir,
        &[
            "next",
            "coord.task-1",
            "--no-cleanup",
            "--with-data",
            r#"{"status": "complete"}"#,
        ],
    );
    let done = run_ok(dir, &["next", "coord", "--no-cleanup"]);
    assert_eq!(done["state"], "summarize", "{done}");
    assert!(done.get("reserved_actions").is_none(), "{done}");
}

#[test]
fn giving_up_leaves_no_retry_on_the_terminal_response() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_example_batch(dir);
    fail_task_1(dir);
    let json = run_ok(dir, &["next", "coord"]);
    assert_eq!(json["state"], "analyze_failures", "{json}");

    let done = run_ok(
        dir,
        &[
            "next",
            "coord",
            "--no-cleanup",
            "--with-data",
            r#"{"decision": "give_up"}"#,
        ],
    );
    assert_eq!(done["state"], "summarize", "{done}");
    assert!(
        done.get("reserved_actions").is_none(),
        "a terminal state takes no retry: {done}"
    );
}

/// A parent whose post-batch state has no `evidence.retry_failed` route: it
/// has moved on from the batch, so no retry is offered there.
const NO_RETRY_ROUTE_PARENT: &str = r#"---
name: no-retry-route
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: batch-worker.md
    transitions:
      - target: report
        when:
          gates.done.all_complete: true
          gates.done.needs_attention: true
      - target: closed
        when:
          gates.done.all_complete: true
          gates.done.needs_attention: false
  report:
    accepts:
      ok:
        type: enum
        values: [yes]
        required: true
    transitions:
      - target: closed
        when:
          ok: yes
  closed:
    terminal: true
---

## plan

Plan.

## report

Report the failures.

## closed

Closed.
"#;

#[test]
fn a_state_that_does_not_route_a_retry_is_not_offered_one() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join(EXAMPLES);
    std::fs::copy(
        examples.join("batch-worker.md"),
        dir.join("batch-worker.md"),
    )
    .unwrap();
    std::fs::write(dir.join("parent.md"), NO_RETRY_ROUTE_PARENT).unwrap();
    run_ok(
        dir,
        &[
            "init",
            "coord",
            "--template",
            dir.join("parent.md").to_str().unwrap(),
        ],
    );
    let tasks = serde_json::json!({
        "tasks": [{"name": "task-1", "waits_on": [], "vars": {"ISSUE_NUMBER": "1"}}]
    });
    run_koto(dir, &["next", "coord", "--with-data", &tasks.to_string()]);
    fail_task_1(dir);

    let json = run_ok(dir, &["next", "coord"]);
    assert_eq!(json["state"], "report", "{json}");
    assert!(json.get("reserved_actions").is_none(), "{json}");
}

#[test]
fn a_failed_task_with_a_dependent_is_retried_by_its_offered_invocation() {
    // t1 fails, t2 waits on it and is skipped, t3 succeeds. The tick that
    // completes the batch leaves the batching state, so no skip marker is
    // written for t2; the offer names only t1, and retrying t1 brings t2
    // back through the scheduler.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let tasks = serde_json::json!({
        "tasks": [
            {"name": "t1", "waits_on": [], "vars": {"ISSUE_NUMBER": "1"}},
            {"name": "t2", "waits_on": ["t1"], "vars": {"ISSUE_NUMBER": "2"}},
            {"name": "t3", "waits_on": [], "vars": {"ISSUE_NUMBER": "3"}},
        ]
    });
    start_example_batch_with(dir, &tasks);
    drive(
        dir,
        "coord.t1",
        serde_json::json!({"status": "blocked", "failure_reason": "tests red"}),
    );
    drive(dir, "coord.t3", serde_json::json!({"status": "complete"}));

    let json = run_ok(dir, &["next", "coord"]);
    assert_eq!(json["state"], "analyze_failures", "{json}");
    let action = retry_action(&json);
    assert_eq!(action["applies_to"], serde_json::json!(["t1"]), "{json}");

    let retried = run_invocation(dir, action["invocation"].as_str().unwrap());
    assert_eq!(retried["state"], "plan_and_await", "{retried}");

    // t1 succeeds on retry; the scheduler then spawns t2, which succeeds.
    drive(dir, "coord.t1", serde_json::json!({"status": "complete"}));
    run_koto(dir, &["next", "coord"]);
    drive(dir, "coord.t2", serde_json::json!({"status": "complete"}));
    let done = run_ok(dir, &["next", "coord", "--no-cleanup"]);
    assert_eq!(done["state"], "summarize", "{done}");
    assert_eq!(done["batch_final_view"]["all_success"], true, "{done}");
}

#[test]
fn a_directed_move_into_analyze_failures_offers_the_retry() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_example_batch(dir);
    fail_task_1(dir);

    // Leave the batching state with `--to` before any tick has seen the
    // batch complete: the directed exit records the batch and offers the
    // retry on its own response.
    let json = run_ok(dir, &["next", "coord", "--to", "analyze_failures"]);
    assert_eq!(json["state"], "analyze_failures", "{json}");
    let action = retry_action(&json);
    assert_eq!(
        action["applies_to"],
        serde_json::json!(["task-1"]),
        "{json}"
    );
}
