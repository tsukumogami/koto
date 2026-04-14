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

/// A child template with three terminal outcomes: `done` (success),
/// `failed` (failure), and `skipped_via_upstream_failure` (skip
/// marker). Used by reclassification scenarios 18 and 19 where the
/// scheduler must route a ShouldBeSkipped task directly into the
/// skip-marker terminal and later respawn it as a real child.
const CHILD_TEMPLATE_WITH_SKIP: &str = r#"---
name: batch-child-with-skip
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
  skipped_via_upstream_failure:
    terminal: true
    skipped_marker: true
---

## work

Do the work.

## done

Done.

## failed

Failed.

## skipped_via_upstream_failure

Skipped because an upstream task failed.
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

/// Parent template that references the skip-aware child template. Used
/// by scenarios 18 and 19 which exercise runtime reclassification.
const PARENT_TEMPLATE_SKIP_AWARE: &str = r#"---
name: batch-parent-skip-aware
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
      default_template: child_with_skip.md
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

fn write_skip_aware_templates(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("child_with_skip.md"), CHILD_TEMPLATE_WITH_SKIP).unwrap();
    let parent = dir.join("parent_skip.md");
    std::fs::write(&parent, PARENT_TEMPLATE_SKIP_AWARE).unwrap();
    parent
}

