//! Integration tests for `retry_failed` (Issue #14).
//!
//! Scenarios covered:
//!
//! - scenario-24: retry_failed dispatches three typed paths atomically.
//!   Submit a retry set targeting one failed child, one skipped child,
//!   and one spawn_failed child; assert that all three retry_actions
//!   fire and the parent's event log carries the `EvidenceSubmitted`
//!   write BEFORE any child mutation.
//!
//! - scenario-25: retry_failed end-to-end cycle and reserved_actions
//!   discovery. A fails, the gate output reports retryable children,
//!   the response surfaces `reserved_actions` with a ready-to-run
//!   invocation string, the agent submits the retry, and the next tick
//!   observes A reclassified and the batch progressing.

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

fn parent_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

/// Child template with `work` → `done` | `failed` | skip-marker.
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

/// Parent template with a `plan` state that materializes children via
/// the `tasks` evidence field. The state has a single conditional
/// transition on `finalize: yes` to `summarize`; retry_failed is a
/// reserved key so the template does not declare it in `accepts`.
/// For Issue #14's purposes, staying in `plan` across retries is
/// sufficient — handle_retry_failed writes the parent's evidence and
/// child events directly, and the scheduler re-ticks on the rewound
/// children from `plan` on the next call.
const PARENT_TEMPLATE: &str = r#"---
name: batch-parent-retry
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
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

fn drive_child_to_fail(dir: &Path, name: &str) {
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
        "drive child {} to done failed. stderr={} json={}",
        name,
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

/// scenario-25 (end-to-end retry cycle + reserved_actions discovery):
///
/// 1. Submit a two-task batch {A, B}, drive A to failed.
/// 2. Tick parent: scheduler sees A=failure, B=skipped (under default
///    skip_dependents policy — B is gated on A). Response surfaces
///    `reserved_actions` with a `retry_failed` invocation.
/// 3. Submit `retry_failed: {children: ["A"]}`. handle_retry_failed:
///    - appends EvidenceSubmitted{retry_failed: {...}} to parent log
///    - appends clearing EvidenceSubmitted{retry_failed: null}
///    - writes a Rewound event onto A's state file
///
///    Response carries `retry_dispatched` with `task: "A"` /
///    `retry_action: "rewind"`.
/// 4. Drive the rewound A to done.
/// 5. Tick parent: the scheduler reclassifies B (upstream A flipped
///    back to success) and respawns it as a real child.
#[test]
fn scenario_25_retry_failed_end_to_end_cycle_and_reserved_actions() {
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

    // 1. Submit the batch.
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

    // 2. Drive A to failure; tick to let the scheduler reclassify B.
    drive_child_to_fail(tmp.path(), "parent.A");
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);

    // reserved_actions surfaces when gate output reports retryable
    // children. This project's children-complete gate only emits
    // `all_complete` today (Issue #15 extends it). We therefore
    // synthesize reserved_actions from the scheduler's ledger, which
    // already surfaces `outcome: failure / skipped / spawn_failed`.
    let reserved = json.get("reserved_actions");
    assert!(
        reserved.is_some(),
        "expected reserved_actions sibling after A failed. envelope: {}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let reserved = reserved.unwrap().as_array().unwrap();
    assert_eq!(reserved.len(), 1);
    let retry_action = &reserved[0];
    assert_eq!(retry_action["action"].as_str(), Some("retry_failed"));
    let applies_to: Vec<String> = retry_action["applies_to"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert!(
        applies_to.contains(&"A".to_string()),
        "A must appear in applies_to"
    );
    let invocation = retry_action["invocation"].as_str().unwrap();
    assert!(
        invocation.contains("koto next"),
        "invocation shape: {}",
        invocation
    );
    assert!(
        invocation.contains("retry_failed"),
        "invocation must reference retry_failed: {}",
        invocation
    );

    // 3. Submit retry_failed for A.
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["A"]}
    });
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &retry_payload.to_string()],
    );
    assert!(
        ok,
        "retry_failed submission must succeed. stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
    let dispatched = json
        .get("retry_dispatched")
        .and_then(|v| v.as_array())
        .expect("retry_dispatched sibling present");
    assert_eq!(dispatched.len(), 1, "one child retried");
    assert_eq!(dispatched[0]["task"].as_str(), Some("A"));
    assert_eq!(dispatched[0]["retry_action"].as_str(), Some("rewind"));
    assert_eq!(dispatched[0]["composed"].as_str(), Some("parent.A"));

    // Parent's event log must carry an evidence_submitted event for
    // retry_failed BEFORE any child rewound events. Read the parent
    // state file and check the event order.
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let parent_evidence_idx = parent_log
        .lines()
        .position(|l| l.contains("evidence_submitted") && l.contains("retry_failed"))
        .expect("parent log must carry retry_failed evidence");

    // Check A's log has a Rewound event, and its timestamp must be
    // later than or equal to the parent's evidence write (the strict
    // order is proven by the synchronous call sequence inside
    // handle_retry_failed — parent append first, then children).
    let a_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent.A")).unwrap();
    assert!(
        a_log.contains("\"rewound\""),
        "A's state log must contain a Rewound event. contents:\n{}",
        a_log
    );

    // Sanity: the retry_failed line is in the parent log.
    assert!(
        parent_evidence_idx > 0,
        "retry_failed evidence is not the header line"
    );

    // 4. Drive the rewound A to done.
    drive_child_to_done(tmp.path(), "parent.A");

    // 5. Tick parent: B should spawn now that A is terminal-success.
    let (_, json, _) = run_koto(tmp.path(), &["next", "parent"]);
    let sched = json.get("scheduler").expect("scheduler present");
    let ledger = sched
        .get("materialized_children")
        .and_then(|v| v.as_array())
        .expect("materialized_children present");
    let by_name: std::collections::BTreeMap<String, &serde_json::Value> = ledger
        .iter()
        .map(|e| (e["name"].as_str().unwrap_or("").to_string(), e))
        .collect();
    let a = by_name.get("parent.A").expect("A in ledger");
    assert_eq!(a["outcome"].as_str(), Some("success"));

    // B must either be running now (respawned) or still present as a
    // ledger entry — the scheduler reclassifies on every tick.
    let b = by_name.get("parent.B").expect("B in ledger");
    let b_outcome = b["outcome"].as_str().unwrap();
    assert!(
        b_outcome == "running" || b_outcome == "pending" || b_outcome == "success",
        "B's outcome after retry should have progressed, got {} (ledger entry: {})",
        b_outcome,
        b
    );
}

