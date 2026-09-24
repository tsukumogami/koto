//! Integration tests for a terminal state's declared `result:` map.
//!
//! A terminal may declare `result:`, and koto resolves it once, on the tick
//! that lands the session there, into the `WorkflowResult`'s `payload`. These
//! tests drive the real binary because what the feature promises is about
//! places a result is read from after the tick: the `koto next` response, the
//! child's own log, a bound request leg, the parent's `ChildCompleted`, and
//! `koto status` on a retained session. They must all carry the same value.

#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::{json, Value};

// ===== Harness =====

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

/// Two terminals with result maps. `work` parks on evidence so a context
/// write can land before the terminal tick.
const CHILD_TEMPLATE: &str = r#"---
name: result-child
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done, fail]
      summary:
        type: string
        required: false
    transitions:
      - target: done
        when:
          marker: done
      - target: done_error
        when:
          marker: fail
  done:
    terminal: true
    result:
      outcome: scoped
      topic: "{{TOPIC}}"
      pr: "${context.home_pr}"
      state: "merge-state:${context.state}"
      echo: "${context.echo}"
  done_error:
    terminal: true
    failure: true
    result:
      outcome: error
      step: "${context.step}"
      pr: "${context.absent}"
      topic: "{{TOPIC}}"
---

## work

Do the work.

## done

Done.

## done_error

Failed.
"#;

/// The same shape with no result map, for the unchanged-behaviour check.
const PLAIN_TEMPLATE: &str = r#"---
name: plain-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
      summary:
        type: string
        required: false
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Do the work.

## done

Done.
"#;

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
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
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions").join(name)
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{name}.state.jsonl"))
}

fn context_add(dir: &Path, session: &str, key: &str, content: &[u8]) {
    let mut cmd = koto_cmd(dir);
    cmd.args(["context", "add", session, key]);
    cmd.write_stdin(content.to_vec());
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "context add {key} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Parent `coord-a` plus a child created under it, shaped so `request
/// bind` accepts the child.
fn setup(dir: &Path, child: &str, child_template: &str) {
    std::fs::write(dir.join("parent.md"), PARENT_TEMPLATE).unwrap();
    std::fs::write(dir.join("child.md"), child_template).unwrap();
    if !state_path(dir, "coord-a").exists() {
        let parent = dir.join("parent.md");
        run_ok(
            dir,
            &["init", "coord-a", "--template", parent.to_str().unwrap()],
        );
    }
    let child_tmpl = dir.join("child.md");
    let (code, out, err) = run(
        dir,
        &[
            "init",
            child,
            "--template",
            child_tmpl.to_str().unwrap(),
            "--parent",
            "coord-a",
        ]
        .into_iter()
        .chain(
            child_template
                .contains("TOPIC:")
                .then_some(["--var", "TOPIC=my-topic"])
                .into_iter()
                .flatten(),
        )
        .collect::<Vec<_>>(),
    );
    assert_eq!(code, 0, "init child\n{out}\n{err}");
    koto::engine::claim::rewrite_header_atomically(&state_path(dir, child), |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("coord-a".into());
        h
    })
    .unwrap();
}

const ONE_LEG: &str = r#"{"legs":[
    {"name":"reviewer-a","role":"security","template":"review","inputs":{"pr":42}}
],"inputs":{"pr":42}}"#;

fn create_and_bind(dir: &Path, child: &str) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            ONE_LEG,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    let id = envelope["request_id"].as_str().unwrap().to_string();
    run_ok(
        dir,
        &["request", "bind", &id, "reviewer-a", "--child", child],
    );
    id
}

/// Every `request_store.result` event on a session's own log.
fn recorded_results(dir: &Path, name: &str) -> Vec<Value> {
    let log = std::fs::read_to_string(state_path(dir, name)).unwrap_or_default();
    log.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["type"] == "request_store.result")
        .map(|e| e["payload"]["result"].clone())
        .collect()
}

fn child_completed_results(dir: &Path, parent: &str) -> Vec<Value> {
    let log = std::fs::read_to_string(state_path(dir, parent)).unwrap_or_default();
    log.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["type"] == "child_completed")
        .map(|e| e["payload"]["result"].clone())
        .collect()
}

fn terminal_index_entries(dir: &Path, session: &str) -> usize {
    let path = koto::engine::terminal_index::terminal_index_path(&dir.join(".koto"));
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&format!("\"{session}\"")))
        .count()
}

