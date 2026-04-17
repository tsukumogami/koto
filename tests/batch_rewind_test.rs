//! Integration tests for batch-aware rewind with epoch filter (Issue #2).
//!
//! Validates:
//! 1. Rewind past a batch state relocates children to an epoch branch.
//! 2. Non-batch rewind remains unchanged.
//! 3. Epoch filter prevents stale ChildCompleted events from poisoning
//!    the gate after rewind.
//! 4. Tilde (`~`) in workflow names is rejected by `koto init`.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn session_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

/// Parent template with a `materialize_children` hook on the `plan`
/// state. Starts in `gather`, transitions to `plan` so rewind has
/// somewhere to go. The `plan` state has a `finalize` field that gates
/// the transition to `summarize`, so the parent stays in `plan` while
/// we drive children.
const PARENT_TEMPLATE: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: gather
states:
  gather:
    transitions:
      - target: plan
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
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## gather

Gather requirements.

## plan

Plan the batch.

## summarize

Summarize results.
"#;

/// Minimal child template.
const CHILD_TEMPLATE: &str = r#"---
name: batch-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do the work.

## done

Done.
"#;

/// Simple non-batch template (no `materialize_children`).
const SIMPLE_TEMPLATE: &str = r#"---
name: simple-workflow
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: middle
  middle:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Start.

## middle

Middle.

## done

Done.
"#;

fn write_batch_templates(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT_TEMPLATE).unwrap();
    parent
}

fn write_simple_template(dir: &Path) -> PathBuf {
    let src = dir.join("simple.md");
    std::fs::write(&src, SIMPLE_TEMPLATE).unwrap();
    src
}

// -----------------------------------------------------------------------
// Test 1: Rewind past batch state relocates children
// -----------------------------------------------------------------------

#[test]
fn rewind_past_batch_state_relocates_children() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_batch_templates(tmp.path());

    // Initialize the parent.
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "orch", "--template", parent_path.to_str().unwrap()],
    );
    assert!(ok, "parent init failed: {}", stderr);

    // Advance from gather to plan.
    let (ok, _, stderr) = run_koto(tmp.path(), &["next", "orch"]);
    assert!(ok, "advance to plan failed: {}", stderr);

    // Submit tasks to materialize children.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "task-a", "waits_on": [], "vars": {}},
            {"name": "task-b", "waits_on": [], "vars": {}},
            {"name": "task-c", "waits_on": [], "vars": {}},
        ]
    });
    let payload_str = payload.to_string();
    let (ok, json, stderr) = run_koto(tmp.path(), &["next", "orch", "--with-data", &payload_str]);
    assert!(
        ok,
        "submit tasks failed: stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Verify children exist.
    let (ok, json, _) = run_koto(tmp.path(), &["workflows", "--children", "orch"]);
    assert!(ok, "workflows --children failed");
    let children = json.as_array().expect("workflows returns array");
    assert_eq!(children.len(), 3, "should have 3 children before rewind");

    // Check that children have `orch.` prefix.
    let child_names: Vec<&str> = children.iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(
        child_names.iter().all(|n| n.starts_with("orch.")),
        "all children should start with 'orch.': {:?}",
        child_names
    );

    // Rewind the parent.
    let (ok, json, stderr) = run_koto(tmp.path(), &["rewind", "orch"]);
    assert!(
        ok,
        "rewind failed: stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Verify response has superseded_branch and children_relocated.
    assert_eq!(
        json["superseded_branch"].as_str(),
        Some("orch~1"),
        "superseded_branch should be orch~1, got: {}",
        json["superseded_branch"]
    );
    assert_eq!(
        json["children_relocated"].as_u64(),
        Some(3),
        "should have relocated 3 children"
    );

    // Verify old children are now at orch~1.* (visible via workflows).
    let (ok, json, _) = run_koto(tmp.path(), &["workflows"]);
    assert!(ok, "workflows failed");
    let all_workflows = json.as_array().expect("workflows returns array");
    let workflow_names: Vec<&str> = all_workflows
        .iter()
        .filter_map(|w| w["name"].as_str())
        .collect();

    // Should see orch~1.task-a, orch~1.task-b, orch~1.task-c.
    assert!(
        workflow_names.contains(&"orch~1.task-a"),
        "orch~1.task-a should exist: {:?}",
        workflow_names
    );
    assert!(
        workflow_names.contains(&"orch~1.task-b"),
        "orch~1.task-b should exist: {:?}",
        workflow_names
    );
    assert!(
        workflow_names.contains(&"orch~1.task-c"),
        "orch~1.task-c should exist: {:?}",
        workflow_names
    );

    // Should NOT see orch.task-a anymore.
    assert!(
        !workflow_names.contains(&"orch.task-a"),
        "orch.task-a should not exist after rewind: {:?}",
        workflow_names
    );

    // Verify relocated children are queryable.
    let (ok, json, _) = run_koto(tmp.path(), &["status", "orch~1.task-a"]);
    assert!(
        ok,
        "status orch~1.task-a should succeed, got: {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Verify the name is now free — can re-init orch.task-a.
    let child_path = tmp.path().join("child.md");
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "orch.task-a",
            "--template",
            child_path.to_str().unwrap(),
            "--parent",
            "orch",
        ],
    );
    assert!(ok, "re-init orch.task-a should succeed: {}", stderr);
}

