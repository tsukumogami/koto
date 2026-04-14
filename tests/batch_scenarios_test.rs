//! End-to-end batch scenarios (Issue #23).
//!
//! This file collects the integration scenarios from
//! `wip/implement-batch-child-spawning-test_plan.md` that prior issues
//! did not already cover. Each test drives the koto binary through the
//! CLI (so every assertion crosses the same serialization boundary the
//! agent sees) and checks both the response-shape snapshot AND the
//! on-disk state-file tail, per Issue #23 AC2.
//!
//! Cross-references:
//!
//! - scenario-15 linear batch → `batch_scheduler_test.rs`.
//! - scenario-17/18/19 reclassification sweep + ready_to_drive →
//!   `batch_scheduler_test.rs`.
//! - scenario-20 spawn_failed per-task accumulation →
//!   `batch_scheduler_test.rs`.
//! - scenario-23/24/25 retry_failed recovery (+ reserved_actions) →
//!   `batch_retry_test.rs`.
//! - scenario-26 crash-resume with half-initialized children →
//!   `batch_scheduler_test.rs` (repair pass) and
//!   `integration_test.rs`.
//! - scenario-27/28 koto status + derive_batch_view →
//!   `batch_scheduler_test.rs`.
//! - scenario-29 auto-reconciliation, scenario-30 push-parent-first →
//!   `batch_session_resolve_test.rs`.
//! - scenario-32 concurrent-tick flock → `batch_lock_test.rs`.
//!
//! Scenarios added here that prior issues did not cover:
//!
//! - scenario-16: diamond DAG with parallel branches.
//! - scenario-31: nested-batch parent rejected via `ChildIsBatchParent`.
//! - scenario-33: limit-exceeded rejection (tasks / waits_on / depth).
//! - scenario-35: mid-flight task-list append is a no-op (identical
//!   resubmission) and R8 rejects mutation of spawned tasks.
//!
//! Note on scenario-39/40 (meta): the CI workflow enforces that `wip/`
//! is empty before merge to main (see `.github/workflows/` and the
//! repository `CLAUDE.md` wip-policy section). The Gherkin feature
//! files under `test/functional/features/` exercise the CLI surface
//! that backs every batch scenario; batch-specific scheduler behavior
//! is asserted through these Rust integration tests against the
//! compiled binary.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
//  Harness helpers (same pattern as batch_scheduler_test.rs and
//  batch_retry_test.rs so the fixtures stay trivially diffable).
// ---------------------------------------------------------------------

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

fn parent_state_path(dir: &Path, name: &str) -> PathBuf {
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

// ---------------------------------------------------------------------
//  Templates. Kept in one place so every test shares one parent shape.
// ---------------------------------------------------------------------

/// Parent template with `materialize_children` on `plan`. Mirrors the
/// shape used in `batch_scheduler_test.rs` and `batch_retry_test.rs`.
const PARENT_TEMPLATE: &str = r#"---
name: batch-parent-scenarios
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

Summarize.
"#;

/// Minimal child template: `work` → `done`, with a single `marker`
/// payload. No `failure` or skip-marker state — enough to carry the
/// diamond-DAG and mid-flight tests through success.
const CHILD_TEMPLATE: &str = r#"---
name: batch-child-scenarios
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

/// A child template that itself declares `materialize_children`, so
/// `classify_child_outcome` flags it as a batch parent. Used only by
/// scenario-31. The template declares `inner_plan` with a
/// `children-complete` gate so E10 compile validation passes.
const NESTED_PARENT_CHILD_TEMPLATE: &str = r#"---
name: batch-child-nested-parent
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [fail]
    transitions:
      - target: failed
        when:
          marker: fail
  failed:
    terminal: true
    failure: true
  inner_plan:
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
      default_template: grandchild.md
    transitions:
      - target: inner_done
        when:
          finalize: yes
  inner_done:
    terminal: true
---

## work

Do the work.

## failed

Failed.

## inner_plan

Plan inner work.

## inner_done

Inner batch complete.
"#;

fn write_templates(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT_TEMPLATE).unwrap();
    parent
}