fn leg_result(dir: &Path, id: &str) -> Value {
    let envelope = run_ok(dir, &["request", "get", id]);
    envelope["legs"]["reviewer-a"]["result"].clone()
}

fn seed_done_context(dir: &Path, child: &str) {
    context_add(dir, child, "home_pr", b"https://example.test/pr/7");
    context_add(dir, child, "state", b"open");
    context_add(dir, child, "echo", b"{{TOPIC}}");
}

fn expected_done_payload() -> Value {
    json!({
        "outcome": "scoped",
        "topic": "my-topic",
        "pr": "https://example.test/pr/7",
        "state": "merge-state:open",
        // Context content holding a reference is copied, not expanded again.
        "echo": "{{TOPIC}}",
    })
}

// ===== The five carriers =====

/// One session, one template, every place a result is read from. The first
/// tick parks the terminal (`--no-cleanup`), which is what makes the child's
/// own log and `koto status` readable; the second tick lets cleanup run,
/// which is what emits the parent's `ChildCompleted`. Both ticks report the
/// value recorded on the first.
#[test]
fn the_declared_payload_rides_all_five_carriers() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    let id = create_and_bind(dir, "child-1");
    seed_done_context(dir, "child-1");

    // Advance-loop terminal write site.
    let next = run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"done","summary":"all good"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(next["action"], "done");
    let result = next["result"].clone();
    assert_eq!(result["status"], "success");
    // `summary` keeps today's derivation: the evidence above was submitted
    // on `work`, not on the terminal, so it falls back to the default.
    assert_eq!(result["summary"], "completed at done");
    assert_eq!(
        result["payload"],
        expected_done_payload(),
        "the payload is exactly the declared map; evidence fields are not merged in"
    );

    // 1. The child's own `request_store.result` event.
    assert_eq!(recorded_results(dir, "child-1"), vec![result.clone()]);
    // 2. The leg's result after promotion.
    assert_eq!(leg_result(dir, &id), result);
    // 3. `koto status` on the retained session.
    let status = run_ok(dir, &["status", "child-1"]);
    assert_eq!(status["is_terminal"], true);
    assert_eq!(status["result"], result);

    // 4. `ChildCompleted` on the parent, emitted once cleanup is allowed.
    assert!(child_completed_results(dir, "coord-a").is_empty());
    let final_tick = run_ok(dir, &["next", "child-1"]);
    assert_eq!(
        final_tick["result"], result,
        "a later tick reports the record"
    );
    assert_eq!(
        child_completed_results(dir, "coord-a"),
        vec![result.clone()]
    );
    assert!(
        !session_dir(dir, "child-1").exists(),
        "the session is cleaned up on the tick without --no-cleanup"
    );
}

/// `koto next --to <terminal>` is the other terminal write site.
#[test]
fn a_directed_transition_to_a_terminal_produces_the_declared_payload() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    context_add(dir, "child-1", "step", b"scope:push");

    let (code, stdout, stderr) = run(
        dir,
        &["next", "child-1", "--to", "done_error", "--no-cleanup"],
    );
    assert_eq!(
        code, 0,
        "the terminal tick still succeeds\n{stdout}\n{stderr}"
    );
    let next: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(next["action"], "done");
    let result = next["result"].clone();
    assert_eq!(result["status"], "failure");
    // `${context.absent}` resolves empty and is listed under `missing`.
    assert_eq!(
        result["payload"],
        json!({
            "outcome": "error",
            "step": "scope:push",
            "pr": "",
            "topic": "my-topic",
            "missing": ["pr"],
        })
    );
    assert_eq!(recorded_results(dir, "child-1"), vec![result.clone()]);
    assert_eq!(run_ok(dir, &["status", "child-1"])["result"], result);
}

// ===== Unresolved references =====

#[test]
fn a_missing_context_key_resolves_empty_and_is_listed() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    // `step` is never written, and `pr` names a key nothing writes.

    let (code, stdout, stderr) = run(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"fail"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let next: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        next["result"]["payload"],
        json!({
            "outcome": "error",
            "step": "",
            "pr": "",
            "topic": "my-topic",
            "missing": ["pr", "step"],
        })
    );
}

