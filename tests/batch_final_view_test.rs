//! `BatchFinalized` and the `batch_final_view` context key are recorded once
//! per completed batch, whichever state the completing tick ends in
//! (Issue #263).
//!
//! The usual parent summarizes its batch in a later state and routes there on
//! `gates.<gate>.all_complete: true`, so the tick that sees the last child
//! finish also leaves the batching state. The record used to be decided on
//! the state the tick stopped in, which has no `materialize_children`, so
//! nothing was written and a consumer reading the key found it missing.
//!
//! Scenarios:
//!
//! - the completing tick auto-advances to a non-terminal state: one event,
//!   naming the batching state, and the key readable from the next state; a
//!   later tick there adds nothing;
//! - the completing tick auto-advances straight to a terminal state: the
//!   terminal response carries `batch_final_view`;
//! - the completing tick stays in the batching state: exactly one event, and
//!   a later tick adds nothing;
//! - a directed `--to` out of a completed batching state records it too.

#![cfg(unix)]

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

/// Run koto and return `(success, last stdout line as JSON, stderr)`.
fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().last().unwrap_or("");
    let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.success(), json, stderr)
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (ok, json, stderr) = run_koto(dir, args);
    assert!(ok, "koto {args:?} failed: stderr={stderr} json={json}");
    json
}

const CHILD_TEMPLATE: &str = r#"---
name: batch-child
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
      - target: failed
        when:
          marker: fail
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## work

Do the work.

## done

Done.

## failed

Failed.
"#;

/// A parent that leaves its batching state on `all_complete`, into
/// `summarize` (non-terminal) or `closed` (terminal) depending on `NEXT`.
fn advancing_parent(next: &str) -> String {
    format!(
        r#"---
name: batch-parent-advance
version: "1.0"
initial_state: plan
states:
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
      - target: {next}
        when:
          gates.done.all_complete: true
  summarize:
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: closed
        when:
          status: ok
  closed:
    terminal: true
---

## plan

Plan the batch.

## summarize

Read `batch_final_view` and summarize the batch.

## closed

Closed.
"#
    )
}

/// A parent that stays in its batching state until told to finish, and can
/// be moved out of it with `--to summarize`.
const STAYING_PARENT: &str = r#"---
name: batch-parent-stay
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
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: closed
        when:
          status: ok
  closed:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize.

## closed

Closed.
"#;

/// Write the child template and `parent` from `parent_template`, init the
/// parent, and submit a two-task batch. Returns nothing; the parent is
/// named `parent`, its children `parent.A` and `parent.B`.
fn start_batch(dir: &Path, parent_template: &str) {
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, parent_template).unwrap();
    run_ok(
        dir,
        &["init", "parent", "--template", parent.to_str().unwrap()],
    );
    let tasks = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    run_ok(dir, &["next", "parent", "--with-data", &tasks.to_string()]);
}

fn drive_child(dir: &Path, name: &str, marker: &str) {
    let data = serde_json::json!({ "marker": marker }).to_string();
    run_ok(dir, &["next", name, "--no-cleanup", "--with-data", &data]);
}

/// The parent's `batch_finalized` events, parsed.
fn batch_finalized_events(dir: &Path) -> Vec<serde_json::Value> {
    let path = sessions_base(dir)
        .join("parent")
        .join("koto-parent.state.jsonl");
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["type"] == "batch_finalized")
        .collect()
}

/// `koto context get parent batch_final_view`, parsed, or `None` when the
/// key is absent.
fn batch_final_view_key(dir: &Path) -> Option<serde_json::Value> {
    let output = koto_cmd(dir)
        .args(["context", "get", "parent", "batch_final_view"])
        .output()
        .unwrap();
    if !output.status.success() {
        return None;
    }
    Some(serde_json::from_slice(&output.stdout).unwrap())
}

