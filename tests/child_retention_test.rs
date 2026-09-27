//! Integration tests for what a session keeps, and what it delivers, on the
//! tick that lands it in a terminal state (koto issue 240).
//!
//! Delivery and retention are separate: every arrival at a terminal records
//! the result on the session's own log, writes the terminal-index entry and
//! notifies the parent, once, whether or not the session is then kept.
//! `--no-cleanup` decides only whether the session stays on disk.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;

// ===== Harness =====

/// A parent whose only exit is unconditional, so it waits for its
/// `children-complete` gate to pass.
const PARENT_WAITS: &str = r#"---
name: parent-waits
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      batch_done:
        type: children-complete
    transitions:
      - target: finished
  finished:
    terminal: true
---

## wait

Wait for the children.

## finished

Done.
"#;

/// A parent whose exit keys on the gate's completeness, the shape
/// `/execute`'s `spawn_and_await` has.
const PARENT_KEYED: &str = r#"---
name: parent-keyed
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      batch_done:
        type: children-complete
    transitions:
      - target: finished
        when:
          gates.batch_done.all_complete: true
  finished:
    terminal: true
---

## wait

Wait for the children.

## finished

Done.
"#;

/// A child with a success terminal and a failure terminal, neither with a
/// `result:` map.
const CHILD: &str = r#"---
name: retention-child
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
      - target: done_blocked
        when:
          marker: fail
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
---

## work

Do the work.

## done

Done.

## done_blocked

Blocked.
"#;

/// A child that starts in its terminal state, the shape an older koto
/// left behind when it parked a child without recording its result.
const CHILD_STARTS_TERMINAL: &str = r#"---
name: retention-parked
version: "1.0"
initial_state: done
states:
  done:
    terminal: true
---

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
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    serde_json::from_str(last).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions").join(name)
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{name}.state.jsonl"))
}

fn write(dir: &Path, file: &str, body: &str) -> String {
    let p = dir.join(file);
    std::fs::write(&p, body).unwrap();
    p.to_str().unwrap().to_string()
}

fn init_parent(dir: &Path, name: &str, template: &str) {
    let t = write(dir, &format!("{name}.md"), template);
    run_ok(dir, &["init", name, "--template", &t]);
}

fn init_child(dir: &Path, name: &str, parent: &str, template: &str) {
    let t = write(dir, "child.md", template);
    run_ok(dir, &["init", name, "--template", &t, "--parent", parent]);
}

fn events(dir: &Path, name: &str) -> Vec<Value> {
    std::fs::read_to_string(state_path(dir, name))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").is_some())
        .collect()
}

fn count(dir: &Path, name: &str, event_type: &str) -> usize {
    events(dir, name)
        .iter()
        .filter(|e| e["type"] == event_type)
        .count()
}

fn child_completed(dir: &Path, parent: &str) -> Vec<Value> {
    events(dir, parent)
        .into_iter()
        .filter(|e| e["type"] == "child_completed")
        .map(|e| e["payload"].clone())
        .collect()
}

fn index_entries(dir: &Path, session: &str) -> usize {
    let path = koto::engine::terminal_index::terminal_index_path(&dir.join(".koto"));
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&format!("\"{session}\"")))
        .count()
}

fn line_count(dir: &Path, name: &str) -> usize {
    std::fs::read_to_string(state_path(dir, name))
        .unwrap_or_default()
        .lines()
        .count()
}

/// The `children-complete` gate's output from a parent tick that the gate
/// still blocks.
fn gate_output(resp: &Value) -> Value {
    resp["blocking_conditions"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["name"] == "batch_done"))
        .map(|c| c["output"].clone())
        .unwrap_or_else(|| panic!("the gate should still block; got {resp}"))
}

fn gate_child<'a>(output: &'a Value, name: &str) -> &'a Value {
    output["children"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["name"] == name))
        .unwrap_or_else(|| panic!("{name} missing from the gate output: {output}"))
}

// ===== Delivery on arrival =====