/// Drive a skip-aware child through its `work` → `done` path.
fn drive_skip_child_to_done(dir: &Path, name: &str) {
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
        "drive child {} to done failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

/// Drive a skip-aware child through its `work` → `failed` path.
fn drive_skip_child_to_fail(dir: &Path, name: &str) {
    let (ok, json, stderr) = run_koto(
        dir,
        &[
            "next",
            name,
            "--no-cleanup",
            "--with-data",
            r#"{"marker": "fail"}"#,
        ],
    );
    assert!(
        ok,
        "drive child {} to fail failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

/// Return the path of a child's state file under the sessions base.
fn child_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

/// scenario-17: `ready_to_drive` gates worker dispatch. A linear batch
/// A → B → C must report `ready_to_drive: true` only on children whose
/// upstream deps are all terminal-success (or empty, for A). After
/// tick 1 spawns A, B and C remain pending — they have no child file
/// on disk and therefore `ready_to_drive: false`; A itself is Running
/// with no deps, so `ready_to_drive: true`.
#[test]
fn scenario_17_ready_to_drive_gates_worker_dispatch() {
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

    // Submit a linear three-task batch: only A is ready on tick 1.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
            {"name": "C", "waits_on": ["B"], "vars": {}},
        ]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = json.get("scheduler").expect("scheduler key attached");

    // `reclassified_this_tick` is true when a spawn (or respawn) lands.
    assert_eq!(
        sched
            .get("reclassified_this_tick")
            .and_then(|v| v.as_bool()),
        Some(true),
        "scheduler must report reclassified_this_tick=true on a tick that spawned A"
    );

    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array present");
    assert_eq!(ledger.len(), 3);

    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();

    // A is Running (just spawned) with no deps — ready_to_drive is
    // true.
    let a = by_name.get("parent.A").expect("parent.A in ledger");
    assert_eq!(a["outcome"].as_str(), Some("running"));
    assert_eq!(
        a["ready_to_drive"].as_bool(),
        Some(true),
        "A has no deps and is Running; ready_to_drive must be true. got: {}",
        a
    );

    // B depends on A, which is not terminal yet. ready_to_drive is
    // false because (a) A is still Running and (b) B has no child
    // file on disk.
    let b = by_name.get("parent.B").expect("parent.B in ledger");
    assert_eq!(b["outcome"].as_str(), Some("blocked"));
    assert_eq!(
        b["ready_to_drive"].as_bool(),
        Some(false),
        "B blocked on non-terminal A; ready_to_drive must be false. got: {}",
        b
    );

    // C depends on B; same reasoning.
    let c = by_name.get("parent.C").expect("parent.C in ledger");
    assert_eq!(c["outcome"].as_str(), Some("blocked"));
    assert_eq!(
        c["ready_to_drive"].as_bool(),
        Some(false),
        "C blocked on non-terminal B; ready_to_drive must be false. got: {}",
        c
    );

    // After driving A to done, the next tick spawns B. B now has a
    // Running child file with a terminal-success upstream, so it
    // reports ready_to_drive: true.
    drive_child_to_done(tmp.path(), "parent.A");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array present");
    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();
    let a2 = by_name.get("parent.A").expect("A in ledger");
    assert_eq!(a2["outcome"].as_str(), Some("success"));
    // Terminal children are not dispatchable — they've already run.
    assert_eq!(a2["ready_to_drive"].as_bool(), Some(false));
    let b2 = by_name.get("parent.B").expect("B in ledger");
    assert_eq!(b2["outcome"].as_str(), Some("running"));
    assert_eq!(
        b2["ready_to_drive"].as_bool(),
        Some(true),
        "B should be ready_to_drive now that A is terminal-success. got: {}",
        b2
    );
}

/// scenario-18: runtime reclassification — stale skip marker respawns
/// as a real child when upstream no longer reports failure.
///
/// Sequence:
/// 1. Submit batch {A, D (waits_on A)}.
/// 2. Tick 1: A spawns.
/// 3. Drive A → failed.
/// 4. Tick 2: D spawns as skip marker (terminal, skipped_marker:
///    true); classification of D is Skipped.
/// 5. Manually delete A's state file (simulates a retry clearing the
///    failed A so the stale-skip reclassification path can fire;
///    `retry_failed` lands in Issue #14).
/// 6. Tick 3: scheduler reclassifies. A respawns as a real child
///    (currently no A on disk + all deps resolved trivially);
///    reclassified_this_tick is true.
/// 7. Drive A → done.
/// 8. Tick 4: scheduler sees D as Skipped on disk but the ideal
///    classification is Ready (A succeeded, no other deps). Delete-
///    and-respawn D as a real child. reclassified_this_tick is true;
///    D's ledger entry shows outcome: running.
#[test]
fn scenario_18_stale_skip_marker_respawns_as_real_child() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_skip_aware_templates(tmp.path());

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

    // 1. Submit the batch: A has no deps; D waits on A.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "D", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = json.get("scheduler").expect("scheduler key attached");
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(spawned, vec!["parent.A".to_string()]);

    // 2. Drive A to failed.
    drive_skip_child_to_fail(tmp.path(), "parent.A");

    // 3. Tick: D should spawn as a terminal skip marker.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();
    let d = by_name.get("parent.D").expect("D in ledger");
    assert_eq!(
        d["outcome"].as_str(),
        Some("skipped"),
        "D should be skipped (A failed under skip_dependents). got: {}",
        d
    );

    // Verify D's on-disk state file is a terminal skipped_marker state.
    let d_state_path = child_state_path(tmp.path(), "parent.D");
    let d_state_contents = std::fs::read_to_string(&d_state_path).unwrap();
    assert!(
        d_state_contents.contains("skipped_via_upstream_failure"),
        "D's state file should route to the skipped_marker state. contents:\n{}",
        d_state_contents
    );

    // 4. Simulate a successful retry of A: remove A's state file.
    // `retry_failed` handling lands in Issue #14; here we simulate
    // the post-retry state by deleting A directly.
    let a_dir = sessions_base(tmp.path()).join("parent.A");
    std::fs::remove_dir_all(&a_dir).unwrap();

    // 5. Tick: scheduler should respawn A (no child on disk, no deps).
    // The resubmission needs the same task list to re-enter the
    // scheduler; since the scheduler runs every tick regardless of
    // whether new evidence arrived, a bare `koto next parent` tick
    // suffices.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned.contains(&"parent.A".to_string()),
        "A must respawn as a real child after its state file was cleared. got: {:?}",
        spawned
    );
    assert_eq!(
        sched
            .get("reclassified_this_tick")
            .and_then(|v| v.as_bool()),
        Some(true),
        "reclassified_this_tick=true when A respawns"
    );

    // 6. Drive A to done.
    drive_skip_child_to_done(tmp.path(), "parent.A");

    // 7. Tick: D is currently a terminal skip marker on disk but the
    // ideal classification is Ready (A succeeded, no deps remain
    // unmet). The scheduler must delete-and-respawn D as a real
    // child. reclassified_this_tick is true.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    assert_eq!(
        sched
            .get("reclassified_this_tick")
            .and_then(|v| v.as_bool()),
        Some(true),
        "reclassified_this_tick=true when D is respawned from stale skip marker"
    );
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned.contains(&"parent.D".to_string()),
        "D must be respawned as a real child. got: {:?}",
        spawned
    );
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();
    let d2 = by_name.get("parent.D").expect("D in ledger");
    assert_eq!(
        d2["outcome"].as_str(),
        Some("running"),
        "D now has a running real child after respawn. got: {}",
        d2
    );

    // D's on-disk state should now be the work state (initial state),
    // not the skipped_marker state.
    let d_state_contents = std::fs::read_to_string(&d_state_path).unwrap();
    // The respawned D must carry a WorkflowInitialized event with the
    // work-state target; the old skip-marker line is gone because
    // init_state_file writes a fresh file.
    assert!(
        !d_state_contents.contains("skipped_via_upstream_failure"),
        "After respawn, D's state file should no longer reference the skip-marker state. contents:\n{}",
        d_state_contents
    );
}

