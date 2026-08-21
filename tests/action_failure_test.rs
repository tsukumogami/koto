//! End-to-end coverage for a failing `default_action` stopping the tick.
//!
//! Before this behavior existed, a state whose action exited non-zero advanced
//! exactly as if it had succeeded, and the `koto next` response said nothing
//! about it. The cases here pin the four things that changed: the tick stops at
//! the state that ran the command, the state's own gates do not run, the
//! command's facts reach the agent under the reserved `__action__` condition,
//! and the author's `fallback` prose rides the directive.
//!
//! DESIGN-koto-runs-commands.md Decisions 3, 4, and 5.

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

fn init_workflow(dir: &Path, name: &str, template: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();

    let output = koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_next(dir: &Path, name: &str) -> serde_json::Value {
    let output = koto_cmd(dir).args(["next", name]).output().unwrap();
    assert!(
        output.status.success(),
        "next should exit 0 for a blocked response: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("next output should be JSON")
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

fn events_of_type(dir: &Path, name: &str, event_type: &str) -> Vec<serde_json::Value> {
    state_log(dir, name)
        .into_iter()
        .filter(|e| e["type"] == event_type || e["event"] == event_type)
        .collect()
}

/// Set up a one-shot workflow, tick it once, and return the response.
fn run_once(dir: &Path, name: &str, template: &str) -> serde_json::Value {
    init_workflow(dir, name, template);
    run_next(dir, name)
}

/// The single `__action__` entry in a blocked response.
fn action_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(
        resp["action"], "gate_blocked",
        "a failing action stops through the gate-blocked path; got {resp}"
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

/// Assert the tick stayed put: the response names the action's state, reports
/// no advance, and the log records no transition.
fn assert_did_not_transition(dir: &Path, name: &str, resp: &serde_json::Value, state: &str) {
    assert_eq!(resp["state"], state);
    assert_eq!(resp["advanced"], false);
    // The log always carries the initial `null -> run` arrival; what must not
    // appear is a transition *out of* the state that ran the command.
    assert!(
        events_of_type(dir, name, "transitioned")
            .iter()
            .all(|e| e["payload"]["from"] != state),
        "a failing action must not transition; log was {:?}",
        state_log(dir, name)
    );
}

// ---------------------------------------------------------------------------
// templates
// ---------------------------------------------------------------------------

/// A state whose action is followed by an unconditional transition to a
/// terminal state. Before the failure stop existed, ticking this template
/// walked straight through to `done` no matter what the command did.
fn template(action_body: &str) -> String {
    format!(
        r#"---
name: failing
version: "1.0"
initial_state: run
states:
  run:
    default_action:
{action_body}
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run the command.

## done

All done.
"#
    )
}

// ---------------------------------------------------------------------------
// the state stops, whether or not it declares gates
// ---------------------------------------------------------------------------

#[test]
fn nonzero_exit_with_no_gates_stops_the_tick() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"echo working; echo broke >&2; exit 3\""),
    );

    assert_did_not_transition(dir.path(), "failing", &resp, "run");

    let cond = action_condition(&resp);
    assert_eq!(cond["type"], "action");
    assert_eq!(cond["status"], "failed");
    assert_eq!(cond["category"], "corrective");
    assert_eq!(cond["agent_actionable"], false);

    let output = &cond["output"];
    assert_eq!(output["state"], "run");
    assert_eq!(output["command"], "echo working; echo broke >&2; exit 3");
    assert_eq!(output["failure_kind"], "nonzero_exit");
    assert_eq!(output["exit_code"], 3);
    assert_eq!(output["stdout"], "working\n");
    assert_eq!(output["stderr"], "broke\n");
    assert_eq!(output["truncated"], false);
}

#[test]
fn nonzero_exit_with_a_passing_gate_stops_the_tick_without_running_it() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gated
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: "exit 3"
    gates:
      always_passes:
        type: command
        command: "true"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run the command.

## done

All done.
"#;
    let resp = run_once(dir.path(), "gated", template);

    assert_did_not_transition(dir.path(), "gated", &resp, "run");
    assert_eq!(
        action_condition(&resp)["output"]["failure_kind"],
        "nonzero_exit"
    );

    // The gate would have passed, and under the rejected alternative it would
    // have advanced the workflow past a command that failed. A state's gates
    // judge the work the action did; the action did not happen.
    assert!(
        events_of_type(dir.path(), "gated", "gate_evaluated").is_empty(),
        "the state's own gates must not run when its action failed; log was {:?}",
        state_log(dir.path(), "gated")
    );
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(
        conditions.len(),
        1,
        "only the action failure blocks; got {conditions:?}"
    );
}

// ---------------------------------------------------------------------------
// the other failure kinds
// ---------------------------------------------------------------------------

#[test]
fn a_command_that_does_not_exist_stops_the_tick() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"koto-no-such-command-xyz\""),
    );

    assert_did_not_transition(dir.path(), "failing", &resp, "run");
    let output = &action_condition(&resp)["output"];
    // `sh -c` finds no such command and reports it: the shell ran, so this is
    // a non-zero exit carrying the shell's own diagnosis rather than a spawn
    // failure.
    assert_eq!(output["failure_kind"], "nonzero_exit");
    assert_eq!(output["exit_code"], 127);
    assert!(
        output["stderr"].as_str().unwrap().contains("not found"),
        "the response should say why: {output}"
    );
}