#[test]
fn context_content_that_is_not_utf8_is_treated_as_missing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    context_add(dir, "child-1", "home_pr", &[0xff, 0xfe, 0xfd]);
    context_add(dir, "child-1", "state", b"open");
    context_add(dir, "child-1", "echo", b"x");

    let (code, stdout, stderr) = run(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let next: Value = serde_json::from_str(&stdout).unwrap();
    let payload = &next["result"]["payload"];
    assert_eq!(payload["pr"], "");
    assert_eq!(payload["missing"], json!(["pr"]));
    assert_eq!(payload["state"], "merge-state:open");
}

// ===== Resolved once =====

/// A parked terminal records its result once; repeat ticks append nothing,
/// and a context write after the terminal changes nothing a reader sees.
#[test]
fn a_parked_terminal_records_once_and_later_context_writes_change_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    // No leg pointer on this session.
    setup(dir, "child-1", CHILD_TEMPLATE);
    seed_done_context(dir, "child-1");

    let first = run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    let result = first["result"].clone();
    assert_eq!(result["payload"], expected_done_payload());
    assert_eq!(recorded_results(dir, "child-1").len(), 1);

    context_add(dir, "child-1", "home_pr", b"https://example.test/pr/999");

    for _ in 0..3 {
        let again = run_ok(dir, &["next", "child-1", "--no-cleanup"]);
        assert_eq!(again["result"], result, "a repeat tick reports the record");
    }
    assert_eq!(
        recorded_results(dir, "child-1").len(),
        1,
        "no further result events on the child's log"
    );
    assert!(
        child_completed_results(dir, "coord-a").is_empty(),
        "a parked terminal child emits no parent event"
    );
    assert_eq!(
        terminal_index_entries(dir, "child-1"),
        0,
        "a parked terminal writes no terminal-index entry"
    );
    assert_eq!(run_ok(dir, &["status", "child-1"])["result"], result);
}

#[test]
fn a_context_write_after_the_terminal_does_not_change_the_leg() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    let id = create_and_bind(dir, "child-1");
    seed_done_context(dir, "child-1");

    let first = run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    let result = first["result"].clone();
    context_add(dir, "child-1", "home_pr", b"https://example.test/pr/999");
    run_ok(dir, &["next", "child-1", "--no-cleanup"]);

    assert_eq!(leg_result(dir, &id), result);
    assert_eq!(run_ok(dir, &["status", "child-1"])["result"], result);
}

// ===== Unchanged surfaces =====

#[test]
fn status_on_a_non_terminal_session_carries_no_result() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", CHILD_TEMPLATE);
    let status = run_ok(dir, &["status", "child-1"]);
    assert_eq!(status["is_terminal"], false);
    assert!(status.get("result").is_none(), "{status}");
}

#[test]
fn a_terminal_without_a_map_keeps_the_evidence_derived_payload() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir, "child-1", PLAIN_TEMPLATE);
    let next = run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            r#"{"marker":"done","summary":"plain"}"#,
            "--no-cleanup",
        ],
    );
    // Today's derivation: terminal evidence only, and none was submitted on
    // `done`, so there is no payload and the summary is the default.
    assert_eq!(
        next["result"],
        json!({
            "status": "success",
            "summary": "completed at done",
        })
    );
    // A parked terminal without a map records nothing, as before.
    assert!(recorded_results(dir, "child-1").is_empty());
    let status = run_ok(dir, &["status", "child-1"]);
    assert_eq!(status["result"], next["result"]);
}

// ===== Compile-time =====

#[test]
fn a_33_key_result_map_is_refused_at_init() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut keys = String::new();
    for i in 0..33 {
        keys.push_str(&format!("      k{i}: v\n"));
    }
    let template = format!(
        "---\nname: big\nversion: \"1.0\"\ninitial_state: done\nstates:\n  done:\n    terminal: true\n    result:\n{keys}---\n\n## done\n\nDone.\n"
    );
    let path = dir.join("big.md");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(template.as_bytes()).unwrap();
    let (code, stdout, stderr) = run(dir, &["template", "compile", path.to_str().unwrap()]);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    let envelope: Value = serde_json::from_str(&stdout).unwrap();
    let text = envelope["error"].as_str().unwrap_or_default().to_string();
    assert!(text.contains("\"done\""), "{text}");
    assert!(text.contains("33"), "{text}");
    assert!(text.contains("32"), "{text}");
}