/// A child ticked to a success terminal with `--no-cleanup` delivers its
/// result, so a parent of either shape converges on it.
#[test]
fn a_kept_child_delivers_its_result_to_either_parent_shape() {
    for template in [PARENT_WAITS, PARENT_KEYED] {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        init_parent(dir, "p", template);
        init_child(dir, "p.leaf", "p", CHILD);

        run_ok(
            dir,
            &[
                "next",
                "p.leaf",
                "--with-data",
                r#"{"marker":"done"}"#,
                "--no-cleanup",
            ],
        );

        assert!(
            session_dir(dir, "p.leaf").exists(),
            "the flag kept the child"
        );
        assert_eq!(count(dir, "p.leaf", "request_store.result"), 1);
        assert_eq!(child_completed(dir, "p").len(), 1);

        let resp = run_ok(dir, &["next", "p"]);
        assert_eq!(resp["action"], "done", "the gate passed: {resp}");
        assert_eq!(resp["state"], "finished");
    }
}

/// The arrival writes one index entry and one parent notice; repeat ticks
/// of the kept session write nothing anywhere.
#[test]
fn repeat_ticks_of_a_kept_child_write_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD);
    run_ok(
        dir,
        &[
            "next",
            "p.leaf",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(index_entries(dir, "p.leaf"), 1);

    let (child_lines, parent_lines) = (line_count(dir, "p.leaf"), line_count(dir, "p"));
    for _ in 0..3 {
        run_ok(dir, &["next", "p.leaf", "--no-cleanup"]);
    }
    assert_eq!(line_count(dir, "p.leaf"), child_lines);
    assert_eq!(line_count(dir, "p"), parent_lines);
    assert_eq!(index_entries(dir, "p.leaf"), 1);
}

/// A root kept at a terminal has one index entry and gains nothing from
/// repeat ticks. It has no parent to notify.
#[test]
fn repeat_ticks_of_a_kept_root_write_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let t = write(dir, "root.md", CHILD);
    run_ok(dir, &["init", "r", "--template", &t]);
    run_ok(
        dir,
        &[
            "next",
            "r",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(index_entries(dir, "r"), 1);
    let lines = line_count(dir, "r");
    for _ in 0..3 {
        run_ok(dir, &["next", "r", "--no-cleanup"]);
    }
    assert_eq!(line_count(dir, "r"), lines);
    assert_eq!(index_entries(dir, "r"), 1);
}

/// A child standing in a terminal with no result recorded for its arrival
/// (what an older koto left behind when it parked a child) delivers on its
/// next tick, which un-sticks its parent.
#[test]
fn a_child_parked_without_a_result_delivers_on_its_next_tick() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_WAITS);
    init_child(dir, "p.leaf", "p", CHILD_STARTS_TERMINAL);
    assert_eq!(count(dir, "p.leaf", "request_store.result"), 0);

    // Before the tick the parent's gate is complete but has no result.
    let before = gate_output(&run_ok(dir, &["next", "p"]));
    assert_eq!(before["all_complete"], true);
    assert_eq!(before["results_in"], false);

    run_ok(dir, &["next", "p.leaf", "--no-cleanup"]);
    assert_eq!(count(dir, "p.leaf", "request_store.result"), 1);
    assert_eq!(index_entries(dir, "p.leaf"), 1);
    assert_eq!(child_completed(dir, "p").len(), 1);

    let resp = run_ok(dir, &["next", "p"]);
    assert_eq!(resp["action"], "done", "results are in: {resp}");
}

