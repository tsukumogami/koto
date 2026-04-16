//! Integration tests for Issue #134: the `children-complete` gate must
//! observe children that auto-clean on terminal.
//!
//! Before the fix, a child that reached its terminal state and triggered
//! auto-cleanup vanished from `backend.list()`, so the parent's next
//! tick classified the task as `pending` (no state file on disk,
//! no entry in `materialized_children` for that task). The batch gate
//! therefore reported `completed: 0, all_complete: false` forever.
//!
//! The fix appends a `ChildCompleted` event to the PARENT'S log just
//! before the child's session is cleaned up. The gate evaluator
//! synthesizes a `ChildSnapshot` from those events for any task not on
//! disk, recovering the terminal outcome.

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

fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

/// A parent that materializes children and exposes a `children-complete`
/// gate on its `plan` state.
const PARENT_TEMPLATE: &str = r#"---
name: batch-parent
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
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize results.
"#;

/// Child template with three terminal outcomes: `done` (success),
/// `failed` (failure), and a `skipped` marker.
const CHILD_TEMPLATE: &str = r#"---
name: batch-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done, fail]
    transitions:
      - target: done
        when:
          marker: done
      - target: failed
        when:
          marker: fail
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## work

Do the work.

## done

Done.

## failed

Failed.
"#;

fn write_templates(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT_TEMPLATE).unwrap();
    parent
}

