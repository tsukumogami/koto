//! Every event on a session's state log lands on its own line, whoever
//! writes it and however the writer ends (koto#419).
//!
//! The defect: an append wrote its line and the line's newline as two
//! writes. A writer killed between them (a timed-out default action's
//! process group gets SIGKILL) left an event with no newline, and the next
//! append landed on the same line. From then on every write to the session
//! failed with `failed to parse last event line: trailing characters`.
//!
//! These tests run real `koto` processes against one session: many writers
//! at once with events of very different sizes, writers killed partway
//! through, a log already holding a merged line, and the background
//! children of a timed-out command still trying to write.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::Duration;

use assert_fs::TempDir;
use serde_json::Value;

/// A session that stays in `work` until it is told otherwise, so writes to
/// it never race a transition.
const TEMPLATE: &str = r#"---
name: appends
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: finished
        when:
          go: done
  finished:
    terminal: true
---

## work

Work.

## finished

Done.
"#;

const NOTE: &str = "shared note";

fn sessions_base(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

fn log_path(dir: &Path) -> PathBuf {
    sessions_base(dir).join("s").join("koto-s.state.jsonl")
}

fn koto(dir: &Path) -> StdCommand {
    let mut cmd = koto_command::koto_std();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    // Keep the user's ~/.koto/config.toml (which may select the cloud
    // backend) out of the test.
    cmd.env("HOME", dir);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    cmd.env_remove("KOTO_TICK_SESSION");
    // Every command runs as `s`'s own agent session, whose anchor holds it,
    // so a `context get` is an own read and appends to `s`'s log: a read
    // with no anchor would go to the host's read spool instead.
    cmd.env("KOTO_ANCHOR", OWN_ANCHOR);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

/// The anchor of the agent session that owns `s`.
const OWN_ANCHOR: &str = "anchor-own";

fn run_ok(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = koto(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "koto {args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A session `s` with a `note` key, plus the files the writers read.
fn setup(dir: &Path) {
    std::fs::create_dir_all(sessions_base(dir)).unwrap();
    run_ok(
        dir,
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "own",
            "--source",
            "startup",
        ],
    );
    let template = dir.join("appends.md");
    std::fs::write(&template, TEMPLATE).unwrap();
    run_ok(
        dir,
        &["init", "s", "--template", template.to_str().unwrap()],
    );
    std::fs::write(dir.join("note.txt"), NOTE).unwrap();
    run_ok(
        dir,
        &["context", "add", "s", "note", "--from-file", "note.txt"],
    );
    // A decision whose event is far larger than a page and than PIPE_BUF.
    let decision = serde_json::json!({
        "choice": "the large one",
        "rationale": "r".repeat(20_000),
        "alternatives_considered": ["x".repeat(2_000)],
    });
    std::fs::write(dir.join("decision.json"), decision.to_string()).unwrap();
}

/// One writer of each kind, by index: a context write (small event), a
/// context read (small event, written by a reader), and a decision (an event
/// of more than 20 KB).
fn writer(dir: &Path, i: usize) -> StdCommand {
    let key = format!("k{i}");
    let args: Vec<&str> = match i % 3 {
        0 => vec!["context", "add", "s", &key, "--from-file", "note.txt"],
        1 => vec!["context", "get", "s", "note"],
        _ => vec!["decisions", "record", "s", "--with-data", "@decision.json"],
    };
    let mut cmd = koto(dir);
    cmd.args(&args);
    cmd
}

/// Under coverage, an instrumented koto writes its profile on exit, and one
/// SIGKILLed while writing it leaves a corrupt file that fails the whole
/// report's merge. A process this file may kill sends its profile nowhere.
const NO_PROFILE: (&str, &str) = ("LLVM_PROFILE_FILE", "/dev/null");

/// What each line after the header holds: the number of JSON values on it,
/// or `None` when it doesn't parse through to its end.
fn values_per_line(content: &str) -> Vec<(String, Option<usize>)> {
    content
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut n = 0;
            for v in serde_json::Deserializer::from_str(l).into_iter::<Value>() {
                if v.is_err() {
                    return (l.to_string(), None);
                }
                n += 1;
            }
            (l.to_string(), Some(n))
        })
        .collect()
}

/// Every line after the header holds exactly one event and seqs run 1..=n.
/// Returns the events.
fn assert_one_event_per_line(path: &Path) -> Vec<Value> {
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.ends_with('\n'), "the log must end in a newline");
    for (line, n) in values_per_line(&content) {
        assert_eq!(
            n,
            Some(1),
            "a line holding other than one event: {line:.300}"
        );
    }
    let events: Vec<Value> = content
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    events
}

fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["type"] == kind).count()
}

/// Sixty writers at once, a third of them writing 20 KB events, in three
/// rounds: no line ever holds more than one event.
#[test]
fn parallel_writers_of_different_sizes_never_share_a_line() {
    const WRITERS: usize = 60;
    const ROUNDS: usize = 3;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);
    let before = assert_one_event_per_line(&log_path(dir));

    for _ in 0..ROUNDS {
        let children: Vec<Child> = (0..WRITERS)
            .map(|i| writer(dir, i).spawn().unwrap())
            .collect();
        for child in children {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "a writer failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    let events = assert_one_event_per_line(&log_path(dir));
    let per_kind = WRITERS / 3 * ROUNDS;
    assert_eq!(
        count(&events, "context_added") - count(&before, "context_added"),
        per_kind
    );
    assert_eq!(
        count(&events, "context_read") - count(&before, "context_read"),
        per_kind
    );
    assert_eq!(count(&events, "decision_recorded"), per_kind);
}

/// Writers killed at staggered moments, many of them partway through their
/// run, never leave a line holding two events. Unless a kill cut the last
/// line short, which ends the test (a truncated tail behaves as it always
/// has), the session then takes a write.
#[test]
fn writers_killed_mid_append_never_merge_two_events() {
    const WRITERS: usize = 45;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);

    for round in 0..4u64 {
        let mut children: Vec<Child> = (0..WRITERS)
            .map(|i| {
                writer(dir, i)
                    .env(NO_PROFILE.0, NO_PROFILE.1)
                    .spawn()
                    .unwrap()
            })
            .collect();
        for (i, child) in children.iter_mut().enumerate() {
            // Staggered kills spread over the writers' run: some before
            // they open the log, some while they hold the lock, some after.
            std::thread::sleep(Duration::from_micros(
                ((i as u64 * 7 + round * 13) % 23) * 150,
            ));
            let _ = child.kill();
        }
        for child in children {
            let _ = child.wait_with_output();
        }
        let content = std::fs::read_to_string(log_path(dir)).unwrap();
        let lines = values_per_line(&content);
        for (n, (line, values)) in lines.iter().enumerate() {
            let last = n == lines.len() - 1;
            match values {
                Some(1) => {}
                // A write cut short by the kill can only ever be the last line.
                None if last => {}
                other => panic!("round {round}: line holds {other:?} values: {line:.300}"),
            }
        }
        if lines.last().is_some_and(|(_, v)| v.is_none()) {
            // A torn tail behaves as it always has; nothing more to check.
            return;
        }
    }

    run_ok(
        dir,
        &["context", "add", "s", "after", "--from-file", "note.txt"],
    );
    let events = assert_one_event_per_line(&log_path(dir));
    assert!(events.iter().any(|e| e["payload"]["key"] == "after"));
}

/// The state a writer killed between its line and its newline leaves, which
/// is how the merge in koto#419 began: the log ends in a complete event with
/// no newline. Twenty writers then append at once. The first of them must
/// start a new line rather than continue that one.
#[test]
fn an_event_left_without_its_newline_still_gets_a_line_of_its_own() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);
    let log = log_path(dir);
    let content = std::fs::read_to_string(&log).unwrap();
    std::fs::write(&log, content.strip_suffix('\n').unwrap()).unwrap();

    let children: Vec<Child> = (0..20).map(|i| writer(dir, i).spawn().unwrap()).collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "a writer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let events = assert_one_event_per_line(&log);
    assert_eq!(events.len(), content.lines().count() - 1 + 20);
}

