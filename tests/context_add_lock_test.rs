//! `koto context add` racing the commands that replace a session's log
//! whole (koto#728).
//!
//! An anchor adoption (the first `koto next` of a session with no anchor)
//! and a `koto session rebind` each rewrite the log's header by replacing
//! the file. `context add` reads the header, stores the content, then
//! appends its `context_added` event; it holds the log's append lock across
//! the three, so a rewrite can't land in between. Every round here runs
//! several adds, an adoption and several rebinds at once against one
//! session and checks that no add was lost: an add that succeeded has its
//! event, and an add that failed did so as busy and stored nothing.
//!
//! The binary is driven directly, since the race is between processes.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_fs::TempDir;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

const ROUNDS: usize = 20;
const ADDS: usize = 6;
const REBINDS: usize = 3;

/// Sessions live outside the anchor directory, so the anchor never holds
/// the session log.
fn sessions_base(home: &Path) -> PathBuf {
    let base = home.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// A `koto` process whose working directory is `cwd` and whose session
/// storage is rooted at `home`, with its output captured.
fn koto_cmd(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = koto_command::koto_std();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    // An anchor named by the shell running the suite is no anchor here.
    cmd.env_remove("KOTO_ANCHOR");
    cmd.env_remove("KOTO_DISPATCHER_ANCHOR");
    cmd.current_dir(cwd);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(home));
    cmd.env("HOME", home);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd
}

struct Run {
    success: bool,
    output: String,
}

fn finish(child: Child) -> Run {
    let out = child.wait_with_output().unwrap();
    Run {
        success: out.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

fn run_koto(home: &Path, cwd: &Path, args: &[&str]) -> Run {
    finish(koto_cmd(home, cwd, args).spawn().unwrap())
}

fn state_path(home: &Path, name: &str) -> PathBuf {
    sessions_base(home)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

fn ctx_path(home: &Path, name: &str, key: &str) -> PathBuf {
    sessions_base(home).join(name).join("ctx").join(key)
}

const PLAIN_TEMPLATE: &str = r#"---
name: plain
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          choice: go
  done:
    terminal: true
---

## start

Decide.

## done

Done.
"#;

/// Init a session anchored at `anchor`, with the template written outside
/// it.
fn init_at(home: &Path, anchor: &Path, name: &str, template: &str) {
    let src = home.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    let run = run_koto(
        home,
        anchor,
        &["init", name, "--template", src.to_str().unwrap()],
    );
    assert!(run.success, "init failed: {}", run.output);
}

/// Strip `execution_dir` from a session's header, so its first `koto next`
/// adopts an anchor, rewriting the header.
fn strip_anchor(home: &Path, name: &str) {
    let path = state_path(home, name);
    let contents = std::fs::read_to_string(&path).unwrap();
    let mut lines = contents.lines();
    let mut header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    header.as_object_mut().unwrap().remove("execution_dir");
    let mut out = serde_json::to_string(&header).unwrap();
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    std::fs::write(&path, out).unwrap();
}

fn dir(base: &Path, name: &str) -> PathBuf {
    let path = base.join(name);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(&path).unwrap()
}

// ---------------------------------------------------------------------------
// the race
// ---------------------------------------------------------------------------

#[test]
fn context_adds_racing_header_rewrites_lose_no_append() {
    for round in 0..ROUNDS {
        let home = TempDir::new().unwrap();
        let anchor = dir(home.path(), "checkout");
        init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
        strip_anchor(home.path(), "wf");

        let inputs: Vec<PathBuf> = (0..ADDS)
            .map(|i| {
                let path = home.path().join(format!("in-{i}.txt"));
                std::fs::write(&path, format!("content {i} of round {round}")).unwrap();
                path
            })
            .collect();

        // Spawned, not run one by one, so the processes overlap. The adds
        // are interleaved with the rewrites rather than all started first.
        // The rebinds alternate between two directories so each one really
        // rewrites the header (a rebind to where the session already is
        // writes nothing), and they and the adds run from a directory that
        // is neither, so no rebind leaves them working from a retired
        // anchor.
        let other = dir(home.path(), "other");
        let targets = [
            other.to_str().unwrap().to_string(),
            anchor.to_str().unwrap().to_string(),
        ];
        let neutral = home.path().to_path_buf();
        let mut adds: Vec<(String, Child)> = Vec::new();
        let mut rewrites: Vec<Child> = Vec::new();
        for (i, input) in inputs.iter().enumerate() {
            let key = format!("k{i}");
            let child = koto_cmd(
                home.path(),
                &neutral,
                &[
                    "context",
                    "add",
                    "wf",
                    &key,
                    "--from-file",
                    input.to_str().unwrap(),
                ],
            )
            .spawn()
            .unwrap();
            adds.push((key, child));
            if i == 1 {
                rewrites.push(
                    koto_cmd(home.path(), &anchor, &["next", "wf"])
                        .spawn()
                        .unwrap(),
                );
            }
            if i < REBINDS {
                rewrites.push(
                    koto_cmd(
                        home.path(),
                        &neutral,
                        &["session", "rebind", "wf", "--to", &targets[i % 2]],
                    )
                    .spawn()
                    .unwrap(),
                );
            }
        }

        let adds: Vec<(String, Run)> = adds
            .into_iter()
            .map(|(key, child)| (key, finish(child)))
            .collect();
        for child in rewrites {
            // Whether a rewrite succeeds is not what's under test; only that
            // it never costs an add its event.
            let _ = finish(child);
        }

        let content = std::fs::read_to_string(state_path(home.path(), "wf")).unwrap();
        let lines: Vec<serde_json::Value> = content
            .lines()
            .map(|l| {
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("round {round}: unparseable line {l:?}: {e}"))
            })
            .collect();
        let (header, events) = lines.split_first().expect("the log has a header");
        assert!(
            header.get("workflow").is_some() && header.get("seq").is_none(),
            "round {round}: the first line is not a header: {header}"
        );
        let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        let expected: Vec<u64> = (1..=events.len() as u64).collect();
        assert_eq!(
            seqs, expected,
            "round {round}: seqs are not 1..n:\n{content}"
        );

        for (key, run) in &adds {
            let logged = events
                .iter()
                .filter(|e| e["type"] == "context_added" && e["payload"]["key"] == key.as_str())
                .count();
            let stored = ctx_path(home.path(), "wf", key).exists();
            if run.success {
                assert_eq!(
                    logged, 1,
                    "round {round}: add of {key} succeeded with {logged} events:\n{content}"
                );
                assert!(
                    stored,
                    "round {round}: add of {key} succeeded, nothing stored"
                );
            } else {
                assert!(
                    run.output.contains("is busy"),
                    "round {round}: add of {key} failed other than busy: {}",
                    run.output
                );
                assert_eq!(
                    logged, 0,
                    "round {round}: a busy add of {key} logged an event:\n{content}"
                );
                assert!(
                    !stored,
                    "round {round}: a busy add of {key} stored content with no event"
                );
            }
        }
    }
}
