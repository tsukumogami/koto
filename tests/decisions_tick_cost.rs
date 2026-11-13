//! The cost the reversal read adds to `koto next` (PRD R36): at a chain
//! depth of five, with 1,000 decisions in each session, the median of five
//! warm runs stays under 50 ms on the local backend, and the read touches
//! only the sessions on the deciding session's own chain.
//!
//! It runs with the rest of the suite, so CI measures it on its own runner
//! for every change. The fixture's sessions are made by the binary; their
//! decisions are copies of one recorded decision with fresh ids, written
//! straight into each session's decision area.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_fs::TempDir;
use serde_json::Value;

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// Sessions above the deciding session.
const DEPTH: usize = 5;
const DECISIONS_PER_SESSION: usize = 1_000;
const RUNS: usize = 5;
const LIMIT: Duration = Duration::from_millis(50);

fn koto(root: &Path, args: &[&str]) -> Value {
    let mut cmd = koto_command::koto_std();
    for name in [
        "CLAUDE_CODE_SESSION_ID",
        "KOTO_ANCHOR",
        "KOTO_AGENT_ID",
        "KOTO_RUN_MODE",
        "KOTO_AUTHORITY",
    ] {
        cmd.env_remove(name);
    }
    cmd.current_dir(root.join("ws"))
        .env("HOME", root.join("home"))
        .env("KOTO_HOME", root.join("home"))
        .env("KOTO_SESSIONS_BASE", root.join("sessions"));
    let out = cmd.args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

/// Fill `session`'s decision area to [`DECISIONS_PER_SESSION`] entries by
/// copying its one recorded decision under fresh ids.
fn fill(sessions: &Path, session: &str, recorded: &Value) {
    let dir = sessions.join(session).join("ctx/koto/decisions");
    let first = recorded["id"].as_str().unwrap().parse::<u64>().unwrap();
    for n in (first + 1)..=(DECISIONS_PER_SESSION as u64) {
        let id = format!("{n:012}");
        let mut body = recorded["decision"].clone();
        body["id"] = Value::from(id.clone());
        std::fs::write(dir.join(&id), serde_json::to_vec(&body).unwrap()).unwrap();
    }
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        DECISIONS_PER_SESSION
    );
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

#[test]
fn tick_cost_at_chain_depth_five_with_1000_decisions_each() {
    let tmp = TempDir::new().unwrap();
    let root: PathBuf = std::fs::canonicalize(tmp.path()).unwrap();
    for dir in ["home", "ws", "sessions"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let template = root.join("work.md");
    std::fs::write(&template, TEMPLATE).unwrap();
    let sessions = root.join("sessions");

    // s0 at the top, s5 the deciding session.
    let names: Vec<String> = (0..=DEPTH).map(|i| format!("s{i}")).collect();
    for (i, name) in names.iter().enumerate() {
        let mut args = vec![
            "init",
            name.as_str(),
            "--template",
            template.to_str().unwrap(),
        ];
        if i > 0 {
            args.extend_from_slice(&["--dispatched-by", names[i - 1].as_str()]);
        }
        koto(&root, &args);
        let recorded = koto(
            &root,
            &[
                "decisions",
                "record",
                name,
                "--kind",
                "judgment",
                "--outcome",
                "settled",
                "--subject",
                "what=fixture",
                "--by",
                "session",
            ],
        );
        fill(&sessions, name, &recorded);
    }

    let deciding = names.last().unwrap().as_str();
    let log = std::fs::read_to_string(
        sessions
            .join(deciding)
            .join(format!("koto-{deciding}.state.jsonl")),
    )
    .unwrap();
    let header: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    let id = header["session_id"].as_str().unwrap().to_string();
    let backend = koto::session::Backend::Local(koto::session::local::LocalBackend::with_base_dir(
        sessions.clone(),
    ));

    // The read reaches exactly the chain.
    let read = koto::decisions::read::aimed_on_chain(&backend, deciding, &id);
    let mut expected: Vec<String> = names.clone();
    expected.reverse();
    assert_eq!(read.sessions, expected);
    assert!(read.unreadable.is_empty(), "{:?}", read.unreadable);

    // One warm-up run, then the measured ones.
    assert!(koto::engine::advance::decision_block(&backend, deciding, &id, &[]).is_none());
    let runs: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let block = koto::engine::advance::decision_block(&backend, deciding, &id, &[]);
            let took = start.elapsed();
            assert!(block.is_none());
            took
        })
        .collect();
    let median = median(runs.clone());
    eprintln!("reversal read at depth {DEPTH}, {DECISIONS_PER_SESSION} decisions each: median {median:?} of {runs:?}");
    assert!(
        median < LIMIT,
        "the reversal read took {median:?} (median of {RUNS}), over {LIMIT:?}"
    );
}
