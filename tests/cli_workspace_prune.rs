//! Integration tests for `koto workspace prune`.
//!
//! Covers the Issue 6 acceptance criteria: terminal-state gate, symlink
//! refusal, `--force`/`--yes` interlock, `--dry-run`, descendant walk,
//! and the no-coordinator-of-record-check property.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

/// Return a `koto` command wired to read sessions from a tempdir.
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

fn session_dir(dir: &Path, id: &str) -> PathBuf {
    sessions_base(dir).join(id)
}

/// A template with one non-terminal state. Used when we want a session
/// stuck in a non-terminal state.
fn non_terminal_template() -> &'static str {
    r#"---
name: non-terminal
version: "1.0"
initial_state: working
states:
  working:
    accepts:
      done:
        type: boolean
        required: true
    transitions:
      - target: finished
        when:
          done: true
  finished:
    terminal: true
---

## working

Do work.

## finished

Done.
"#
}

/// A template whose initial state is itself terminal. Used to land a
/// session in `completed` immediately after init.
fn terminal_at_init_template() -> &'static str {
    r#"---
name: instant-done
version: "1.0"
initial_state: done
states:
  done:
    terminal: true
---

## done

Done.
"#
}

fn init_workflow(dir: &Path, name: &str, template_content: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();
    let out = koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init failed for {}: {}",
        name,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_child_workflow(dir: &Path, parent: &str, name: &str, template_content: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();
    let out = koto_cmd(dir)
        .args([
            "init",
            name,
            "--template",
            src.to_str().unwrap(),
            "--parent",
            parent,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init child failed for {}: {}",
        name,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn cancel_workflow(dir: &Path, name: &str) {
    let out = koto_cmd(dir).args(["cancel", name]).output().unwrap();
    assert!(
        out.status.success(),
        "cancel failed for {}: {}",
        name,
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// Terminal-state gate
// ---------------------------------------------------------------------------

#[test]
fn non_terminal_root_rejects_with_exit_2() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "live-wf", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "non-terminal root must exit 2");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let msg = json["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("not terminal") || msg.contains("current state"),
        "error message should name the current state, got: {}",
        msg
    );
    assert!(
        session_dir(dir.path(), "live-wf").exists(),
        "session directory must NOT be removed on rejection"
    );
}

#[test]
fn completed_workflow_proceeds_with_yes() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "done-wf", terminal_at_init_template());
    assert!(session_dir(dir.path(), "done-wf").exists());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "done-wf", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune of completed workflow should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !session_dir(dir.path(), "done-wf").exists(),
        "completed session directory must be removed"
    );
}

#[test]
fn cancelled_workflow_proceeds_with_yes() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "aban-wf", non_terminal_template());
    cancel_workflow(dir.path(), "aban-wf");

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "aban-wf", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune of abandoned workflow should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!session_dir(dir.path(), "aban-wf").exists());
}

#[test]
fn force_bypasses_terminal_gate_with_yes() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args([
            "workspace",
            "prune",
            "--root",
            "live-wf",
            "--force",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--force --yes must bypass terminal gate: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!session_dir(dir.path(), "live-wf").exists());
}

#[test]
fn force_alone_aborts_on_n_response() {
    // --force without --yes prompts interactively. Drive stdin with
    // "n\n" to assert the abort path.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "live-wf", "--force"])
        .write_stdin("n\n")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "operator-declined prune must exit 2"
    );
    assert!(
        session_dir(dir.path(), "live-wf").exists(),
        "session must remain when operator declines the prompt"
    );
}

#[test]
fn force_alone_aborts_on_eof() {
    // Closed stdin counts as negative consent.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "live-wf", "--force"])
        .write_stdin("")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(session_dir(dir.path(), "live-wf").exists());
}

#[test]
fn force_alone_proceeds_on_y_response() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "live-wf", "--force"])
        .write_stdin("y\n")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "operator-confirmed prune must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!session_dir(dir.path(), "live-wf").exists());
}

// ---------------------------------------------------------------------------
// Dry-run
// ---------------------------------------------------------------------------

#[test]
fn dry_run_on_terminal_root_prints_and_exits_zero() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "done-wf", terminal_at_init_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "done-wf", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dry-run on a terminal root must exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        session_dir(dir.path(), "done-wf").exists(),
        "dry-run must NOT remove the session directory"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("done-wf"),
        "dry-run output should mention the root id, got:\n{}",
        stdout
    );
}

#[test]
fn dry_run_still_enforces_terminal_gate() {
    // Judgment call (see commit message): dry-run respects the
    // terminal gate so an operator cannot enumerate live descendants
    // as a side-channel. Combine with --force to override.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-wf", non_terminal_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "live-wf", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "dry-run on non-terminal root must reject without --force"
    );
}

#[test]
fn dry_run_with_force_lists_descendants_without_removing() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "live-parent", non_terminal_template());
    init_child_workflow(
        dir.path(),
        "live-parent",
        "live-child",
        non_terminal_template(),
    );

    let out = koto_cmd(dir.path())
        .args([
            "workspace",
            "prune",
            "--root",
            "live-parent",
            "--dry-run",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "dry-run --force must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("live-child"),
        "dry-run output should enumerate descendants, got:\n{}",
        stdout
    );
    assert!(
        session_dir(dir.path(), "live-parent").exists(),
        "dry-run must not remove parent"
    );
    assert!(
        session_dir(dir.path(), "live-child").exists(),
        "dry-run must not remove child"
    );
}

