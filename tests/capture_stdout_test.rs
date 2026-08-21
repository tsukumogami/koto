//! End-to-end coverage for `capture_stdout_as`: a command's stdout delivered
//! under a declared name and read by a later state.
//!
//! The motivating case is a workflow that tells an agent to run
//! `git rev-parse --abbrev-ref HEAD` and then use the answer. koto could
//! always run the command; what it could not do was let the answer reach the
//! state that needed it. These cases pin that it now does -- including when
//! the engine auto-advances from the producing state through to the reading
//! state inside one tick, which is the case the per-tick overlay exists for --
//! and that each of the three delivery failures stops the tick at the state
//! that ran the command.
//!
//! DESIGN-koto-runs-commands.md Decisions 1 and 2.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

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

fn init_workflow(dir: &Path, name: &str, template: &str) -> std::process::Output {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();

    koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap()
}

fn init_ok(dir: &Path, name: &str, template: &str) {
    let output = init_workflow(dir, name, template);
    assert!(
        output.status.success(),
        "init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_next(dir: &Path, name: &str) -> serde_json::Value {
    run_next_with(dir, name, &[])
}

fn run_next_with(dir: &Path, name: &str, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["next", name];
    args.extend_from_slice(extra);
    let output = koto_cmd(dir).args(&args).output().unwrap();
    assert!(
        output.status.success(),
        "next should exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("next output should be JSON")
}

/// Run a tick expected to fail, returning the exit code and the parsed body.
fn run_next_failing(dir: &Path, name: &str, extra: &[&str]) -> (i32, serde_json::Value) {
    let mut args = vec!["next", name];
    args.extend_from_slice(extra);
    let output = koto_cmd(dir).args(&args).output().unwrap();
    let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "expected JSON, got {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output.status.code().unwrap_or(-1), body)
}

fn state_log(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("state log line should be JSON"))
        .collect()
}

fn captured_events(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    state_log(dir, name)
        .into_iter()
        .filter(|e| e["type"] == "variable_captured")
        .collect()
}

/// The single `__action__` entry in a blocked response.
fn action_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(
        resp["action"], "gate_blocked",
        "a failed capture stops through the gate-blocked path; got {resp}"
    );
    let conditions = resp["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array");
    let found: Vec<&serde_json::Value> = conditions
        .iter()
        .filter(|c| c["name"] == "__action__")
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one __action__ condition, got {conditions:?}"
    );
    found[0]
}

// ---------------------------------------------------------------------------
// templates
// ---------------------------------------------------------------------------

/// detect (captures BRANCH) -> report, which stops for evidence and reads the
/// captured name in its directive. One `koto next` walks both states, so this
/// is the auto-advance case in its smallest form.
fn read_in_directive_template(command: &str) -> String {
    format!(
        r#"---
name: reader
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "{command}"
      capture_stdout_as: BRANCH
      fallback: "Find the branch name yourself and submit it."
    transitions:
      - target: report
  report:
    accepts:
      ack:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          ack: ok
  done:
    terminal: true
---

## detect

Detect the branch.

## report

Working on branch {{{{BRANCH}}}}.

## done

Done.
"#
    )
}

// ---------------------------------------------------------------------------
// the value reaches a later state -- one assertion per overlay site
// ---------------------------------------------------------------------------

#[test]
fn a_captured_value_renders_in_a_later_states_directive_in_the_same_tick() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("echo main"),
    );

    let resp = run_next(dir.path(), "reader");

    assert_eq!(resp["state"], "report");
    assert_eq!(
        resp["directive"], "Working on branch main.",
        "the reading state's directive must render the value the earlier \
         state captured in this same tick; got {resp}"
    );

    let events = captured_events(dir.path(), "reader");
    assert_eq!(
        events.len(),
        1,
        "log was {:?}",
        state_log(dir.path(), "reader")
    );
    assert_eq!(events[0]["payload"]["key"], "BRANCH");
    assert_eq!(events[0]["payload"]["value"], "main");
}

