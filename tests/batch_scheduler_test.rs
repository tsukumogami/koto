//! Integration tests for the batch scheduler (Issue #12).
//!
//! Focus: scenario-15 — linear three-task batch runs to completion.
//!
//! The scheduler ticks once per `koto next <parent>` call. Each tick
//! classifies every submitted task from disk state and spawns ready
//! tasks. We drive the test tick-by-tick:
//!
//! - Tick 1: submit tasks A, B (waits_on A), C (waits_on B). Only A
//!   is ready; B and C are blocked.
//! - Tick 2: manually drive child A to a terminal state, then tick
//!   the parent. Classifier sees A as terminal-success, so B becomes
//!   Ready and is spawned.
//! - Tick 3: drive child B to terminal; tick parent. C is spawned.
//! - Tick 4: drive child C to terminal; tick parent. All three
//!   children terminal-success; the parent's children-complete gate
//!   passes and the parent transitions to the terminal `summarize`
//!   state. The scheduler produces `NoBatch` on the terminal state,
//!   so no scheduler key is attached to the final response.
//!
//! The test also guards against the "no-op on fully spawned batch"
//! invariant by re-running the parent tick after all three children
//! are alive-but-not-terminal: `spawned_this_tick` must be empty.

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

/// A parent template that declares a `materialize_children` hook on
/// its `plan` state. The `done` gate routes to `summarize` only when
/// the agent explicitly signals completion (`finalize: yes`), so the
/// parent stays in `plan` while we observe tick-by-tick scheduler
/// behavior.
///
/// Why not a `children-complete` gate? Issue #5 extends that gate to
/// read the batch definition from parent evidence; today's
/// implementation says `all_complete: true` as soon as a single
/// child on disk is terminal, which would transition the parent out
/// of `plan` after the first child finishes. That's a bug we don't
/// need to unblock for Issue #12 — the scheduler itself behaves
/// correctly regardless.
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

/// A minimal child template with a single `work` → `done` transition
/// and no variable requirements. The child's `work` state accepts a
/// trivial `marker` field so we can drive it to completion from
/// integration-test land with a single `koto next --with-data` call.
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

fn write_templates(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT_TEMPLATE).unwrap();
    parent
}

fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    // Parse the last non-empty line of stdout as JSON; koto emits one
    // JSON envelope per call.
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

/// Drive a freshly-spawned child through its `work` → `done`
/// transition by submitting a marker payload. `--no-cleanup` keeps
/// the child's state file on disk after it reaches the terminal
/// state so the parent's scheduler can classify it as `Success` on
/// subsequent ticks.
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

#[test]
fn linear_three_task_batch_runs_to_all_spawned() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    // Initialize the parent.
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

    // Tick 1: submit the task list. A has no deps; B depends on A;
    // C depends on B. Only A should spawn this tick.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
            {"name": "C", "waits_on": ["B"], "vars": {}},
        ]
    });
    let payload_str = payload.to_string();
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent", "--with-data", &payload_str]);
    let sched = json
        .get("scheduler")
        .expect("scheduler key attached after evidence submission");
    assert_eq!(
        sched.get("kind").and_then(|v| v.as_str()),
        Some("scheduled"),
        "expected scheduled outcome, got: {}",
        serde_json::to_string_pretty(sched).unwrap()
    );
    let spawned: Vec<String> = sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        spawned,
        vec!["parent.A".to_string()],
        "tick 1 must spawn only A; got {:?}",
        spawned
    );

    // Re-tick immediately. A is Running (not terminal), so nothing
    // new should spawn — the idempotency invariant.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned2: Vec<String> = sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned2.is_empty(),
        "no-op re-tick must not re-spawn; got {:?}",
        spawned2
    );

    // Tick 2: drive A to terminal, then re-tick parent. B should
    // spawn.
    drive_child_to_done(tmp.path(), "parent.A");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        spawned,
        vec!["parent.B".to_string()],
        "tick 2 must spawn B; got {:?}",
        spawned
    );

    // Tick 3: drive B to terminal, then re-tick parent. C should
    // spawn.
    drive_child_to_done(tmp.path(), "parent.B");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        spawned,
        vec!["parent.C".to_string()],
        "tick 3 must spawn C; got {:?}",
        spawned
    );

    // Now every task is materialized: spawned_this_tick is empty on
    // subsequent ticks until a child terminates.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned2: Vec<String> = sched
        .get("spawned_this_tick")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned2.is_empty(),
        "fully-spawned-but-not-done re-tick must be no-op; got {:?}",
        spawned2
    );

    // Sanity: the ledger lists all three children with their
    // outcomes. A and B are terminal-success; C is still Running.
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array present");
    assert_eq!(
        ledger.len(),
        3,
        "ledger must list all three submitted tasks"
    );
    let outcomes: std::collections::BTreeMap<String, String> = ledger
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().unwrap_or("").to_string();
            let outcome = entry["outcome"].as_str().unwrap_or("").to_string();
            (name, outcome)
        })
        .collect();
    assert_eq!(outcomes.get("parent.A"), Some(&"success".to_string()));
    assert_eq!(outcomes.get("parent.B"), Some(&"success".to_string()));
    assert_eq!(outcomes.get("parent.C"), Some(&"running".to_string()));
}

