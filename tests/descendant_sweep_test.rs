//! Integration tests for removing a parent's kept descendants with it
//! (koto issue 240).
//!
//! A child kept at a failure terminal, or with `--no-cleanup`, must not
//! outlive a parent that koto removes or replaces. The sweep that does this
//! runs without anyone naming the sessions it removes, so it never touches
//! live work, a session it can't read, or anything under a live session.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;

// ===== Harness =====

/// A parent that waits for `go` and then finishes.
const PARENT: &str = r#"---
name: sweep-parent
version: "1.0"
initial_state: wait
states:
  wait:
    accepts:
      go:
        type: enum
        required: true
        values: [done, fail]
    transitions:
      - target: finished
        when:
          go: done
      - target: failed
        when:
          go: fail
  finished:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## wait

Wait.

## finished

Done.

## failed

Failed.
"#;

/// A child with a success and a failure terminal.
const CHILD: &str = r#"---
name: sweep-child
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
      - target: done_blocked
        when:
          marker: fail
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
---

## work

Work.

## done

Done.

## done_blocked

Blocked.
"#;

/// A batch parent that materializes children and finishes on `finalize`.
const BATCH_PARENT: &str = r#"---
name: sweep-batch
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
      batch_done:
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

Plan.

## summarize

Summarize.
"#;

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn run_ok(dir: &Path, args: &[&str]) -> Value {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(
        code, 0,
        "expected success from {args:?}\n{stdout}\n{stderr}"
    );
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    serde_json::from_str(last).unwrap_or(Value::Null)
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions").join(name)
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{name}.state.jsonl"))
}

fn exists(dir: &Path, name: &str) -> bool {
    session_dir(dir, name).exists()
}

fn write(dir: &Path, file: &str, body: &str) -> String {
    let p = dir.join(file);
    std::fs::write(&p, body).unwrap();
    p.to_str().unwrap().to_string()
}

fn init_root(dir: &Path, name: &str, template: &str) {
    let t = write(dir, &format!("{name}.md"), template);
    run_ok(dir, &["init", name, "--template", &t]);
}

fn init_child(dir: &Path, name: &str, parent: &str) {
    let t = write(dir, "child.md", CHILD);
    run_ok(dir, &["init", name, "--template", &t, "--parent", parent]);
}