#[test]
fn a_captured_value_routes_a_later_states_vars_when_clause_in_the_same_tick() {
    let dir = TempDir::new().unwrap();
    // `route` transitions on whether BRANCH is set. Both targets exist, so a
    // clause reading a stale map would route to `unset` rather than fail to
    // compile.
    let template = r#"---
name: router
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: route
  route:
    transitions:
      - target: was_set
        when:
          vars.BRANCH:
            is_set: true
      - target: was_unset
        when:
          vars.BRANCH:
            is_set: false
  was_set:
    terminal: true
  was_unset:
    terminal: true
---

## detect

Detect the branch.

## route

Route.

## was_set

Set.

## was_unset

Unset.
"#;
    init_ok(dir.path(), "router", template);

    let resp = run_next(dir.path(), "router");

    assert_eq!(
        resp["state"], "was_set",
        "a vars.* when clause in the same tick must see the captured value; got {resp}"
    );
}

#[test]
fn a_captured_value_substitutes_into_a_later_states_gate_command_in_the_same_tick() {
    let dir = TempDir::new().unwrap();
    // The gate command writes what it was handed, so the file is a direct
    // record of the string the gate ran -- not an inference from pass/fail.
    let template = r#"---
name: gater
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: check
  check:
    gates:
      record:
        type: command
        command: "printf %s '{{BRANCH}}' > gate-saw.txt"
    transitions:
      - target: done
  done:
    terminal: true
---

## detect

Detect the branch.

## check

Check.

## done

Done.
"#;
    init_ok(dir.path(), "gater", template);

    run_next(dir.path(), "gater");

    let seen = std::fs::read_to_string(dir.path().join("gate-saw.txt")).unwrap();
    assert_eq!(
        seen, "main",
        "a later state's gate command must substitute the captured value"
    );
}

#[test]
fn a_captured_value_substitutes_into_a_later_states_action_command_in_the_same_tick() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: acter
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: use
  use:
    default_action:
      command: "printf %s '{{BRANCH}}' > action-saw.txt"
    transitions:
      - target: done
  done:
    terminal: true
---

## detect

Detect the branch.

## use

Use it.

## done

Done.
"#;
    init_ok(dir.path(), "acter", template);

    run_next(dir.path(), "acter");

    let seen = std::fs::read_to_string(dir.path().join("action-saw.txt")).unwrap();
    assert_eq!(
        seen, "main",
        "a later state's action command must substitute the captured value"
    );
}

#[test]
fn a_captured_value_renders_on_a_later_tick_of_the_same_session() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("echo main"),
    );

    let first = run_next(dir.path(), "reader");
    assert_eq!(first["directive"], "Working on branch main.");

    // The second tick starts with an empty overlay, so this value can only
    // come from the log.
    let second = run_next(dir.path(), "reader");
    assert_eq!(second["state"], "report");
    assert_eq!(second["directive"], "Working on branch main.");
    assert_eq!(
        captured_events(dir.path(), "reader").len(),
        1,
        "the second tick must not re-run the action or re-capture"
    );
}

#[test]
fn a_captured_value_holding_a_variable_token_is_not_re_expanded() {
    // The allowlist forbids braces, so a value carrying a `{{...}}` token
    // cannot be delivered at all: the substitution layering that would keep
    // it literal never gets the chance to matter. That is the answer to
    // "what if the output looks like a template" -- it is rejected at the
    // door, not expanded later.
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        // Assembled from two arguments so the template source itself carries
        // no `{{NAME}}` token -- the compiler would reject one as an
        // undeclared reference before the command ever ran.
        &read_in_directive_template("printf '%s%s' '{{' 'OTHER}}'"),
    );

    let resp = run_next(dir.path(), "reader");
    let cond = action_condition(&resp);
    assert_eq!(cond["output"]["failure_kind"], "capture_failed");
    assert_eq!(
        cond["output"]["capture_error"]["case"],
        "disallowed_character"
    );
}

// ---------------------------------------------------------------------------
// the three delivery failures
// ---------------------------------------------------------------------------

#[test]
fn empty_output_fails_the_capture_and_stops_the_tick() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("printf ' '"),
    );

    let resp = run_next(dir.path(), "reader");

    assert_eq!(
        resp["state"], "detect",
        "the tick stops where the command ran"
    );
    assert_eq!(resp["advanced"], false);
    let cond = action_condition(&resp);
    assert_eq!(cond["type"], "action");
    assert_eq!(cond["status"], "failed");
    assert_eq!(cond["output"]["failure_kind"], "capture_failed");
    assert_eq!(cond["output"]["state"], "detect");
    assert_eq!(cond["output"]["capture_error"]["key"], "BRANCH");
    assert_eq!(cond["output"]["capture_error"]["case"], "empty");
    assert!(
        cond["output"]["exit_code"].is_null(),
        "the command exited zero, so no exit code is reported: {cond}"
    );
    assert!(
        resp["directive"]
            .as_str()
            .unwrap()
            .contains("Find the branch name yourself"),
        "a failed capture delivers the author's fallback prose: {resp}"
    );
    assert!(
        captured_events(dir.path(), "reader").is_empty(),
        "a failed capture appends no variable_captured event"
    );
}