/// Drive a child to its `done` terminal state WITHOUT `--no-cleanup`,
/// so the child's session directory is auto-cleaned immediately after
/// the terminal response. This is the scenario #134 is about: the next
/// `koto next <parent>` call cannot see the child on disk anymore.
fn drive_child_to_done_with_cleanup(dir: &Path, name: &str) {
    let (ok, json, stderr) = run_koto(dir, &["next", name, "--with-data", r#"{"marker": "done"}"#]);
    assert!(
        ok,
        "drive child {} to done (with cleanup) failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

/// Drive a child to its `failed` terminal state WITHOUT `--no-cleanup`.
fn drive_child_to_fail_with_cleanup(dir: &Path, name: &str) {
    let (ok, json, stderr) = run_koto(dir, &["next", name, "--with-data", r#"{"marker": "fail"}"#]);
    assert!(
        ok,
        "drive child {} to fail (with cleanup) failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

fn parent_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

fn child_session_dir(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir).join(name)
}

/// AC1 — a child that reaches `done` and auto-cleans must still count
/// toward the parent's `children-complete` gate.
///
/// Before the fix: the post-cleanup `koto next parent` call reported
/// `completed: 0, all_complete: false, success: 0` because the child's
/// session directory was gone and no classification entry existed.
/// After the fix: the `ChildCompleted` event on the parent's log
/// synthesizes a snapshot with outcome=success, and the gate reports
/// `completed: 1, all_complete: true, success: 1`.
#[test]
fn cleaned_up_child_still_counts_toward_all_complete() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);

    // Submit a single-task batch.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // Drive the child to done with auto-cleanup. After this call the
    // child's session directory is gone.
    drive_child_to_done_with_cleanup(tmp.path(), "parent.A");
    assert!(
        !child_session_dir(tmp.path(), "parent.A").exists(),
        "pre-condition: child session directory must be cleaned up"
    );

    // Pre-condition: the parent's log must contain a ChildCompleted
    // event referencing task A with outcome "success".
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let child_completed_lines: Vec<&str> = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"child_completed\""))
        .collect();
    assert_eq!(
        child_completed_lines.len(),
        1,
        "expected exactly one ChildCompleted event on parent log, got {}:\n{}",
        child_completed_lines.len(),
        parent_log
    );
    let ev: serde_json::Value = serde_json::from_str(child_completed_lines[0]).unwrap();
    assert_eq!(ev["payload"]["task_name"], "A");
    assert_eq!(ev["payload"]["outcome"], "success");
    assert_eq!(ev["payload"]["final_state"], "done");
    assert_eq!(ev["payload"]["child_name"], "parent.A");

    // Tick the parent. The scheduler queries the batch gate, which must
    // now report the single task as terminal-success thanks to the
    // ChildCompleted event replay.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    assert_eq!(ledger.len(), 1, "ledger must list the single task");
    // `materialized_children` carries short task name in `task` (not
    // `task_name`; that projection only exists on the `koto status`
    // batch view).
    assert_eq!(ledger[0]["task"], "A");
    assert_eq!(
        ledger[0]["outcome"],
        "success",
        "cleaned-up child must classify as success via ChildCompleted event replay; got ledger: {}",
        serde_json::to_string_pretty(sched).unwrap()
    );

    // The `batch` section surfaced on the response envelope (or via
    // `koto status`) must report the task as complete.
    let (ok, status, _) = run_koto(tmp.path(), &["status", "parent"]);
    assert!(ok);
    let batch = status
        .get("batch")
        .expect("batch section present for batch-scoped parent");
    let summary = &batch["summary"];
    assert_eq!(summary["total"], 1, "summary total: {}", batch);
    assert_eq!(summary["success"], 1, "summary success: {}", batch);
    assert_eq!(summary["pending"], 0);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["skipped"], 0);
}

/// AC2 — a mix of cleaned-up and still-live children must report
/// correct counts. Two tasks A and B; A auto-cleans on success, B is
/// driven with `--no-cleanup` so it stays on disk as terminal.
#[test]
fn mixed_cleaned_and_live_children_report_correct_counts() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);

    // Submit two independent tasks.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // A auto-cleans on terminal.
    drive_child_to_done_with_cleanup(tmp.path(), "parent.A");
    assert!(!child_session_dir(tmp.path(), "parent.A").exists());

    // B stays on disk.
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "parent.B",
            "--no-cleanup",
            "--with-data",
            r#"{"marker": "done"}"#,
        ],
    );
    assert!(ok, "drive B failed: {}", stderr);
    assert!(child_session_dir(tmp.path(), "parent.B").exists());

    // Tick parent and verify counts.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    assert_eq!(ledger.len(), 2);
    let outcomes: std::collections::BTreeMap<String, String> = ledger
        .iter()
        .map(|e| {
            (
                e["task"].as_str().unwrap_or("").to_string(),
                e["outcome"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(outcomes.get("A"), Some(&"success".to_string()));
    assert_eq!(outcomes.get("B"), Some(&"success".to_string()));

    let (ok, status, _) = run_koto(tmp.path(), &["status", "parent"]);
    assert!(ok);
    let batch = status.get("batch").expect("batch section present");
    let summary = &batch["summary"];
    assert_eq!(summary["total"], 2);
    assert_eq!(summary["success"], 2);
    assert_eq!(summary["pending"], 0);
}

/// AC3 — a cleaned-up child whose final state is `failed` must be
/// observable with outcome=failure. The failure_mode projection on the
/// per-task entry should be populated from the event's final_state.
#[test]
fn cleaned_up_failure_preserved() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);

    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // Drive A to failed with auto-cleanup.
    drive_child_to_fail_with_cleanup(tmp.path(), "parent.A");
    assert!(!child_session_dir(tmp.path(), "parent.A").exists());

    // Pre-condition: ChildCompleted event has outcome=failure.
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let ccl: Vec<&str> = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"child_completed\""))
        .collect();
    assert_eq!(ccl.len(), 1);
    let ev: serde_json::Value = serde_json::from_str(ccl[0]).unwrap();
    assert_eq!(ev["payload"]["outcome"], "failure");
    assert_eq!(ev["payload"]["final_state"], "failed");

    // Tick parent and verify.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["task"], "A");
    assert_eq!(ledger[0]["outcome"], "failure");

    // Batch summary: failed=1.
    let (ok, status, _) = run_koto(tmp.path(), &["status", "parent"]);
    assert!(ok);
    let batch = status.get("batch").expect("batch section");
    let summary = &batch["summary"];
    assert_eq!(summary["failed"], 1);
    assert_eq!(summary["success"], 0);
    assert_eq!(summary["pending"], 0);

    // The per-task entry should carry the failure reason projected
    // from the child's final state. The batch view uses `reason` +
    // `reason_source="state_name"` (the gate's failure_mode is projected
    // into this pair on the batch-view path).
    let tasks = batch["tasks"].as_array().expect("tasks array");
    let task_a = tasks.iter().find(|t| t["task_name"] == "A").expect("A");
    assert_eq!(task_a["outcome"], "failure");
    assert_eq!(
        task_a["reason"], "failed",
        "reason must reflect the child's final state: {}",
        task_a
    );
    assert_eq!(task_a["reason_source"], "state_name");
}

