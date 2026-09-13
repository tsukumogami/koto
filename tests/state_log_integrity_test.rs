//! Regression tests for koto#236 and koto#200: a context write to a session
//! that has no usable state log must be refused, and must leave nothing
//! behind.
//!
//! The bug: `handle_add` (src/cli/context.rs) called `ContextStore::add`,
//! which `create_dir_all`s the session's ctx directory, then appended through
//! `persistence::append_event`, which opened the state file with
//! `create(true)` and never checked for a header. So the first line written
//! was a `context_added` event, and every later read failed with `failed to
//! parse header: missing field workflow`. It happens wherever the log is
//! absent: a name that was never initialized, a batch child that is still
//! blocked, a session removed by terminal cleanup, and a child a parent
//! rewind moved to a new name.
//!
//! The contract these tests pin: `context add` and `context remove` on a
//! session with no log exit 2 with `workflow '<name>' not found` and create
//! neither a log nor a session directory. A log that exists without a header
//! is refused too, and left exactly as it was.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    // Keep the user's ~/.koto/config.toml (which may select the cloud
    // backend) out of the test.
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir).join(name)
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{}.state.jsonl", name))
}

fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

/// Run `koto context add <session> <key> --from-file <file>`.
fn context_add(dir: &Path, session: &str, key: &str) -> std::process::Output {
    let src = dir.join("context-input.md");
    std::fs::write(&src, "some context\n").unwrap();
    koto_cmd(dir)
        .args([
            "context",
            "add",
            session,
            key,
            "--from-file",
            src.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn context_remove(dir: &Path, session: &str, key: &str) -> std::process::Output {
    koto_cmd(dir)
        .args(["context", "remove", session, key])
        .output()
        .unwrap()
}

/// Whether `koto context exists` reports the key present.
fn context_key_present(dir: &Path, session: &str, key: &str) -> bool {
    koto_cmd(dir)
        .args(["context", "exists", session, key])
        .output()
        .unwrap()
        .status
        .success()
}

/// Describe what is on disk at `path`, for assertion messages.
fn describe_log(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Err(_) => "<no state log>".to_string(),
        Ok(content) => format!("state log at {}:\n{}", path.display(), content),
    }
}

/// Assert the not-found refusal: exit 2, the flat error naming the session,
/// and no state log afterwards.
fn assert_not_found(out: &std::process::Output, dir: &Path, session: &str, command: &str) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{command} on {session}: expected exit 2; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("{command}: expected a JSON error, got {stdout}: {e}"));
    assert_eq!(
        body["error"],
        format!("workflow '{session}' not found"),
        "{command}: wrong error body {body}"
    );
    assert_eq!(body["command"], command);
    let log = state_path(dir, session);
    assert!(
        !log.exists(),
        "{command} must not create a state log, but {}",
        describe_log(&log)
    );
}

/// The refusal comes before the store is touched, so a session that had no
/// directory still has none: no ctx/ content is kept for a session that
/// isn't there.
fn assert_no_session_dir(dir: &Path, session: &str, what: &str) {
    let path = session_dir(dir, session);
    assert!(
        !path.exists(),
        "{what}: the refusal must not create {}",
        path.display()
    );
}

// ---------------------------------------------------------------------------
// A session name that was never initialized
// ---------------------------------------------------------------------------

#[test]
fn context_add_refuses_a_never_initialized_session() {
    let tmp = TempDir::new().unwrap();

    let out = context_add(tmp.path(), "never-started", "context.md");

    assert_not_found(&out, tmp.path(), "never-started", "context add");
    assert_no_session_dir(tmp.path(), "never-started", "context add");
}

/// Two adds to a missing log, the way #236's reporter hit it. Before the fix
/// both succeeded and both events carried `"seq":1`.
#[test]
fn repeated_context_add_to_a_missing_log_is_refused_each_time() {
    let tmp = TempDir::new().unwrap();

    let first = context_add(tmp.path(), "never-started", "context.md");
    let second = context_add(tmp.path(), "never-started", "notes.md");

    assert_not_found(&first, tmp.path(), "never-started", "context add");
    assert_not_found(&second, tmp.path(), "never-started", "context add");
    assert_no_session_dir(tmp.path(), "never-started", "context add");
}