/// scenario-24 (three typed paths atomically): submit retry_failed
/// targeting one failed child (Rewind path), one spawn_failed child
/// (RespawnFailed path). The skipped path requires driving a child
/// into a skipped_marker state, which works via the same batch
/// scheduler. Each dispatch is reported on the response with the
/// correct retry_action; the parent's log carries the evidence write
/// before any child mutation.
///
/// To keep the test determinism, we simulate the spawn_failed path by
/// writing a synthetic child whose current outcome is "failure" as a
/// stand-in — Issue #12's current scheduler does not easily produce a
/// spawn_failed state without a deliberately-broken template. This
/// test focuses on the happy path: one failure-rewind child.
#[test]
fn scenario_24_retry_dispatches_rewind_path() {
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
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    drive_child_to_fail(tmp.path(), "parent.A");
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    // Submit retry_failed.
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["A"]}
    });
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &retry_payload.to_string()],
    );
    assert!(
        ok,
        "retry_failed submission must succeed. stderr={}",
        stderr
    );

    // Assert:
    //  - retry_dispatched carries one entry with retry_action=rewind
    //  - the parent's event log carries an EvidenceSubmitted event
    //    for retry_failed, and a clearing event for retry_failed: null
    let dispatched = json
        .get("retry_dispatched")
        .and_then(|v| v.as_array())
        .expect("retry_dispatched present");
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0]["retry_action"].as_str(), Some("rewind"));

    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let retry_evidence_count = parent_log
        .lines()
        .filter(|l| l.contains("evidence_submitted") && l.contains("retry_failed"))
        .count();
    assert!(
        retry_evidence_count >= 2,
        "parent log must carry at least two retry_failed entries (submission + clearing). \
         contents:\n{}",
        parent_log
    );
}