#[test]
fn a_batch_completed_by_a_tick_that_advances_out_is_recorded_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, &advancing_parent("summarize"));

    drive_child(dir, "parent.A", "done");
    // One child still running: the parent stays and records nothing.
    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(json["state"], "plan", "{json}");
    assert!(batch_finalized_events(dir).is_empty());
    assert!(batch_final_view_key(dir).is_none());

    // The last child finishes; this tick clears the gate and leaves `plan`.
    drive_child(dir, "parent.B", "fail");
    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(
        json["state"], "summarize",
        "the completing tick should advance out of the batching state: {json}"
    );

    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 1, "exactly one BatchFinalized: {events:?}");
    assert_eq!(
        events[0]["payload"]["state"], "plan",
        "the event names the state that owns the batch"
    );
    let view = &events[0]["payload"]["view"];
    assert_eq!(view["total"], 2, "{view}");
    assert_eq!(view["all_complete"], true, "{view}");
    assert_eq!(view["any_failed"], true, "{view}");

    // A consumer in the next state reads the key, and it is the event's view.
    let key = batch_final_view_key(dir)
        .expect("batch_final_view is readable from the state after the batch");
    assert_eq!(&key, view);

    // A later tick in `summarize` records nothing more.
    run_ok(dir, &["next", "parent"]);
    assert_eq!(batch_finalized_events(dir).len(), 1);
}

#[test]
fn a_batch_completed_by_a_tick_that_lands_on_a_terminal_state_is_in_the_response() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, &advancing_parent("closed"));

    drive_child(dir, "parent.A", "done");
    drive_child(dir, "parent.B", "done");
    let json = run_ok(dir, &["next", "parent", "--no-cleanup"]);
    assert_eq!(json["action"], "done", "{json}");
    assert_eq!(json["state"], "closed", "{json}");
    assert_eq!(
        json["batch_final_view"]["all_success"], true,
        "the terminal response carries the view: {json}"
    );
    assert_eq!(json["batch"]["phase"], "final", "{json}");
    assert_eq!(batch_finalized_events(dir).len(), 1);
    assert!(batch_final_view_key(dir).is_some());
}

#[test]
fn a_batch_completed_while_staying_in_the_batching_state_is_recorded_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, STAYING_PARENT);

    drive_child(dir, "parent.A", "done");
    drive_child(dir, "parent.B", "done");
    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(json["state"], "plan", "{json}");
    assert_eq!(batch_finalized_events(dir).len(), 1);
    let key = batch_final_view_key(dir).expect("the key is written");
    assert_eq!(key["all_success"], true, "{key}");

    // Re-ticking in place adds nothing.
    run_ok(dir, &["next", "parent"]);
    assert_eq!(batch_finalized_events(dir).len(), 1);

    // Leaving afterwards adds nothing either: the batch was already recorded.
    let finish = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ],
        "finalize": "yes"
    });
    let json = run_ok(dir, &["next", "parent", "--with-data", &finish.to_string()]);
    assert_eq!(json["state"], "summarize", "{json}");
    assert_eq!(batch_finalized_events(dir).len(), 1);
}

#[test]
fn a_directed_exit_from_a_completed_batch_records_it() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, STAYING_PARENT);

    drive_child(dir, "parent.A", "done");
    drive_child(dir, "parent.B", "done");
    // Leave with `--to` before any tick has seen the batch complete.
    let json = run_ok(dir, &["next", "parent", "--to", "summarize"]);
    assert_eq!(json["state"], "summarize", "{json}");

    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["payload"]["state"], "plan");
    assert!(batch_final_view_key(dir).is_some());
}

#[test]
fn a_directed_exit_from_an_incomplete_batch_records_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, STAYING_PARENT);

    drive_child(dir, "parent.A", "done");
    let json = run_ok(dir, &["next", "parent", "--to", "summarize"]);
    assert_eq!(json["state"], "summarize", "{json}");
    assert!(batch_finalized_events(dir).is_empty());
    assert!(batch_final_view_key(dir).is_none());
}

/// Two batching states in sequence, each leaving on `all_complete`. Each
/// gate watches its own fan-out through `name_filter`; without one, the
/// second gate would count the first batch's children as its own.
const TWO_BATCH_PARENT: &str = r#"---
name: batch-parent-two
version: "1.0"
initial_state: plan1
states:
  plan1:
    accepts:
      tasks1:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
        name_filter: "parent.b1-"
    materialize_children:
      from_field: tasks1
      default_template: child.md
    transitions:
      - target: plan2
        when:
          gates.done.all_complete: true
  plan2:
    accepts:
      tasks2:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
        name_filter: "parent.b2-"
    materialize_children:
      from_field: tasks2
      default_template: child.md
    transitions:
      - target: summarize
        when:
          gates.done.all_complete: true
  summarize:
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: closed
        when:
          status: ok
  closed:
    terminal: true
