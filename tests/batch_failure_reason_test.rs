//! A failed child's `reason` in the parent's batch view comes from the
//! `failure_reason` the child wrote during its current run, and falls back to
//! the child's state name (Issue #278).
//!
//! The reason is resolved with the child's result on its terminal tick and
//! recorded beside it, on the child's `request_store.result` and the
//! parent's `child_completed`.
//!
//! Scenarios:
//!
//! - a transition's `context_assignments` writes the reason: the gate output,
//!   the frozen `batch_final_view` and `koto status` carry it with
//!   `reason_source: "failure_reason"`, while `failure_mode` stays the state
//!   name;
//! - `koto context add` writes the reason: the same;
//! - evidence alone writes nothing, so the reason is the state name;
//! - an assignment on a state reached by auto-advance resolves
//!   `${evidence.<field>}` against no evidence and writes an empty value over
//!   the real one, so the reason falls back to the state name;
//! - a child restarted by `retry_failed` does not show its previous run's
//!   reason;
//! - a multi-line or over-long reason is folded to one line and cut to 500
//!   characters;
//! - a cleaned-up failed child keeps its reason, from the parent's copy;
//! - a reason written after the child failed changes nothing.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
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

/// A worker that can fail three ways: with evidence only (`fail_plain`),
/// with an assignment on the state that takes the evidence (`fail_assign`),
/// or through `hop`, an auto-advancing state whose outgoing edge repeats the
/// assignment (`fail_hop`). `hop` declares `failure_reason` in its own
/// `accepts` only because the compiler requires an assignment's
/// `${evidence.<field>}` to name a field its source state accepts; the
/// declaration does not make the evidence reach it.
const CHILD_TEMPLATE: &str = r#"---
name: reason-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: enum
        required: true
        values: [done, fail_plain, fail_assign, fail_hop]
      failure_reason:
        type: string
        required: false
    transitions:
      - target: done
        when:
          status: done
      - target: failed
        when:
          status: fail_plain
      - target: failed
        when:
          status: fail_assign
        context_assignments:
          failure_reason: "${evidence.failure_reason}"
      - target: hop
        when:
          status: fail_hop
        context_assignments:
          failure_reason: "${evidence.failure_reason}"
  hop:
    accepts:
      failure_reason:
        type: string
        required: false
    transitions:
      - target: failed
        context_assignments:
          failure_reason: "${evidence.failure_reason}"
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
  skipped:
    terminal: true
    skipped_marker: true
---

## work

Work.

## hop

Hop.

## done

Done.

## failed

Failed.

## skipped

Skipped.
"#;

/// A parent that stays in its batching state, so a failed child can be
/// retried there.
const PARENT_TEMPLATE: &str = r#"---
name: reason-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: [yes]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: closed
        when:
          finalize: yes
  closed:
    terminal: true
---

## plan

Plan.

## closed

Closed.
"#;

/// Init `parent` and submit tasks `A` and `B`.
fn start_batch(dir: &Path) {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT_TEMPLATE).unwrap();
    run_ok(
        dir,
        &["init", "parent", "--template", parent.to_str().unwrap()],
    );
    let tasks = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    // The gate blocks while the children run, which exits non-zero.
    run_koto(dir, &["next", "parent", "--with-data", &tasks.to_string()]);
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

/// The gate's `children[]` entry for `parent.A`, from a parent tick taken
/// while `B` still runs.
fn gate_entry_for_a(dir: &Path) -> serde_json::Value {
    let (_, json, _) = run_koto(dir, &["next", "parent"]);
    let gate = &json["blocking_conditions"][0]["output"];
    gate["children"]
        .as_array()
        .unwrap_or_else(|| panic!("no gate output in {json}"))
        .iter()
        .find(|c| c["name"] == "parent.A")
        .cloned()
        .unwrap_or_else(|| panic!("no parent.A in {gate}"))
}

