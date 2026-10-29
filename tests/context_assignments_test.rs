//! End-to-end coverage for transition `context_assignments` (koto#204).
//!
//! Each test drives the real binary: a transition fires on one of the three
//! paths that append a `Transitioned` event (evidence-resolved, gate-resolved
//! auto-advance, skip_if), and `koto context get` reads the assigned value
//! back. The atomicity tests make the store unwritable across the transition
//! and check that a read still answers the value, from the log and without
//! storing it, and that the session's next tick stores it.

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    // No reader's anchor: a read here is anchorless.
    cmd.env_remove("KOTO_ANCHOR");
    cmd
}

fn init(dir: &Path, name: &str, template: &str) {
    let src = dir.join(format!("{name}.md"));
    std::fs::write(&src, template).unwrap();
    let out = koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn next(dir: &Path, name: &str, data: Option<&str>) -> serde_json::Value {
    let mut cmd = koto_cmd(dir);
    // `--no-cleanup` keeps a session that reaches its terminal, so its
    // context can still be read afterwards.
    cmd.args(["next", name, "--no-cleanup"]);
    if let Some(d) = data {
        cmd.args(["--with-data", d]);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|_| {
        panic!(
            "invalid JSON from next: stdout={stdout} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// `koto context get`; `None` when the key is absent.
fn ctx_get(dir: &Path, name: &str, key: &str) -> Option<String> {
    let out = koto_cmd(dir)
        .args(["context", "get", name, key])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap())
}

fn transitioned_events(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "transitioned")
        .collect()
}

const EVIDENCE_TEMPLATE: &str = r#"---
name: evidence-assign
version: "1.0"
initial_state: work
variables:
  TOPIC:
    default: widgets
states:
  work:
    accepts:
      decision:
        type: enum
        values: [ok, fail]
        required: true
      detail:
        type: string
        required: false
    transitions:
      - target: done
        when:
          decision: ok
        context_assignments:
          outcome: landed
          reason: "blocked: ${evidence.detail}"
          topic: "{{TOPIC}}"
      - target: failed
        when:
          decision: fail
        context_assignments:
          failure_reason: "failed: ${evidence.detail}"
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## work

Decide.

## done

Done.

## failed

Failed.
"#;

#[test]
fn evidence_path_writes_the_taken_edges_assignments() {
    let dir = TempDir::new().unwrap();
    init(dir.path(), "wf", EVIDENCE_TEMPLATE);

    let resp = next(
        dir.path(),
        "wf",
        Some(r#"{"decision": "ok", "detail": "disk full"}"#),
    );
    assert_eq!(resp["state"], "done", "got: {resp}");

    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "reason").as_deref(),
        Some("blocked: disk full")
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "topic").as_deref(),
        Some("widgets")
    );
    // The edge that did not fire wrote nothing.
    assert_eq!(ctx_get(dir.path(), "wf", "failure_reason"), None);

    // The event carries the resolved values.
    let events = transitioned_events(dir.path(), "wf");
    let last = events.last().unwrap();
    assert_eq!(
        last["payload"]["context_assignments"]["reason"], "blocked: disk full",
        "got: {last}"
    );
}

#[test]
fn absent_optional_evidence_resolves_empty_and_the_transition_happens() {
    let dir = TempDir::new().unwrap();
    init(dir.path(), "wf", EVIDENCE_TEMPLATE);

    let resp = next(dir.path(), "wf", Some(r#"{"decision": "fail"}"#));
    assert_eq!(resp["state"], "failed", "got: {resp}");
    assert_eq!(
        ctx_get(dir.path(), "wf", "failure_reason").as_deref(),
        Some("failed: ")
    );
    assert_eq!(ctx_get(dir.path(), "wf", "outcome"), None);
}

#[test]
fn resolved_values_are_stored_literally() {
    let dir = TempDir::new().unwrap();
    init(dir.path(), "wf", EVIDENCE_TEMPLATE);

    next(
        dir.path(),
        "wf",
        Some(r#"{"decision": "ok", "detail": "{{TOPIC}}"}"#),
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "reason").as_deref(),
        Some("blocked: {{TOPIC}}")
    );
}

#[test]
fn gate_path_resolves_gate_output_and_absent_path_is_empty() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-assign
version: "1.0"
initial_state: check
states:
  check:
    gates:
      ci:
        type: command
        command: "true"
    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
        context_assignments:
          code: "exit=${gates.ci.exit_code}"
          missing: "${gates.ci.no.such.path}"
  done:
    terminal: true
---

## check

Check.

## done

Done.
"#;
    init(dir.path(), "wf", template);
    let resp = next(dir.path(), "wf", None);
    assert_eq!(resp["state"], "done", "got: {resp}");
    assert_eq!(ctx_get(dir.path(), "wf", "code").as_deref(), Some("exit=0"));
    assert_eq!(ctx_get(dir.path(), "wf", "missing").as_deref(), Some(""));
}

#[test]
fn skip_if_path_writes_assignments_including_a_capture_from_the_same_tick() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: skip-assign
version: "1.0"
initial_state: detect
variables:
  TOPIC:
    default: widgets
states:
  detect:
    default_action:
      command: "echo feature-x"
      capture_stdout_as: BRANCH
    transitions:
      - target: route
  route:
    skip_if:
      vars.TOPIC:
        is_set: true
    transitions:
      - target: done
        context_assignments:
          branch: "{{BRANCH}}"
          step: skipped
  done:
    terminal: true
---

## detect

Detect.

## route

Route.

## done

Done.
"#;
    init(dir.path(), "wf", template);
    let resp = next(dir.path(), "wf", None);
    assert_eq!(resp["state"], "done", "got: {resp}");

    let events = transitioned_events(dir.path(), "wf");
    assert!(
        events
            .iter()
            .any(|e| e["payload"]["condition_type"] == "skip_if"
                && e["payload"]["context_assignments"]["step"] == "skipped"),
        "expected a skip_if transition carrying the assignment; got: {events:?}"
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "step").as_deref(),
        Some("skipped")
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "branch").as_deref(),
        Some("feature-x")
    );
}