/// Drive a freshly spawned `CHILD_TEMPLATE` child through work → done.
fn drive_child_to_done(dir: &Path, name: &str) {
    let (ok, json, stderr) = run_koto(
        dir,
        &[
            "next",
            name,
            "--no-cleanup",
            "--with-data",
            r#"{"marker": "done"}"#,
        ],
    );
    assert!(
        ok,
        "drive child {} failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

fn init_parent(dir: &Path, parent_path: &Path) {
    let (ok, _, stderr) = run_koto(
        dir,
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "parent init failed: {}", stderr);
}

fn scheduler_block(json: &serde_json::Value) -> &serde_json::Value {
    json.get("scheduler").unwrap_or_else(|| {
        panic!(
            "scheduler key missing from response: {}",
            serde_json::to_string_pretty(json).unwrap_or_default()
        )
    })
}

fn spawned_this_tick(sched: &serde_json::Value) -> Vec<String> {
    sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn ledger_by_name(sched: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let name = e["name"].as_str().unwrap_or("").to_string();
                    (name, e.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

// =====================================================================
//  scenario-16: diamond DAG with parallel branches
// =====================================================================
//
// Shape:
//
//       A
//      / \
//     B   C
//      \ /
//       D
//
// Expectations (per `wip/implement-batch-child-spawning-test_plan.md`
// §Scenario 16 and DESIGN-batch-child-spawning.md §Decision 8):
//
// - Tick 1: submit the batch; only A is Ready (everything else blocks
//   on A).
// - Tick 2: A terminal-success → B and C both become Ready in the same
//   tick; they spawn together. D is still Blocked (waits_on both).
// - Tick 3: drive B to done (C still Running). No new spawn — D still
//   Blocked.
// - Tick 4: drive C to done. D becomes Ready and spawns.
// - Tick 5: drive D to done.
// - Final tick: every task terminal-success; the ledger reports each
//   child with outcome=success; the parent's state-file tail carries
//   matching `evidence_submitted` events (one per resubmission-less
//   tick is append-free after the scheduler finishes).
#[test]
fn scenario_16_diamond_dag_parallel_branches() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    // Tick 1 — submit the diamond.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
            {"name": "C", "waits_on": ["A"], "vars": {}},
            {"name": "D", "waits_on": ["B", "C"], "vars": {}},
        ]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = scheduler_block(&json);
    assert_eq!(
        sched["kind"].as_str(),
        Some("scheduled"),
        "scheduler outcome kind on tick 1: {}",
        sched
    );
    let mut spawned = spawned_this_tick(sched);
    spawned.sort();
    assert_eq!(
        spawned,
        vec!["parent.A".to_string()],
        "tick 1 must spawn A alone (B/C blocked on A, D blocked on both)"
    );

    // Ledger assertions: B, C, D all reported as `blocked` with the
    // right waits_on shape.
    let by_name = ledger_by_name(sched);
    assert_eq!(by_name.get("parent.A").unwrap()["outcome"], "running");
    assert_eq!(by_name.get("parent.B").unwrap()["outcome"], "blocked");
    assert_eq!(by_name.get("parent.C").unwrap()["outcome"], "blocked");
    assert_eq!(by_name.get("parent.D").unwrap()["outcome"], "blocked");

    // Tick 2 — drive A done, re-tick. B and C should both fire.
    drive_child_to_done(tmp.path(), "parent.A");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = scheduler_block(&json);
    let mut spawned = spawned_this_tick(sched);
    spawned.sort();
    assert_eq!(
        spawned,
        vec!["parent.B".to_string(), "parent.C".to_string()],
        "tick 2 must spawn B and C in one tick (parallel branches)"
    );
    let by_name = ledger_by_name(sched);
    assert_eq!(by_name.get("parent.D").unwrap()["outcome"], "blocked");

    // Tick 3 — drive B done. C still running; D must stay blocked.
    drive_child_to_done(tmp.path(), "parent.B");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = scheduler_block(&json);
    let spawned = spawned_this_tick(sched);
    assert!(
        spawned.is_empty(),
        "D must not spawn until BOTH B and C are terminal; got {:?}",
        spawned
    );
    let by_name = ledger_by_name(sched);
    assert_eq!(by_name.get("parent.B").unwrap()["outcome"], "success");
    assert_eq!(by_name.get("parent.C").unwrap()["outcome"], "running");
    assert_eq!(by_name.get("parent.D").unwrap()["outcome"], "blocked");

    // Tick 4 — drive C done. D is now the only Ready task; it spawns.
    drive_child_to_done(tmp.path(), "parent.C");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = scheduler_block(&json);
    let spawned = spawned_this_tick(sched);
    assert_eq!(
        spawned,
        vec!["parent.D".to_string()],
        "tick 4 must spawn D now that B and C are terminal"
    );

    // Tick 5 — drive D done, re-tick. All four children terminal.
    drive_child_to_done(tmp.path(), "parent.D");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = scheduler_block(&json);
    let by_name = ledger_by_name(sched);
    for child in ["parent.A", "parent.B", "parent.C", "parent.D"] {
        assert_eq!(
            by_name.get(child).unwrap()["outcome"],
            "success",
            "{} must be terminal-success at end of diamond",
            child
        );
    }

    // On-disk tail assertion (AC2). The parent log must carry exactly
    // one `evidence_submitted` event for the diamond's `tasks` payload
    // (we submitted once on tick 1).
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let task_submissions = parent_log
        .lines()
        .filter(|l| l.contains("evidence_submitted") && l.contains("\"tasks\""))
        .count();
    assert_eq!(
        task_submissions, 1,
        "diamond should have one tasks submission; parent log:\n{}",
        parent_log
    );
}

// =====================================================================
//  scenario-31: nested-batch parent rejected via ChildIsBatchParent
// =====================================================================
//
// A child whose compiled template carries a `materialize_children` hook
// on any state is a batch parent. `retry_failed` targeting such a
// child rejects pre-dispatch with
// `InvalidRetryReason::ChildIsBatchParent { children: [...] }` per
// Decision 9 / design §1752-1763.
//
// Setup:
// - Parent batch contains two tasks: `leaf` (uses the default leaf
//   template) and `nested` (overrides to NESTED_PARENT_CHILD_TEMPLATE,
//   which declares `materialize_children`).
// - Drive `nested` to a failed terminal so it would otherwise be
//   retryable on outcome alone.
// - Submit `retry_failed: {children: ["nested"]}`.
//
// Expected:
// - Exit is non-zero (retry rejection).
// - Response envelope is `{"action": "error", "batch": {
//     "kind": "invalid_retry_request",
//     "reason": {"reason": "child_is_batch_parent", "children": ["nested"]}
//   }}`.
// - Parent state file is byte-identical before and after the retry
//   (atomicity — no `evidence_submitted` or `rewound` writes).
#[test]
fn scenario_31_retry_rejected_when_child_is_batch_parent() {
    let tmp = TempDir::new().unwrap();

    // Parent + leaf child template.
    let parent_path = write_templates(tmp.path());
    // Nested-batch child template plus a trivial grandchild for
    // compile-time E9 resolution.
    std::fs::write(
        tmp.path().join("nested_child.md"),
        NESTED_PARENT_CHILD_TEMPLATE,
    )
    .unwrap();
    std::fs::write(tmp.path().join("grandchild.md"), CHILD_TEMPLATE).unwrap();

    init_parent(tmp.path(), &parent_path);

    // Submit the batch: `leaf` uses the default child.md, `nested`
    // overrides to the batch-parent template.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "leaf", "waits_on": [], "vars": {}},
            {
                "name": "nested",
                "waits_on": [],
                "vars": {},
                "template": "nested_child.md"
            },
        ]
    });
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    assert!(
        ok,
        "batch submission must succeed. stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let sched = scheduler_block(&json);
    let mut spawned = spawned_this_tick(sched);
    spawned.sort();
    assert_eq!(
        spawned,
        vec!["parent.leaf".to_string(), "parent.nested".to_string()],
        "both children should spawn on tick 1. sched={}",
        serde_json::to_string_pretty(sched).unwrap_or_default()
    );

    // Drive `nested` to failed (the template accepts marker=fail) so
    // outcome=failure → would normally be retryable on outcome alone.
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "parent.nested",
            "--no-cleanup",
            "--with-data",
            r#"{"marker": "fail"}"#,
        ],
    );
    assert!(
        ok,
        "drive nested to failed: stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Drive `leaf` to done so the only candidate for retry is the
    // batch-parent child.
    drive_child_to_done(tmp.path(), "parent.leaf");

    // Tick once so the scheduler observes the terminal outcomes; this
    // is the "normal state" against which we diff the state file.
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    // Capture parent log + nested child log pre-retry for atomicity
    // check.
    let parent_log_before =
        std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let nested_log_before =
        std::fs::read_to_string(parent_state_path(tmp.path(), "parent.nested")).unwrap();

    // Submit the retry targeting the nested-batch child.
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["nested"]}
    });
    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &retry_payload.to_string()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "retry_failed naming a batch-parent child must fail"
    );

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or_else(|e| {
        panic!("parse json from stdout={}: err={}", stdout, e);
    });

    // Response-shape snapshot.
    assert_eq!(
        json["action"].as_str(),
        Some("error"),
        "envelope action: {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let batch = json
        .get("batch")
        .expect("batch error payload present on envelope");
    assert_eq!(batch["kind"].as_str(), Some("invalid_retry_request"));
    assert_eq!(
        batch["reason"]["reason"].as_str(),
        Some("child_is_batch_parent"),
        "inner reason code: {}",
        serde_json::to_string_pretty(batch).unwrap_or_default()
    );
    let children: Vec<String> = batch["reason"]["children"]
        .as_array()
        .expect("children array present")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(
        children,
        vec!["nested".to_string()],
        "children must name the rejected batch-parent child"
    );

    // On-disk state-file tail assertion (AC2): atomicity — no mutation.
    let parent_log_after =
        std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        parent_log_before, parent_log_after,
        "ChildIsBatchParent rejection must not mutate the parent's state file"
    );
    let nested_log_after =
        std::fs::read_to_string(parent_state_path(tmp.path(), "parent.nested")).unwrap();
    assert_eq!(
        nested_log_before, nested_log_after,
        "ChildIsBatchParent rejection must not mutate the nested child's state file"
    );
    assert!(
        !nested_log_after.contains("\"rewound\""),
        "nested child must not carry a rewound event after rejection"
    );
}