#[test]
fn oversized_output_fails_the_capture() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("head -c 5000 /dev/zero | tr '\\\\0' a"),
    );

    let resp = run_next(dir.path(), "reader");

    assert_eq!(resp["state"], "detect");
    let cond = action_condition(&resp);
    assert_eq!(cond["output"]["failure_kind"], "capture_failed");
    let error = &cond["output"]["capture_error"];
    assert_eq!(error["key"], "BRANCH");
    assert_eq!(error["case"], "too_large");
    assert_eq!(error["bytes"], 5000);
    assert_eq!(error["limit"], 4096);
}

#[test]
fn output_the_allowlist_rejects_fails_the_capture_and_names_the_position() {
    let dir = TempDir::new().unwrap();
    // An interior newline is the case that makes multi-line capture
    // structurally impossible: trimming removes the surrounding whitespace,
    // and what is left still cannot pass the value allowlist.
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("printf 'main\\\\nsecond\\\\n'"),
    );

    let resp = run_next(dir.path(), "reader");

    assert_eq!(resp["state"], "detect");
    let cond = action_condition(&resp);
    assert_eq!(cond["output"]["failure_kind"], "capture_failed");
    let error = &cond["output"]["capture_error"];
    assert_eq!(error["key"], "BRANCH");
    assert_eq!(error["case"], "disallowed_character");
    assert_eq!(
        error["position"], 4,
        "the first rejected character is the newline after 'main': {error}"
    );
}

// ---------------------------------------------------------------------------
// lifetime and identity
// ---------------------------------------------------------------------------

#[test]
fn re_entering_the_producing_state_takes_the_later_value() {
    let dir = TempDir::new().unwrap();
    // `report` loops back to `detect`, so the command runs twice and the
    // second answer must win.
    let template = r#"---
name: looper
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "cat branch.txt"
      capture_stdout_as: BRANCH
    transitions:
      - target: report
  report:
    accepts:
      again:
        type: enum
        required: true
        values: [yes, no]
    transitions:
      - target: detect
        when:
          again: yes
      - target: done
        when:
          again: no
  done:
    terminal: true
---

## detect

Detect the branch.

## report

Working on branch {{BRANCH}}.

## done

Done.
"#;
    std::fs::write(dir.path().join("branch.txt"), "first\n").unwrap();
    init_ok(dir.path(), "looper", template);

    let first = run_next(dir.path(), "looper");
    assert_eq!(first["directive"], "Working on branch first.");

    std::fs::write(dir.path().join("branch.txt"), "second\n").unwrap();
    let second = run_next_with(dir.path(), "looper", &["--with-data", r#"{"again":"yes"}"#]);

    assert_eq!(second["state"], "report");
    assert_eq!(
        second["directive"], "Working on branch second.",
        "re-entering the producing state means the later value wins: {second}"
    );
    assert_eq!(captured_events(dir.path(), "looper").len(), 2);
}

#[test]
fn a_rewind_past_the_producing_state_leaves_the_value_in_place() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "reader",
        &read_in_directive_template("echo main"),
    );

    run_next(dir.path(), "reader");
    koto_cmd(dir.path())
        .args(["rewind", "reader"])
        .output()
        .unwrap();

    // The rewind appends an event and truncates nothing, so the value is
    // still on the log and still bound. This is documented behaviour, not an
    // accident: a rewind does not unwind what a command already did.
    assert_eq!(
        captured_events(dir.path(), "reader").len(),
        1,
        "a rewind removes no captured value; log was {:?}",
        state_log(dir.path(), "reader")
    );
    assert!(
        state_log(dir.path(), "reader")
            .iter()
            .any(|e| e["type"] == "rewound"),
        "the rewind should have been recorded"
    );
}

// ---------------------------------------------------------------------------
// a name that was never delivered
// ---------------------------------------------------------------------------