/// The log as koto#419 found it: the last line holds a complete event
/// followed by another, with no newline between them. A write to the
/// session used to fail with `persistence_error`; now it splits the line,
/// keeps the log as it was, says so on stderr, and lands its own event.
#[test]
fn a_merged_last_line_is_split_by_the_next_write() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);
    run_ok(dir, &["context", "get", "s", "note"]);
    let log = log_path(dir);
    let content = std::fs::read_to_string(&log).unwrap();
    let body = content.strip_suffix('\n').unwrap();
    let cut = body.rfind('\n').unwrap();
    let merged = format!("{}{}\n", &body[..cut], &body[cut + 1..]);
    std::fs::write(&log, &merged).unwrap();
    let lines_before = merged.lines().count();

    // A read sees both merged events, and warns.
    let out = run_ok(dir, &["status", "s"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("holds 2 events written with no newline between them"),
        "{stderr}"
    );

    let out = run_ok(
        dir,
        &["context", "add", "s", "after", "--from-file", "note.txt"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("held 2 events written with no newline between them")
            && stderr.contains("split it into one line per event"),
        "{stderr}"
    );

    let events = assert_one_event_per_line(&log);
    assert_eq!(
        events.len(),
        lines_before - 1 + 1 + 1,
        "both merged events kept, one added"
    );
    let backups: Vec<PathBuf> = std::fs::read_dir(log.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with("-merged.bak"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), merged);

    // The session is fully usable again.
    run_ok(dir, &["context", "get", "s", "after"]);
    assert_one_event_per_line(&log);
}

/// A genuinely truncated last line behaves as it did: reads recover with
/// the truncated-line warning, and a write is refused with the same error.
#[test]
fn a_truncated_last_line_behaves_as_before() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);
    let log = log_path(dir);
    let content = std::fs::read_to_string(&log).unwrap();
    let truncated = &content[..content.len() - 20];
    std::fs::write(&log, truncated).unwrap();

    let out = run_ok(dir, &["status", "s"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: truncated final line"));

    let out = koto(dir)
        .args(["context", "add", "s", "after", "--from-file", "note.txt"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("failed to parse last event line"),
        "{stdout}"
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), truncated);
}

/// The background children and grandchildren of a command that timed out
/// keep calling `koto` against the session until the timeout kills their
/// process group. Once the runner returns, nothing they started writes to
/// the log again, and every line holds one event.
#[test]
fn a_timed_out_commands_children_never_write_after_it_returns() {
    use koto::action::{run_shell_command, CommandEnv, FailureKind};
    use koto::redact::Redactor;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    setup(dir);
    // A cleared environment, so the writers' koto inherits no
    // `KOTO_SESSION_*` from the shell running the tests.
    let env = CommandEnv::cleared(
        vec![
            (
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            ),
            (
                "KOTO".to_string(),
                koto_command::koto_bin().to_string_lossy().into_owned(),
            ),
            (
                "KOTO_SESSIONS_BASE".to_string(),
                sessions_base(dir).to_string_lossy().into_owned(),
            ),
            ("HOME".to_string(), dir.to_string_lossy().into_owned()),
            // The timeout kills these writers wherever they are.
            (NO_PROFILE.0.to_string(), NO_PROFILE.1.to_string()),
        ],
        Redactor::empty(),
    );
    let out = run_shell_command(
        "( i=0; while :; do i=$((i+1)); \"$KOTO\" context add s c$i --from-file note.txt \
           >/dev/null 2>&1; done ) & \
         sh -c '( while :; do \"$KOTO\" decisions record s --with-data @decision.json \
           >/dev/null 2>&1; done ) & wait' & \
         ( while :; do \"$KOTO\" context get s note >/dev/null 2>&1; done ) & \
         sleep 60",
        dir,
        2,
        &env,
    );
    assert_eq!(
        out.failure_kind,
        Some(FailureKind::TimedOut),
        "{}",
        out.stderr
    );

    let log = log_path(dir);
    let at_return = std::fs::read(&log).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        std::fs::read(&log).unwrap(),
        at_return,
        "something the timed-out command started wrote after it returned"
    );
    let events = assert_one_event_per_line(&log);
    assert!(
        count(&events, "context_added") > 1,
        "the writers ran before the timeout"
    );

    run_ok(
        dir,
        &["context", "add", "s", "after", "--from-file", "note.txt"],
    );
    assert_one_event_per_line(&log);
}