/// Return the parent's state-file path inside the sessions base.
fn parent_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

/// Submitting a task list that contains a cycle (A waits_on B, B
/// waits_on A) must be rejected PRE-APPEND by R0-R9 validation. The
/// caller receives a `BatchError::InvalidBatchDefinition` envelope
/// describing the cycle, and the parent's event log must remain free of
/// the `EvidenceSubmitted` event that would otherwise carry the
/// malformed payload. This is the "zero state on parent's event log"
/// guarantee from Issue #9.
#[test]
fn cycle_rejection_before_append() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    // Initialize the parent.
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

    // Capture the state-file contents as they stand pre-submission —
    // exactly one `WorkflowInitialized` line.
    let state_path = parent_state_path(tmp.path(), "parent");
    let before = std::fs::read_to_string(&state_path).unwrap();
    let before_count = before
        .lines()
        .filter(|l| l.contains("evidence_submitted"))
        .count();
    assert_eq!(
        before_count, 0,
        "pre-condition: no evidence_submitted events before the cycle submission"
    );

    // Submit a cyclic batch: A waits_on B, B waits_on A.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": ["B"], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
        ]
    });
    let payload_str = payload.to_string();
    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &payload_str])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "cyclic batch submission must fail"
    );

    // The envelope is the batch-error shape: `{"action": "error",
    // "batch": {"kind": "invalid_batch_definition", "reason": ...}}`.
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        json.get("action").and_then(|v| v.as_str()),
        Some("error"),
        "expected batch error envelope, got: {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let batch = json
        .get("batch")
        .expect("batch key present on error envelope");
    assert_eq!(
        batch.get("kind").and_then(|v| v.as_str()),
        Some("invalid_batch_definition"),
        "expected invalid_batch_definition, got: {}",
        serde_json::to_string_pretty(batch).unwrap_or_default()
    );
    let reason = batch.get("reason").expect("reason field present");
    // The reason payload uses `tag = "reason"` for its discriminator
    // (see `InvalidBatchReasonPayload` in `src/cli/batch_error.rs`);
    // the cycle path lifts a `cycle` array of task names alongside it.
    assert_eq!(
        reason.get("reason").and_then(|v| v.as_str()),
        Some("cycle"),
        "expected cycle reason, got: {}",
        serde_json::to_string_pretty(reason).unwrap_or_default()
    );
    let cycle = reason
        .get("cycle")
        .and_then(|v| v.as_array())
        .expect("cycle field is an array");
    assert!(
        cycle.iter().any(|v| v.as_str() == Some("A"))
            && cycle.iter().any(|v| v.as_str() == Some("B")),
        "cycle must name both tasks A and B, got: {:?}",
        cycle
    );

    // Post-condition: the parent's event log has no `evidence_submitted`
    // entry (Option A guarantee — validation runs pre-append).
    let after = std::fs::read_to_string(&state_path).unwrap();
    let after_count = after
        .lines()
        .filter(|l| l.contains("evidence_submitted"))
        .count();
    assert_eq!(
        after_count, 0,
        "cycle rejection must be pre-append; no EvidenceSubmitted should land on parent's log. \
         Log contents:\n{}",
        after
    );
}