/// A kept child that fails, then is rewound, is running again: the gate
/// reports it pending and carries no result for it, rather than the
/// failure from its earlier arrival.
#[test]
fn a_rewound_child_reports_no_stale_result() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD);
    run_ok(
        dir,
        &[
            "next",
            "p.leaf",
            "--with-data",
            r#"{"marker":"fail"}"#,
            "--no-cleanup",
        ],
    );
    run_ok(dir, &["rewind", "p.leaf"]);

    let output = gate_output(&run_ok(dir, &["next", "p"]));
    let leaf = gate_child(&output, "p.leaf");
    assert_eq!(leaf["outcome"], "pending", "{output}");
    assert!(leaf.get("result").is_none(), "no stale result: {leaf}");

    // Its next terminal records the new arrival's result, which is what the
    // parent then reads.
    run_ok(
        dir,
        &[
            "next",
            "p.leaf",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(child_completed(dir, "p").len(), 2);
    let resp = run_ok(dir, &["next", "p"]);
    assert_eq!(resp["action"], "done", "the new result is in: {resp}");
}

/// A kept child that fails, is rewound, and lands in a terminal again has no
/// result for that new arrival until its terminal tick records one. In that
/// window the gate must not read the parent's copy from the earlier
/// arrival: the child reads as having no result yet.
#[test]
fn a_child_back_in_a_terminal_reports_no_stale_result() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_WAITS);
    init_child(dir, "p.leaf", "p", CHILD);
    run_ok(
        dir,
        &[
            "next",
            "p.leaf",
            "--with-data",
            r#"{"marker":"fail"}"#,
            "--no-cleanup",
        ],
    );
    run_ok(dir, &["rewind", "p.leaf"]);
    // Land it in the terminal again without the tick that records the
    // result: the state a crash between the two appends would leave.
    repeat_last_transition_into(dir, "p.leaf", "done_blocked");

    let resp = run_ok(dir, &["next", "p"]);
    let output = gate_output(&resp);
    let leaf = gate_child(&output, "p.leaf");
    assert_eq!(leaf["outcome"], "failure", "{output}");
    assert!(leaf.get("result").is_none(), "no stale result: {leaf}");
    assert_eq!(output["results_in"], false, "{output}");
}

/// Whether the parent log holds no notice, one, or two for a child's
/// arrival, a child still on disk is classified and dereferenced from its
/// own log.
#[test]
fn zero_one_or_two_parent_notices_give_the_same_gate() {
    // Hold the gate open with a second child that is still working, so the
    // gate reports its per-child view.
    let setup = |dir: &Path| {
        init_parent(dir, "p", PARENT_KEYED);
        init_child(dir, "p.leaf", "p", CHILD);
        init_child(dir, "p.busy", "p", CHILD);
    };
    let fail = r#"{"marker":"fail"}"#;

    // Zero: the parent log is unwritable during the child's terminal tick.
    let zero = TempDir::new().unwrap();
    setup(zero.path());
    let parent_log = state_path(zero.path(), "p");
    set_writable(&parent_log, false);
    let (code, _, stderr) = run(
        zero.path(),
        &["next", "p.leaf", "--with-data", fail, "--no-cleanup"],
    );
    set_writable(&parent_log, true);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(child_completed(zero.path(), "p").len(), 0);

    // One: the ordinary arrival.
    let one = TempDir::new().unwrap();
    setup(one.path());
    run_ok(
        one.path(),
        &["next", "p.leaf", "--with-data", fail, "--no-cleanup"],
    );
    assert_eq!(child_completed(one.path(), "p").len(), 1);

    // Two: the same notice appended again.
    let two = TempDir::new().unwrap();
    setup(two.path());
    run_ok(
        two.path(),
        &["next", "p.leaf", "--with-data", fail, "--no-cleanup"],
    );
    duplicate_last_child_completed(two.path(), "p");
    assert_eq!(child_completed(two.path(), "p").len(), 2);

    for tmp in [&zero, &one, &two] {
        let output = gate_output(&run_ok(tmp.path(), &["next", "p"]));
        let leaf = gate_child(&output, "p.leaf");
        assert_eq!(leaf["outcome"], "failure", "{output}");
        assert_eq!(leaf["result"]["status"], "failure", "{output}");
        assert_eq!(
            leaf["result"]["summary"], "failed at done_blocked",
            "{output}"
        );
    }
}

/// With the parent log unwritable on the terminal tick, the tick still
/// exits 0, warns, and keeps the child so the parent can still see it. The
/// next tick, with the parent writable, delivers the notice and removes the
/// child.
#[test]
fn a_failed_parent_notice_keeps_the_child_until_it_is_delivered() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD);

    let parent_log = state_path(dir, "p");
    set_writable(&parent_log, false);
    let (code, _, stderr) = run(
        dir,
        &["next", "p.leaf", "--with-data", r#"{"marker":"done"}"#],
    );
    set_writable(&parent_log, true);
    assert_eq!(code, 0);
    assert!(
        stderr.contains("warning"),
        "the failure is reported: {stderr}"
    );
    assert!(session_dir(dir, "p.leaf").exists(), "the child is kept");
    assert_eq!(child_completed(dir, "p").len(), 0);

    run_ok(dir, &["next", "p.leaf"]);
    assert!(
        !session_dir(dir, "p.leaf").exists(),
        "removed once delivered"
    );
    assert_eq!(child_completed(dir, "p").len(), 1);
}