fn fail(dir: &Path, name: &str) {
    run_ok(dir, &["next", name, "--with-data", r#"{"marker":"fail"}"#]);
}

fn done_kept(dir: &Path, name: &str) {
    run_ok(
        dir,
        &[
            "next",
            name,
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
}

fn finish_parent(dir: &Path, name: &str) {
    run_ok(dir, &["next", name, "--with-data", r#"{"go":"done"}"#]);
}

fn set_writable(path: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if writable { 0o644 } else { 0o444 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// ===== Reach =====

#[test]
fn removes_terminal_descendants_with_the_parent() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.failed", "p");
    init_child(dir, "p.kept", "p");
    init_child(dir, "p.kept.gc", "p.kept");
    fail(dir, "p.failed");
    fail(dir, "p.kept.gc");
    done_kept(dir, "p.kept");
    assert!(exists(dir, "p.failed") && exists(dir, "p.kept") && exists(dir, "p.kept.gc"));

    finish_parent(dir, "p");
    assert!(!exists(dir, "p"), "the parent is removed");
    assert!(
        !exists(dir, "p.failed"),
        "its kept failed child goes with it"
    );
    assert!(
        !exists(dir, "p.kept"),
        "a child kept with --no-cleanup goes too"
    );
    assert!(
        !exists(dir, "p.kept.gc"),
        "and a terminal grandchild under it"
    );
}

#[test]
fn leaves_live_children_and_their_subtrees() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.failed", "p");
    init_child(dir, "p.live", "p");
    init_child(dir, "p.live.gc", "p.live");
    fail(dir, "p.failed");
    fail(dir, "p.live.gc");

    finish_parent(dir, "p");
    assert!(!exists(dir, "p.failed"));
    assert!(exists(dir, "p.live"), "a live child is left alone");
    assert!(exists(dir, "p.live.gc"), "and so is everything under it");
}

#[test]
fn keeps_a_terminal_child_with_a_live_grandchild() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.t", "p");
    init_child(dir, "p.t.live", "p.t");
    fail(dir, "p.t");

    finish_parent(dir, "p");
    assert!(
        exists(dir, "p.t"),
        "not removed while something under it is live"
    );
    assert!(exists(dir, "p.t.live"));
}

#[test]
fn a_kept_parent_keeps_its_children() {
    // A parent at its failure terminal.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.failed", "p");
    fail(dir, "p.failed");
    run_ok(dir, &["next", "p", "--with-data", r#"{"go":"fail"}"#]);
    assert!(exists(dir, "p") && exists(dir, "p.failed"));

    // A parent at a success terminal kept with --no-cleanup.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.failed", "p");
    fail(dir, "p.failed");
    run_ok(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"go":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert!(exists(dir, "p") && exists(dir, "p.failed"));
}

#[test]
fn skips_an_unclassifiable_descendant() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.bad", "p");
    init_child(dir, "p.failed", "p");
    fail(dir, "p.bad");
    fail(dir, "p.failed");
    // Corrupt p.bad's log in the middle, so it can no longer be read.
    let path = state_path(dir, "p.bad");
    let body = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = body.lines().collect();
    lines.insert(1, "not json");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    finish_parent(dir, "p");
    assert!(!exists(dir, "p"), "the parent is still removed");
    assert!(
        exists(dir, "p.bad"),
        "an unreadable descendant is left alone"
    );
    assert!(!exists(dir, "p.failed"));
}

/// A descendant whose compiled template file is gone can't be classified,
/// so it is left alone too.
#[test]
fn skips_a_descendant_whose_template_is_missing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    // Its own template, so deleting its compiled copy touches nothing else.
    let t = write(
        dir,
        "orphan-child.md",
        &CHILD.replace("sweep-child", "orphan-child"),
    );
    run_ok(
        dir,
        &["init", "p.orphan", "--template", &t, "--parent", "p"],
    );
    init_child(dir, "p.failed", "p");
    fail(dir, "p.orphan");
    fail(dir, "p.failed");
    let init = std::fs::read_to_string(state_path(dir, "p.orphan"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e["type"] == "workflow_initialized")
        .unwrap();
    let compiled = init["payload"]["template_path"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::remove_file(&compiled).unwrap();

    finish_parent(dir, "p");
    assert!(!exists(dir, "p"), "the parent is still removed");
    assert!(
        exists(dir, "p.orphan"),
        "an unclassifiable descendant is left alone"
    );
    assert!(!exists(dir, "p.failed"));
}

#[test]
fn stops_on_a_parent_cycle() {
    // a -> b -> a: remove `a`, then re-create it under its former child.
    // The sweep is driven through the replace path: a terminal tick would not
    // sweep here (`a` has no batch hook and no reported child), and the walk
    // is the same function either way.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "a", CHILD);
    init_child(dir, "b", "a");
    fail(dir, "b");
    run_ok(dir, &["session", "cleanup", "a"]);
    let t = write(dir, "child.md", CHILD);
    run_ok(dir, &["init", "a", "--template", &t, "--parent", "b"]);
    done_kept(dir, "a");

    // Replacing the finished `a` sweeps its descendants and must terminate.
    run_ok(
        dir,
        &[
            "init",
            "a",
            "--template",
            &t,
            "--attach-live",
            "--replace-terminal",
        ],
    );
    assert!(exists(dir, "a"));
    assert!(exists(dir, "b"), "a session in a cycle is not removed");
}

#[test]
fn prune_terminates_on_a_cycle() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "a", CHILD);
    init_child(dir, "b", "a");
    fail(dir, "b");
    run_ok(dir, &["session", "cleanup", "a"]);
    let t = write(dir, "child.md", CHILD);
    run_ok(dir, &["init", "a", "--template", &t, "--parent", "b"]);
    done_kept(dir, "a");

    let (code, out, err) = run(dir, &["workspace", "prune", "--root", "a", "--dry-run"]);
    assert_eq!(code, 0, "{out} {err}");
}

#[test]
fn replace_terminal_removes_retained_children() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "r", CHILD);
    init_child(dir, "r.c", "r");
    fail(dir, "r.c");
    done_kept(dir, "r");

    let t = write(dir, "r.md", CHILD);
    run_ok(
        dir,
        &[
            "init",
            "r",
            "--template",
            &t,
            "--attach-live",
            "--replace-terminal",
        ],
    );
    assert!(!exists(dir, "r.c"), "the old run's kept child is removed");
    let status = run_ok(dir, &["status", "r"]);
    assert_eq!(status["current_state"], "work", "r is a fresh session");
}

#[test]
fn prune_and_session_cleanup_reclaim_retained_sessions() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    // Two retained roots, each with a retained failed child.
    for root in ["r1", "r2"] {
        init_root(dir, root, CHILD);
        init_child(dir, &format!("{root}.a"), root);
        init_child(dir, &format!("{root}.b"), root);
        fail(dir, &format!("{root}.a"));
        fail(dir, &format!("{root}.b"));
        fail(dir, root);
    }

    run_ok(dir, &["workspace", "prune", "--root", "r1", "--yes"]);
    assert!(!exists(dir, "r1") && !exists(dir, "r1.a") && !exists(dir, "r1.b"));
    assert!(exists(dir, "r2") && exists(dir, "r2.a") && exists(dir, "r2.b"));

    run_ok(dir, &["session", "cleanup", "r2.a"]);
    assert!(!exists(dir, "r2.a"));
    assert!(exists(dir, "r2") && exists(dir, "r2.b"));
}

#[test]
fn a_retained_child_with_a_lost_notice_is_still_swept() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write(dir, "child.md", CHILD);
    init_root(dir, "p", BATCH_PARENT);
    run_ok(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"tasks":[{"name":"leaf","waits_on":[]}]}"#,
        ],
    );
    // The child fails while the parent's log is unwritable, so its notice
    // is lost; it is kept anyway, because a failure terminal always is.
    let parent_log = state_path(dir, "p");
    set_writable(&parent_log, false);
    run(
        dir,
        &["next", "p.leaf", "--with-data", r#"{"marker":"fail"}"#],
    );
    set_writable(&parent_log, true);
    let log = std::fs::read_to_string(&parent_log).unwrap();
    assert!(!log.contains("child_completed"), "the notice was lost");
    assert!(exists(dir, "p.leaf"));

    run_ok(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"tasks":[{"name":"leaf","waits_on":[]}],"finalize":"yes"}"#,
        ],
    );
    assert!(!exists(dir, "p"));
    assert!(!exists(dir, "p.leaf"), "swept via the parent's batch hook");
}