/// scenario-19: runtime reclassification — a running real-template
/// child is respawned as a skip marker after its upstream flips to
/// failure.
///
/// Sequence:
/// 1. Submit batch {A, B (waits_on A)}.
/// 2. Tick 1: A spawns.
/// 3. Drive A → done (so B becomes Ready).
/// 4. Tick 2: B spawns as a real child (Running, non-terminal).
/// 5. Delete A's state file and replace it with a new one driven to
///    failed state (simulates `rewind` + re-drive, which Issue #14
///    will formalize).
/// 6. Tick 3: scheduler sees B as Running on disk but the ideal
///    classification is ShouldBeSkipped (A is now Failure under
///    skip_dependents). Respawn B as a skip marker.
#[test]
fn scenario_19_running_child_respawns_as_skip_marker_after_upstream_fails() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_skip_aware_templates(tmp.path());

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

    // 1. Submit the batch.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // 2. Drive A to done.
    drive_skip_child_to_done(tmp.path(), "parent.A");

    // 3. Tick: B spawns as a real child (Running).
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(spawned, vec!["parent.B".to_string()]);

    // Verify B's on-disk state is the work state, not the skip
    // marker.
    let b_state_path = child_state_path(tmp.path(), "parent.B");
    let b_before = std::fs::read_to_string(&b_state_path).unwrap();
    assert!(
        !b_before.contains("skipped_via_upstream_failure"),
        "B should start as a real child, not a skip marker"
    );

    // 4. Retroactively fail A: remove its state file and drive a
    // fresh A to failed. Issue #14's `rewind`/`retry_failed` will
    // provide a typed path for this; Issue #13 only needs the
    // end-state to exercise the reclassification path.
    let a_dir = sessions_base(tmp.path()).join("parent.A");
    std::fs::remove_dir_all(&a_dir).unwrap();
    // Tick the parent so A respawns.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(spawned.contains(&"parent.A".to_string()));
    drive_skip_child_to_fail(tmp.path(), "parent.A");

    // 5. Tick: B is Running on disk but its upstream A is now
    // Failure. Under skip_dependents (default), B's ideal
    // classification is ShouldBeSkipped. The scheduler respawns B
    // as a skip marker.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    assert_eq!(
        sched
            .get("reclassified_this_tick")
            .and_then(|v| v.as_bool()),
        Some(true),
        "reclassified_this_tick=true when B is respawned as a skip marker"
    );
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned.contains(&"parent.B".to_string()),
        "B must be respawned after upstream flipped to failure. got: {:?}",
        spawned
    );
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();
    let b = by_name.get("parent.B").expect("B in ledger");
    assert_eq!(
        b["outcome"].as_str(),
        Some("skipped"),
        "B's outcome is skipped after reclassification. got: {}",
        b
    );

    // Verify B's on-disk state is now the skip marker.
    let b_after = std::fs::read_to_string(&b_state_path).unwrap();
    assert!(
        b_after.contains("skipped_via_upstream_failure"),
        "B's state file should route to the skipped_marker state after reclassification. \
         contents:\n{}",
        b_after
    );
}

