//! Integration tests for `koto workspace prune`.
//!
//! Covers the Issue 6 acceptance criteria: terminal-state gate, symlink
//! refusal, `--force`/`--yes` interlock, `--dry-run`, descendant walk,
//! and the no-coordinator-of-record-check property.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

/// Return a `koto` command wired to read sessions from a tempdir.
/// Every command runs with a one-second retention, so a finished tree is
/// released once [`settle`] has waited it out.
fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    for var in [
        "CLAUDE_CODE_SESSION_ID",
        "KOTO_ANCHOR",
        "KOTO_DISPATCHER_ANCHOR",
        "KOTO_SESSION_BACKEND",
        "KOTO_WORKFLOWS_DIR",
    ] {
        cmd.env_remove(var);
    }
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd.env("KOTO_RETENTION_KEEP", "1s");
    cmd.env("KOTO_RETENTION_IDLE_PERIOD", "1h");
    cmd.env("KOTO_RETENTION_STALE_AFTER", "2h");
    cmd
}

/// Wait out the one-second retention.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1300));
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
    settle();

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
    settle();

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
fn force_passes_the_terminal_gate_but_not_the_retention_rule() {
    // `--yes --force` triggers the polish-pass second confirmation
    // prompt that demands the literal `force-prune` string. With the
    // phrase entered the gate is passed, and the rule still keeps the
    // live root: `--force` never overrides it.
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
        .write_stdin("force-prune\n")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--force --yes (with force-prune phrase) must pass the terminal gate: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(session_dir(dir.path(), "live-wf").exists());
    let v = last_json(&out.stdout);
    assert_eq!(v["pruned"], false);
    assert_eq!(v["kept"][0]["name"], "live-wf");
    assert!(
        v["kept"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("alive in its hierarchy: live-wf"),
        "{v}"
    );
}

#[test]
fn force_yes_aborts_when_phrase_not_entered() {
    // The polish-pass second confirmation: --yes --force without the
    // literal "force-prune" phrase aborts at exit code 2 and leaves
    // the session intact.
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
        .write_stdin("yes\n") // Wrong phrase.
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "--yes --force without 'force-prune' phrase must abort with exit 2"
    );
    assert!(
        session_dir(dir.path(), "live-wf").exists(),
        "session must remain when force-prune phrase is not entered"
    );
}

#[test]
fn force_yes_aborts_on_empty_stdin() {
    // EOF on stdin counts as negative consent to the second
    // confirmation, same as the first confirmation.
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
        .write_stdin("")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(session_dir(dir.path(), "live-wf").exists());
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
fn force_alone_proceeds_on_y_response_and_the_rule_still_keeps_a_live_root() {
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
    assert!(session_dir(dir.path(), "live-wf").exists());
    assert_eq!(last_json(&out.stdout)["pruned"], false);
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
    settle();

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
    settle();

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
    settle();

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

/// The decider ledger lives next to the session tree, not in it, and prune
/// only removes session directories: its bytes survive a prune untouched.
#[test]
fn prune_leaves_the_decider_ledger_unchanged() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "p1", terminal_at_init_template());
    init_child_workflow(dir.path(), "p1", "c1", terminal_at_init_template());

    let koto_root = dir.path().join(".koto");
    std::fs::create_dir_all(&koto_root).unwrap();
    let ledger = koto_root.join("_decider_ledger.jsonl");
    let planted = concat!(
        r#"{"kind":"consulted","v":1,"at":"2026-01-01T00:00:00.000Z","session":"c1","session_id":"u1","state":"review","visit_seq":2,"provider":"jev","model":"m","outcome":"not_applied","latency_ms":5,"directive_bytes":10,"endpoint_origin":"default","fields":{}}"#,
        "\n",
        r#"{"kind":"answered","v":1,"at":"2026-01-01T00:00:01.000Z","session":"c1","session_id":"u1","state":"review","visit_seq":2,"values":{"verdict":"exit"}}"#,
        "\n",
    );
    std::fs::write(&ledger, planted).unwrap();
    settle();

    let out = koto_cmd(dir.path())
        .args(["workspace", "prune", "--root", "p1", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!session_dir(dir.path(), "p1").exists());
    assert!(!session_dir(dir.path(), "c1").exists());
    assert_eq!(std::fs::read(&ledger).unwrap(), planted.as_bytes());
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
    // a check in.) The retention rule decides what goes, not the
    // coordinator.
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

// ---------------------------------------------------------------------------
// The retention rule
// ---------------------------------------------------------------------------

/// The JSON object the verb prints last, after its preview text (and on
/// the same line as an answered prompt, which prints no newline).
fn last_json(stdout: &[u8]) -> serde_json::Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains("{\""))
        .unwrap_or_else(|| panic!("no JSON in:\n{text}"));
    serde_json::from_str(&line[line.find("{\"").unwrap()..]).unwrap()
}