// =====================================================================
//  scenario-33: limit-exceeded rejection (tasks / waits_on / depth)
// =====================================================================
//
// Per `src/engine/batch_validation.rs`:
//   MAX_TASKS_PER_SUBMISSION = 1000
//   MAX_WAITS_ON_PER_TASK    = 10
//   MAX_DAG_DEPTH            = 50
//
// Each violation must reject pre-append with
// `BatchError::LimitExceeded { which: LimitKind::{Tasks|WaitsOn|Depth},
// limit, actual, task: Option<String> }`. Task-scoped violations
// (WaitsOn) carry the offending task name; global violations (Tasks,
// Depth) omit it.
//
// Each sub-test: submit a violating batch, assert the JSON envelope
// shape, AND confirm the parent's state file tail is byte-identical to
// the post-init baseline (R6 runs pre-append per Issue #9 AC).
#[test]
fn scenario_33_limit_exceeded_tasks_over_1000() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    let baseline = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();

    // 1001 independent tasks.
    let tasks: Vec<serde_json::Value> = (0..1001)
        .map(|i| serde_json::json!({"name": format!("t{}", i), "waits_on": [], "vars": {}}))
        .collect();
    let payload = serde_json::json!({"tasks": tasks});
    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &payload.to_string()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "1001-task submission must reject");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap();

    assert_eq!(json["action"].as_str(), Some("error"));
    let batch = json.get("batch").expect("batch payload present");
    assert_eq!(batch["kind"].as_str(), Some("limit_exceeded"));
    assert_eq!(batch["which"].as_str(), Some("tasks"));
    assert_eq!(batch["limit"].as_u64(), Some(1000));
    assert_eq!(batch["actual"].as_u64(), Some(1001));
    assert!(
        batch.get("task").is_none() || batch["task"].is_null(),
        "tasks-limit violations are global; task field must be absent/null. payload={}",
        serde_json::to_string_pretty(batch).unwrap()
    );

    // On-disk tail: pre-append rejection, no growth.
    let after = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        baseline, after,
        "LimitExceeded {{Tasks}} must reject pre-append"
    );
}