/// A success terminal kept with `--no-cleanup` and ticked again without it
/// is removed. The index gains nothing; the parent gains exactly one more
/// notice, carrying the arrival's result, sent before the removal.
#[test]
fn a_kept_child_ticked_without_the_flag_is_removed_and_renotifies_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD);
    run_ok(
        dir,
        &[
            "next",
            "p.leaf",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    let first = child_completed(dir, "p");
    assert_eq!(first.len(), 1);
    let child_results = count(dir, "p.leaf", "request_store.result");
    assert_eq!(child_results, 1);

    let resp = run_ok(dir, &["next", "p.leaf"]);
    assert_eq!(resp["action"], "done");
    assert!(!session_dir(dir, "p.leaf").exists());
    assert_eq!(index_entries(dir, "p.leaf"), 1);
    let after = child_completed(dir, "p");
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after.iter().all(|n| n["result"] == first[0]["result"]));
}

// ===== Helpers that change files =====

fn set_writable(path: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if writable { 0o644 } else { 0o444 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Append a copy of the session's last `transitioned` event into `target`
/// under the next sequence number, as a tick that crashed right after the
/// transition would leave the log.
fn repeat_last_transition_into(dir: &Path, name: &str, target: &str) {
    let path = state_path(dir, name);
    let body = std::fs::read_to_string(&path).unwrap();
    let evs: Vec<Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("seq").is_some())
        .collect();
    let last_seq = evs.last().unwrap()["seq"].as_u64().unwrap();
    let mut ev = evs
        .iter()
        .rev()
        .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == target)
        .unwrap()
        .clone();
    ev["seq"] = Value::from(last_seq + 1);
    let mut body = body;
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&ev.to_string());
    body.push('\n');
    std::fs::write(&path, body).unwrap();
}

/// Append a copy of the parent's last `child_completed` event under the
/// next sequence number, as a second notice for the same arrival would.
fn duplicate_last_child_completed(dir: &Path, parent: &str) {
    let path = state_path(dir, parent);
    let body = std::fs::read_to_string(&path).unwrap();
    let evs: Vec<Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("seq").is_some())
        .collect();
    let last_seq = evs.last().unwrap()["seq"].as_u64().unwrap();
    let mut dup = evs
        .iter()
        .rev()
        .find(|e| e["type"] == "child_completed")
        .unwrap()
        .clone();
    dup["seq"] = Value::from(last_seq + 1);
    let mut body = body;
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&dup.to_string());
    body.push('\n');
    std::fs::write(&path, body).unwrap();
}

// ===== Failure terminals are kept =====

/// A batch parent that materializes children from `tasks` and accepts
/// `retry_failed`, holding in `plan` until told to finish.
const BATCH_PARENT: &str = r#"---
name: retention-batch
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
      batch_done:
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

Summarize.
"#;

/// A batch parent with two materialized children from [`CHILD`]: `p.leaf`,
/// the one under test, and `p.busy`, left working so the parent's gate keeps
/// reporting its per-child view.
fn batch_with_leaf(dir: &Path) {
    write(dir, "child.md", CHILD);
    init_parent(dir, "p", BATCH_PARENT);
    run_ok(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"tasks":[{"name":"leaf","waits_on":[]},{"name":"busy","waits_on":[]}]}"#,
        ],
    );
    assert!(
        session_dir(dir, "p.leaf").exists(),
        "the batch spawned the leaf"
    );
}

fn context_add(dir: &Path, session: &str, key: &str, content: &str) {
    let file = dir.join("ctx-input.txt");
    std::fs::write(&file, content).unwrap();
    run(
        dir,
        &[
            "context",
            "add",
            session,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ],
    );
}

fn context_get(dir: &Path, session: &str, key: &str) -> String {
    let (code, out, err) = run(dir, &["context", "get", session, key]);
    assert_eq!(code, 0, "context get {session} {key}: {err}");
    out
}

const FAIL: &str = r#"{"marker":"fail"}"#;
const DONE: &str = r#"{"marker":"done"}"#;