// ---------------------------------------------------------------------------
// Symlink refusal — load-bearing security tests
// ---------------------------------------------------------------------------

#[test]
fn symlink_root_rejected() {
    // Per Implementation-altitude touch-up #2 (design line 2092): a
    // symlinked root MUST be rejected via lstat() before any
    // directory traversal.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "real-wf", terminal_at_init_template());

    // Create a symlink in the sessions dir pointing at the real session
    // dir, then attempt to prune through the symlink id.
    let real = session_dir(dir.path(), "real-wf");
    let link = sessions_base(dir.path()).join("link-wf");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(not(unix))]
    {
        eprintln!("skipping symlink test on non-unix");
        return;
    }

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "link-wf", "--yes"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "symlinked root must reject; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let msg = json["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("symlink not permitted"),
        "error must say 'symlink not permitted', got: {}",
        msg
    );
    // The real session must be untouched.
    assert!(
        real.exists(),
        "real session must be untouched after symlink rejection"
    );
    // The symlink itself remains (we refuse, we don't remove).
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "symlink must remain after rejection"
    );
}

#[test]
fn inside_tree_symlink_not_followed() {
    // Per design Security Considerations 1975-1978: a symlink INSIDE a
    // session's directory whose target lives outside the workspace
    // must NOT be followed during reclaim. fs::remove_dir_all (the
    // primitive backend.cleanup uses) does not follow symlinks; this
    // test verifies that property end-to-end.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "done-wf", terminal_at_init_template());

    // Create a file OUTSIDE the workspace and a symlink to it INSIDE
    // the session directory.
    let outside_dir = TempDir::new().unwrap();
    let outside_file = outside_dir.path().join("precious.txt");
    std::fs::write(&outside_file, "must survive").unwrap();

    let inside_session = session_dir(dir.path(), "done-wf");
    let inside_symlink = inside_session.join("escape-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_file, &inside_symlink).unwrap();
    #[cfg(not(unix))]
    {
        eprintln!("skipping inside-tree symlink test on non-unix");
        return;
    }

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "done-wf", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune must succeed even with internal symlinks: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !inside_session.exists(),
        "session directory must be removed"
    );
    // The symlink target must survive.
    assert!(
        outside_file.exists(),
        "symlink target outside the workspace must NOT be followed/removed"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        "must survive",
        "symlink target content must be unchanged"
    );
}

// ---------------------------------------------------------------------------
// Descendant walk
// ---------------------------------------------------------------------------

#[test]
fn successful_prune_removes_root_and_descendants() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "p1", terminal_at_init_template());
    init_child_workflow(dir.path(), "p1", "c1", terminal_at_init_template());
    init_child_workflow(dir.path(), "p1", "c2", terminal_at_init_template());
    init_child_workflow(dir.path(), "c1", "gc1", terminal_at_init_template());

    assert!(session_dir(dir.path(), "p1").exists());
    assert!(session_dir(dir.path(), "c1").exists());
    assert!(session_dir(dir.path(), "c2").exists());
    assert!(session_dir(dir.path(), "gc1").exists());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "p1", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune of terminal tree must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!session_dir(dir.path(), "p1").exists());
    assert!(!session_dir(dir.path(), "c1").exists());
    assert!(!session_dir(dir.path(), "c2").exists());
    assert!(!session_dir(dir.path(), "gc1").exists());
}

#[test]
fn unrelated_sessions_untouched() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "doomed", terminal_at_init_template());
    init_workflow(dir.path(), "survivor", terminal_at_init_template());

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "doomed", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!session_dir(dir.path(), "doomed").exists());
    assert!(
        session_dir(dir.path(), "survivor").exists(),
        "unrelated sessions must NOT be affected"
    );
}

// ---------------------------------------------------------------------------
// --root validation (parse-time)
// ---------------------------------------------------------------------------

#[test]
fn invalid_root_rejects_at_parse_time() {
    let dir = TempDir::new().unwrap();

    // Path-traversal attempt.
    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "../escape", "--yes"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "injection attempt must reject with exit 2; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let msg = json["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("invalid --root") || msg.contains("session ID"),
        "error must indicate invalid root: {}",
        msg
    );
}

#[test]
fn empty_root_rejects() {
    let dir = TempDir::new().unwrap();
    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

// ---------------------------------------------------------------------------
// Multi-coordinator: no coordinator_of_record check
// ---------------------------------------------------------------------------

#[test]
fn no_coordinator_of_record_check() {
    // Decision 4 line 578: prune is intentionally an operator-driven
    // action; multi-coord workspaces can prune any reachable root.
    // Build a session, prune it without setting a coordinator-of-record
    // and assert success. (The verb never reads the field; this test
    // documents that property and would fail if a future change wires
    // a check in.)
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "any-coord", terminal_at_init_template());
    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "any-coord", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune must not depend on coordinator_of_record: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