#[test]
fn scenario_33_limit_exceeded_waits_on_over_10() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    let baseline = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();

    // 11 parents + one task that waits on all of them.
    let mut tasks: Vec<serde_json::Value> = (0..11)
        .map(|i| serde_json::json!({"name": format!("p{}", i), "waits_on": [], "vars": {}}))
        .collect();
    let deps: Vec<String> = (0..11).map(|i| format!("p{}", i)).collect();
    tasks.push(serde_json::json!({"name": "late", "waits_on": deps, "vars": {}}));
    let payload = serde_json::json!({"tasks": tasks});

    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &payload.to_string()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "waits_on=11 submission must reject"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap();

    assert_eq!(json["action"].as_str(), Some("error"));
    let batch = json.get("batch").expect("batch payload present");
    assert_eq!(batch["kind"].as_str(), Some("limit_exceeded"));
    assert_eq!(batch["which"].as_str(), Some("waits_on"));
    assert_eq!(batch["limit"].as_u64(), Some(10));
    assert_eq!(batch["actual"].as_u64(), Some(11));
    // Task-scoped violation: name MUST be populated.
    assert_eq!(
        batch["task"].as_str(),
        Some("late"),
        "waits_on violations are task-scoped; task field must name the offender. payload={}",
        serde_json::to_string_pretty(batch).unwrap()
    );

    let after = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        baseline, after,
        "LimitExceeded {{WaitsOn}} must reject pre-append"
    );
}