#[test]
fn a_failure_terminal_keeps_a_root() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let t = write(dir, "root.md", CHILD);
    run_ok(dir, &["init", "r", "--template", &t]);
    context_add(dir, "r", "plan.md", "the running record");

    let resp = run_ok(dir, &["next", "r", "--with-data", FAIL]);
    assert_eq!(resp["action"], "done");
    assert!(
        session_dir(dir, "r").exists(),
        "a failure terminal keeps the root"
    );
    assert_eq!(context_get(dir, "r", "plan.md"), "the running record");
    let status = run_ok(dir, &["status", "r"]);
    assert_eq!(status["is_terminal"], true);
    assert_eq!(status["current_state"], "done_blocked");
    assert_eq!(status["result"]["status"], "failure");
    let (_, workflows, _) = run(dir, &["workflows"]);
    assert!(
        workflows.contains("\"r\""),
        "koto workflows lists it: {workflows}"
    );

    // A root sent to its failure terminal with `--to` is kept too.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let t = write(dir, "root.md", CHILD);
    run_ok(dir, &["init", "r", "--template", &t]);
    run_ok(dir, &["next", "r", "--to", "done_blocked"]);
    assert!(session_dir(dir, "r").exists());
    assert_eq!(
        run_ok(dir, &["status", "r"])["current_state"],
        "done_blocked"
    );
}

#[test]
fn a_failure_terminal_keeps_a_child() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    context_add(dir, "p.leaf", "failure_reason", "the evidence came first");

    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    assert!(
        session_dir(dir, "p.leaf").exists(),
        "a failure terminal keeps the child"
    );
    assert_eq!(
        context_get(dir, "p.leaf", "failure_reason"),
        "the evidence came first"
    );
    let status = run_ok(dir, &["status", "p.leaf"]);
    assert_eq!(status["is_terminal"], true);
    assert_eq!(status["current_state"], "done_blocked");
    assert_eq!(status["result"]["status"], "failure");
    let (_, workflows, _) = run(dir, &["workflows"]);
    assert!(
        workflows.contains("p.leaf"),
        "koto workflows lists it: {workflows}"
    );
}

#[test]
fn a_directed_failure_terminal_keeps_the_session() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    context_add(dir, "p.leaf", "failure_reason", "directed");

    let resp = run_ok(dir, &["next", "p.leaf", "--to", "done_blocked"]);
    assert_eq!(resp["state"], "done_blocked");
    assert!(session_dir(dir, "p.leaf").exists());
    assert_eq!(context_get(dir, "p.leaf", "failure_reason"), "directed");
    assert_eq!(child_completed(dir, "p").len(), 1);
}

#[test]
fn a_retained_failure_stays_on_a_flagless_tick() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    run_ok(dir, &["next", "p.leaf"]);
    assert!(session_dir(dir, "p.leaf").exists());
}

#[test]
fn the_gate_reads_a_retained_failure_for_either_parent_shape() {
    for template in [PARENT_WAITS, PARENT_KEYED] {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        init_parent(dir, "p", template);
        init_child(dir, "p.leaf", "p", CHILD);
        // A second child still working holds the gate open, so it reports
        // its per-child view for either parent shape.
        init_child(dir, "p.busy", "p", CHILD);
        run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);

        let output = gate_output(&run_ok(dir, &["next", "p"]));
        let leaf = gate_child(&output, "p.leaf");
        assert_eq!(leaf["outcome"], "failure", "{output}");
        assert_eq!(leaf["result"]["status"], "failure", "{output}");
        assert!(
            !output["outstanding"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o == "p.leaf"),
            "the failed child's result is in: {output}"
        );

        // With the busy child finished too, every result is in.
        run_ok(dir, &["next", "p.busy", "--with-data", DONE]);
        let resp = run_ok(dir, &["next", "p"]);
        assert_eq!(
            resp["action"], "done",
            "results_in lets the gate pass: {resp}"
        );
    }
}

#[test]
fn retry_failed_reaches_a_retained_child() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);

    let (code, out, err) = run(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"retry_failed":{"children":["leaf"]}}"#,
        ],
    );
    assert_eq!(code, 0, "retry_failed is accepted: {out} {err}");
    assert!(!out.contains("unknown_children"), "{out}");

    let status = run_ok(dir, &["status", "p.leaf"]);
    assert_eq!(
        status["current_state"], "work",
        "rewound to its initial state"
    );
    let output = gate_output(&run_ok(dir, &["next", "p"]));
    assert_eq!(
        gate_child(&output, "p.leaf")["outcome"],
        "pending",
        "{output}"
    );

    // Its next terminal is a new arrival: exactly one new notice.
    let before = child_completed(dir, "p").len();
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    assert_eq!(child_completed(dir, "p").len(), before + 1);
}

