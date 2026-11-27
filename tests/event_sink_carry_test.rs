//! Carrying dropped-record counts to the event sink, driven through the
//! koto binary against the stub sink of `tests/support/event_sink.rs`.
//!
//! A record that was queued and not delivered is counted in its session's
//! `telemetry.json`. The count leaves with the next call that is delivered,
//! as `--carry N`, and is zeroed as it is taken, under a lock, so two
//! processes working on one session never carry the same count. A session
//! removed while it still holds a count hands it to its store's orphan
//! count, which a later flush from any session of that store carries out.

#![cfg(unix)]

#[path = "support/event_sink.rs"]
mod event_sink;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use event_sink::{sent, SinkHome, Stub};
use serde_json::Value;

const TEMPLATE: &str = r#"---
name: sink-carry
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

fn ok(home: &SinkHome, args: &[&str]) -> String {
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let out = home.koto().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "koto {args:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The count in `path`, 0 when the file isn't there.
fn count_at(path: &std::path::Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v["dropped"].as_u64())
        .unwrap_or(0)
}

fn orphans(home: &SinkHome) -> u64 {
    count_at(
        &home
            .koto_home()
            .join("sessions")
            .join("_telemetry_orphans.json"),
    )
}

/// Create session `name` while the sink refuses, so its two first records
/// are counted as dropped, then let the sink accept again.
fn session_with_two_dropped(home: &SinkHome, stub: &Stub, name: &str) {
    let before = stub.call_count();
    // The call this command makes exits 3; every earlier call keeps the
    // code it had.
    let mut codes = vec![0; before];
    codes.push(3);
    codes.push(0);
    stub.exit_codes(&codes);
    ok(home, &["init", name, "--template", "template.md"]);
    assert_eq!(stub.call_count(), before + 1);
    assert_eq!(
        home.dropped(name),
        2,
        "the refused call's records weren't counted"
    );
}

#[test]
fn carry_sent_and_zeroed() {
    let (home, stub) = SinkHome::with_stub("carry");
    session_with_two_dropped(&home, &stub, "wf");
    // The refused call carried nothing: there was nothing to carry yet.
    assert_eq!(stub.calls()[0].carry(), None);

    // The next command's call is delivered, and takes the count with it.
    ok(&home, &["next", "wf", "--no-cleanup"]);
    let calls = stub.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].argv,
        vec!["send", "--scope", "koto-journal", "--carry", "2"]
    );
    assert_eq!(home.dropped("wf"), 0, "the carried count wasn't zeroed");

    // It went out once: the command after that carries nothing.
    ok(&home, &["next", "wf", "--no-cleanup"]);
    let calls = stub.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].carry(), None);
    assert_eq!(home.dropped("wf"), 0);
}

#[test]
fn a_refused_carry_comes_back_and_goes_out_later() {
    let (home, stub) = SinkHome::with_stub("carry-back");
    session_with_two_dropped(&home, &stub, "wf");
    // Refuse the next call too: it carries the 2, and they come back with
    // its own record on top.
    stub.exit_codes(&[3, 3, 0]);
    ok(&home, &["next", "wf", "--no-cleanup"]);
    let calls = stub.calls();
    assert_eq!(calls[1].carry(), Some(2));
    assert_eq!(home.dropped("wf"), 2 + calls[1].lines.len() as u64);
    let owed = home.dropped("wf");

    ok(&home, &["next", "wf", "--no-cleanup"]);
    assert_eq!(stub.calls()[2].carry(), Some(owed));
    assert_eq!(home.dropped("wf"), 0);
}