#[test]
fn scenario_33_limit_exceeded_depth_over_50() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    let baseline = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();

    // Linear chain of 51 tasks: t0 → t1 → … → t50 (51 nodes = depth 51).
    let tasks: Vec<serde_json::Value> = (0..51)
        .map(|i| {
            let waits_on: Vec<String> = if i == 0 {
                Vec::new()
            } else {
                vec![format!("t{}", i - 1)]
            };
            serde_json::json!({
                "name": format!("t{}", i),
                "waits_on": waits_on,
                "vars": {}
            })
        })
        .collect();
    let payload = serde_json::json!({"tasks": tasks});

    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &payload.to_string()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "depth-51 chain must reject");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap();

    assert_eq!(json["action"].as_str(), Some("error"));
    let batch = json.get("batch").expect("batch payload present");
    assert_eq!(batch["kind"].as_str(), Some("limit_exceeded"));
    assert_eq!(batch["which"].as_str(), Some("depth"));
    assert_eq!(batch["limit"].as_u64(), Some(50));
    assert_eq!(batch["actual"].as_u64(), Some(51));
    assert!(
        batch.get("task").is_none() || batch["task"].is_null(),
        "depth violations are global (not task-scoped); task field must be absent/null. payload={}",
        serde_json::to_string_pretty(batch).unwrap()
    );

    let after = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        baseline, after,
        "LimitExceeded {{Depth}} must reject pre-append"
    );
}

