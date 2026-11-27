//! The flush points inside a command, driven through the koto binary
//! against the stub sink of `tests/support/event_sink.rs`.
//!
//! A command doesn't hold its session records until it exits. It sends what
//! is queued before it starts a gate's or an action's command, before it
//! calls a decider, before it waits, and on the terminal tick before the
//! session directory can be removed. Two of those are reached here with
//! records queued by the command itself: the command a gate runs, and the
//! terminal tick. The decider and wait sites have a test each beside their
//! code, since a command reaches them with an empty queue unless something
//! was journaled just before.

#![cfg(unix)]

#[path = "support/event_sink.rs"]
mod event_sink;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use event_sink::{sent, SinkHome};
use serde_json::Value;

/// `start` takes one answer and moves to `check`, whose gate runs
/// `probe.sh`; a passing gate finishes.
const GATED: &str = r#"---
name: sink-flush
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: check
        when:
          go: "yes"
  check:
    gates:
      probe:
        type: command
        command: sh ./probe.sh
    transitions:
      - target: done
        when:
          gates.probe.exit_code: 0
  done:
    terminal: true
---

## start

Start.

## check

Check.

## done

Done.
"#;

/// `start` takes one answer and finishes.
const PLAIN: &str = r#"---
name: sink-terminal
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Start.

## done

Done.
"#;

fn kinds(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap_or("?").to_string())
        .collect()
}

fn koto(home: &SinkHome, args: &[&str]) -> String {
    let out = home.koto().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "koto {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn action_flush_precedes_gate_spawn() {
    let (home, stub) = SinkHome::with_stub("flush-gate");
    std::fs::write(home.work.join("template.md"), GATED).unwrap();
    // The gate's command copies what the sink had received in full, from
    // calls that had already ended, at the moment the command started.
    let ctl = home.home.parent().unwrap().join("sink-ctl");
    std::fs::write(
        home.work.join("probe.sh"),
        format!(
            "for call in '{ctl}'/calls/*; do\n\
             \x20   if [ -f \"$call/done\" ]; then cat \"$call/stdin\"; fi\n\
             done >seen_at_start\n\
             exit 0\n",
            ctl = ctl.display()
        ),
    )
    .unwrap();

    koto(&home, &["init", "wf", "--template", "template.md"]);
    let after_init = stub.call_count();
    assert_eq!(after_init, 1);
    // The call the tick makes before the gate takes two seconds. A flush
    // inside the command's time limit would show up in the gate's duration.
    stub.sleeps(&["0", "2", "0"]);
    koto(
        &home,
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"go":"yes"}"#,
            "--no-cleanup",
        ],
    );

    // Everything the tick journaled before it ran the gate had reached the
    // sink, in a call that had ended, when the gate's command started.
    let calls = stub.calls();
    assert!(calls.len() > after_init, "the tick made no call");
    let before_gate = sent(&calls[after_init..after_init + 1]);
    assert!(!before_gate.is_empty());
    let seen: Vec<Value> = std::fs::read_to_string(home.work.join("seen_at_start"))
        .expect("the gate's command ran")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let first = sent(&calls[..after_init]);
    assert_eq!(
        kinds(&seen),
        kinds(&[first.clone(), before_gate.clone()].concat()),
        "the gate's command started before the flush had returned"
    );
    // The journal's order agrees: those records are the ones written
    // before the gate's own.
    let journal = home.journal();
    assert_eq!(
        kinds(&journal[..seen.len()]),
        kinds(&seen),
        "the flush sent something other than what led up to the gate"
    );

    // The command's time limit opened after the flush returned: the gate
    // took nothing like the two seconds the sink's call did.
    let gate = journal
        .iter()
        .find(|r| r.get("koto.check.duration_ms").is_some())
        .unwrap_or_else(|| panic!("no gate record in {:?}", kinds(&journal)));
    let took = gate["koto.check.duration_ms"].as_u64().unwrap();
    assert!(took < 2000, "the gate's {took} ms includes the flush");

    // Across the session the sink got exactly what the journal holds.
    assert_eq!(kinds(&sent(&stub.calls())), kinds(&journal));
}

#[test]
fn terminal_flush_precedes_removal() {
    let (home, stub) = SinkHome::with_stub("flush-terminal");
    std::fs::write(home.work.join("template.md"), PLAIN).unwrap();
    koto(&home, &["init", "wf", "--template", "template.md"]);
    let dir = home.session_dir("wf");
    assert!(dir.is_dir());
    // Every call from here on notes whether the session directory exists.
    stub.set("require_dir", dir.to_str().unwrap());
    let after_init = stub.call_count();

    // The terminal tick, with cleanup: it removes the session.
    koto(&home, &["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert!(!dir.exists(), "the terminal tick removed the session");

    let calls = stub.calls();
    let terminal = calls[after_init..]
        .iter()
        .find(|c| kinds(&sent(std::slice::from_ref(c))).contains(&"terminal".to_string()))
        .expect("a call carried the terminal record");
    assert_eq!(
        terminal.dir_exists,
        Some(true),
        "the terminal record was sent after the session directory was removed"
    );
    assert!(terminal.done);
}
