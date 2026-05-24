//! Integration tests for KT1 Issue 13's epoch fence on child-log writes.
//!
//! Inline unit tests in `src/engine/epoch.rs` cover the pure validator's
//! happy path, stale-epoch, future-epoch, missing-flag, exit-code
//! mapping, error-message clarity, and the child-vs-top-level scope
//! rule. This file exercises the cross-cutting CLI integration ACs:
//!
//! - Happy path: matching epoch → child write succeeds, exit 0.
//! - Mismatch: stale epoch → exit 65, no partial write (stat-before /
//!   stat-after assertion).
//! - Future epoch → also rejected (strict equality discipline).
//! - Pre-persistence rejection: log file size and mtime unchanged on
//!   the rejection path.
//! - Redelegation-then-stale-write regression: synthesize the full
//!   case-3b/3c scenario (header dispatch_epoch bumped) and verify
//!   the original agent's stale-epoch abandon write is rejected while
//!   a fresh-epoch write proceeds.
//! - `--dispatch-epoch` missing on a child-log write is rejected.
//! - Parent-workflow writes are NOT under the fence (the flag is
//!   ignored / optional for parent ticks).

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use koto::engine::claim::rewrite_header_atomically;

const PARENT_TEMPLATE: &str = r#"---
name: parent-coord
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      result:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## gather

Gather evidence.

## done

Done.
"#;

const CHILD_TEMPLATE: &str = r#"---
name: child-task
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do work.

## done

Done.
"#;

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

fn write_template(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn run_koto(dir: &Path, args: &[&str]) -> (bool, i32, String, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (output.status.success(), code, stdout, stderr)
}

/// Set up a parent + child session pair in `dir`. Returns the child's
/// state-file path.
///
/// The child's header is initialized via `koto init --parent` (giving
/// it `parent_workflow = Some(...)`) and then mutated to carry
/// `needs_agent = Some(true)` + a `role` + a `coordinator_of_record`
/// so the [`crate::engine::epoch::fence_applies_to`] gate fires. A
/// regular `--parent` child without `needs_agent` is a batch-spawned
/// child and is NOT under the fence by design.
fn setup_parent_with_child(dir: &Path, parent: &str, child: &str) -> PathBuf {
    let parent_tmpl = write_template(dir, "parent.md", PARENT_TEMPLATE);
    let child_tmpl = write_template(dir, "child.md", CHILD_TEMPLATE);
    let (ok, code, stdout, stderr) = run_koto(
        dir,
        &["init", parent, "--template", parent_tmpl.to_str().unwrap()],
    );
    assert!(
        ok,
        "init parent failed code={} stdout={} stderr={}",
        code, stdout, stderr
    );
    let (ok, code, stdout, stderr) = run_koto(
        dir,
        &[
            "init",
            child,
            "--template",
            child_tmpl.to_str().unwrap(),
            "--parent",
            parent,
        ],
    );
    assert!(
        ok,
        "init child failed code={} stdout={} stderr={}",
        code, stdout, stderr
    );
    let child_path = session_state_path(dir, child);
    // Mark the child as a request-store dispatched child so the
    // fence applies. dispatch_epoch starts at 0 (the StateFileHeader
    // default); individual tests bump it via rewrite_header_atomically
    // to simulate case-3b/3c recovery.
    rewrite_header_atomically(&child_path, |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("coord-a".into());
        h.requested_by = Some(parent.to_string());
        h
    })
    .expect("mark child as request-store dispatched");
    child_path
}

/// Stat the file's (size, mtime_nanos). Used by the pre-persistence
/// rejection AC: any fence rejection must leave both unchanged.
fn stat_size_mtime(path: &Path) -> (u64, u128) {
    let meta = std::fs::metadata(path).unwrap();
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (meta.len(), mtime)
}

// ----- Happy path: matching epoch -----------------------------------------

