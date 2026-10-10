//! Concurrent appends to one session log (DESIGN-koto-failure-reporting.md,
//! Decision 4: the per-session append lock).
//!
//! Every `koto context get` now appends a `context_read`, so readers append
//! while a tick appends. Each append reads the log's last seq and writes the
//! next one; without a lock around that window two processes can take the
//! same seq. This test runs twenty `koto context get` processes against one
//! session while a `koto next` advances it and a thread appends through the
//! idempotent path, and checks the log comes out with every seq exactly once,
//! in order, and every read logged exactly once. With the append lock removed
//! it fails: two appends take the same seq, and the tick's next read of the
//! log refuses it as corrupted ("sequence gap").

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use assert_fs::TempDir;
use koto::engine::persistence::append_event_idempotent;
use koto::engine::types::EventPayload;
use serde_json::Value;

const READERS: usize = 20;
const IDEMPOTENT_APPENDS: usize = 10;
const ROUNDS: usize = 3;
const NOTE: &str = "shared note";

fn sessions_base(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

fn koto(dir: &Path) -> StdCommand {
    let mut cmd = StdCommand::new(env!("CARGO_BIN_EXE_koto"));
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    cmd
}

fn run_ok(dir: &Path, args: &[&str]) {
    let out = koto(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "koto {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A chain of states, each behind a passing command gate that takes a
/// moment, so one `koto next` appends a gate_evaluated and a transitioned
/// per state spread over the time the readers run.
fn chain_template(states: usize) -> String {
    let mut yaml =
        String::from("---\nname: append-race\nversion: \"1.0\"\ninitial_state: s0\nstates:\n");
    for i in 0..states {
        yaml.push_str(&format!(
            "  s{i}:\n    gates:\n      g:\n        type: command\n        command: \"sleep 0.02\"\n    transitions:\n      - target: s{}\n",
            i + 1
        ));
    }
    yaml.push_str(&format!("  s{states}:\n    terminal: true\n---\n"));
    for i in 0..=states {
        yaml.push_str(&format!("\n## s{i}\n\nStep {i}.\n"));
    }
    yaml
}

#[test]
fn concurrent_appends_keep_seq_unique_and_every_read_logged_once() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(sessions_base(dir)).unwrap();
    let template = dir.join("race.md");
    std::fs::write(&template, chain_template(12)).unwrap();
    run_ok(
        dir,
        &["init", "s", "--template", template.to_str().unwrap()],
    );
    let note = dir.join("note.txt");
    std::fs::write(&note, NOTE).unwrap();
    run_ok(
        dir,
        &[
            "context",
            "add",
            "s",
            "note",
            "--from-file",
            note.to_str().unwrap(),
        ],
    );
    let log = sessions_base(dir).join("s").join("koto-s.state.jsonl");

    for round in 0..ROUNDS {
        let tick = koto(dir)
            .args(["next", "s", "--no-cleanup"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                koto(dir)
                    .args(["context", "get", "s", "note"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let log_for_thread = log.clone();
        let idempotent = std::thread::spawn(move || {
            for i in 0..IDEMPOTENT_APPENDS {
                let intent = format!("round {round} append {i}");
                append_event_idempotent(
                    &log_for_thread,
                    &EventPayload::IntentUpdated {
                        intent: intent.clone(),
                    },
                    "2026-01-01T00:00:00Z",
                    "s",
                    Some(&format!("race-{round}-{i}")),
                )
                .unwrap();
            }
        });

        for reader in readers {
            let out = reader.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "a reader failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
        }
        let out = tick.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "koto next failed: {} {}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        idempotent.join().unwrap();
    }

    let raw = std::fs::read_to_string(&log).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("torn line {l:?}: {e}")))
        .collect();
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
    assert_eq!(
        seqs, expected,
        "seq must run 1..=n with no gap or duplicate"
    );

    let reads = events
        .iter()
        .filter(|e| e["type"] == "context_read" && e["payload"]["reader"] == "cli")
        .count();
    assert_eq!(reads, READERS * ROUNDS, "every read logged exactly once");
    let idempotent = events
        .iter()
        .filter(|e| {
            e["type"] == "intent_updated"
                && e["payload"]["intent"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("round "))
        })
        .count();
    assert_eq!(idempotent, IDEMPOTENT_APPENDS * ROUNDS);
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "transitioned" && e["payload"]["to"] == "s12"),
        "the tick advanced through the chain"
    );
}