#[test]
fn context_remove_refuses_a_never_initialized_session() {
    let tmp = TempDir::new().unwrap();

    let out = context_remove(tmp.path(), "never-started", "context.md");

    assert_not_found(&out, tmp.path(), "never-started", "context remove");
    assert_no_session_dir(tmp.path(), "never-started", "context remove");
}

/// A session directory with no log in it (a ctx store left behind, or a
/// crash before init's rename). Before the fix `context remove` appended a
/// headerless `context_removed` line here.
#[test]
fn context_remove_refuses_a_session_dir_without_a_log() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(session_dir(tmp.path(), "dir-only").join("ctx")).unwrap();

    let out = context_remove(tmp.path(), "dir-only", "context.md");

    assert_not_found(&out, tmp.path(), "dir-only", "context remove");
}

// ---------------------------------------------------------------------------
// A log that exists but has no header (what older versions left on disk)
// ---------------------------------------------------------------------------

/// The on-disk shape #236 reports: two `context_added` lines, both seq 1, no
/// header. `backend.exists` is true for this file, so the refusal has to come
/// from the header check, and it must still store nothing and append nothing.
#[test]
fn context_add_refuses_a_headerless_log_and_leaves_it_untouched() {
    let tmp = TempDir::new().unwrap();
    let name = "headerless";
    std::fs::create_dir_all(session_dir(tmp.path(), name)).unwrap();
    let log = state_path(tmp.path(), name);
    let content = concat!(
        r#"{"seq":1,"timestamp":"2026-09-07T03:13:54.846Z","type":"context_added","payload":{"key":"context.md","hash":"a84c","size":167}}"#,
        "\n",
        r#"{"seq":1,"timestamp":"2026-09-07T03:13:54.906Z","type":"context_added","payload":{"key":"notes.md","hash":"b95d","size":12}}"#,
        "\n",
    );
    std::fs::write(&log, content).unwrap();

    let out = context_add(tmp.path(), name, "late.md");

    assert_eq!(
        out.status.code(),
        Some(3),
        "a corrupt log is an infrastructure error; stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("state log has no header"),
        "the error must say the log has no header; stdout={stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        content,
        "the refused add must leave the log byte-for-byte unchanged"
    );
    assert!(
        !context_key_present(tmp.path(), name, "late.md"),
        "the refused add must not store the content"
    );

    // Every reader now names the condition instead of serde's
    // "missing field `workflow`".
    let (ok, json, _) = run_koto(tmp.path(), &["status", name]);
    assert!(!ok, "status on a headerless log must fail");
    let message = json["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("state log has no header")
            && message.contains("`context_added` event (seq 1)"),
        "status must name the headerless log; got {json}"
    );
}

// ---------------------------------------------------------------------------
// The #236 shape: a blocked batch child that has no log yet
// ---------------------------------------------------------------------------

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

/// The same parent with a state in front of the batch, so a rewind has
/// somewhere to go back to.
const REWINDABLE_PARENT_TEMPLATE: &str = r#"---
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
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          gates.done.all_complete: true
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

fn write_templates(dir: &Path, parent_template: &str) -> PathBuf {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, parent_template).unwrap();
    parent
}

/// Look up one child's `outcome` in a `koto next` response's scheduler ledger.
fn ledger_outcome(json: &serde_json::Value, child: &str) -> Option<String> {
    json.get("scheduler")?["materialized_children"]
        .as_array()?
        .iter()
        .find(|e| e["name"] == child)
        .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
}

