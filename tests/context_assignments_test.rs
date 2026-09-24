//! End-to-end coverage for transition `context_assignments` (koto#204).
//!
//! Each test drives the real binary: a transition fires on one of the three
//! paths that append a `Transitioned` event (evidence-resolved, gate-resolved
//! auto-advance, skip_if), and `koto context get` reads the assigned value
//! back. The atomicity test makes the store unwritable across the transition
//! and checks that the next read still sees the value.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
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

#[test]
fn failed_store_write_is_restored_on_the_next_read() {
    let dir = TempDir::new().unwrap();
    // `work` assigns `outcome` on its way to `mid`; `mid` routes on a
    // context-exists gate over that key, so a context gate is one of the
    // readers that has to see the value.
    let template = r#"---
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
    init(dir.path(), "wf", template);

    // Make the store unwritable: its directory is a regular file, so every
    // write after the event append fails. The log lives beside it and is
    // unaffected.
    let ctx = sessions_base(dir.path()).join("wf").join("ctx");
    let _ = std::fs::remove_dir_all(&ctx);
    std::fs::write(&ctx, b"not a directory").unwrap();

    let resp = next(dir.path(), "wf", Some(r#"{"decision": "ok"}"#));
    assert_eq!(resp["state"], "mid", "got: {resp}");
    let events = transitioned_events(dir.path(), "wf");
    assert_eq!(
        events.last().unwrap()["payload"]["context_assignments"]["outcome"],
        "landed"
    );

    // Restore the store. The next tick's context gate sees the value...
    std::fs::remove_file(&ctx).unwrap();
    let resp = next(dir.path(), "wf", None);
    assert_eq!(resp["state"], "done", "got: {resp}");
    // ...and so does `koto context get`.
    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
    );
}

#[test]
fn context_get_alone_restores_a_value_the_store_missed() {
    let dir = TempDir::new().unwrap();
    init(dir.path(), "wf", EVIDENCE_TEMPLATE);

    let ctx = sessions_base(dir.path()).join("wf").join("ctx");
    let _ = std::fs::remove_dir_all(&ctx);
    std::fs::write(&ctx, b"not a directory").unwrap();
    let resp = next(dir.path(), "wf", Some(r#"{"decision": "ok"}"#));
    assert_eq!(resp["state"], "done", "got: {resp}");
    std::fs::remove_file(&ctx).unwrap();

    assert_eq!(
        ctx_get(dir.path(), "wf", "outcome").as_deref(),
        Some("landed")
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
