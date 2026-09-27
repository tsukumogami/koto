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