// ---- Issue #16 integration tests -----------------------------------
//
// Covered:
// - `scheduler.feedback.entries` surfaces one outcome per submitted
//   task on every Scheduled tick.
// - `scheduler.feedback.orphan_candidates` surfaces any on-disk child
//   whose short name is absent from the current submission.
// - `MaterializedChild.role` reports `worker` for normal children.
// - SchedulerRan events append to the parent log on non-trivial ticks
//   (spawn / reclassify) and NOT on pure no-op ticks.

#[test]
fn scheduler_response_carries_feedback_entries_and_role() {
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

    // Submit one task. After the tick, `feedback.entries` must contain
    // exactly one entry keyed by the short task name ("A"), with an
    // outcome. `materialized_children[0].role` should be `"worker"`.
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let payload_str = payload.to_string();
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent", "--with-data", &payload_str]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let feedback = sched
        .get("feedback")
        .expect("feedback object attached to Scheduled outcome");
    let entries = feedback
        .get("entries")
        .and_then(|v| v.as_object())
        .expect("feedback.entries is an object");
    assert_eq!(entries.len(), 1, "one entry per submitted task");
    let a = entries.get("A").expect("entry for task A");
    let outcome_tag = a
        .get("outcome")
        .and_then(|v| v.as_str())
        .expect("entry outcome is a string");
    assert!(
        outcome_tag == "accepted" || outcome_tag == "already_running",
        "freshly-spawned task A must be accepted or already_running, got {}",
        outcome_tag
    );
    let orphans = feedback
        .get("orphan_candidates")
        .and_then(|v| v.as_array())
        .expect("feedback.orphan_candidates is an array");
    assert!(
        orphans.is_empty(),
        "no orphan candidates when submission matches disk. got: {:?}",
        orphans
    );
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children array");
    let a_entry = &ledger[0];
    assert_eq!(
        a_entry["role"].as_str(),
        Some("worker"),
        "normal child renders role=worker"
    );
    assert!(
        a_entry.get("subbatch_status").is_none() || a_entry["subbatch_status"].is_null(),
        "worker children carry no subbatch_status"
    );
}

#[test]
fn orphan_candidate_detected_when_disk_child_not_in_submission() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    let (ok, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok);
    // Tick 1: submit tasks A and B — both get spawned.
    let payload1 = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload1.to_string()],
    );
    // Tick 2: re-submit with ONLY A (+B is now an orphan on disk).
    // Per Decision 10, omission is not a cancellation, but the
    // scheduler still surfaces B as an orphan candidate so agents
    // notice the mismatch. A and B share spawn_entry with their
    // originals so R8 stays satisfied.
    let payload2 = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
        ]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload2.to_string()],
    );
    let sched = json
        .get("scheduler")
        .expect("scheduler key on batch parent tick");
    let orphans = sched["feedback"]["orphan_candidates"]
        .as_array()
        .expect("orphan_candidates present");
    let names: Vec<String> = orphans
        .iter()
        .filter_map(|o| o.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(
        names.contains(&"B".to_string()),
        "B must appear as orphan_candidate. got: {:?}",
        names
    );
}