---

## plan1

First batch.

## plan2

Second batch.

## summarize

Summarize the second batch.

## closed

Closed.
"#;

/// Each batch in a session is recorded as its own, and the key holds the
/// latest one (Issue #275). Judged log-wide, the first batch's event
/// suppressed the second's, so the key kept the first batch's view.
#[test]
fn a_second_batch_in_the_same_session_is_recorded_with_its_own_view() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("child.md"), CHILD_TEMPLATE).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, TWO_BATCH_PARENT).unwrap();
    run_ok(
        dir,
        &["init", "parent", "--template", parent.to_str().unwrap()],
    );

    // First batch: one task, finished; the tick leaves plan1 for plan2.
    let first = serde_json::json!({"tasks1": [{"name": "b1-A", "waits_on": [], "vars": {}}]});
    run_ok(dir, &["next", "parent", "--with-data", &first.to_string()]);
    drive_child(dir, "parent.b1-A", "done");
    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(json["state"], "plan2", "{json}");
    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["payload"]["state"], "plan1");
    let first_view = batch_final_view_key(dir).expect("the first batch is recorded");
    assert_eq!(first_view["all_success"], true, "{first_view}");

    // Second batch: its own task, which fails; the tick leaves plan2.
    let second = serde_json::json!({"tasks2": [{"name": "b2-B", "waits_on": [], "vars": {}}]});
    run_ok(dir, &["next", "parent", "--with-data", &second.to_string()]);
    drive_child(dir, "parent.b2-B", "fail");
    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(json["state"], "summarize", "{json}");

    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 2, "one event per batch: {events:?}");
    assert_eq!(events[1]["payload"]["state"], "plan2");
    let second_view = batch_final_view_key(dir).expect("the key is still there");
    assert_eq!(
        &second_view, &events[1]["payload"]["view"],
        "the key holds the second batch's view, not the first's"
    );
    assert_eq!(second_view["any_failed"], true, "{second_view}");
}

/// A parent whose next state stops the tick with an error: its gate reads a
/// capture the run has not delivered, which koto refuses before running it.
const ERRORING_NEXT_PARENT: &str = r#"---
name: batch-parent-erroring-next
version: "1.0"
initial_state: plan
states:
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
    gates:
      probe:
        type: command
        command: 'test "{{TOKEN}}" = "x"'
    transitions:
      - target: producer
        when:
          gates.probe.exit_code: 0
  producer:
    default_action:
      command: 'echo x'
      capture_stdout_as: TOKEN
    transitions:
      - target: closed
  closed:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize.

## producer

Produce.

## closed

Closed.
"#;

/// A tick that leaves the batching state and then stops on an error still
/// records the batch: the transitions it made are on disk, and the next
/// tick starts past the batching state.
#[test]
fn a_tick_that_leaves_the_batch_and_then_errors_still_records_it() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, ERRORING_NEXT_PARENT);
    drive_child(dir, "parent.A", "done");
    drive_child(dir, "parent.B", "done");

    let (ok, json, _) = run_koto(dir, &["next", "parent"]);
    assert!(!ok, "the tick should stop on the capture refusal: {json}");

    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["payload"]["state"], "plan");
    assert!(batch_final_view_key(dir).is_some());
}

/// Children that finish without `--no-cleanup` are removed at their
/// terminal, and the parent reads their results from its own log. The
/// batch they complete is recorded the same way.
#[test]
fn a_batch_of_children_that_clean_up_is_recorded_when_the_tick_advances_out() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    start_batch(dir, &advancing_parent("summarize"));
    for (child, marker) in [("parent.A", "done"), ("parent.B", "fail")] {
        let data = serde_json::json!({ "marker": marker }).to_string();
        run_ok(dir, &["next", child, "--with-data", &data]);
    }

    let json = run_ok(dir, &["next", "parent"]);
    assert_eq!(json["state"], "summarize", "{json}");
    let events = batch_finalized_events(dir);
    assert_eq!(events.len(), 1, "{events:?}");
    let key = batch_final_view_key(dir).expect("the key is written");
    assert_eq!(key["total"], 2, "{key}");
    assert_eq!(key["any_failed"], true, "{key}");
}