/// `work` assigns `outcome` on its way to `mid`; `mid` routes on a
/// context-exists gate over that key, so a context gate is one of the
/// readers that has to see the value.
const LOST_WRITE_TEMPLATE: &str = r#"---
name: atomic-assign
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      decision:
        type: enum
        values: [ok]
        required: true
    transitions:
      - target: mid
        when:
          decision: ok
        context_assignments:
          outcome: landed
  mid:
    gates:
      has_outcome:
        type: context-exists
        key: outcome
    transitions:
      - target: done
        when:
          gates.has_outcome.exists: true
  done:
    terminal: true
---

## work

Work.

## mid

Mid.

## done

Done.
"#;

/// Start `wf` on [`LOST_WRITE_TEMPLATE`] and take the transition that
/// assigns `outcome` with the store unwritable, so the store write is lost
/// and only the log holds the value; then make the store writable again.
/// `wf` waits in `mid`, its gate blocked on the missing key.
fn lose_the_assigned_write(dir: &Path) {
    init(dir, "wf", LOST_WRITE_TEMPLATE);

    // Make the store unwritable: its directory is a regular file, so every
    // write after the event append fails. The log lives beside it and is
    // unaffected.
    let ctx = sessions_base(dir).join("wf").join("ctx");
    let _ = std::fs::remove_dir_all(&ctx);
    std::fs::write(&ctx, b"not a directory").unwrap();

    let resp = next(dir, "wf", Some(r#"{"decision": "ok"}"#));
    assert_eq!(resp["state"], "mid", "got: {resp}");
    let events = transitioned_events(dir, "wf");
    assert_eq!(
        events.last().unwrap()["payload"]["context_assignments"]["outcome"],
        "landed"
    );
    std::fs::remove_file(&ctx).unwrap();
}

/// Every file of `wf`'s session directory but the host's read spool, with
/// its bytes.
fn session_files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![sessions_base(dir).join("wf")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.ends_with("context-reads.jsonl") {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn failed_store_write_is_restored_by_the_next_tick() {
    let dir = TempDir::new().unwrap();
    lose_the_assigned_write(dir.path());

    // The next tick stores the value before its context gate reads it...
    let resp = next(dir.path(), "wf", None);
    assert_eq!(resp["state"], "done", "got: {resp}");
    // ...and so `koto context get` reads it from the store.
    assert_eq!(
        std::fs::read_to_string(sessions_base(dir.path()).join("wf/ctx/outcome")).unwrap(),
        "landed"
    );
    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
    );
}

/// A read from another process after the lost write answers the assigned
/// value from the log and stores nothing: `get`, `exists` and `list` report
/// it, and the session's files are byte-identical. The session's next tick
/// is what stores it.
#[test]
fn reads_answer_a_lost_value_from_the_log_and_store_nothing() {
    let dir = TempDir::new().unwrap();
    lose_the_assigned_write(dir.path());
    let before = session_files(dir.path());

    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
    );
    let exists = koto_cmd(dir.path())
        .args(["context", "exists", "wf", "outcome"])
        .output()
        .unwrap();
    assert_eq!(exists.status.code(), Some(0), "{exists:?}");
    let list = koto_cmd(dir.path())
        .args(["context", "list", "wf"])
        .output()
        .unwrap();
    assert!(list.status.success(), "{list:?}");
    let keys: Vec<String> = serde_json::from_slice(&list.stdout).unwrap();
    assert!(keys.contains(&"outcome".to_string()), "{keys:?}");

    assert_eq!(session_files(dir.path()), before);
    assert!(!sessions_base(dir.path()).join("wf/ctx/outcome").exists());

    let resp = next(dir.path(), "wf", None);
    assert_eq!(resp["state"], "done", "got: {resp}");
    assert_eq!(
        std::fs::read_to_string(sessions_base(dir.path()).join("wf/ctx/outcome")).unwrap(),
        "landed"
    );
}

#[test]
fn a_later_context_add_replaces_an_assignment() {
    let dir = TempDir::new().unwrap();
    init(dir.path(), "wf", EVIDENCE_TEMPLATE);
    next(dir.path(), "wf", Some(r#"{"decision": "ok"}"#));
    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
    );

    let out = koto_cmd(dir.path())
        .args(["context", "add", "wf", "outcome"])
        .write_stdin("overridden")
        .output()
        .unwrap();
    assert!(out.status.success());
    // The assignment is not restored over the later write.
    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("overridden")
    );
}