/// Finish `B`, let the parent freeze the view, and return `parent.A`'s entry
/// from the `batch_final_view` context key and its `reason` in `koto status`.
fn frozen_entry_and_status_reason_for_a(dir: &Path) -> (serde_json::Value, serde_json::Value) {
    drive(dir, "parent.B", serde_json::json!({"status": "done"}));
    run_ok(dir, &["next", "parent"]);
    let output = koto_cmd(dir)
        .args(["context", "get", "parent", "batch_final_view"])
        .output()
        .unwrap();
    assert!(output.status.success(), "batch_final_view is written");
    let view: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = view["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "parent.A")
        .cloned()
        .unwrap_or_else(|| panic!("no parent.A in {view}"));
    let status = run_ok(dir, &["status", "parent"]);
    let reason = status["batch"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["task_name"] == "A")
        .map(|t| t["reason"].clone())
        .unwrap_or_else(|| panic!("no task A in {status}"));
    (entry, reason)
}

fn assert_reason(entry: &serde_json::Value, reason: &str, source: &str) {
    assert_eq!(entry["outcome"], "failure", "{entry}");
    assert_eq!(entry["reason"], reason, "{entry}");
    assert_eq!(entry["reason_source"], source, "{entry}");
    assert_eq!(
        entry["failure_mode"], "failed",
        "failure_mode stays the state name: {entry}"
    );
}

#[test]
fn an_assigned_failure_reason_is_the_reason_on_every_surface() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": "API quota exhausted"}),
    );
    assert_reason(
        &gate_entry_for_a(dir),
        "API quota exhausted",
        "failure_reason",
    );

    let (frozen, status_reason) = frozen_entry_and_status_reason_for_a(dir);
    assert_reason(&frozen, "API quota exhausted", "failure_reason");
    assert_eq!(status_reason, "API quota exhausted");
}

#[test]
fn a_failure_reason_added_with_koto_context_add_is_the_reason() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    let output = koto_cmd(dir)
        .args(["context", "add", "parent.A", "failure_reason"])
        .write_stdin("disk full\n")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    drive(dir, "parent.A", serde_json::json!({"status": "fail_plain"}));

    assert_reason(&gate_entry_for_a(dir), "disk full", "failure_reason");
    let (frozen, _) = frozen_entry_and_status_reason_for_a(dir);
    assert_reason(&frozen, "disk full", "failure_reason");
}

#[test]
fn evidence_alone_writes_no_reason_so_the_state_name_is_used() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_plain", "failure_reason": "never stored"}),
    );
    assert_reason(&gate_entry_for_a(dir), "failed", "state_name");
    let (frozen, status_reason) = frozen_entry_and_status_reason_for_a(dir);
    assert_reason(&frozen, "failed", "state_name");
    assert_eq!(status_reason, "failed");
}

#[test]
fn an_assignment_on_an_auto_advanced_state_overwrites_the_reason_with_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    // `work` stores "real cause", then `hop` auto-advances in the same tick:
    // its assignment sees no evidence and stores an empty value.
    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_hop", "failure_reason": "real cause"}),
    );
    let output = koto_cmd(dir)
        .args(["context", "get", "parent.A", "failure_reason"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "the chained assignment stored an empty value: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    assert_reason(&gate_entry_for_a(dir), "failed", "state_name");
}

#[test]
fn a_retried_child_does_not_show_its_previous_runs_reason() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": "first run"}),
    );
    assert_reason(&gate_entry_for_a(dir), "first run", "failure_reason");

    // Retry A. Its context still holds "first run", but the rewind starts a
    // new run, which fails without writing a reason.
    let retry = serde_json::json!({"retry_failed": {"children": ["A"]}});
    run_ok(dir, &["next", "parent", "--with-data", &retry.to_string()]);
    drive(dir, "parent.A", serde_json::json!({"status": "fail_plain"}));
    assert_reason(&gate_entry_for_a(dir), "failed", "state_name");

    // A reason the new run writes is shown.
    run_ok(dir, &["next", "parent", "--with-data", &retry.to_string()]);
    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": "second run"}),
    );
    assert_reason(&gate_entry_for_a(dir), "second run", "failure_reason");
}

