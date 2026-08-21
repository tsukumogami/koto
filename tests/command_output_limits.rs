//! End-to-end coverage for the two shared-path defects: the pipe-buffer
//! deadlock in `run_shell_command` and the migration notice that reprinted on
//! every invocation.
//!
//! Both defects live under the command runner that gate evaluation and
//! `default_action` execution share, so the cases here exercise gates and
//! actions alike. Every size that has to clear the kernel pipe buffer is
//! derived from a buffer measured at run time rather than from an assumed
//! 64 KB, because the buffer is a platform property and the deadlock's
//! boundary moves with it.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use koto::action::{run_shell_command, FailureKind, MAX_ACTION_OUTPUT_BYTES};
use koto::gate::{evaluate_gates, GateOutcome};
use koto::template::types::Gate;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Note the CLI appends to a stream whose tail was dropped.
const TRUNCATION_NOTE: &str = "... [output truncated]";

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

fn koto_binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("koto")
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

/// The one `default_action_executed` event in the log.
fn action_event(dir: &Path, name: &str) -> serde_json::Value {
    let mut events: Vec<serde_json::Value> = state_log(dir, name)
        .into_iter()
        .filter(|e| {
            e["type"] == "default_action_executed" || e["event"] == "default_action_executed"
        })
        .collect();
    assert_eq!(
        events.len(),
        1,
        "expected exactly one default_action_executed event, log was {:?}",
        state_log(dir, name)
    );
    events.pop().unwrap()
}

// ---------------------------------------------------------------------------
// pipe buffer measurement
// ---------------------------------------------------------------------------