#[test]
fn a_command_that_cannot_be_started_stops_the_tick() {
    let dir = TempDir::new().unwrap();
    // The working directory does not exist, so the child is never spawned and
    // no exit status is ever obtained.
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"echo hi\"\n      working_dir: \"no-such-dir\""),
    );

    assert_did_not_transition(dir.path(), "failing", &resp, "run");
    let output = &action_condition(&resp)["output"];
    assert_eq!(output["failure_kind"], "spawn_failed");
    assert!(
        output.get("exit_code").is_none(),
        "a command that never ran has no exit status to report: {output}"
    );
    assert!(
        output["stderr"]
            .as_str()
            .unwrap()
            .contains("failed to spawn command"),
        "the response should say the command could not be started: {output}"
    );
}

#[test]
fn a_command_that_exceeds_its_timeout_stops_the_tick() {
    let dir = TempDir::new().unwrap();
    // A polling action whose command succeeds but whose gate never does:
    // the deadline is what ends it, so the reported kind is the timeout
    // rather than the command's own status.
    let template = r#"---
name: slow
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: "echo polling"
      polling:
        interval_secs: 1
        timeout_secs: 1
    gates:
      never_passes:
        type: command
        command: "false"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Poll for it.

## done

All done.
"#;
    let resp = run_once(dir.path(), "slow", template);

    assert_did_not_transition(dir.path(), "slow", &resp, "run");
    let output = &action_condition(&resp)["output"];
    assert_eq!(output["failure_kind"], "timed_out");
    assert!(
        output.get("exit_code").is_none(),
        "a timed-out command reports no exit status: {output}"
    );
    assert!(
        output["stderr"].as_str().unwrap().contains("timed out"),
        "the response should say it timed out: {output}"
    );
}

// ---------------------------------------------------------------------------
// the fallback prose
// ---------------------------------------------------------------------------

#[test]
fn the_fallback_prose_rides_the_directive() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template(
            "      command: \"exit 1\"\n      fallback: \"The build did not run. Check the toolchain.\"",
        ),
    );

    let directive = resp["directive"]
        .as_str()
        .expect("directive should be text");
    let prose_at = directive
        .find("The build did not run. Check the toolchain.")
        .unwrap_or_else(|| panic!("fallback prose should reach the agent: {directive}"));
    let phase_at = directive
        .find("Run the command.")
        .unwrap_or_else(|| panic!("phase directive should still be present: {directive}"));
    assert!(
        prose_at < phase_at,
        "the fallback should precede the phase directive it explains: {directive}"
    );
    // Not in `details`: that field can be withheld, and a fallback the agent
    // may not receive is not a fallback.
    assert!(
        resp.get("details").is_none() || resp["details"].is_null(),
        "the fallback must not be routed through details: {resp}"
    );
}

#[test]
fn an_action_with_no_fallback_still_stops_with_an_unprefixed_directive() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"exit 1\""),
    );

    assert_did_not_transition(dir.path(), "failing", &resp, "run");
    assert_eq!(
        resp["directive"], "Run the command.",
        "with no fallback declared the directive carries no prefix"
    );
}

#[test]
fn a_gate_failure_after_a_successful_action_carries_no_fallback() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gatefail
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: "exit 0"
      fallback: "The action failed."
    gates:
      never_passes:
        type: command
        command: "false"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run the command.

## done

All done.
"#;
    let resp = run_once(dir.path(), "gatefail", template);

    assert_eq!(resp["action"], "gate_blocked");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert!(
        conditions.iter().all(|c| c["name"] != "__action__"),
        "the action succeeded, so nothing should be reported against it: {conditions:?}"
    );
    assert!(
        !resp["directive"]
            .as_str()
            .unwrap()
            .contains("The action failed."),
        "prose about a failure that did not happen must not reach the agent: {resp}"
    );
}

// ---------------------------------------------------------------------------
// ordering against requires_confirmation
// ---------------------------------------------------------------------------

#[test]
fn a_failing_action_stops_as_a_failure_even_when_confirmation_is_requested() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"exit 4\"\n      requires_confirmation: true"),
    );

    assert_ne!(
        resp["action"], "confirm",
        "a confirm stop carries no indication anything went wrong: {resp}"
    );
    assert_did_not_transition(dir.path(), "failing", &resp, "run");
    assert_eq!(action_condition(&resp)["output"]["exit_code"], 4);
}

#[test]
fn a_succeeding_action_still_reaches_the_confirm_stop() {
    let dir = TempDir::new().unwrap();
    let resp = run_once(
        dir.path(),
        "failing",
        &template("      command: \"echo ready\"\n      requires_confirmation: true"),
    );

    assert_eq!(
        resp["action"], "confirm",
        "confirmation is untouched on the success path: {resp}"
    );
    assert_eq!(resp["action_output"]["exit_code"], 0);
}

// ---------------------------------------------------------------------------
// the reserved name
// ---------------------------------------------------------------------------

#[test]
fn a_gate_named_action_is_rejected_at_compile_time() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("collide-template.md");
    std::fs::write(
        &src,
        r#"---
name: collide
version: "1.0"
initial_state: run
states:
  run:
    gates:
      __action__:
        type: command
        command: "true"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run it.

## done

All done.
"#,
    )
    .unwrap();

    let output = koto_cmd(dir.path())
        .args(["init", "collide", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a gate colliding with the reserved condition name must not compile"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stderr.contains("reserved for default_action failures")
            || stdout.contains("reserved for default_action failures"),
        "stdout={stdout} stderr={stderr}"
    );
}