#[test]
fn a_multi_line_reason_is_folded_onto_one_line() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(
        dir,
        "parent.A",
        serde_json::json!({
            "status": "fail_assign",
            "failure_reason": "  build failed:\n\terror[E0308]: mismatched types\r\n  at src/lib.rs:3 \n"
        }),
    );
    assert_reason(
        &gate_entry_for_a(dir),
        "build failed: error[E0308]: mismatched types at src/lib.rs:3",
        "failure_reason",
    );
}

#[test]
fn an_over_long_reason_is_cut_to_500_characters() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    // Multi-byte characters, so the cut is by character and not by byte.
    let long: String = "é".repeat(600);
    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": long}),
    );
    let entry = gate_entry_for_a(dir);
    let reason = entry["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("{entry}"));
    assert_eq!(reason.chars().count(), 500, "{reason}");
    assert!(reason.ends_with("..."), "{reason}");
    assert_eq!(reason, format!("{}...", "é".repeat(497)));

    // Exactly 500 characters is kept whole.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);
    let exact: String = "x".repeat(500);
    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": exact}),
    );
    assert_eq!(gate_entry_for_a(dir)["reason"], "x".repeat(500));
}

#[test]
fn a_cleaned_up_failed_child_keeps_its_reason() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": "flaky network"}),
    );
    // With the child's log gone, the parent's `child_completed` copy answers.
    run_ok(dir, &["session", "cleanup", "parent.A"]);
    assert_reason(&gate_entry_for_a(dir), "flaky network", "failure_reason");
}

#[test]
fn a_reason_written_after_the_child_failed_changes_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir);

    drive(dir, "parent.A", serde_json::json!({"status": "fail_plain"}));
    // The reason was resolved with the result on the terminal tick; a later
    // write reaches neither the live gate output nor the frozen view, so
    // the two never disagree.
    let output = koto_cmd(dir)
        .args(["context", "add", "parent.A", "failure_reason"])
        .write_stdin("too late")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_reason(&gate_entry_for_a(dir), "failed", "state_name");
    let (frozen, status_reason) = frozen_entry_and_status_reason_for_a(dir);
    assert_reason(&frozen, "failed", "state_name");
    assert_eq!(status_reason, "failed");
}

/// A parent that leaves its batching state for a terminal state on the tick
/// its batch completes.
const LEAVING_PARENT_TEMPLATE: &str = r#"---
name: reason-parent-leave
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
      default_template: child.md
    transitions:
      - target: closed
        when:
          gates.done.all_complete: true
  closed:
    terminal: true
---

## plan

Plan.

## closed

Closed.
"#;

#[test]
fn a_reason_reaches_the_terminal_response_when_the_completing_tick_leaves() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, LEAVING_PARENT_TEMPLATE).unwrap();
    run_ok(
        dir,
        &["init", "parent", "--template", parent.to_str().unwrap()],
    );
    let tasks = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    run_koto(dir, &["next", "parent", "--with-data", &tasks.to_string()]);

    drive(
        dir,
        "parent.A",
        serde_json::json!({"status": "fail_assign", "failure_reason": "tests red on main"}),
    );
    drive(dir, "parent.B", serde_json::json!({"status": "done"}));

    // One tick completes the batch and lands on the terminal state.
    let json = run_ok(dir, &["next", "parent", "--no-cleanup"]);
    assert_eq!(json["action"], "done", "{json}");
    let entry = json["batch_final_view"]["children"]
        .as_array()
        .unwrap_or_else(|| panic!("no batch_final_view in {json}"))
        .iter()
        .find(|c| c["name"] == "parent.A")
        .cloned()
        .unwrap_or_else(|| panic!("no parent.A in {json}"));
    assert_reason(&entry, "tests red on main", "failure_reason");
}
