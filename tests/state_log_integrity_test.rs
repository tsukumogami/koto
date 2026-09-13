//! Regression tests for #236: `koto context add` against a session whose
//! state log does not exist must fail, and must not create the log.
//!
//! The mechanism on main: `handle_add` (src/cli/context.rs) calls
//! `ContextStore::add`, which `create_dir_all`s the session's ctx directory,
//! then `SessionBackend::append_event`, which opens the state file with
//! `create(true).append(true)` (src/engine/persistence.rs). Nothing checks
//! that a header exists first, so the first line written is a
//! `context_added` event and every later read fails with
//! `failed to parse header: missing field workflow`.
//!
//! Each test below fails on main and passes once `context add` (and
//! `context remove`, which has the same shape) refuses a session that has
//! no state log.

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

fn state_path(dir: &Path, name: &str) -> PathBuf {
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

/// Describe what is on disk at `path`, for assertion messages. On main the
/// bug produces a log whose first line is an event, not a header.
fn describe_log(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Err(_) => "<no state log>".to_string(),
        Ok(content) => {
            let first = content.lines().next().unwrap_or("");
            let first_has_workflow = serde_json::from_str::<serde_json::Value>(first)
                .ok()
                .and_then(|v| v.get("workflow").cloned())
                .is_some();
            format!(
                "state log created at {} (first line has `workflow`: {}):\n{}",
                path.display(),
                first_has_workflow,
                content
            )
        }
    }
}