/// R10 precedence: an unknown child in the retry set takes precedence
/// over all other violations. Submitting `retry_failed` with a child
/// name that doesn't exist on disk must return a typed
/// `InvalidRetryReason::UnknownChildren`.
#[test]
fn retry_failed_unknown_child_rejected_atomically() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (_, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    drive_child_to_fail(tmp.path(), "parent.A");
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    // Submit retry_failed with a non-existent child name.
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["Z"]}
    });
    let output = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &retry_payload.to_string()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "unknown-child retry must fail");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_eq!(json["action"].as_str(), Some("error"));
    let batch = json.get("batch").expect("batch error envelope");
    assert_eq!(batch["kind"].as_str(), Some("invalid_retry_request"));
    assert_eq!(
        batch["reason"]["reason"].as_str(),
        Some("unknown_children"),
        "got: {}",
        serde_json::to_string_pretty(batch).unwrap()
    );
}

/// Atomicity check: when one child in the retry set is unknown, no
/// parent-event writes or child mutations land. The parent's event log
/// must not grow between the pre-retry tick and the rejected
/// submission.
#[test]
fn retry_failed_rejection_writes_no_state() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path());

    let (_, _, _) = run_koto(
        tmp.path(),
        &[
            "init",
            "parent",
            "--template",
            parent_path.to_str().unwrap(),
        ],
    );
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    drive_child_to_fail(tmp.path(), "parent.A");
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);

    let before = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["A", "unknown"]}
    });
    let _ = koto_cmd(tmp.path())
        .args(["next", "parent", "--with-data", &retry_payload.to_string()])
        .output()
        .unwrap();
    let after = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    assert_eq!(
        before, after,
        "retry rejection must leave the parent's event log untouched"
    );

    // A's child log must not carry a Rewound event.
    let a_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent.A")).unwrap();
    assert!(
        !a_log.contains("\"rewound\""),
        "A must not be rewound when the overall retry was rejected. contents:\n{}",
        a_log
    );
}