/// #236 as reported: tick a batch parent so a child whose `waits_on` is unmet
/// comes back `blocked` with no state log, then `context add <parent>.<child>`.
/// The refusal must not strand the batch: once the dependency finishes, the
/// next parent tick spawns the child cleanly, with no leftover context.
#[test]
fn context_add_refuses_a_blocked_batch_child_and_the_child_still_spawns() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path(), PARENT_TEMPLATE);

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
            {"name": "B", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    assert_eq!(
        ledger_outcome(&json, "parent.B").as_deref(),
        Some("blocked"),
        "precondition: B must be blocked; json={json} stderr={stderr}"
    );
    assert!(
        !state_path(tmp.path(), "parent.B").exists(),
        "precondition: a blocked child has no state log yet"
    );
    assert!(
        state_path(tmp.path(), "parent.A").exists(),
        "precondition: the ready sibling was spawned"
    );

    let out = context_add(tmp.path(), "parent.B", "context.md");
    assert_not_found(&out, tmp.path(), "parent.B", "context add");
    assert_no_session_dir(tmp.path(), "parent.B", "context add on a blocked child");

    // The ready sibling still accepts context: the refusal is about the
    // missing log, not about batch children in general.
    let out = context_add(tmp.path(), "parent.A", "context.md");
    assert!(
        out.status.success(),
        "context add on a spawned child must still work; stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Finish A, then tick the parent: B's dependency is met and it spawns.
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent.A", "--with-data", r#"{"marker": "x"}"#],
    );
    assert!(ok, "driving A to terminal failed: {json} {stderr}");
    let (_, json, stderr) = run_koto(tmp.path(), &["next", "parent"]);
    assert_eq!(
        ledger_outcome(&json, "parent.B").as_deref(),
        Some("running"),
        "B must spawn once A is done; json={json} stderr={stderr}"
    );

    let (ok, json, stderr) = run_koto(tmp.path(), &["status", "parent.B"]);
    assert!(ok, "the spawned child must be readable: {json} {stderr}");
    assert!(
        !context_key_present(tmp.path(), "parent.B", "context.md"),
        "the refused add must not have left context for B to inherit"
    );
}

// ---------------------------------------------------------------------------
// A session deleted by koto's terminal cleanup
// ---------------------------------------------------------------------------

/// A workflow that reaches a terminal state without `--no-cleanup` has its
/// session directory removed. A late `context add` to the same name, from an
/// agent that didn't notice the session finished, must not resurrect it as a
/// headerless log.
#[test]
fn context_add_refuses_a_session_removed_by_terminal_cleanup() {
    let tmp = TempDir::new().unwrap();
    let tpl = tmp.path().join("child.md");
    std::fs::write(&tpl, CHILD_TEMPLATE).unwrap();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "finished", "--template", tpl.to_str().unwrap()],
    );
    assert!(ok, "init failed: {}", stderr);
    assert!(
        state_path(tmp.path(), "finished").exists(),
        "precondition: init writes the log"
    );

    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "finished", "--with-data", r#"{"marker": "x"}"#],
    );
    assert!(ok, "drive to terminal failed: {} json={}", stderr, json);
    assert!(
        !session_dir(tmp.path(), "finished").exists(),
        "precondition: terminal cleanup removed the session directory"
    );

    let out = context_add(tmp.path(), "finished", "late.md");
    assert_not_found(&out, tmp.path(), "finished", "context add");
    assert_no_session_dir(tmp.path(), "finished", "context add after cleanup");
}

// ---------------------------------------------------------------------------
// A child moved by a parent rewind
// ---------------------------------------------------------------------------

/// Rewinding a batch parent moves each `<parent>.<task>` child to
/// `<parent>~N.<task>`. A writer still using the old name used to recreate it
/// as a one-line headerless log, the shape #200 reports.
#[test]
fn context_add_refuses_a_child_moved_by_a_parent_rewind() {
    let tmp = TempDir::new().unwrap();
    let parent_path = write_templates(tmp.path(), REWINDABLE_PARENT_TEMPLATE);

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "orch", "--template", parent_path.to_str().unwrap()],
    );
    assert!(ok, "parent init failed: {}", stderr);
    let (ok, json, stderr) = run_koto(tmp.path(), &["next", "orch"]);
    assert!(ok, "advancing into the batch state failed: {json} {stderr}");
    let tasks = serde_json::json!({
        "tasks": [{"name": "task-a", "waits_on": [], "vars": {}}]
    });
    let (_, json, stderr) = run_koto(
        tmp.path(),
        &["next", "orch", "--with-data", &tasks.to_string()],
    );
    assert!(
        state_path(tmp.path(), "orch.task-a").exists(),
        "precondition: the child was spawned; json={json} stderr={stderr}"
    );

    let (ok, json, stderr) = run_koto(tmp.path(), &["rewind", "orch"]);
    assert!(ok, "rewind failed: {json} {stderr}");
    assert!(
        !session_dir(tmp.path(), "orch.task-a").exists(),
        "precondition: rewind moved the child away from its old name"
    );
    assert!(
        state_path(tmp.path(), "orch~1.task-a").exists(),
        "precondition: the child's history now lives under orch~1.task-a"
    );

    let out = context_add(tmp.path(), "orch.task-a", "late.md");
    assert_not_found(&out, tmp.path(), "orch.task-a", "context add");
    assert_no_session_dir(tmp.path(), "orch.task-a", "context add after rewind");
}