#[test]
fn child_write_with_matching_epoch_succeeds() {
    let tmp = TempDir::new().unwrap();
    let child_path = setup_parent_with_child(tmp.path(), "parent-coord", "child-task");
    // Child's dispatch_epoch defaults to 0 via the StateFileHeader's
    // serde default. Present epoch 0 → match → write succeeds.
    let (ok, code, _stdout, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "child-task",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"status":"ok"}"#,
        ],
    );
    assert!(
        ok,
        "matching epoch must succeed; code={} stderr={}",
        code, stderr
    );
    assert_eq!(code, 0, "exit must be 0 on success");
    let _ = child_path;
}

// ----- Mismatch: stale epoch (the load-bearing redelegation case) --------

#[test]
fn child_write_with_stale_epoch_is_rejected_with_exit_65_and_no_partial_write() {
    let tmp = TempDir::new().unwrap();
    let child_path = setup_parent_with_child(tmp.path(), "parent-coord", "child-task");
    // Bump dispatch_epoch to 1 via the persistence helper Issue 11 exposes
    // (simulates case-3b/3c recovery walk's epoch bump).
    rewrite_header_atomically(&child_path, |mut h| {
        h.dispatch_epoch = 1;
        h
    })
    .expect("epoch bump");

    let (size_before, mtime_before) = stat_size_mtime(&child_path);

    // Original agent presents epoch 0 — stale, must reject.
    let (ok, code, stdout, _stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "child-task",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"status":"abandoned"}"#,
        ],
    );
    assert!(!ok, "stale epoch must reject");
    assert_eq!(code, 65, "stale epoch must exit 65 (EX_DATAERR)");
    assert!(
        stdout.contains("epoch_fence_violation"),
        "envelope must name the variant: {}",
        stdout
    );
    assert!(
        stdout.contains("\"expected_dispatch_epoch\":1"),
        "envelope must name expected epoch: {}",
        stdout
    );
    assert!(
        stdout.contains("\"presented_dispatch_epoch\":0"),
        "envelope must name presented epoch: {}",
        stdout
    );

    // Pre-persistence rejection AC: size + mtime unchanged.
    let (size_after, mtime_after) = stat_size_mtime(&child_path);
    assert_eq!(
        size_before, size_after,
        "rejection must not append to the log"
    );
    assert_eq!(
        mtime_before, mtime_after,
        "rejection must not touch the log's mtime"
    );
}

// ----- Mismatch: future epoch (strict equality, not lower-bound) ---------

#[test]
fn child_write_with_future_epoch_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let _ = setup_parent_with_child(tmp.path(), "parent-coord", "child-task");
    // Header has dispatch_epoch = 0; present epoch 5 → strict-equality
    // discipline rejects.
    let (ok, code, stdout, _stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "child-task",
            "--dispatch-epoch",
            "5",
            "--with-data",
            r#"{"status":"ok"}"#,
        ],
    );
    assert!(!ok, "future epoch must reject");
    assert_eq!(code, 65);
    assert!(
        stdout.contains("\"expected_dispatch_epoch\":0")
            && stdout.contains("\"presented_dispatch_epoch\":5"),
        "envelope must name both epochs: {}",
        stdout
    );
}

// ----- --dispatch-epoch missing on a child-log write is rejected ---------

#[test]
fn child_write_missing_dispatch_epoch_flag_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let _ = setup_parent_with_child(tmp.path(), "parent-coord", "child-task");
    // No --dispatch-epoch flag; the fence treats this as implicit mismatch.
    let (ok, code, stdout, _stderr) = run_koto(
        tmp.path(),
        &["next", "child-task", "--with-data", r#"{"status":"ok"}"#],
    );
    assert!(!ok, "missing --dispatch-epoch must reject on a child");
    assert_eq!(code, 65);
    assert!(
        stdout.contains("epoch_fence_violation"),
        "envelope must name the variant: {}",
        stdout
    );
    // The u32::MAX sentinel is what validate_epoch reports for the
    // missing-flag branch; the envelope mirrors that.
    assert!(
        stdout.contains("\"presented_dispatch_epoch\":4294967295"),
        "missing-flag must produce u32::MAX sentinel in envelope: {}",
        stdout
    );
}

