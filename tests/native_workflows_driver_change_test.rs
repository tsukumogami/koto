//! A run's `/workflows` view follows the Claude Code session driving it
//! (koto#361).
//!
//! koto finds a session's `/workflows` folder from `CLAUDE_CODE_SESSION_ID`
//! on its first commit and stores it under `workflows/publish-location`.
//! When a different Claude Code session takes over the run (a resume, or a
//! rebind to another checkout with a new agent session), the next commit
//! writes into the new session's folder, the stored key names that folder,
//! and the old folder's file stops changing. A location set with
//! `koto workflows publish` stays put.
//!
//! Every test drives the koto binary with a temporary `HOME` holding fake
//! Claude Code transcripts (`~/.claude/projects/<project>/<sid>.jsonl`) and
//! an environment built from scratch, so nothing reaches the real home or
//! the Claude Code session running the suite.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const NAME: &str = "demo";
const TEMPLATE_FILE: &str = "demo.md";
const LOCATION: &str = "workflows/publish-location";
const EXPLICIT: &str = "workflows/publish-explicit";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Each state but the last waits for one submission.
const TEMPLATE: &str = r#"---
name: demo
version: "1.0"
initial_state: one
states:
  one:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: two
        when:
          go: go
  two:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: three
        when:
          go: go
  three:
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

## one

One.

## two

Two.

## three

Three.

## done

Done.
"#;

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// One isolated host: a `HOME` with Claude Code's projects folder, a template
/// cache, and working directories under one temporary directory.
struct Env {
    root: TempDir,
    home: PathBuf,
    cache: PathBuf,
}

impl Env {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        Env { root, home, cache }
    }

    fn projects(&self) -> PathBuf {
        self.home.join(".claude").join("projects")
    }

    /// Write the transcript Claude Code keeps for `sid` under `project`.
    fn transcript(&self, project: &str, sid: &str) {
        let dir = mk(self.projects().join(project));
        std::fs::write(dir.join(format!("{sid}.jsonl")), b"").unwrap();
    }

    /// The `/workflows` folder Claude Code reads for `sid` under `project`.
    fn folder(&self, project: &str, sid: &str) -> PathBuf {
        self.projects().join(project).join(sid).join("workflows")
    }

    /// A working directory holding the template.
    fn workspace(&self, name: &str) -> PathBuf {
        let ws = mk(self.root.path().join(name));
        std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        ws
    }

    /// A koto command in `ws` driven by Claude Code session `driver`, with
    /// nothing inherited from the test process.
    fn cmd(&self, ws: &Path, driver: Option<&str>) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", SYSTEM_PATH)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache);
        if let Some(sid) = driver {
            cmd.env("CLAUDE_CODE_SESSION_ID", sid);
        }
        cmd
    }

    fn koto(&self, ws: &Path, driver: Option<&str>, args: &[&str]) -> Out {
        let o = self.cmd(ws, driver).args(args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, ws: &Path, driver: Option<&str>, args: &[&str]) -> Out {
        let out = self.koto(ws, driver, args);
        assert!(out.ok(), "koto {args:?}: {}", out.describe());
        out
    }

    fn init(&self, ws: &Path, driver: Option<&str>) {
        self.ok(ws, driver, &["init", NAME, "--template", TEMPLATE_FILE]);
    }

    /// Submit the one value each state waits for, moving to the next state.
    fn advance(&self, ws: &Path, driver: Option<&str>) {
        self.ok(ws, driver, &["next", NAME, "--with-data", r#"{"go":"go"}"#]);
    }

    fn context(&self, ws: &Path, key: &str) -> String {
        self.ok(ws, None, &["context", "get", NAME, key])
            .stdout
            .trim()
            .to_string()
    }
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// The one `koto-*.json` file in `dir`, if there is exactly one.
fn rendered(dir: &Path) -> Option<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("koto-") && n.ends_with(".json"))
        })
        .collect();
    match files.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The current state the rendered file in `dir` shows.