/// `koto init` with extra arguments and environment.
fn init_with(dir: &Path, name: &str, template_content: &str, extra: &[&str], env: &[(&str, &str)]) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();
    let mut cmd = koto_cmd(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut args = vec!["init", name, "--template", src.to_str().unwrap()];
    args.extend_from_slice(extra);
    let out = cmd.args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "init failed for {}: {}",
        name,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// SHA-256 of every file in each named session's directory.
fn snapshot(dir: &Path, names: &[&str]) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    use sha2::{Digest, Sha256};
    let mut out = std::collections::BTreeMap::new();
    for name in names {
        let mut stack = vec![session_dir(dir, name)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file() {
                    out.insert(
                        p.clone(),
                        Sha256::digest(std::fs::read(&p).unwrap()).to_vec(),
                    );
                }
            }
        }
    }
    out
}

fn prune_yes(dir: &Path, root: &str, env: &[(&str, &str)]) -> (serde_json::Value, String) {
    let mut cmd = koto_cmd(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .args(["workspace", "prune", "--root", root, "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "prune failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        last_json(&out.stdout),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

fn kept_reasons(v: &serde_json::Value) -> std::collections::BTreeMap<String, String> {
    v["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["name"].as_str().unwrap().to_string(),
                k["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_subtree_member_with_a_live_dispatcher_stays_and_a_released_one_goes() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    // x is the root, finished. Under it: c1, dispatched by the live
    // session disp, with gc1 under it; and c2, released.
    init_workflow(d, "x", terminal_at_init_template());
    init_workflow(d, "disp", non_terminal_template());
    init_with(
        d,
        "c1",
        terminal_at_init_template(),
        &["--parent", "x", "--dispatched-by", "disp"],
        &[],
    );
    init_child_workflow(d, "c1", "gc1", terminal_at_init_template());
    init_child_workflow(d, "x", "c2", terminal_at_init_template());
    settle();

    let kept = ["x", "c1", "gc1", "disp"];
    let before = snapshot(d, &kept);
    let (v, text) = prune_yes(d, "x", &[]);
    assert_eq!(v["removed"], serde_json::json!(["c2"]), "{v:#}");
    assert_eq!(v["pruned"], false);
    assert_eq!(v["descendants_removed"], 1);
    let reasons = kept_reasons(&v);
    assert_eq!(
        reasons.keys().cloned().collect::<Vec<_>>(),
        vec!["c1", "gc1", "x"]
    );
    for (name, reason) in &reasons {
        assert!(reason.contains("disp (koto_session)"), "{name}: {reason}");
    }
    // Each kept entry carries the hierarchy JSON its judgment printed.
    assert_eq!(v["kept"][0]["hierarchy"]["kept"], true);
    assert!(text.contains("keep (3):"), "{text}");
    assert!(!session_dir(d, "c2").exists());
    for name in kept {
        assert!(session_dir(d, name).exists(), "{name} was removed");
    }
    // No kept child is left naming a removed parent, and nothing kept was
    // written to.
    assert_eq!(snapshot(d, &kept), before);

    // With the dispatcher gone, the rest goes after the retention:
    // deepest first, the root last.
    let out = koto_cmd(d)
        .args(["session", "cleanup", "disp"])
        .output()
        .unwrap();
    assert!(out.status.success());
    settle();
    let (v, _) = prune_yes(d, "x", &[]);
    assert_eq!(v["removed"], serde_json::json!(["gc1", "c1", "x"]), "{v:#}");
    assert_eq!(v["pruned"], true);
    assert_eq!(v["descendants_removed"], 2);
}

#[test]
fn a_finished_tree_inside_its_retention_is_kept_and_listed() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    init_workflow(d, "x", terminal_at_init_template());
    init_child_workflow(d, "x", "c", terminal_at_init_template());
    let before = snapshot(d, &["x", "c"]);
    let (v, _) = prune_yes(d, "x", &[("KOTO_RETENTION_KEEP", "1h")]);
    assert_eq!(v["removed"], serde_json::json!([]));
    let reasons = kept_reasons(&v);
    for name in ["x", "c"] {
        assert!(
            reasons[name].contains("the retention passes at"),
            "{name}: {}",
            reasons[name]
        );
    }
    assert_eq!(snapshot(d, &["x", "c"]), before);
}

#[test]
fn a_stale_anchor_holding_the_root_is_listed_with_its_close_command() {
    let stale = [
        ("KOTO_RETENTION_KEEP", "1s"),
        ("KOTO_RETENTION_STALE_AFTER", "2s"),
    ];
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let out = koto_cmd(d)
        .args([
            "session",
            "anchor",
            "open",
            "--harness-session",
            "S1",
            "--source",
            "startup",
            "--observed-key",
            "K1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    init_with(
        d,
        "x",
        terminal_at_init_template(),
        &[],
        &[("KOTO_ANCHOR", "anchor-S1")],
    );
    init_child_workflow(d, "x", "c", terminal_at_init_template());
    std::thread::sleep(std::time::Duration::from_millis(2300));

    let command = "koto session anchor close --key='K1' --harness-session='S1'";
    let before = snapshot(d, &["x", "c", "anchor-S1"]);
    let (v, text) = prune_yes(d, "x", &stale);
    // c is judged with the root's lineage to it dropped, so it goes; the
    // root, which the anchor holds, stays.
    assert_eq!(v["removed"], serde_json::json!(["c"]), "{v:#}");
    assert_eq!(v["stale_anchors"][0]["name"], "anchor-S1");
    assert_eq!(v["stale_anchors"][0]["close_command"], command);
    assert!(text.contains(command), "{text}");
    assert!(kept_reasons(&v)["x"].contains("anchor-S1 (anchor, observed, stale)"));
    assert!(!session_dir(d, "anchor-S1-closed").exists());
    let after = snapshot(d, &["x", "anchor-S1"]);
    for (path, hash) in &after {
        assert_eq!(before.get(path), Some(hash), "{} changed", path.display());
    }

    // Running the printed command, then a prune after the retention,
    // removes the root.
    let out = koto_cmd(d)
        .args([
            "session",
            "anchor",
            "close",
            "--key=K1",
            "--harness-session=S1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    settle();
    let (v, _) = prune_yes(d, "x", &stale);
    assert_eq!(v["removed"], serde_json::json!(["x"]), "{v:#}");
}

#[test]
fn a_malformed_retention_setting_refuses_naming_it() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "x", terminal_at_init_template());
    settle();
    let out = koto_cmd(dir.path())
        .env("KOTO_RETENTION_KEEP", "7x")
        .args(["workspace", "prune", "--root", "x", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["error"].as_str().unwrap().contains("retention.keep"),
        "{v}"
    );
    assert!(session_dir(dir.path(), "x").exists());
}