#[test]
fn reading_an_undelivered_capture_stops_with_a_typed_error() {
    let dir = TempDir::new().unwrap();
    // `start` routes either through the producing state or straight past it,
    // so reaching `report` without a value is a legitimate run rather than a
    // broken template.
    let template = r#"---
name: skipper
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      route:
        type: enum
        required: true
        values: [detect, skip]
    transitions:
      - target: detect
        when:
          route: detect
      - target: report
        when:
          route: skip
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: report
  report:
    accepts:
      ack:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          ack: ok
  done:
    terminal: true
---

## start

Choose.

## detect

Detect the branch.

## report

Working on branch {{BRANCH}}.

## done

Done.
"#;
    init_ok(dir.path(), "skipper", template);

    let (code, body) = run_next_failing(
        dir.path(),
        "skipper",
        &["--with-data", r#"{"route":"skip"}"#],
    );

    assert_eq!(
        code, 3,
        "the stop is an infrastructure-class refusal: {body}"
    );
    assert_eq!(body["error"]["code"], "capture_unset");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("BRANCH") && message.contains("detect") && message.contains("report"),
        "the message names the variable, the reading state, and the state that \
         would have delivered it: {message}"
    );
}

#[test]
fn a_terminal_state_whose_prose_is_never_rendered_is_not_refused() {
    let dir = TempDir::new().unwrap();
    // `done` is terminal, so its prose never reaches the agent -- a terminal
    // response carries no directive. Refusing the tick for a name that would
    // never have been shown would turn a finished workflow into an error.
    let template = r#"---
name: ender
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      route:
        type: enum
        required: true
        values: [detect, skip]
    transitions:
      - target: detect
        when:
          route: detect
      - target: done
        when:
          route: skip
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Choose.

## detect

Detect the branch.

## done

Finished on branch {{BRANCH}}.
"#;
    init_ok(dir.path(), "ender", template);

    let resp = run_next_with(dir.path(), "ender", &["--with-data", r#"{"route":"skip"}"#]);

    assert_eq!(resp["action"], "done", "got {resp}");
    assert_eq!(resp["state"], "done");
}

#[test]
fn init_rejects_a_var_named_after_a_capture() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("reader-template.md");
    std::fs::write(&src, read_in_directive_template("echo main")).unwrap();

    let output = koto_cmd(dir.path())
        .args([
            "init",
            "reader",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "BRANCH=main",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a capture name is not an init-time variable: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("BRANCH"),
        "the refusal names the variable: {combined}"
    );
}

// ---------------------------------------------------------------------------
// a state that declares no capture is untouched
// ---------------------------------------------------------------------------

#[test]
fn a_state_without_a_capture_name_appends_no_capture_event() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: plain
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: "echo main"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run.

## done

Done.
"#;
    init_ok(dir.path(), "plain", template);

    // `--no-cleanup` keeps the session on disk after it reaches its terminal
    // state, so the log is still there to inspect.
    let resp = run_next_with(dir.path(), "plain", &["--no-cleanup"]);

    assert_eq!(resp["state"], "done");
    assert!(
        captured_events(dir.path(), "plain").is_empty(),
        "log was {:?}",
        state_log(dir.path(), "plain")
    );
}

// ---------------------------------------------------------------------------
// compile-time rules
// ---------------------------------------------------------------------------

#[test]
fn a_capture_name_colliding_with_a_declared_variable_is_a_compile_error() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: collide
version: "1.0"
initial_state: detect
variables:
  BRANCH:
    description: the branch
states:
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: done
  done:
    terminal: true
---

## detect

Detect.

## done

Done.
"#;
    let output = init_workflow(dir.path(), "collide", template);

    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("BRANCH") && combined.contains("variables block"),
        "the error names the collision: {combined}"
    );
}

#[test]
fn two_states_capturing_the_same_name_is_a_compile_error() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: twice
version: "1.0"
initial_state: first
states:
  first:
    default_action:
      command: "echo one"
      capture_stdout_as: BRANCH
    transitions:
      - target: second
  second:
    default_action:
      command: "echo two"
      capture_stdout_as: BRANCH
    transitions:
      - target: done
  done:
    terminal: true
---

## first

One.

## second

Two.

## done

Done.
"#;
    let output = init_workflow(dir.path(), "twice", template);

    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("BRANCH"),
        "the error names the duplicated capture: {combined}"
    );
}

#[test]
fn a_typo_in_a_reference_is_still_a_compile_error() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: typo
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    transitions:
      - target: report
  report:
    terminal: true
---

## detect

Detect.

## report

Working on branch {{BRANHC}}.
"#;
    let output = init_workflow(dir.path(), "typo", template);

    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("BRANHC"),
        "the error names the misspelled reference: {combined}"
    );
}