#[test]
fn rewind_reaches_a_retained_session() {
    for (parented, name) in [(true, "p.leaf"), (false, "r")] {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        if parented {
            batch_with_leaf(dir);
        } else {
            let t = write(dir, "root.md", CHILD);
            run_ok(dir, &["init", "r", "--template", &t]);
        }
        run_ok(dir, &["next", name, "--with-data", FAIL]);
        run_ok(dir, &["rewind", name]);
        let status = run_ok(dir, &["status", name]);
        assert_eq!(status["current_state"], "work", "{name}");
        assert_eq!(status["is_terminal"], false, "{name}");
    }
}

#[test]
fn a_removed_session_is_still_refused() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    run_ok(dir, &["next", "p.leaf", "--with-data", DONE]);
    assert!(
        !session_dir(dir, "p.leaf").exists(),
        "a success terminal is removed"
    );

    let (_, out, _) = run(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"retry_failed":{"children":["leaf"]}}"#,
        ],
    );
    assert!(out.contains("unknown_children"), "{out}");
    let (code, out, err) = run(dir, &["rewind", "p.leaf"]);
    assert_ne!(code, 0);
    assert!(
        format!("{out}{err}").contains("not found"),
        "rewind names the missing workflow: {out} {err}"
    );
}

#[test]
fn a_new_arrival_notifies_once() {
    // A retried child that reaches a terminal again sends one new notice,
    // and the gate reports the new result.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD);
    init_child(dir, "p.busy", "p", CHILD);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    run_ok(dir, &["rewind", "p.leaf"]);
    run_ok(
        dir,
        &["next", "p.leaf", "--with-data", DONE, "--no-cleanup"],
    );
    let notices = child_completed(dir, "p");
    assert_eq!(notices.len(), 2);

    let output = gate_output(&run_ok(dir, &["next", "p"]));
    assert_eq!(gate_child(&output, "p.leaf")["result"]["status"], "success");
}

/// A child whose failure terminal declares a way out: an operator can move
/// it straight to the success terminal with `--to`.
const CHILD_WITH_RECOVERY: &str = r#"---
name: retention-recoverable
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
      - target: done_blocked
        when:
          marker: fail
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
    transitions:
      - target: done
---

## work

Do the work.

## done

Done.

## done_blocked

Blocked.
"#;

/// A directed move from one terminal to another is a new arrival: one new
/// notice, carrying the new terminal.
#[test]
fn a_directed_move_between_terminals_notifies_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD_WITH_RECOVERY);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    assert_eq!(child_completed(dir, "p").len(), 1);

    let resp = run_ok(dir, &["next", "p.leaf", "--to", "done", "--no-cleanup"]);
    assert_eq!(resp["state"], "done");
    assert_eq!(
        resp["retention"],
        serde_json::json!({"retained": true, "reason": "no_cleanup"})
    );
    let notices = child_completed(dir, "p");
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[1]["final_state"], "done");
    assert_eq!(notices[1]["result"]["status"], "success");

    // Without the flag, the same move lands in a success terminal and the
    // session is removed after its notice.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "p", PARENT_KEYED);
    init_child(dir, "p.leaf", "p", CHILD_WITH_RECOVERY);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    let resp = run_ok(dir, &["next", "p.leaf", "--to", "done"]);
    assert_eq!(resp["retention"], serde_json::json!({"retained": false}));
    assert!(!session_dir(dir, "p.leaf").exists());
    assert_eq!(child_completed(dir, "p").len(), 2);
}

#[test]
fn a_retried_child_that_succeeds_is_removed() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    batch_with_leaf(dir);
    run_ok(dir, &["next", "p.leaf", "--with-data", FAIL]);
    run_ok(
        dir,
        &[
            "next",
            "p",
            "--with-data",
            r#"{"retry_failed":{"children":["leaf"]}}"#,
        ],
    );
    run_ok(dir, &["next", "p.leaf", "--with-data", DONE]);
    assert!(
        !session_dir(dir, "p.leaf").exists(),
        "a success terminal is removed"
    );

    let output = gate_output(&run_ok(dir, &["next", "p"]));
    let leaf = gate_child(&output, "p.leaf");
    assert_eq!(leaf["outcome"], "success", "{output}");
    assert_eq!(leaf["result"]["status"], "success", "{output}");
}