/// Assert the refusal shape: non-zero exit and no state log on disk.
fn assert_refused(out: &std::process::Output, log: &Path, what: &str) {
    assert!(
        !log.exists(),
        "{what}: must not create a state log, but {}",
        describe_log(log)
    );
    assert!(
        !out.status.success(),
        "{what}: must fail on a session with no state log; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// (a) a session name that was never initialized
// ---------------------------------------------------------------------------

/// The simplest shape of #236: no `koto init` ever ran for this name.
///
/// On main this succeeds and leaves a log whose single line is
/// `{"seq":1,...,"type":"context_added",...}` with no `workflow` field.
#[test]
fn context_add_refuses_a_never_initialized_session() {
    let tmp = TempDir::new().unwrap();
    let log = state_path(tmp.path(), "never-started");
    assert!(!log.exists(), "precondition: no log yet");

    let out = context_add(tmp.path(), "never-started", "context.md");
    assert_refused(&out, &log, "context add on a never-initialized session");
}

/// Two adds to a missing log, the way #236's reporter hit it. On main both
/// succeed and both events carry `"seq":1`: `read_last_seq`
/// (src/engine/persistence.rs) skips line 1 as if it were the header, so
/// the one event on disk is invisible and the next seq is computed as 1
/// again. The assertion message prints the log so the duplicate is on
/// record in the failure output.
#[test]
fn repeated_context_add_does_not_build_a_headerless_log() {
    let tmp = TempDir::new().unwrap();
    let log = state_path(tmp.path(), "never-started");

    let first = context_add(tmp.path(), "never-started", "context.md");
    let second = context_add(tmp.path(), "never-started", "notes.md");
    assert_refused(&first, &log, "first context add on a missing log");
    assert_refused(&second, &log, "second context add on a missing log");
}

/// `context remove` has the same shape as `add` (store call, then an
/// unconditional `append_event`). With no session directory at all it
/// already fails on main, but only by accident: `ContextStore::remove`
/// does not create the directory, so the `create(true)` open in
/// `append_event` hits ENOENT. This pins that outcome.
#[test]
fn context_remove_refuses_a_never_initialized_session() {
    let tmp = TempDir::new().unwrap();
    let log = state_path(tmp.path(), "never-started");

    let out = koto_cmd(tmp.path())
        .args(["context", "remove", "never-started", "context.md"])
        .output()
        .unwrap();
    assert_refused(&out, &log, "context remove on a never-initialized session");
}

/// When the session directory exists but the log does not (a ctx store
/// left behind by an earlier `context add`, or a crash between
/// `init_state_file`'s `create_dir_all` and its rename), `context remove`
/// appends a headerless `context_removed` line on main.
#[test]
fn context_remove_refuses_a_session_dir_without_a_log() {
    let tmp = TempDir::new().unwrap();
    let session_dir = sessions_base(tmp.path()).join("dir-only");
    std::fs::create_dir_all(session_dir.join("ctx")).unwrap();
    let log = state_path(tmp.path(), "dir-only");
    assert!(!log.exists(), "precondition: directory but no log");

    let out = koto_cmd(tmp.path())
        .args(["context", "remove", "dir-only", "context.md"])
        .output()
        .unwrap();
    assert_refused(&out, &log, "context remove on a directory with no log");
}

/// Whatever `context add` does to a session with no log, it must not
/// leave behind a log that koto itself cannot read. On main `koto status`
/// then fails with `state file corrupted: failed to parse header: missing
/// field workflow`, the error #236 reports; the assertion message
/// carries it.
#[test]
fn context_add_never_leaves_an_unreadable_log() {
    let tmp = TempDir::new().unwrap();
    let log = state_path(tmp.path(), "never-started");

    let _ = context_add(tmp.path(), "never-started", "context.md");
    if !log.exists() {
        return;
    }
    let out = koto_cmd(tmp.path())
        .args(["status", "never-started"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "a log written by context add must be readable; status stdout={} stderr={}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        describe_log(&log)
    );
}

// ---------------------------------------------------------------------------
// (b) the #236 shape: a blocked batch child that has no log yet
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

/// Reproduces #236 as reported: tick a batch parent so a child whose
/// `waits_on` is unmet comes back `blocked` with no state log, then
/// `context add <parent>.<child>`.
///
/// On main the add succeeds, and afterwards `koto status parent.B` fails
/// with `state file corrupted: failed to parse header: missing field
/// workflow`. The ready sibling `parent.A` is unaffected because its log
/// already has a header.
#[test]
fn context_add_refuses_a_blocked_batch_child_with_no_log() {
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
            {"name": "B", "waits_on": ["A"], "vars": {}},
        ]
    });
    let (_, json, stderr) = run_koto(
        tmp.path(),
        &["next", "parent", "--with-data", &payload.to_string()],
    );
    let sched = json
        .get("scheduler")
        .unwrap_or_else(|| panic!("scheduler key expected; json={json} stderr={stderr}"));
    let ledger = sched["materialized_children"]
        .as_array()
        .expect("materialized_children array");
    let outcome_of = |name: &str| {
        ledger
            .iter()
            .find(|e| e["name"] == name)
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
    };
    assert_eq!(
        outcome_of("parent.B").as_deref(),
        Some("blocked"),
        "precondition: B must be blocked; ledger={}",
        serde_json::to_string_pretty(ledger).unwrap()
    );

    let child_log = state_path(tmp.path(), "parent.B");
    assert!(
        !child_log.exists(),
        "precondition: a blocked child has no state log yet"
    );
    assert!(
        state_path(tmp.path(), "parent.A").exists(),
        "precondition: the ready sibling was spawned"
    );

    let out = context_add(tmp.path(), "parent.B", "context.md");
    assert_refused(&out, &child_log, "context add on a blocked child");

    // The ready sibling still accepts context: the refusal is about the
    // missing log, not about batch children in general.
    let out = context_add(tmp.path(), "parent.A", "context.md");
    assert!(
        out.status.success(),
        "context add on a spawned child must still work; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (ok, _, stderr) = run_koto(tmp.path(), &["status", "parent.A"]);
    assert!(ok, "spawned child must stay readable: {}", stderr);
}

// ---------------------------------------------------------------------------
// (c) a session deleted by koto's terminal cleanup
// ---------------------------------------------------------------------------

/// A workflow that reaches a terminal state without `--no-cleanup` has its
/// session directory removed (`finish_terminal_tick` -> `backend.cleanup`
/// in src/cli/mod.rs). A late `context add` to the same name, e.g. from an
/// agent that did not notice the session finished, must not resurrect it
/// as a headerless log.
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
    let log = state_path(tmp.path(), "finished");
    assert!(log.exists(), "precondition: init writes the log");

    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["next", "finished", "--with-data", r#"{"marker": "x"}"#],
    );
    assert!(ok, "drive to terminal failed: {} json={}", stderr, json);
    assert!(
        !sessions_base(tmp.path()).join("finished").exists(),
        "precondition: terminal cleanup removed the session directory"
    );

    let out = context_add(tmp.path(), "finished", "late.md");
    assert_refused(&out, &log, "context add after terminal cleanup");
}