/// Measure this platform's pipe buffer by filling one until it refuses more.
///
/// The write end is put in non-blocking mode and written to until it returns
/// `EAGAIN`; the byte count at that point is the capacity a child gets before
/// its next write blocks. Measuring beats assuming: Linux ships 64 KB but the
/// value is tunable, and the deadlock this file guards against starts exactly
/// one byte past whatever it is here.
fn measured_pipe_buffer_bytes() -> usize {
    let mut fds = [0 as libc::c_int; 2];
    assert_eq!(
        unsafe { libc::pipe(fds.as_mut_ptr()) },
        0,
        "pipe() failed while measuring the buffer"
    );
    let (read_fd, write_fd) = (fds[0], fds[1]);

    let flags = unsafe { libc::fcntl(write_fd, libc::F_GETFL) };
    assert_ne!(flags, -1, "F_GETFL failed");
    assert_ne!(
        unsafe { libc::fcntl(write_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1,
        "F_SETFL O_NONBLOCK failed"
    );

    let chunk = [0u8; 1024];
    let mut written = 0usize;
    // A pipe that never fills would hang the suite, so stop well short of
    // any plausible capacity and let the assertion below report it.
    while written < 64 * 1024 * 1024 {
        let n =
            unsafe { libc::write(write_fd, chunk.as_ptr() as *const libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
        written += n as usize;
    }

    unsafe {
        libc::close(write_fd);
        libc::close(read_fd);
    }
    written
}

#[test]
fn the_pipe_buffer_is_measurable_and_plausible() {
    let measured = measured_pipe_buffer_bytes();
    assert!(
        (4 * 1024..64 * 1024 * 1024).contains(&measured),
        "measured pipe buffer {measured} is outside any plausible range, so \
         every size derived from it in this file is meaningless"
    );
}

/// A shell command that writes `bytes` bytes of filler to stdout and then
/// exits with `exit_code`.
fn emit_command(bytes: usize, exit_code: i32) -> String {
    format!("head -c {bytes} /dev/zero | tr '\\0' 'x'\nexit {exit_code}\n")
}

/// Just past the measured buffer: the smallest size that used to deadlock.
fn just_above_buffer() -> usize {
    measured_pipe_buffer_bytes() + 4096
}

/// Several megabytes, far past any buffer and far past the retention bound.
const SEVERAL_MEGABYTES: usize = 5 * 1024 * 1024;

// ---------------------------------------------------------------------------
// R18: gates
// ---------------------------------------------------------------------------

fn gate_template(bytes: usize, exit_code: i32) -> String {
    let command = emit_command(bytes, exit_code)
        .lines()
        .map(|l| format!("          {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"---
name: loud-gate
version: "1.0"
initial_state: check
states:
  check:
    gates:
      loud:
        type: command
        command: |
{command}
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Run the loud gate.

## done

All done.
"#
    )
}

/// Run the loud-gate workflow once and return the parsed `koto next` response.
fn run_loud_gate(bytes: usize, exit_code: i32) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "loud-gate", &gate_template(bytes, exit_code));

    let output = koto_cmd(dir.path())
        .args(["next", "loud-gate"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should exit 0 for a gate result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("next output should be JSON")
}

/// Assert a gate result was judged on the command's exit status rather than
/// reported as a timeout.
///
/// Before the reader threads landed, a gate emitting past the pipe buffer
/// blocked on write while koto blocked in `wait_timeout`, so this same
/// workflow burned the full timeout and came back `timed_out` with its output
/// discarded — whatever the command's real exit status was.
fn assert_gate_failed_not_timed_out(json: &serde_json::Value, expected_exit: i64) {
    let conditions = json["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array");
    assert_eq!(conditions.len(), 1, "response was {json}");
    assert_eq!(conditions[0]["name"].as_str(), Some("loud"));
    assert_eq!(
        conditions[0]["status"].as_str(),
        Some("failed"),
        "gate should be judged on its exit status, not reported as a timeout: {json}"
    );
    assert_eq!(conditions[0]["output"]["exit_code"], expected_exit);
}

#[test]
fn gate_just_above_the_pipe_buffer_is_judged_on_its_exit_status() {
    let bytes = just_above_buffer();

    let passed = run_loud_gate(bytes, 0);
    assert_eq!(
        passed["state"].as_str(),
        Some("done"),
        "a passing loud gate should advance the workflow: {passed}"
    );

    let failed = run_loud_gate(bytes, 3);
    assert_gate_failed_not_timed_out(&failed, 3);
}

#[test]
fn gate_at_several_megabytes_is_judged_on_its_exit_status() {
    let passed = run_loud_gate(SEVERAL_MEGABYTES, 0);
    assert_eq!(
        passed["state"].as_str(),
        Some("done"),
        "a multi-megabyte passing gate should advance the workflow: {passed}"
    );

    let failed = run_loud_gate(SEVERAL_MEGABYTES, 3);
    assert_gate_failed_not_timed_out(&failed, 3);
}

/// R19 for gates. Gate evidence carries the exit status and no stdout, so the
/// bound and its marking sit where the gate evaluator meets the runner: the
/// evaluator's `CommandOutput` keeps the first `MAX_ACTION_OUTPUT_BYTES` and
/// says it dropped the rest. Before this bound covered gate commands, a gate
/// retained its entire output in memory.
#[test]
fn gate_command_output_is_bounded_and_flagged_as_truncated() {
    let dir = TempDir::new().unwrap();
    let out = run_shell_command(&emit_command(SEVERAL_MEGABYTES, 0), dir.path(), 30);

    assert_eq!(out.exit_code, 0);
    assert_eq!(out.failure_kind, None);
    assert_eq!(
        out.stdout.len(),
        MAX_ACTION_OUTPUT_BYTES,
        "the gate runner should retain exactly the bound"
    );
    assert!(
        out.truncated,
        "dropping bytes should be reported, not silent"
    );
}

/// The gate evaluator itself, not just the runner underneath it: a gate whose
/// command outruns the pipe buffer reaches `Passed`/`Failed` rather than
/// `TimedOut`.
#[test]
fn gate_evaluator_does_not_report_a_loud_gate_as_timed_out() {
    let dir = TempDir::new().unwrap();
    let bytes = just_above_buffer();

    for (exit_code, expected) in [(0, GateOutcome::Passed), (3, GateOutcome::Failed)] {
        let mut gates = BTreeMap::new();
        gates.insert(
            "loud".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: emit_command(bytes, exit_code),
                timeout: 30,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(
            results["loud"].outcome, expected,
            "exit {exit_code} at {bytes} bytes should map to {expected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// R18/R19/R25: default_action
// ---------------------------------------------------------------------------

fn action_template(bytes: usize) -> String {
    let command = emit_command(bytes, 0)
        .lines()
        .map(|l| format!("        {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"---
name: loud-action
version: "1.0"
initial_state: emit
states:
  emit:
    default_action:
      command: |
{command}
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## emit

Emit the output.

## done

All done.
"#
    )
}

/// Run the loud-action workflow once, returning the response and the
/// directory so the event log can be read from it.
fn run_loud_action(dir: &Path, bytes: usize) -> serde_json::Value {
    init_workflow(dir, "loud-action", &action_template(bytes));

    let output = koto_cmd(dir)
        .args(["next", "loud-action"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("next output should be JSON")
}

/// R18 for actions, at both sizes. The two channels that carry an action's
/// output at this point in the sequence are the confirmation response's
/// `action_output` and the `default_action_executed` event; delivery through a
/// declared capture name does not exist yet.
///
/// Before the reader threads landed, both sizes came back `exit_code: -1` with
/// empty stdout after burning the full timeout.
fn assert_action_output_is_delivered(dir: &Path, json: &serde_json::Value) {
    assert_eq!(
        json["action"].as_str(),
        Some("confirm"),
        "the action should have stopped for confirmation, not failed: {json}"
    );
    assert_eq!(
        json["action_output"]["exit_code"], 0,
        "exit -1 here means the command was killed rather than run: {json}"
    );

    let stdout = json["action_output"]["stdout"].as_str().unwrap();
    assert!(
        stdout.starts_with("xxxxxxxx"),
        "the command's own output should reach the response"
    );

    let event = action_event(dir, "loud-action");
    assert_eq!(event["payload"]["exit_code"], 0);
    assert!(
        event["payload"]["stdout"]
            .as_str()
            .unwrap()
            .starts_with("xxxxxxxx"),
        "the command's own output should reach the event log"
    );
}

#[test]
fn action_just_above_the_pipe_buffer_delivers_its_output() {
    let dir = TempDir::new().unwrap();
    let json = run_loud_action(dir.path(), just_above_buffer());
    assert_action_output_is_delivered(dir.path(), &json);
}

#[test]
fn action_at_several_megabytes_delivers_its_output() {
    let dir = TempDir::new().unwrap();
    let json = run_loud_action(dir.path(), SEVERAL_MEGABYTES);
    assert_action_output_is_delivered(dir.path(), &json);
}

/// R19 and R25 for actions: over-bound output is cut at the stated bound and
/// says so, in the response and in the event log alike.
#[test]
fn action_output_above_the_bound_is_marked_truncated_in_both_channels() {
    let dir = TempDir::new().unwrap();
    let json = run_loud_action(dir.path(), MAX_ACTION_OUTPUT_BYTES + 4096);

    let stdout = json["action_output"]["stdout"].as_str().unwrap();
    assert!(
        stdout.ends_with(TRUNCATION_NOTE),
        "the response should say the output was cut, not just be short; \
         it ended with {:?}",
        &stdout[stdout.len().saturating_sub(40)..]
    );
    assert_eq!(
        stdout.len(),
        MAX_ACTION_OUTPUT_BYTES + TRUNCATION_NOTE.len() + 1,
        "the cut should land at the stated bound"
    );

    let event = action_event(dir.path(), "loud-action");
    assert_eq!(
        event["payload"]["truncated"], true,
        "the event log should carry the truncation flag: {event}"
    );
    assert!(event["payload"]["stdout"]
        .as_str()
        .unwrap()
        .ends_with(TRUNCATION_NOTE));
}

/// The complement: output that fits is not marked, so the note means
/// something. A workflow whose action stays under the bound writes the same
/// event it always did.
#[test]
fn action_output_below_the_bound_is_not_marked_truncated() {
    let dir = TempDir::new().unwrap();
    let json = run_loud_action(dir.path(), MAX_ACTION_OUTPUT_BYTES - 4096);

    let stdout = json["action_output"]["stdout"].as_str().unwrap();
    assert!(!stdout.contains(TRUNCATION_NOTE));
    assert_eq!(stdout.len(), MAX_ACTION_OUTPUT_BYTES - 4096);

    let event = action_event(dir.path(), "loud-action");
    assert_eq!(event["payload"]["truncated"], false);
}

/// A genuine timeout hands back what the command managed to produce. The old
/// timeout path returned two empty strings and the agent learned nothing about
/// how far the command got.
#[test]
fn a_timed_out_command_still_delivers_the_output_it_produced() {
    let dir = TempDir::new().unwrap();
    let out = run_shell_command("echo started; echo warned >&2; sleep 60", dir.path(), 1);

    assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
    assert_eq!(out.stdout.trim(), "started");
    assert!(out.stderr.contains("warned"));
    assert!(out.stderr.contains("timed out"));
}

// ---------------------------------------------------------------------------
// R18/R20: koto invoking koto
// ---------------------------------------------------------------------------

/// The two defects meeting each other, which is how they were found.
///
/// The action creates the condition that used to reprint on every invocation —
/// old-layout sessions whose names are already taken at the flat level — and
/// then calls `koto` twice. The first nested call is the one that migrates, so
/// its notice goes down the pipe koto is draining, well past the buffer. The
/// second call writes its stderr to a file, which is where this test reads the
/// answer to "does the condition come back".
///
/// Before the reader threads, the first nested call filled the pipe and the
/// action burned its whole timeout for an `exit_code: -1`. Before the
/// quarantine fix, the migration never completed, so the second call reported
/// the same conflicts all over again.
#[test]
fn nested_koto_under_the_repeated_notice_condition_completes_and_settles() {
    let dir = TempDir::new().unwrap();
    let home_sessions = dir.path().join(".koto").join("sessions");

    // Enough colliding sessions that the notice clears the measured buffer
    // several times over: the shortest form of the message is around 90 bytes.
    let collisions = (measured_pipe_buffer_bytes() / 64).max(256);

    let bin_dir = koto_binary().parent().unwrap().to_path_buf();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Migration runs against `~/.koto/sessions`, not against the base
    // `KOTO_SESSIONS_BASE` names, so the nested calls are pointed at HOME.
    let script = format!(
        r#"        set -e
        unset KOTO_SESSIONS_BASE
        base="$HOME/.koto/sessions"
        names=""
        i=0
        while [ $i -lt {collisions} ]; do
          names="$names collide-session-$i"
          i=$((i + 1))
        done
        mkdir -p "$base/abcdef1234567890"
        (cd "$base" && mkdir -p $names)
        (cd "$base/abcdef1234567890" && mkdir -p $names)
        koto session list >/dev/null
        koto session list >/dev/null 2>"{second_err}"
        echo "second-run-bytes=$(wc -c <"{second_err}")"
"#,
        collisions = collisions,
        second_err = dir.path().join("second.err").display(),
    );

    let template = format!(
        r#"---
name: nested-koto
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: |
{script}
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Call koto from inside koto.

## done

All done.
"#
    );

    init_workflow(dir.path(), "nested-koto", &template);

    let output = koto_cmd(dir.path())
        .env("PATH", &path)
        .args(["next", "nested-koto"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be JSON");

    // No deadlock and no false timeout: the action ran to completion.
    assert_eq!(
        json["action"].as_str(),
        Some("confirm"),
        "the nested call should have completed: {json}"
    );
    assert_eq!(
        json["action_output"]["exit_code"], 0,
        "exit -1 here is the pipe-buffer deadlock timing out: {json}"
    );

    let action_stdout = json["action_output"]["stdout"].as_str().unwrap();
    assert!(
        action_stdout.contains("second-run-bytes=0"),
        "the condition should be reported once and then be gone, but the \
         second nested call still wrote to stderr: {action_stdout}"
    );

    // The first nested call really did face the condition, so the completion
    // above is not a workflow that quietly did nothing.
    let action_stderr = json["action_output"]["stderr"].as_str().unwrap();
    assert!(
        action_stderr.contains("migration conflict"),
        "the first nested call should have reported the conflicts: {}",
        &action_stderr[..action_stderr.len().min(400)]
    );

    // Nothing was deleted to make the condition go away.
    let quarantined = home_sessions
        .join(".migration-conflicts")
        .join("abcdef1234567890")
        .join("collide-session-0");
    assert!(
        quarantined.exists(),
        "the colliding copies should be preserved, not removed"
    );
    assert!(
        !home_sessions.join("abcdef1234567890").exists(),
        "the old-layout directory should have drained"
    );
}