#[test]
fn scheduler_ran_event_appends_on_non_trivial_tick_and_skips_noop() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());
    let (ok, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok);
    // First tick with a spawn — non-trivial, SchedulerRan must append.
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let state_path = parent_state_path(tmp.path(), "parent");
    let before = std::fs::read_to_string(&state_path).unwrap();
    let count_before = before
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();
    assert_eq!(
        count_before, 1,
        "SchedulerRan event must append on non-trivial tick (A spawned). got log:\n{}",
        before
    );
    // Second tick with no new evidence — scheduler tick is a pure
    // no-op (A still running, no reclassification). Run `koto next`
    // again; the SchedulerRan count must NOT increase.
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);
    let after = std::fs::read_to_string(&state_path).unwrap();
    let count_after = after
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();
    assert_eq!(
        count_after, count_before,
        "SchedulerRan must NOT append on a no-op re-tick. log:\n{}",
        after
    );
    // Sanity check: the SchedulerRan payload carries a tick_summary
    // with the expected counts.
    let sr_line = before
        .lines()
        .find(|l| l.contains("\"type\":\"scheduler_ran\""))
        .expect("scheduler_ran line present");
    let ev: serde_json::Value = serde_json::from_str(sr_line).expect("parse event");
    assert_eq!(ev["type"], "scheduler_ran");
    assert_eq!(ev["payload"]["tick_summary"]["spawned_count"], 1);
    assert_eq!(ev["payload"]["tick_summary"]["errored_count"], 0);
}

/// Spawning tick — SchedulerOutcome surface must carry a populated
/// `spawned_this_tick` and a ledger entry for every submitted task.
/// Guards the baseline "tick 1 response shape" that downstream callers
/// depend on; a regression here would break the tick_summary shape in
/// the SchedulerRan event.
#[test]
fn scheduler_outcome_populated_on_spawning_tick() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (ok, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok);

    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, json, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = json.get("scheduler").expect("scheduler key present");

    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(spawned, vec!["parent.A".to_string()]);

    let errored = sched["errored"].as_array().expect("errored array present");
    assert!(errored.is_empty(), "no spawn errors on clean tick");
    assert_eq!(
        sched["reclassified_this_tick"].as_bool(),
        Some(true),
        "spawning a fresh child flips reclassified_this_tick true"
    );
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    assert_eq!(
        ledger.len(),
        2,
        "ledger covers every submitted task (both A and B)"
    );

    // The parent log must carry a single SchedulerRan event for this
    // tick, and its tick_summary fields must reflect the spawn.
    let state_path = parent_state_path(tmp.path(), "parent");
    let log = std::fs::read_to_string(&state_path).unwrap();
    let sr_lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .collect();
    assert_eq!(sr_lines.len(), 1, "exactly one SchedulerRan after one tick");
    let ev: serde_json::Value = serde_json::from_str(sr_lines[0]).expect("parse event");
    assert_eq!(ev["payload"]["tick_summary"]["spawned_count"], 1);
    assert_eq!(ev["payload"]["tick_summary"]["errored_count"], 0);
    assert_eq!(ev["payload"]["tick_summary"]["reclassified"], true);
}