// =====================================================================
//  scenario-35: mid-flight task-list append semantics
// =====================================================================
//
// Decision 10 (design §1857-1905) pins the mutation rules:
//
// - Identical resubmission: appends `EvidenceSubmitted` for audit, the
//   scheduler tick finds every task already spawned → no-op for spawn
//   behavior. Per-entry feedback reports `already_running` or
//   `already_terminal_*`.
// - Augmented resubmission (adds NEW tasks, preserves existing entries
//   byte-for-byte): union-by-name. The new names materialize as
//   children; existing spawned entries remain locked.
// - Mutating resubmission (changes `template` / `vars` / `waits_on`
//   for an already-spawned task): R8 rejects pre-append with
//   `InvalidBatchReason::SpawnedTaskMutated`. No state change.
//
// This test exercises the mutation-rejection path — the sharpest
// "mid-flight append is a no-op" signal in v1. `update_tasks` (the
// primitive that would make augmented submissions add-only-valid) is
// deferred to v1.1; in v1 the design explicitly says omission is a
// no-op and mutation rejects.
#[test]
fn scenario_35_mid_flight_mutation_rejected_via_r8() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    // Tick 1: submit A, B, C.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
            {"name": "C", "waits_on": ["B"], "vars": {}},
        ]
    });
    let (_, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = scheduler_block(&json);
    assert_eq!(
        spawned_this_tick(sched),
        vec!["parent.A".to_string()],
        "tick 1 must spawn A; stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );

    // Capture post-tick-1 parent log; mutation rejection must leave it
    // unchanged.
    let log_before = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();

    // Tick 2: resubmit an AUGMENTED-AND-MUTATED task list. A's
    // `template` flips from the default (null/omitted) to
    // "alt_child.md" — R8 mutation on the already-spawned child A. A
    // new task D is also appended, but the whole submission rejects
    // as soon as R8 catches the mutation on A.
    //
    // We write `alt_child.md` alongside so template_not_found doesn't
    // mask the R8 rejection (R8 runs before template resolution, but
    // defensively we keep both files on disk so diagnostics stay
    // readable if something else fires first).
    std::fs::write(tmp.path().join("alt_child.md"), CHILD_TEMPLATE).unwrap();
    let mutated = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}, "template": "alt_child.md"},
            {"name": "B", "waits_on": ["A"], "vars": {}},
            {"name": "C", "waits_on": ["B"], "vars": {}},
            {"name": "D", "waits_on": ["C"], "vars": {}},
        ]
    });
    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &mutated.to_string()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "mid-flight MUTATION (not just append) must reject via R8"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_eq!(json["action"].as_str(), Some("error"));
    let batch = json.get("batch").expect("batch envelope present");
    assert_eq!(batch["kind"].as_str(), Some("invalid_batch_definition"));
    assert_eq!(
        batch["reason"]["reason"].as_str(),
        Some("spawned_task_mutated"),
        "R8 must fire with a typed spawned_task_mutated reason: {}",
        serde_json::to_string_pretty(batch).unwrap_or_default()
    );
    assert_eq!(
        batch["reason"]["task"].as_str(),
        Some("A"),
        "the mutated task name must be echoed back"
    );
    let changed = batch["reason"]["changed_fields"]
        .as_array()
        .expect("changed_fields array present");
    assert!(
        !changed.is_empty(),
        "changed_fields must enumerate the diff: {}",
        serde_json::to_string_pretty(batch).unwrap_or_default()
    );

    // On-disk tail: mutation rejection is pre-append, so the log is
    // byte-identical to tick 1's end-state.
    let log_after = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        log_before, log_after,
        "R8 rejection must not grow the parent's state file"
    );

    // Child D must not exist on disk — atomicity of the whole
    // submission. (The append of D would have happened AFTER the R8
    // check on A, so D's state file should never have been created.)
    let d_path = parent_state_path(tmp.path(), "parent.D");
    assert!(
        !d_path.exists(),
        "D must not be materialized when the enclosing submission was R8-rejected. path={:?}",
        d_path
    );
}

// An identical (byte-for-byte) resubmission appends an
// `EvidenceSubmitted` event for audit, per design §1899-1905, and the
// scheduler tick observes every existing child as already spawned —
// `spawned_this_tick` is empty. The per-entry feedback map records
// each already-materialized entry. This guards the "no-op spawn
// behavior" half of scenario-35.
#[test]
fn scenario_35_identical_resubmission_is_noop_spawn() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    init_parent(tmp.path(), &parent_path);

    // Tick 1: submit A (sole task).
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = scheduler_block(&json);
    assert_eq!(spawned_this_tick(sched), vec!["parent.A".to_string()]);

    // Tick 2: resubmit the SAME payload byte-for-byte while A is still
    // running.
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    assert!(
        ok,
        "identical resubmission must pass validation. stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let sched = scheduler_block(&json);

    // No new spawns (A is already running).
    let spawned = spawned_this_tick(sched);
    assert!(
        spawned.is_empty(),
        "identical resubmission must be a no-op for spawn behavior; got {:?}",
        spawned
    );

    // The feedback map (if present on the outcome) flags A as
    // already_running; tolerate backends that elide empty feedback by
    // falling back to the ledger check.
    let feedback_entries = sched
        .get("feedback")
        .and_then(|f| f.get("entries"))
        .and_then(|e| e.as_object());
    if let Some(entries) = feedback_entries {
        if let Some(v) = entries.get("A") {
            let outcome = v.get("outcome").and_then(|o| o.as_str()).unwrap_or("");
            assert!(
                outcome == "already_running"
                    || outcome == "accepted"
                    || outcome == "already_terminal_success",
                "identical resubmission feedback for A: {} (entry={})",
                outcome,
                v
            );
        }
    }

    // Parent log must carry two evidence_submitted events for `tasks`
    // — one per tick — documenting the audit trail per design.
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let submissions = parent_log
        .lines()
        .filter(|l| l.contains("evidence_submitted") && l.contains("\"tasks\""))
        .count();
    assert_eq!(
        submissions, 2,
        "identical resubmission still appends for audit; parent log:\n{}",
        parent_log
    );
}