// -----------------------------------------------------------------------
// Test 2: Non-batch rewind unchanged
// -----------------------------------------------------------------------

#[test]
fn non_batch_rewind_unchanged() {
    let tmp = TempDir::new().unwrap();
    let src = write_simple_template(tmp.path());
    let src_str = src.to_str().unwrap();

    // Init and advance twice so we can rewind.
    let (ok, _, stderr) = run_koto(tmp.path(), &["init", "simple-wf", "--template", src_str]);
    assert!(ok, "init failed: {}", stderr);

    // Append transition events manually to advance to "middle".
    let state_path = session_state_path(tmp.path(), "simple-wf");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&state_path)
            .unwrap();
        writeln!(
            f,
            r#"{{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{{"from":"start","to":"middle","condition_type":"gate"}}}}"#
        )
        .unwrap();
    }

    let (ok, json, stderr) = run_koto(tmp.path(), &["rewind", "simple-wf"]);
    assert!(
        ok,
        "rewind failed: stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // superseded_branch should be null (no batch state).
    assert!(
        json["superseded_branch"].is_null(),
        "superseded_branch should be null for non-batch rewind, got: {}",
        json["superseded_branch"]
    );
    assert_eq!(
        json["children_relocated"].as_u64(),
        Some(0),
        "children_relocated should be 0"
    );
    assert_eq!(
        json["state"].as_str(),
        Some("start"),
        "should rewind to start"
    );
}

// -----------------------------------------------------------------------
// Test 3: Epoch filter -- stale ChildCompleted ignored after rewind
// -----------------------------------------------------------------------

#[test]
fn epoch_filter_stale_child_completed_ignored_after_rewind() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_batch_templates(tmp.path());

    // Initialize the parent.
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "epoch-parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);

    // Advance from gather to plan.
    let (ok, _, stderr) = run_koto(tmp.path(), &["next", "epoch-parent"]);
    assert!(ok, "advance to plan failed: {}", stderr);

    // Submit tasks and spawn children.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "task-x", "waits_on": [], "vars": {}},
        ]
    });
    let payload_str = payload.to_string();
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["next", "epoch-parent", "--with-data", &payload_str],
    );
    assert!(ok, "submit tasks failed: {}", stderr);

    // Drive child to completion (no cleanup so the parent log gets
    // a ChildCompleted event on the next tick).
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "epoch-parent.task-x",
            "--no-cleanup",
            "--with-data",
            r#"{"marker": "done"}"#,
        ],
    );
    assert!(ok, "drive child failed: {}", stderr);

    // Tick the parent once more so it sees the terminal child and
    // appends a ChildCompleted event.
    let (ok, json, _) = run_koto(tmp.path(), &["next", "epoch-parent"]);
    assert!(ok, "parent tick after child done failed");

    // Verify the parent is still in `plan` (the transition to
    // summarize requires `finalize: yes`).
    assert_eq!(
        json["state"].as_str(),
        Some("plan"),
        "parent should still be in plan after child done, got: {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Now rewind the parent past the batch state.
    let (ok, json, stderr) = run_koto(tmp.path(), &["rewind", "epoch-parent"]);
    assert!(
        ok,
        "rewind failed: stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    assert_eq!(
        json["superseded_branch"].as_str(),
        Some("epoch-parent~1"),
        "superseded_branch should be epoch-parent~1"
    );

    // After rewind the parent is back in `gather`. Advance to `plan`.
    // extract_tasks is epoch-aware — it ignores the pre-rewind evidence,
    // so the scheduler sees NoBatch (no tasks submitted in this epoch).
    let (ok, json, _) = run_koto(tmp.path(), &["next", "epoch-parent"]);
    assert!(ok, "advance to plan after rewind failed");
    assert!(
        json.get("scheduler").is_none()
            || json["scheduler"]["materialized_children"]
                .as_array()
                .map_or(true, |a| a.is_empty()),
        "scheduler should see NoBatch (stale evidence filtered): {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Now submit a fresh task list in the current epoch. The stale
    // ChildCompleted from the prior epoch must be ignored — task-x
    // should spawn fresh (Running), not be classified as terminal
    // from the old completion event.
    let fresh_payload = serde_json::json!({
        "tasks": [{"name": "task-x", "waits_on": [], "vars": {}}]
    });
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "epoch-parent",
            "--with-data",
            &fresh_payload.to_string(),
        ],
    );
    assert!(ok, "fresh task submission failed: {}", stderr);

    let sched = json
        .get("scheduler")
        .expect("scheduler key present after fresh submission");
    let materialized = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children should be present");
    let task_x = materialized
        .iter()
        .find(|c| c["task"].as_str() == Some("task-x"))
        .expect("task-x should appear in materialized_children");

    // Must be Running/pending (freshly spawned), NOT success. If the
    // epoch filter failed, the stale ChildCompleted would make the
    // scheduler think the child is already terminal.
    let outcome = task_x["outcome"].as_str().unwrap_or("");
    assert!(
        outcome == "running" || outcome == "pending",
        "task-x should be running/pending (freshly spawned), not terminal. \
         outcome={}, scheduler={:?}",
        outcome,
        sched
    );
}