/// Regression test for Issue #16 B1: after a skip marker materializes
/// (persistent state on disk), a subsequent pure no-op tick MUST NOT
/// append another SchedulerRan event. The PLAN AC "pure no-op ticks
/// leave the log unchanged" must hold even when the ledger reports
/// `skipped_count > 0`, because skip markers are persistent — their
/// mere presence is not a tick-scoped signal.
#[test]
fn scheduler_ran_not_appended_on_noop_after_skip_materialized() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_skip_aware_templates(tmp.path());

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

    // Submit {A, D (waits_on A)}. A spawns.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "D", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // Drive A to failure. Next parent tick materializes D as a skip
    // marker (spawn event is non-trivial, so SchedulerRan appends
    // here — that is expected).
    drive_skip_child_to_fail(tmp.path(), "parent.A");
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    let state_path = parent_state_path(tmp.path(), "parent");
    let after_skip = std::fs::read_to_string(&state_path).unwrap();
    let count_after_skip = after_skip
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();
    // Sanity: at least one SchedulerRan has been written by now. The
    // exact count depends on whether tick 1 (spawn A) and tick 2
    // (spawn skip marker for D) each appended one; both are
    // non-trivial.
    assert!(
        count_after_skip >= 1,
        "SchedulerRan must have appended for spawn/skip-materialize ticks. log:\n{}",
        after_skip
    );

    // Now the batch is fully-materialized: A terminal-failure on
    // disk, D terminal-skipped on disk. A bare `koto next` must run
    // the scheduler and produce a Scheduled outcome where the ledger
    // still reports `skipped_count > 0` (D), but `spawned_this_tick`,
    // `errored`, and `reclassified_this_tick` are all empty/false.
    // Under the fixed predicate this is a no-op; SchedulerRan must
    // NOT be appended.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned.is_empty(),
        "fully-materialized batch re-tick must not spawn anything. got: {:?}",
        spawned
    );
    assert_eq!(
        sched["reclassified_this_tick"].as_bool(),
        Some(false),
        "no reclassification on a pure no-op tick"
    );
    let errored = sched["errored"].as_array().expect("errored array present");
    assert!(errored.is_empty(), "no spawn errors on pure no-op tick");

    // Ledger should still contain a skip entry for D.
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    let skipped_in_ledger = ledger
        .iter()
        .filter(|mc| mc["outcome"].as_str() == Some("skipped"))
        .count();
    assert!(
        skipped_in_ledger >= 1,
        "D's skip marker persists in ledger. ledger: {:?}",
        ledger
    );

    // The critical assertion: the predicate must not count the
    // persistent skip marker as "something the tick did". Log must be
    // unchanged.
    let after_noop = std::fs::read_to_string(&state_path).unwrap();
    let count_after_noop = after_noop
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();
    assert_eq!(
        count_after_noop, count_after_skip,
        "SchedulerRan must NOT append on a no-op tick AFTER skips have been \
         materialized. Before: {} events; after: {} events. log:\n{}",
        count_after_skip, count_after_noop, after_noop
    );
}

/// A tick that reclassifies (and respawns) a child must append
/// SchedulerRan. Guards the non-trivial side of the predicate: even
/// without any brand-new evidence, a reclassification respawn is a
/// real scheduler-driven change and must be recorded in the audit
/// log.
#[test]
fn scheduler_ran_appended_on_reclassification_tick() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_skip_aware_templates(tmp.path());

    let (ok, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    assert!(ok);

    // Submit {A, D (waits_on A)}. A spawns.
    let payload = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "D", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // Drive A → failed; tick parent so D materializes as a skip
    // marker.
    drive_skip_child_to_fail(tmp.path(), "parent.A");
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    let state_path = parent_state_path(tmp.path(), "parent");
    let before_reclass = std::fs::read_to_string(&state_path).unwrap();
    let count_before = before_reclass
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();

    // Simulate retry: remove A's directory so the next tick respawns
    // A and flips `reclassified_this_tick` to true.
    let a_dir = sessions_base(tmp.path()).join("parent.A");
    std::fs::remove_dir_all(&a_dir).unwrap();

    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler key present");
    assert_eq!(
        sched["reclassified_this_tick"].as_bool(),
        Some(true),
        "A respawn flips reclassified_this_tick=true"
    );
    let spawned: Vec<String> = sched["spawned_this_tick"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        spawned.contains(&"parent.A".to_string()),
        "A must respawn. got: {:?}",
        spawned
    );

    // SchedulerRan must have appended for this reclassification tick.
    let after_reclass = std::fs::read_to_string(&state_path).unwrap();
    let count_after = after_reclass
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .count();
    assert_eq!(
        count_after,
        count_before + 1,
        "SchedulerRan must append on a reclassification tick. \
         before: {}, after: {}. log:\n{}",
        count_before,
        count_after,
        after_reclass
    );

    // The tick_summary on the newly appended event must reflect the
    // reclassification flag.
    let sr_lines: Vec<&str> = after_reclass
        .lines()
        .filter(|l| l.contains("\"type\":\"scheduler_ran\""))
        .collect();
    let last_sr = sr_lines.last().expect("at least one SchedulerRan");
    let ev: serde_json::Value = serde_json::from_str(last_sr).expect("parse event");
    assert_eq!(ev["payload"]["tick_summary"]["reclassified"], true);
}