#[test]
fn the_response_states_retention() {
    let cases = [
        (
            FAIL,
            false,
            serde_json::json!({"retained": true, "reason": "failure_terminal"}),
        ),
        (
            FAIL,
            true,
            serde_json::json!({"retained": true, "reason": "failure_terminal"}),
        ),
        (
            DONE,
            true,
            serde_json::json!({"retained": true, "reason": "no_cleanup"}),
        ),
        (DONE, false, serde_json::json!({"retained": false})),
    ];
    for (evidence, flag, expected) in cases {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let t = write(dir, "root.md", CHILD);
        run_ok(dir, &["init", "r", "--template", &t]);
        let mut args = vec!["next", "r", "--with-data", evidence];
        if flag {
            args.push("--no-cleanup");
        }
        let resp = run_ok(dir, &args);
        assert_eq!(resp["retention"], expected, "{evidence} flag={flag}");
    }

    // A second tick of a kept failure terminal reports the same.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let t = write(dir, "root.md", CHILD);
    run_ok(dir, &["init", "r", "--template", &t]);
    let first = run_ok(dir, &["next", "r", "--with-data", FAIL]);
    let second = run_ok(dir, &["next", "r"]);
    let kept = serde_json::json!({"retained": true, "reason": "failure_terminal"});
    assert_eq!(first["retention"], kept);
    assert_eq!(second["retention"], kept);
}

#[test]
fn the_help_text_describes_retention() {
    let tmp = TempDir::new().unwrap();
    let (_, out, _) = run(tmp.path(), &["next", "--help"]);
    assert!(
        out.contains("Keep the session after it reaches a terminal state (a failure terminal is always kept)"),
        "{out}"
    );
    assert!(!out.contains("useful for debugging"), "{out}");
}

#[test]
fn init_on_a_retained_root_name_is_refused() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let t = write(dir, "root.md", CHILD);
    run_ok(dir, &["init", "r", "--template", &t]);
    run_ok(dir, &["next", "r", "--with-data", FAIL]);
    let (code, out, err) = run(dir, &["init", "r", "--template", &t]);
    assert_ne!(code, 0);
    assert!(
        format!("{out}{err}").contains("koto session cleanup"),
        "the refusal names the remedy: {out} {err}"
    );
}

// ===== A bound leg =====

const LEG_PARENT: &str = r#"---
name: leg-coord
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

Gather.

## done

Done.
"#;

const ONE_LEG: &str = r#"{"legs":[
    {"name":"reviewer-a","role":"security","template":"review","inputs":{"pr":42}}
],"inputs":{"pr":42}}"#;

#[test]
fn a_bound_failed_child_resolves_its_leg_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_parent(dir, "coord-a", LEG_PARENT);
    init_child(dir, "child-1", "coord-a", CHILD);
    koto::engine::claim::rewrite_header_atomically(&state_path(dir, "child-1"), |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("coord-a".into());
        h
    })
    .unwrap();
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
        &["request", "bind", &id, "reviewer-a", "--child", "child-1"],
    );

    run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            FAIL,
        ],
    );
    assert!(
        session_dir(dir, "child-1").exists(),
        "the failed child is kept"
    );
    let leg = |dir: &Path| run_ok(dir, &["request", "get", &id])["legs"]["reviewer-a"].clone();
    let first = leg(dir);
    assert_eq!(first["disposition"], "resolved", "{first}");
    assert_eq!(first["result_source"], "promoted", "{first}");
    assert_eq!(first["result"]["status"], "failure", "{first}");
    assert_eq!(first["result_final_state"], "done_blocked", "{first}");

    // A later arrival (rewound, then done) leaves the resolved leg alone.
    run_ok(dir, &["rewind", "child-1"]);
    run_ok(
        dir,
        &[
            "next",
            "child-1",
            "--dispatch-epoch",
            "0",
            "--with-data",
            DONE,
            "--no-cleanup",
        ],
    );
    assert_eq!(leg(dir), first, "the first result stands");
}