/// A descendant bound to a request leg that is still open (its promotion
/// failed and awaits a retry) is left alone, so the leg can still resolve.
#[test]
fn keeps_a_descendant_whose_leg_is_still_open() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_root(dir, "p", PARENT);
    init_child(dir, "p.bound", "p");
    init_child(dir, "p.failed", "p");
    koto::engine::claim::rewrite_header_atomically(&state_path(dir, "p.bound"), |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("p".into());
        h
    })
    .unwrap();
    let legs = r#"{"legs":[{"name":"reviewer-a","role":"security","template":"review","inputs":{"pr":1}}],"inputs":{"pr":1}}"#;
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            legs,
            "--requested-by",
            "p",
            "--coordinator-of-record",
            "p",
        ],
    );
    let id = envelope["request_id"].as_str().unwrap().to_string();
    run_ok(
        dir,
        &["request", "bind", &id, "reviewer-a", "--child", "p.bound"],
    );

    // The promotion fails because the request store can't be written.
    let request_dir = dir.join(".koto").join("requests").join(&id);
    set_dir_writable(&request_dir, false);
    run(
        dir,
        &[
            "next",
            "p.bound",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"fail"}"#,
        ],
    );
    set_dir_writable(&request_dir, true);
    let leg = run_ok(dir, &["request", "get", &id])["legs"]["reviewer-a"].clone();
    assert_eq!(
        leg["disposition"], "open",
        "the promotion did not land: {leg}"
    );
    fail(dir, "p.failed");

    finish_parent(dir, "p");
    assert!(!exists(dir, "p.failed"));
    assert!(
        exists(dir, "p.bound"),
        "a child whose leg is still open is kept"
    );
}

fn set_dir_writable(path: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if writable { 0o755 } else { 0o555 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    for entry in std::fs::read_dir(path).unwrap().flatten() {
        let file_mode = if writable { 0o644 } else { 0o444 };
        let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(file_mode));
    }
}