/// AC4 — when a child was cleaned up (leaving a stale
/// `ChildCompleted{failure}` on the parent's log) and is then respawned
/// on disk at a non-terminal state, the parent's gate must classify the
/// task from the fresh on-disk state, NOT from the stale event.
///
/// Scenario construction:
///
/// 1. Drive `parent.A` to `failed` with auto-cleanup. Parent's log now
///    carries `ChildCompleted{task=A, outcome=failure}`. The child
///    session directory is gone.
/// 2. Verify the scheduler observes the failure from the event replay
///    (outcome = failure, reserved_actions surfaces retry_failed).
/// 3. Manually re-init the child at its initial `work` state via
///    `koto init parent.A --parent parent`. Simulates what a retry
///    respawn would do: a fresh on-disk child in a non-terminal state
///    alongside the stale event on the parent's log.
/// 4. Tick parent. The gate must read the fresh on-disk state, classify
///    A as `running` (non-terminal), and NOT report it as `failed`. The
///    batch summary's `failed` count must be 0.
///
/// This pins the precedence rule documented at the top of
/// `augment_snapshots_with_child_completed` and the identical block in
/// `build_children_complete_output` end-to-end, closing the gap the
/// unit tests leave around the full CLI surface (scheduler output +
/// batch summary projection).
#[test]
fn retry_respawn_shadows_stale_child_completed_event() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);

    // 1. Submit a single-task batch and drive A to `failed` with
    //    auto-cleanup. This seeds the ChildCompleted{failure} event.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    drive_child_to_fail_with_cleanup(tmp.path(), "parent.A");
    assert!(
        !child_session_dir(tmp.path(), "parent.A").exists(),
        "pre-condition: child session directory must be cleaned up"
    );

    // Pre-condition: parent log has exactly one ChildCompleted event
    // with outcome=failure.
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let ccl: Vec<&str> = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"child_completed\""))
        .collect();
    assert_eq!(ccl.len(), 1, "expected one ChildCompleted event on parent");
    let ev: serde_json::Value = serde_json::from_str(ccl[0]).unwrap();
    assert_eq!(ev["payload"]["outcome"], "failure");

    // 2. Tick parent. Scheduler sees failure via event replay and
    //    surfaces retry_failed as a reserved action.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["task"], "A");
    assert_eq!(
        ledger[0]["outcome"], "failure",
        "pre-condition: scheduler must observe the cleaned-up failure from the event"
    );

    let reserved = json
        .get("reserved_actions")
        .and_then(|v| v.as_array())
        .expect("reserved_actions present when a task failed");
    assert!(
        reserved
            .iter()
            .any(|e| e["action"].as_str() == Some("retry_failed")),
        "expected retry_failed in reserved_actions: {}",
        serde_json::to_string_pretty(reserved).unwrap()
    );

    // 3. Simulate the retry respawn: re-init A on disk at its initial
    //    state. `koto init --parent parent` is the same primitive the
    //    batch scheduler uses to spawn children, so this mirrors what
    //    happens when the respawn paths in `retry.rs` re-create a
    //    child directly (RespawnSkipped / RespawnFailed both call
    //    `init_child_from_parent`; we're not exercising the Rewind path
    //    here because the old child's log is gone, and the stale-event
    //    shadow rule must hold for all respawn flavours).
    let child_template = tmp.path().join("child.md");
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent.A",
            "--template",
            child_template.to_str().unwrap(),
            "--parent",
            "parent",
        ],
    );
    assert!(ok, "re-init of parent.A failed: {}", stderr);
    assert!(
        child_session_dir(tmp.path(), "parent.A").exists(),
        "post-condition: respawned child must be on disk"
    );

    // Sanity: the respawned child is at its initial (non-terminal)
    // state `work`.
    let (ok, status, _) = run_koto(tmp.path(), &["status", "parent.A"]);
    assert!(ok);
    assert_eq!(
        status["current_state"].as_str(),
        Some("work"),
        "respawned child must be at its initial non-terminal state"
    );

    // Also sanity: the stale ChildCompleted event is still on the
    // parent's log (the scheduler hasn't removed it; the shadow is
    // purely a read-path concern).
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let stale_cnt = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"child_completed\"") && l.contains("\"failure\""))
        .count();
    assert_eq!(
        stale_cnt, 1,
        "stale ChildCompleted{{failure}} must still be on the parent's log"
    );

    // 4. Tick parent again. The on-disk fresh child must shadow the
    //    stale event: scheduler classifies A as `running` (non-terminal
    //    on disk), and the batch summary reports failed=0.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["task"], "A");
    assert_eq!(
        ledger[0]["outcome"],
        "running",
        "on-disk fresh child must shadow the stale ChildCompleted event; \
         scheduler must classify as running, not failure. ledger: {}",
        serde_json::to_string_pretty(sched).unwrap()
    );

    // Batch summary projection (via `koto status`) must agree: the
    // second read path (`build_children_complete_output`) applies the
    // same on-disk-wins rule, so `failed` is 0 and `running` (or
    // `in_flight`) accounts for A.
    let (ok, status, _) = run_koto(tmp.path(), &["status", "parent"]);
    assert!(ok);
    let batch = status
        .get("batch")
        .expect("batch section present for batch-scoped parent");
    let summary = &batch["summary"];
    assert_eq!(
        summary["failed"], 0,
        "stale failure must not appear in the batch summary; summary: {}",
        batch
    );
    assert_eq!(
        summary["success"], 0,
        "A hasn't completed yet after respawn; summary: {}",
        batch
    );
    // `total` stays 1; A is in-flight (running). Total pending+running
    // must cover A — we don't pin the exact bucket because different
    // summary projections label in-flight differently ("in_flight" vs
    // "running"), but the sum of non-terminal buckets must be 1.
    assert_eq!(summary["total"], 1);
    let non_terminal: i64 = ["pending", "running", "in_flight", "blocked"]
        .iter()
        .map(|k| summary.get(*k).and_then(|v| v.as_i64()).unwrap_or(0))
        .sum();
    assert_eq!(
        non_terminal, 1,
        "A must be accounted as non-terminal (running/in-flight/pending), not failed; summary: {}",
        batch
    );
}
