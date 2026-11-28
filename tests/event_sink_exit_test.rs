//! Sending on the way out, and what a sink that fails costs a command,
//! driven through the koto binary against the stub sink of
//! `tests/support/event_sink.rs`.
//!
//! Every exit goes through one helper that runs the registered exit hooks,
//! and the flush of queued session records is the first hook registered, so
//! it runs last. A command that ends with records still queued sends them
//! before it exits, whichever way it ends. A sink that hangs is killed with
//! its whole process group once the flush's deadline passes, a sink that
//! refuses the records is called once, and in both cases the command's own
//! result is what it would have been with no sink.

#![cfg(unix)]

#[path = "support/event_sink.rs"]
mod event_sink;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use event_sink::{sent, SinkHome};
use serde_json::Value;
use std::time::{Duration, Instant};

const TEMPLATE: &str = r#"---
name: sink-exit
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes", "other"]
    transitions:
      - target: done
        when:
          go: "yes"
      - target: other_end
        when:
          go: "other"
  done:
    terminal: true
  other_end:
    terminal: true
---

## start

Start.

## done

Done.

## other_end

Other end.
"#;

struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    took: Duration,
}

fn run(home: &SinkHome, args: &[&str]) -> Ran {
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let started = Instant::now();
    let out = home.koto().args(args).output().unwrap();
    Ran {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        took: started.elapsed(),
    }
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect()
}

/// Whether a process with this pid still exists.
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn exit_helper_flushes_queued_records() {
    let (home, stub) = SinkHome::with_stub("exit");

    // A command that returns normally: `main` ends in the exit helper.
    let init = run(&home, &["init", "wf", "--template", "template.md"]);
    assert_eq!(init.code, Some(0), "{}", init.stderr);
    let calls = stub.calls();
    assert_eq!(calls.len(), 1, "init made {} calls", calls.len());
    assert_eq!(
        kinds(&sent(&calls)),
        vec!["session_started", "state_entered"]
    );

    // A tick that prints a directive and returns.
    let next = run(&home, &["next", "wf"]);
    assert_eq!(next.code, Some(0), "{}", next.stderr);
    let calls = stub.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(kinds(&sent(&calls[1..])), vec!["response"]);

    // A command that fails and exits non-zero sends nothing new, since it
    // journaled nothing, and makes no call for an empty queue.
    let bad = run(&home, &["next", "wf", "--with-data", r#"{"go":"nope"}"#]);
    assert_ne!(bad.code, Some(0), "{}", bad.stdout);
    assert_eq!(stub.call_count(), 2, "an empty queue made a call");

    // The terminal tick sends its records, the `terminal` among them,
    // before it removes the session, and then ends the process from inside
    // the command, through the exit helper, with nothing left to send.
    let done = run(&home, &["next", "wf", "--with-data", r#"{"go":"other"}"#]);
    assert!(done.stdout.contains("\"done\""), "{}", done.stdout);
    let calls = stub.calls();
    assert_eq!(calls.len(), 3);
    let last = sent(&calls[2..]);
    assert_eq!(kinds(&last).last(), Some(&"terminal"), "{:?}", kinds(&last));
    assert_eq!(last.last().unwrap()["koto.terminal"], "other_end");

    // Across the session, the sink got exactly what the journal holds.
    let journal = home.journal();
    assert_eq!(kinds(&sent(&calls)), kinds(&journal));
    assert!(calls.iter().all(|c| c.done));
}

#[test]
fn a_sink_that_refuses_is_called_once_and_the_command_is_unchanged() {
    let (home, stub) = SinkHome::with_stub("refuse");
    stub.exit_codes(&[3]);
    let init = run(&home, &["init", "wf", "--template", "template.md"]);
    assert_eq!(init.code, Some(0), "{}", init.stderr);
    assert!(
        init.stderr.is_empty(),
        "the sink's failure was reported: {}",
        init.stderr
    );
    assert_eq!(stub.call_count(), 1);
    // Both records of the refused call are counted for the session.
    assert_eq!(home.dropped("wf"), 2);
    // The journal is written as with no sink.
    assert_eq!(
        kinds(&home.journal()),
        vec!["session_started", "state_entered"]
    );

    // The count grows with each command the sink refuses.
    let next = run(&home, &["next", "wf", "--no-cleanup"]);
    assert_eq!(next.code, Some(0), "{}", next.stderr);
    assert_eq!(stub.call_count(), 2);
    assert_eq!(home.dropped("wf"), 3);
}

#[test]
fn a_sink_that_says_sending_is_off_is_called_once_and_nothing_is_counted() {
    let (home, stub) = SinkHome::with_stub("off-at-sink");
    stub.exit_codes(&[2]);
    let init = run(&home, &["init", "wf", "--template", "template.md"]);
    assert_eq!(init.code, Some(0), "{}", init.stderr);
    assert_eq!(stub.call_count(), 1);
    assert_eq!(home.dropped("wf"), 0);
    assert!(!home.session_dir("wf").join("telemetry.json").exists());
}

#[test]
fn hung_sink_killed_with_group() {
    let (home, stub) = SinkHome::with_stub("hung");
    // The stub starts a child of its own and then sleeps far past any
    // deadline.
    stub.set("fork", "yes");
    stub.sleeps(&["120"]);
    let init = run(&home, &["init", "wf", "--template", "template.md"]);

    // The command itself is unharmed: same exit, same output, no word of
    // the sink.
    assert_eq!(init.code, Some(0), "{}", init.stderr);
    assert!(init.stderr.is_empty(), "{}", init.stderr);
    // The flush is bounded at 3.0 s; the rest is the command and the test
    // host.
    assert!(
        init.took >= Duration::from_millis(2850),
        "the sink wasn't given its time: {:?}",
        init.took
    );
    assert!(
        init.took < Duration::from_secs(20),
        "the command waited on the sink: {:?}",
        init.took
    );

    let calls = stub.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert!(call.read, "the sink never read its input");
    assert!(!call.done, "the sink ran to its own exit");

    // Nothing of the sink's group is left: not the sink, and not the
    // child it started. A killed process can linger as a zombie for a
    // moment until it is reaped, so this is polled briefly.
    let child = call.child_pid.expect("the stub recorded no child");
    let until = Instant::now() + Duration::from_secs(5);
    while alive(child) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(child), "the sink's child {child} outlived the kill");

    // It counts as a failed call: the chunk's records are counted for the
    // session.
    assert_eq!(home.dropped("wf"), call.lines.len() as u64);
    assert_eq!(home.dropped("wf"), 2);
}