#[test]
fn concurrent_processes_never_double_carry() {
    const PROCESSES: usize = 8;
    const ROUNDS: u64 = 4;
    let (home, stub) = SinkHome::with_stub("carry-race");
    ok(&home, &["init", "wf", "--template", "template.md"]);
    let count_file = home.session_dir("wf").join("telemetry.json");

    for round in 1..=ROUNDS {
        // A count left by earlier, failed flushes.
        let seeded = 1000 * round + 7;
        std::fs::write(&count_file, format!("{{\"dropped\":{seeded}}}")).unwrap();
        let before = stub.call_count();

        // Several commands on the one session at once. Each prints the
        // state's directive, journals a `response` and so flushes.
        std::thread::scope(|scope| {
            for _ in 0..PROCESSES {
                scope.spawn(|| {
                    let out = home.koto().args(["status", "wf"]).output().unwrap();
                    assert!(
                        out.status.success(),
                        "{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                });
            }
        });

        // What must hold however the processes interleaved, and however
        // slow the host was: no count is handed to two calls. Each command
        // queues one record and makes one call. A call that ran to its end
        // was posted, and what it carried stays out. A call the host was
        // too slow to finish in time is killed, which is not a failure of
        // this test: its command puts back what the call carried and
        // counts its own one record. So the carries of the finished calls,
        // plus what is left in the file, can't come to more than the
        // seeded count plus one for each command whose call didn't finish.
        // Two processes carrying the same count would push them over.
        let calls = stub.calls();
        let finished: Vec<_> = calls[before..].iter().filter(|c| c.done).collect();
        assert!(finished.len() <= PROCESSES);
        let unfinished = (PROCESSES - finished.len()) as u64;
        let carried: u64 = finished.iter().filter_map(|c| c.carry()).sum();
        let left = home.dropped("wf");
        let detail = format!(
            "round {round}: {} of {PROCESSES} calls finished, carrying {:?}, and the file holds {left}",
            finished.len(),
            finished.iter().map(|c| c.carry()).collect::<Vec<_>>()
        );
        assert!(carried + left <= seeded + unfinished, "{detail}");
        // On a host that kept up, nothing was lost either: the count went
        // out exactly once, or a process that couldn't get the lock in
        // time left it in the file for the next flush.
        if unfinished == 0 {
            assert_eq!(carried + left, seeded, "{detail}");
        }
    }
}

#[test]
fn a_removed_sessions_count_is_carried_by_a_later_flush() {
    let (home, stub) = SinkHome::with_stub("carry-orphan");
    session_with_two_dropped(&home, &stub, "gone");

    // Removing the session moves its count to the store's orphans.
    ok(&home, &["session", "cleanup", "gone"]);
    assert!(!home.session_dir("gone").exists());
    assert_eq!(orphans(&home), 2);
    let after_cleanup = stub.call_count();

    // A flush from another session of the same store carries it out.
    ok(&home, &["init", "other", "--template", "template.md"]);
    let calls = stub.calls();
    assert_eq!(calls.len(), after_cleanup + 1);
    let call = calls.last().unwrap();
    assert_eq!(call.carry(), Some(2));
    assert!(sent(std::slice::from_ref(call))
        .iter()
        .all(|r| r["session"] == "other"));
    assert_eq!(orphans(&home), 0);
    assert_eq!(home.dropped("other"), 0);
}

#[test]
fn the_terminal_ticks_own_flush_carries_the_count_of_the_session_it_removes() {
    let (home, stub) = SinkHome::with_stub("carry-terminal");
    session_with_two_dropped(&home, &stub, "wf");

    // The tick that reaches the terminal state removes the session, and
    // its records are sent as the process exits, after the removal. The
    // session's count has moved to the orphans by then, and that same
    // flush carries it.
    let out = ok(&home, &["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert!(out.contains("\"done\""), "{out}");
    assert!(
        !home.session_dir("wf").exists(),
        "the session wasn't removed"
    );

    let calls = stub.calls();
    let call = calls.last().unwrap();
    assert_eq!(call.carry(), Some(2));
    let kinds: Vec<String> = sent(std::slice::from_ref(call))
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("terminal"),
        "{kinds:?}"
    );
    assert_eq!(orphans(&home), 0);
}

#[test]
fn a_host_that_drops_nothing_gains_no_count_files() {
    let (home, stub) = SinkHome::with_stub("carry-none");
    ok(&home, &["init", "wf", "--template", "template.md"]);
    ok(&home, &["next", "wf", "--no-cleanup"]);
    ok(&home, &["init", "other", "--template", "template.md"]);
    ok(&home, &["session", "cleanup", "other"]);
    assert!(stub.calls().iter().all(|c| c.carry().is_none()));
    let names = |dir: &std::path::Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.contains("telemetry"))
            .collect()
    };
    assert!(names(&home.session_dir("wf")).is_empty());
    assert!(names(&home.koto_home().join("sessions")).is_empty());
}