// ----- Redelegation-then-stale-write regression (the headline AC) --------

#[test]
fn redelegation_bump_rejects_stale_writer_and_accepts_fresh_writer() {
    let tmp = TempDir::new().unwrap();
    let child_path = setup_parent_with_child(tmp.path(), "parent-coord", "child-task");
    // Step 1: child claimed by coord A, dispatch_epoch = 0, agent A1
    // baked with epoch 0. (Implicit: header default is 0.)
    //
    // Step 2: coord A crashes; case-3b/3c recovery fires and bumps
    // dispatch_epoch to 1.
    rewrite_header_atomically(&child_path, |mut h| {
        h.dispatch_epoch = 1;
        h
    })
    .expect("epoch bump");

    // Step 3: agent A1's SubagentStop hook fires with stale epoch 0.
    // Must reject without corrupting state.
    let (size_before, mtime_before) = stat_size_mtime(&child_path);
    let (ok, code, stdout, _stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "child-task",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"status":"abandoned"}"#,
        ],
    );
    assert!(!ok, "stale-writer abandon must reject");
    assert_eq!(code, 65);
    assert!(stdout.contains("epoch_fence_violation"));
    let (size_after_reject, mtime_after_reject) = stat_size_mtime(&child_path);
    assert_eq!(size_before, size_after_reject);
    assert_eq!(mtime_before, mtime_after_reject);

    // Step 4: agent A2 presents fresh epoch 1; must succeed.
    // Use --no-cleanup so the terminal-state transition does not
    // auto-clean the child session; we need to inspect the log
    // afterward to verify the stale-writer's abandon evidence is
    // absent and the fresh-writer's ok evidence is present.
    let (ok, code, _stdout, stderr) = run_koto(
        tmp.path(),
        &[
            "next",
            "child-task",
            "--no-cleanup",
            "--dispatch-epoch",
            "1",
            "--with-data",
            r#"{"status":"ok"}"#,
        ],
    );
    assert!(
        ok,
        "fresh-epoch write must succeed after redelegation; code={} stderr={}",
        code, stderr
    );
    assert_eq!(code, 0, "fresh-epoch exit 0");
    // The original abandon did NOT corrupt state: the next read
    // observes a single fresh-epoch evidence event (plus the
    // workflow-init event), not a leading abandon event.
    let log = std::fs::read_to_string(&child_path).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    // Header + WorkflowInitialized + 1+ events for the fresh write.
    assert!(
        !log.contains("\"status\":\"abandoned\""),
        "stale-writer's abandon must not be in the log: {:?}",
        lines
    );
    assert!(
        log.contains("\"status\":\"ok\""),
        "fresh-writer's ok evidence must be in the log: {:?}",
        lines
    );
}

// ----- Parent-workflow writes are NOT under the fence --------------------

#[test]
fn parent_workflow_write_is_not_under_the_fence() {
    let tmp = TempDir::new().unwrap();
    let parent_tmpl = write_template(tmp.path(), "parent.md", PARENT_TEMPLATE);
    // Top-level workflow (no --parent): header.parent_workflow == None.
    let (ok, code, stdout, stderr) = run_koto(
        tmp.path(),
        &[
            "init",
            "lone-coord",
            "--template",
            parent_tmpl.to_str().unwrap(),
        ],
    );
    assert!(
        ok,
        "init failed: code={} stdout={} stderr={}",
        code, stdout, stderr
    );

    // A top-level workflow's --with-data write does NOT require
    // --dispatch-epoch (the fence does not apply).
    let (ok, code, _stdout, stderr) = run_koto(
        tmp.path(),
        &["next", "lone-coord", "--with-data", r#"{"result":"value"}"#],
    );
    assert!(
        ok,
        "parent-workflow write without --dispatch-epoch must succeed; code={} stderr={}",
        code, stderr
    );
    assert_eq!(code, 0);
}