fn state_in(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(rendered(dir)?).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v["koto"]["currentState"].as_str().map(str::to_string)
}

/// The issue's reproduction: a run driven by `sess-one`, resumed from
/// `sess-two`.
#[test]
fn a_resumed_run_renders_into_the_new_sessions_folder() {
    let env = Env::new();
    env.transcript("proj", "sess-one");
    env.transcript("proj", "sess-two");
    let one = env.folder("proj", "sess-one");
    let two = env.folder("proj", "sess-two");
    let ws = env.workspace("ws");

    env.init(&ws, Some("sess-one"));
    env.advance(&ws, Some("sess-one"));
    assert_eq!(state_in(&one).as_deref(), Some("two"));
    assert_eq!(env.context(&ws, LOCATION), one.to_str().unwrap());
    let one_file = rendered(&one).unwrap();
    let one_bytes = std::fs::read(&one_file).unwrap();

    env.advance(&ws, Some("sess-two"));
    assert_eq!(
        state_in(&two).as_deref(),
        Some("three"),
        "Expected: the file appears under sess-two. Actual: it keeps going to sess-one"
    );
    assert_eq!(env.context(&ws, LOCATION), two.to_str().unwrap());
    assert_eq!(
        std::fs::read(&one_file).unwrap(),
        one_bytes,
        "sess-one's file changed after sess-two took over"
    );

    // Later commits keep going to sess-two's folder.
    env.advance(&ws, Some("sess-two"));
    assert_eq!(state_in(&two).as_deref(), Some("done"));
    assert_eq!(std::fs::read(&one_file).unwrap(), one_bytes);
}

/// The rebind shape: the run moves to another checkout, picked up by a new
/// Claude Code session whose transcript lives under another project folder.
#[test]
fn a_rebound_run_renders_into_the_new_sessions_project_folder() {
    let env = Env::new();
    env.transcript("proj-a", "sess-one");
    env.transcript("proj-b", "sess-two");
    let one = env.folder("proj-a", "sess-one");
    let two = env.folder("proj-b", "sess-two");
    let ws_a = env.workspace("ws-a");
    let ws_b = env.workspace("ws-b");

    env.init(&ws_a, Some("sess-one"));
    env.advance(&ws_a, Some("sess-one"));
    assert_eq!(state_in(&one).as_deref(), Some("two"));
    let one_file = rendered(&one).unwrap();
    let one_bytes = std::fs::read(&one_file).unwrap();

    env.ok(&ws_b, Some("sess-two"), &["session", "rebind", NAME]);
    env.advance(&ws_b, Some("sess-two"));

    assert_eq!(state_in(&two).as_deref(), Some("three"));
    assert_eq!(env.context(&ws_b, LOCATION), two.to_str().unwrap());
    assert_eq!(
        std::fs::read(&one_file).unwrap(),
        one_bytes,
        "sess-one's file changed after the rebind"
    );
}

/// A location set with `koto workflows publish` stays where it was put when
/// another Claude Code session drives the run.
#[test]
fn an_explicitly_published_location_does_not_move() {
    let env = Env::new();
    env.transcript("proj", "sess-one");
    env.transcript("proj", "sess-two");
    let one = env.folder("proj", "sess-one");
    let two = env.folder("proj", "sess-two");
    let ws = env.workspace("ws");

    env.init(&ws, None);
    env.ok(
        &ws,
        Some("sess-two"),
        &[
            "workflows",
            "publish",
            "--dir",
            one.to_str().unwrap(),
            "--session",
            NAME,
        ],
    );
    assert_eq!(env.context(&ws, LOCATION), one.to_str().unwrap());
    assert_eq!(env.context(&ws, EXPLICIT), one.to_str().unwrap());

    env.advance(&ws, Some("sess-two"));

    assert_eq!(state_in(&one).as_deref(), Some("two"));
    assert!(
        rendered(&two).is_none(),
        "the explicit location moved to sess-two's folder"
    );
    assert_eq!(env.context(&ws, LOCATION), one.to_str().unwrap());
}