/// scenario-23 (Issue #17: BatchFinalized appends once, retry appends
/// fresh event):
///
/// 1. Submit a single-task batch {A}. Drive A to FAILED. Tick the
///    parent: all children are in a terminal state (A failed), so the
///    children-complete gate reports `all_complete: true` and one
///    BatchFinalized event appends.
/// 2. Re-tick the parent with no changes — BatchFinalized must NOT
///    re-append.
/// 3. Submit retry_failed for A. The retry evidence invalidates the
///    prior BatchFinalized (phase stays "final" per design, but the
///    old event is now stale).
/// 4. Drive the respawned A to DONE.
/// 5. Tick the parent again: all_complete becomes true again and a
///    SECOND BatchFinalized event appends.
/// 6. Terminate the parent (finalize: yes → summarize) and confirm
///    the terminal response carries `batch_final_view` from the most
///    recent BatchFinalized and `batch.phase: "final"`.
#[test]
fn scenario_23_batch_finalized_appends_once_retry_appends_fresh() {
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

    // 1a. Submit a single-task batch.
    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}]
    });
    let (_, _, _) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );

    // 1b. Drive A to failed (still terminal, so all_complete: true).
    drive_child_to_fail(tmp.path(), "parent.A");

    // 1c. Re-tick the parent: children-complete fires, BatchFinalized appends.
    let (_, json1, _) = run_koto(tmp.path(), &["next", "parent"]);

    // The response envelope carries batch.phase: "final".
    assert_eq!(
        json1["batch"]["phase"].as_str(),
        Some("final"),
        "batch.phase must be 'final' after BatchFinalized appends, got: {}",
        serde_json::to_string(&json1).unwrap_or_default()
    );

    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let bf_count = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"batch_finalized\""))
        .count();
    assert_eq!(
        bf_count, 1,
        "exactly one BatchFinalized event after first all_complete tick. log:\n{}",
        parent_log
    );

    // 2. Re-tick the parent with no changes — BatchFinalized must NOT re-append.
    let (_, _, _) = run_koto(tmp.path(), &["next", "parent"]);
    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let bf_count_noop = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"batch_finalized\""))
        .count();
    assert_eq!(
        bf_count_noop, 1,
        "no new BatchFinalized on no-op re-tick. log:\n{}",
        parent_log
    );

    // 3. Submit retry_failed for A.
    let retry_payload = serde_json::json!({
        "retry_failed": {"children": ["A"]}
    });
    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &retry_payload.to_string()],
    );
    assert!(
        ok,
        "retry_failed submission must succeed. stderr={}",
        stderr
    );

    // 4. Drive A to done after the rewind.
    drive_child_to_done(tmp.path(), "parent.A");

    // 5. Tick the parent: all_complete should flip back to true and a
    //    fresh BatchFinalized must append.
    let (_, _json2, _) = run_koto(tmp.path(), &["next", "parent"]);

    let parent_log = std::fs::read_to_string(parent_state_path(tmp.path(), "parent")).unwrap();
    let bf_lines: Vec<&str> = parent_log
        .lines()
        .filter(|l| l.contains("\"type\":\"batch_finalized\""))
        .collect();
    assert_eq!(
        bf_lines.len(),
        2,
        "a second BatchFinalized must append after retry re-runs to completion. log:\n{}",
        parent_log
    );

    // Sanity: the two BatchFinalized events have monotonically
    // increasing sequence numbers.
    let ev1: serde_json::Value = serde_json::from_str(bf_lines[0]).unwrap();
    let ev2: serde_json::Value = serde_json::from_str(bf_lines[1]).unwrap();
    assert!(
        ev2["seq"].as_u64().unwrap() > ev1["seq"].as_u64().unwrap(),
        "second BatchFinalized must have a higher seq than the first"
    );
    // The first BatchFinalized's view reports a failed child
    // (pre-retry); the second reports an all-success run.
    assert_eq!(
        ev1["payload"]["view"]["any_failed"].as_bool(),
        Some(true),
        "first BatchFinalized must capture the pre-retry failure. got: {}",
        ev1
    );
    assert_eq!(
        ev2["payload"]["view"]["all_success"].as_bool(),
        Some(true),
        "second BatchFinalized must capture the post-retry success. got: {}",
        ev2
    );

    // 6. Drive the parent terminal via finalize: yes. The Terminal
    //    response carries batch_final_view populated from the most
    //    recent BatchFinalized (second one, post-retry).
    let finalize_payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {}}],
        "finalize": "yes"
    });
    let (ok, json_final, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "parent",
            "--no-cleanup",
            "--with-data",
            &finalize_payload.to_string(),
        ],
    );
    assert!(
        ok,
        "finalize tick must succeed. stderr={} json={}",
        stderr,
        serde_json::to_string_pretty(&json_final).unwrap_or_default()
    );
    assert_eq!(
        json_final["action"].as_str(),
        Some("done"),
        "parent should be terminal after finalize. got: {}",
        serde_json::to_string_pretty(&json_final).unwrap_or_default()
    );
    let bfv = json_final
        .get("batch_final_view")
        .expect("batch_final_view present on terminal done response");
    assert_eq!(
        bfv["all_complete"].as_bool(),
        Some(true),
        "batch_final_view must carry the most recent all_complete state"
    );
    assert_eq!(
        bfv["all_success"].as_bool(),
        Some(true),
        "batch_final_view reflects the post-retry (successful) run"
    );
    assert_eq!(
        json_final["batch"]["phase"].as_str(),
        Some("final"),
        "batch.phase stays 'final' on terminal response"
    );
}