// -----------------------------------------------------------------------
// Test 4: Tilde in session name rejected
// -----------------------------------------------------------------------

#[test]
fn tilde_in_workflow_name_rejected() {
    let tmp = TempDir::new().unwrap();
    let src = write_simple_template(tmp.path());
    let src_str = src.to_str().unwrap();

    let output = koto_cmd(tmp.path())
        .args(["init", "my~workflow", "--template", src_str])
        .output()
        .unwrap();

    assert!(!output.status.success(), "init with tilde should fail");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("error output should be JSON");
    let error = json["error"].as_str().expect("error field present");
    assert!(
        error.contains('~'),
        "error should mention the tilde character: {}",
        error
    );
}

// -----------------------------------------------------------------------
// Test 5: koto status shows superseded_branches after batch rewind
// -----------------------------------------------------------------------

#[test]
fn status_shows_superseded_branches_after_rewind() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_batch_templates(tmp.path());

    // Init parent, advance to plan, submit tasks.
    let (ok, _, _) = run_koto(
        tmp.path(),
        &["init", "orch", "--template", parent_path.to_str().unwrap()],
    );
    assert!(ok);
    let (ok, _, _) = run_koto(tmp.path(), &["next", "orch"]);
    assert!(ok);
    let payload = serde_json::json!({
        "tasks": [{"name": "t1", "waits_on": [], "vars": {}}]
    });
    let (ok, _, _) = run_koto(
        tmp.path(),
        &["next", "orch", "--with-data", &payload.to_string()],
    );
    assert!(ok);

    // Before rewind: status should NOT have superseded_branches.
    let (ok, json, _) = run_koto(tmp.path(), &["status", "orch"]);
    assert!(ok);
    assert!(
        json.get("superseded_branches").is_none(),
        "no superseded_branches before rewind: {}",
        json
    );

    // Rewind.
    let (ok, _, _) = run_koto(tmp.path(), &["rewind", "orch"]);
    assert!(ok);

    // After rewind: status should show superseded_branches.
    let (ok, json, _) = run_koto(tmp.path(), &["status", "orch"]);
    assert!(ok);
    let branches = json["superseded_branches"]
        .as_array()
        .expect("superseded_branches should be an array");
    assert_eq!(branches.len(), 1, "should have 1 superseded branch");
    assert_eq!(
        branches[0].as_str(),
        Some("orch~1"),
        "branch name should be orch~1"
    );

    // koto workflows --children orch~1 should list the relocated child.
    let (ok, json, _) = run_koto(tmp.path(), &["workflows", "--children", "orch~1"]);
    assert!(ok);
    let children = json.as_array().expect("workflows returns array");
    assert_eq!(
        children.len(),
        1,
        "superseded branch should have 1 child: {}",
        json
    );
    assert_eq!(
        children[0]["name"].as_str(),
        Some("orch~1.t1"),
        "child name should be orch~1.t1"
    );
}
