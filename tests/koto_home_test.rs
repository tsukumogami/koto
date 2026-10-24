//! `KOTO_HOME` moves the whole koto root: sessions, requests, config and the
//! prune walk land under it, and the home directory's `.koto` is never read
//! or written. Unset, the root is `~/.koto` as before.
//!
//! Every command runs with `HOME` and `KOTO_HOME` at scratch directories and
//! with every variable that could point koto at another store removed, and
//! the first thing each test does is read the root back with
//! `koto session dir` and require it to be the scratch one.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// `start` dumps its command environment into the session directory, then
/// waits for `go`, which ends it at `done`.
const TEMPLATE: &str = r#"---
name: koto-home
version: "1.0"
initial_state: start
states:
  start:
    gates:
      env_dump:
        type: command
        command: "env > {{SESSION_DIR}}/env.txt"
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

/// A one-second retention, so prune removes a session that just finished.
const FAST: [(&str, &str); 3] = [
    ("KOTO_RETENTION_KEEP", "1s"),
    ("KOTO_RETENTION_IDLE_PERIOD", "1h"),
    ("KOTO_RETENTION_STALE_AFTER", "2h"),
];

struct Env {
    _tmp: TempDir,
    /// What `HOME` points at.
    home: PathBuf,
    /// What `KOTO_HOME` points at, when it is set.
    koto_home: PathBuf,
    cache: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let env = Env {
            home: root.join("home"),
            koto_home: root.join("koto-home"),
            cache: root.join("cache"),
            work: root.join("work"),
            template: root.join("koto-home.md"),
            _tmp: tmp,
        };
        for d in [&env.home, &env.koto_home, &env.cache, &env.work] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&env.template, TEMPLATE).unwrap();
        env
    }

    /// A koto command in the scratch work directory with `HOME` at the
    /// scratch home and `KOTO_HOME` at the scratch root when `with_koto_home`.
    fn cmd(&self, with_koto_home: bool) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("KOTO_TICK_SESSION")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        if with_koto_home {
            cmd.env("KOTO_HOME", &self.koto_home);
        }
        cmd
    }

    fn run(&self, with_koto_home: bool, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut cmd = self.cmd(with_koto_home);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, with_koto_home: bool, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run(with_koto_home, args, env);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("koto {args:?}: not JSON: {e}\n{out}"))
    }

    /// The root koto resolves, read before anything else runs.
    fn root(&self, with_koto_home: bool) -> PathBuf {
        PathBuf::from(self.run(with_koto_home, &["session", "dir"], &[]).trim())
    }

    /// Start `name` and take it to `done`, keeping its session directory.
    fn finish(&self, with_koto_home: bool, name: &str) {
        let template = self.template.to_str().unwrap();
        self.run(with_koto_home, &["init", name, "--template", template], &[]);
        self.run(with_koto_home, &["next", name], &[]);
        self.run(
            with_koto_home,
            &[
                "next",
                name,
                "--with-data",
                r#"{"go": "yes"}"#,
                "--no-cleanup",
            ],
            &[],
        );
    }
}

/// Every file under `dir`, by relative path, with its contents.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// The `KOTO_HOME` line a session's gate command saw, if any.
fn koto_home_seen(session_dir: &Path) -> Option<String> {
    let env = std::fs::read_to_string(session_dir.join("env.txt")).unwrap();
    env.lines()
        .find_map(|l| l.strip_prefix("KOTO_HOME=").map(str::to_string))
}

#[test]
fn unset_koto_home_keeps_the_root_under_home() {
    let env = Env::new();
    assert_eq!(env.root(false), env.home.join(".koto"));

    env.finish(false, "plain");
    let dir = PathBuf::from(env.run(false, &["session", "dir", "plain"], &[]).trim());
    assert_eq!(dir, env.home.join(".koto/sessions/plain"));
    assert!(dir.join("koto-plain.state.jsonl").is_file());
    assert_eq!(
        koto_home_seen(&dir),
        None,
        "a command saw KOTO_HOME it wasn't given"
    );
    assert!(env.home.join(".koto/config.toml").is_file());
    assert!(snapshot(&env.koto_home).is_empty());
}

#[test]
fn koto_home_holds_every_path_and_home_is_untouched() {
    let env = Env::new();

    // A store under HOME that a KOTO_HOME run must neither read nor prune.
    assert_eq!(env.root(false), env.home.join(".koto"));
    env.finish(false, "home-only");
    let home_before = snapshot(&env.home);
    assert!(home_before
        .keys()
        .any(|p| p.ends_with("koto-home-only.state.jsonl")));

    // Fail closed: nothing else runs unless the root is the scratch one.
    assert_eq!(env.root(true), env.koto_home);

    // init and the tick.
    env.finish(true, "moved");
    let dir = PathBuf::from(env.run(true, &["session", "dir", "moved"], &[]).trim());
    assert_eq!(dir, env.koto_home.join("sessions/moved"));
    assert!(dir.join("koto-moved.state.jsonl").is_file());
    assert!(
        env.koto_home.join("config.toml").is_file(),
        "config.toml not under KOTO_HOME"
    );

    // A command koto runs is given the same root.
    assert_eq!(
        koto_home_seen(&dir).as_deref(),
        Some(env.koto_home.to_str().unwrap())
    );

    // session list sees only the KOTO_HOME store.
    let list = env.json(true, &["session", "list"], &[]);
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"moved"), "{list:#}");
    assert!(!names.contains(&"home-only"), "{list:#}");

    // A request verb writes and reads the request store under KOTO_HOME.
    let created = env.json(
        true,
        &[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"a","role":"r","template":"t","inputs":{}}],"inputs":{}}"#,
            "--requested-by",
            "coord",
            "--coordinator-of-record",
            "coord",
        ],
        &[],
    );
    let id = created["request_id"].as_str().unwrap();
    assert!(env.koto_home.join("requests").join(id).is_dir());
    env.json(true, &["request", "get", id], &[]);

    // The prune walk is the KOTO_HOME store: it removes the finished session
    // there and leaves the one under HOME alone.
    std::thread::sleep(Duration::from_millis(1300));
    let pruned = env.json(true, &["session", "prune", "--apply", "--all"], &FAST);
    let removed: Vec<&str> = pruned["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(removed, vec!["moved"], "{pruned:#}");
    assert!(!dir.exists());

    assert_eq!(
        snapshot(&env.home),
        home_before,
        "HOME changed under KOTO_HOME"
    );
}

#[test]
fn relative_koto_home_is_taken_against_the_current_directory() {
    let env = Env::new();
    let out = env
        .cmd(false)
        .env("KOTO_HOME", "rel-root")
        .args(["session", "dir"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        PathBuf::from(String::from_utf8(out.stdout).unwrap().trim()),
        env.work.join("rel-root")
    );
}
