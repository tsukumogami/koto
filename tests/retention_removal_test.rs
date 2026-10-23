//! What is actually removed or kept at each removal point once the terminal
//! tick, the parent sweep and the same-name replace follow the retention
//! rule (DESIGN-session-retention.md, Decisions 3 and 4).
//!
//! The engine's tests check these scenarios as verdicts; these drive the
//! real binary through each removal step -- terminal ticks, the sweep a
//! parent's tick would run, `koto session prune` preview and apply, `koto
//! workspace prune --root`, and the re-run's replace -- and check what
//! stays, what goes, and the reason each kept session is given. Every step
//! is a single-writer step too: every session other than the one the
//! command acts on hashes the same before and after it. A terminal tick
//! also delivers the session's completion to its koto parent, so that
//! parent's log may grow by `child_completed` events and nothing else.
//! Other sessions are read with commands that record no read (`koto session
//! hierarchy`, `koto context list`), and, in Finished child, Finished parent
//! and Grandchild, with `koto context get` and `koto context exists` from
//! another agent session, whose read records go to that session's anchor
//! (`Env::read_from_another_agent_session`), and from a shell with no
//! anchor, whose reads go to the host-local read spool
//! (`Env::read_with_no_anchor`).
//!
//! The workspace-prune steps of these scenarios live here rather than in
//! `tests/cli_workspace_prune.rs` because they need the same single-writer
//! harness as every other step of the scenario; that file covers the
//! subtree walk and the verb's own flags.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `start` waits for `go`; `yes` reaches the success terminal `done`.
const TEMPLATE: &str = r#"---
name: removal-test
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

const BUCKET: &str = "koto-retention-removal";

/// The host-local read spool beside a session's local copy. It holds reads
/// of the session made with no reader's anchor and is never the session's
/// state, so the single-writer hashes leave it out and the read steps check
/// it on their own.
const SPOOL: &str = "context-reads.jsonl";

/// The cloud backend's host-local record of what it last knew of a
/// session's copy in the bucket, beside the session's local copy and never
/// uploaded. A read refreshes it; it is no part of the session's state, so
/// the single-writer hashes leave it out too.
const CLOUD_CACHE: [&str; 3] = ["cloud-sync.json", "cloud-manifest.json", "cloud-tick.id"];

/// A one-second retention, so a test can wait it out.
const FAST: [(&str, &str); 3] = [
    ("KOTO_RETENTION_KEEP", "1s"),
    ("KOTO_RETENTION_IDLE_PERIOD", "1h"),
    ("KOTO_RETENTION_STALE_AFTER", "2h"),
];

/// A retention no test waits out.
const LONG: [(&str, &str); 2] = [
    ("KOTO_RETENTION_KEEP", "1h"),
    ("KOTO_RETENTION_STALE_AFTER", "2h"),
];

/// Wait out [`FAST`]'s retention.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

type Snapshot = BTreeMap<String, BTreeMap<PathBuf, String>>;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    root: PathBuf,
    template: PathBuf,
    work: PathBuf,
    cloud: bool,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("removal-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            root,
            template,
            work,
            cloud: false,
        }
    }

    fn cloud(s3: &FakeS3) -> Self {
        let mut env = Env::new();
        std::fs::create_dir_all(env.work.join(".koto")).unwrap();
        std::fs::write(
            env.work.join(".koto/config.toml"),
            format!(
                "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nendpoint = {:?}\n\
                 bucket = {:?}\nregion = \"us-east-1\"\npath_style = true\n",
                s3.endpoint(),
                BUCKET
            ),
        )
        .unwrap();
        env.cloud = true;
        env
    }

    /// The bucket prefix of this workspace's sessions.
    fn prefix(&self) -> String {
        let digest = Sha256::digest(self.work.to_string_lossy().as_bytes());
        hex::encode(digest)[..16].to_string()
    }

    fn cmd(&self, env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("KOTO_SESSION_BACKEND")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        for (k, v) in FAST {
            cmd.env(k, v);
        }
        if self.cloud {
            cmd.env("AWS_ACCESS_KEY_ID", "test-access-key")
                .env("AWS_SECRET_ACCESS_KEY", "test-secret-key");
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        self.cmd(env).args(args).output().unwrap()
    }

    fn ok_env(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run(args, env);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_env(args, &[])
    }

    fn init_with(&self, name: &str, extra: &[&str], env: &[(&str, &str)]) -> Value {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok_env(&args, env)
    }

    fn init(&self, name: &str) {
        self.init_with(name, &[], &[]);
    }

    fn dispatched(&self, name: &str, by: &str) {
        self.init_with(name, &["--dispatched-by", by], &[]);
    }

    fn child(&self, name: &str, parent: &str) {
        self.init_with(name, &["--parent", parent], &[]);
    }

    /// Tick `name` to `done`, with or without `--no-cleanup`, and return
    /// the response.
    fn tick(&self, name: &str, no_cleanup: bool) -> Value {
        let mut args = vec!["next", name, "--with-data", r#"{"go": "yes"}"#];
        if no_cleanup {
            args.push("--no-cleanup");
        }
        let v = self.ok(&args);
        assert_eq!(v["action"], "done", "{v:#}");
        v
    }

    fn replace(&self, name: &str, env: &[(&str, &str)]) -> std::process::Output {
        self.run(
            &[
                "init",
                name,
                "--template",
                self.template.to_str().unwrap(),
                "--replace-terminal",
            ],
            env,
        )
    }

    fn add_key(&self, name: &str, key: &str, body: &str) {
        let file = self.root.join(format!("key-{name}"));
        std::fs::write(&file, body).unwrap();
        self.ok(&[
            "context",
            "add",
            name,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ]);
    }

    /// Read `key` of `name` from another agent session, as an agent driving
    /// another session would: `koto context get` and `koto context exists`
    /// with `KOTO_ANCHOR` naming that agent session's anchor,
    /// `anchor-reader` (opened here on first use). Each is a single-writer
    /// step in which only the reader's anchor may change, so `name` hashes
    /// the same, and each adds one read record to the anchor naming `name`
    /// and `key` (DESIGN-session-retention.md, Decision 5).
    fn read_from_another_agent_session(&self, name: &str, key: &str) {
        const READER: &str = "anchor-reader";
        if !self.exists(READER) {
            let out = self.run(
                &[
                    "session",
                    "anchor",
                    "open",
                    "--harness-session",
                    "reader",
                    "--source",
                    "startup",
                ],
                &[],
            );
            assert!(out.status.success(), "anchor open: {out:?}");
        }
        let env = [("KOTO_ANCHOR", READER)];
        for (verb, access) in [("get", "content"), ("exists", "presence")] {
            let before = self.events_text(READER).len();
            let spooled = self.spooled(name).len();
            self.step(&[READER], None, &[], || {
                let out = self.run(&["context", verb, name, key], &env);
                assert!(out.status.success(), "context {verb} {name} {key}: {out:?}");
            });
            let added: Vec<Value> = self.events_text(READER)[before..]
                .iter()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert_eq!(added.len(), 1, "{added:#?}");
            assert_eq!(added[0]["type"], "context_read");
            assert_eq!(added[0]["payload"]["session"], name);
            assert_eq!(added[0]["payload"]["key"], key);
            assert_eq!(added[0]["payload"]["access"], access);
            assert_eq!(
                self.spooled(name).len(),
                spooled,
                "an anchored read spooled"
            );
        }
    }

    /// Read `key` of `name` with no reader's anchor, as a bare shell would:
    /// `koto context get` and `koto context exists` are each a single-writer
    /// step in which no session's files change (the read spool aside), and
    /// each adds one read to the spool beside `name`.
    fn read_with_no_anchor(&self, name: &str, key: &str) {
        for (verb, access) in [("get", "content"), ("exists", "presence")] {
            let before = self.spooled(name).len();
            self.step(&[], None, &[], || {
                let out = self.run(&["context", verb, name, key], &[]);
                assert!(out.status.success(), "context {verb} {name} {key}: {out:?}");
            });
            let spooled = self.spooled(name);
            assert_eq!(spooled.len(), before + 1, "{spooled:#?}");
            let last = &spooled[before];
            assert_eq!(last["type"], "context_read");
            assert_eq!(last["payload"]["key"], key);
            assert_eq!(last["payload"]["access"], access);
            assert!(last["payload"].get("session").is_none());
        }
    }

    /// The lines of the read spool beside `name`.
    fn spooled(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.sessions().join(name).join(SPOOL))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The keys `name` holds, read with a command that records no read.
    fn keys(&self, name: &str) -> Vec<String> {
        let v = self.ok(&["context", "list", name]);
        v.as_array()
            .unwrap_or_else(|| panic!("context list {name}: {v:#}"))
            .iter()
            .map(|k| k.as_str().unwrap().to_string())
            .collect()
    }

    fn hierarchy(&self, name: &str, env: &[(&str, &str)]) -> Value {
        self.ok_env(&["session", "hierarchy", name, "--json"], env)
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).is_dir()
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn header(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.state_path(name)).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    fn events_text(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap()
            .lines()
            .skip(1)
            .map(str::to_string)
            .collect()
    }

    /// Set every event's timestamp in `name`'s log to `at`.
    fn set_all_event_times(&self, name: &str, at: SystemTime) {
        let path = self.state_path(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        for line in lines.iter_mut().skip(1) {
            let mut event: Value = serde_json::from_str(line).unwrap();
            event["timestamp"] = Value::from(koto::engine::types::iso8601_at(at));
            *line = serde_json::to_string(&event).unwrap();
        }
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    }

    /// Every session directory's name.
    fn all(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.sessions())
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// SHA-256 of every file in session `name`'s directory.
    fn hashes(&self, name: &str) -> BTreeMap<PathBuf, String> {
        let root = self.sessions().join(name);
        let mut out = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file()
                    && !p.ends_with(SPOOL)
                    && !CLOUD_CACHE.iter().any(|f| p.ends_with(f))
                {
                    let digest = Sha256::digest(std::fs::read(&p).unwrap());
                    out.insert(
                        p.strip_prefix(&root).unwrap().to_path_buf(),
                        hex::encode(digest),
                    );
                }
            }
        }
        out
    }

    fn snapshot_except(&self, exempt: &[&str]) -> Snapshot {
        self.all()
            .into_iter()
            .filter(|n| !exempt.contains(&n.as_str()))
            .map(|n| {
                let h = self.hashes(&n);
                (n, h)
            })
            .collect()
    }

    /// Run one removal or tick step as a single writer: every session
    /// other than `exempt` hashes the same afterwards unless the step
    /// removed it, and only sessions in `may_remove` may go. `parent`, a
    /// ticked session's koto parent, may only have `child_completed`
    /// events appended. Returns what the step printed.
    fn step<T>(
        &self,
        exempt: &[&str],
        parent: Option<&str>,
        may_remove: &[&str],
        f: impl FnOnce() -> T,
    ) -> T {
        let mut skip: Vec<&str> = exempt.to_vec();
        if let Some(p) = parent {
            skip.push(p);
        }
        let before = self.snapshot_except(&skip);
        let parent_before = parent.map(|p| std::fs::read(self.state_path(p)).unwrap());
        let out = f();
        for (name, hashes) in &before {
            if !self.exists(name) {
                assert!(
                    may_remove.contains(&name.as_str()),
                    "{name} was removed by a step that may not remove it"
                );
                continue;
            }
            assert_eq!(
                &self.hashes(name),
                hashes,
                "{name} changed during a step that doesn't write it"
            );
        }
        if let (Some(p), Some(before)) = (parent, parent_before) {
            let after = std::fs::read(self.state_path(p)).unwrap();
            assert!(after.starts_with(&before), "{p}'s log was rewritten");
            let added = String::from_utf8(after[before.len()..].to_vec()).unwrap();
            for line in added.lines() {
                let e: Value = serde_json::from_str(line).unwrap();
                assert_eq!(e["type"], "child_completed", "{p} got {line}");
            }
        }
        out
    }

    fn prune(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let mut all = vec!["session", "prune"];
        all.extend_from_slice(args);
        self.ok_env(&all, env)
    }

    fn workspace_prune(&self, root: &str, env: &[(&str, &str)]) -> Value {
        // The JSON summary is the last line; the preview is text.
        let out = self.run(&["workspace", "prune", "--root", root, "--yes"], env);
        assert!(
            out.status.success(),
            "workspace prune failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        serde_json::from_str(text.lines().last().unwrap()).unwrap()
    }

    fn remove_request_store(&self) {
        let dir = self.home.join(".koto/requests");
        if dir.exists() {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

fn retention(v: &Value) -> (bool, Option<String>) {
    (
        v["retention"]["retained"].as_bool().unwrap(),
        v["retention"]["reason"].as_str().map(str::to_string),
    )
}

/// The kept entry for `name` in a prune's output, by its `detail` (session
/// prune) or `reason` (workspace prune).
fn kept_reason(v: &Value, name: &str) -> String {
    let k = v["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {v:#}"));
    k.get("detail")
        .or_else(|| k.get("reason"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string()
}

fn listed(v: &Value, field: &str) -> Vec<String> {
    let mut names: Vec<String> = v[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.get("name")
                .and_then(Value::as_str)
                .or_else(|| r.as_str())
                .unwrap()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

fn keeper_names(h: &Value) -> Vec<String> {
    listed(h, "keepers")
}

// ===== The terminal tick =====

#[test]
fn a_lone_session_is_removed_at_its_terminal_as_before() {
    let env = Env::new();
    env.init("bystander");
    env.init("lone");
    let v = env.step(&["lone"], None, &["lone"], || env.tick("lone", false));
    assert_eq!(retention(&v), (false, None));
    assert!(v.get("kept_children").is_none(), "{v:#}");
    assert!(!env.exists("lone"));
    assert!(env.exists("bystander"));
}

#[test]
fn a_session_named_only_by_another_is_kept_with_reason_hierarchy() {
    let env = Env::new();
    env.init("s");
    // t holds the link; s holds nothing.
    env.dispatched("t", "s");
    assert!(!env.sessions().join("s/ctx/koto").exists());
    let v = env.step(&["s"], None, &[], || env.tick("s", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    assert!(env.exists("s"));
}

#[test]
fn a_session_with_a_link_is_kept_with_or_without_the_flag_and_after_a_release() {
    let env = Env::new();
    // With the flag: the edge comes first, so the reason is hierarchy.
    env.init("a1");
    env.dispatched("b1", "a1");
    let v = env.step(&["b1"], None, &[], || env.tick("b1", true));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));

    // Without it, after every other member was released past the
    // retention: the tick judges no retention.
    env.init("a2");
    env.dispatched("b2", "a2");
    env.tick("a2", false);
    settle();
    // a2 is past the retention from its stop: b2, still running, is all
    // that keeps the hierarchy.
    let h = env.hierarchy("a2", &[]);
    assert_eq!(keeper_names(&h), vec!["b2"], "{h:#}");
    let v = env.step(&["b2"], None, &[], || env.tick("b2", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    assert!(env.exists("b2") && env.exists("a2"));
}

#[test]
fn a_malformed_retention_setting_never_fails_a_tick() {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    let v = env.ok_env(
        &["next", "b", "--with-data", r#"{"go": "yes"}"#],
        &[("KOTO_RETENTION_KEEP", "7x")],
    );
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    env.init("lone");
    let v = env.ok_env(
        &["next", "lone", "--with-data", r#"{"go": "yes"}"#],
        &[("KOTO_RETENTION_KEEP", "7x")],
    );
    assert_eq!(retention(&v), (false, None));
}

// ===== Finished parent, finished child, grandchild =====

#[test]
fn a_finished_parent_is_kept_and_lists_its_terminal_children() {
    let env = Env::new();
    env.init("a");
    env.add_key("a", "notes/a", "a's notes");
    env.child("b", "a");
    env.child("c", "a");
    // b finishes and is kept: it has lineage to a. Its completion reaches a.
    let v = env.step(&["b"], Some("a"), &[], || env.tick("b", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    assert!(env
        .events_text("a")
        .iter()
        .any(|e| e.contains("child_completed")));

    // a finishes without --no-cleanup while c runs: kept, and the sweep
    // that would have removed b removes nothing and lists it.
    let v = env.step(&["a"], None, &[], || env.tick("a", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let kept = v["kept_children"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{v:#}");
    assert_eq!(kept[0]["name"], "b");
    assert_eq!(kept[0]["reason"], "hierarchy");
    assert!(
        kept[0]["detail"]
            .as_str()
            .unwrap()
            .contains("parent_workflow names a"),
        "{v:#}"
    );
    assert!(env.exists("a") && env.exists("b") && env.exists("c"));

    // a's log and keys stay readable from c.
    assert!(env.keys("a").contains(&"notes/a".to_string()));
    let h = env.hierarchy("c", &[]);
    assert!(listed(&h, "members").contains(&"a".to_string()), "{h:#}");
    env.read_from_another_agent_session("a", "notes/a");
    env.read_with_no_anchor("a", "notes/a");

    // The prunes name c as why a and b are kept.
    for args in [&[][..], &["--apply", "--all"][..]] {
        let out = env.step(&[], None, &[], || env.prune(args, &[]));
        for name in ["a", "b"] {
            let why = kept_reason(&out, name);
            assert!(why.contains("c (koto_session)"), "{name}: {why}");
        }
    }
    let out = env.step(&[], None, &[], || env.workspace_prune("a", &LONG));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    assert!(kept_reason(&out, "a").contains("c (koto_session)"));
}

/// B, dispatched by A and a koto child of X, finishes while A is alive.
fn finished_child(no_request_store: bool) {
    let env = Env::new();
    env.init("a");
    env.init("x");
    env.init_with("b", &["--parent", "x", "--dispatched-by", "a"], &[]);
    env.add_key("b", "notes/b", "b's notes");
    if no_request_store {
        env.remove_request_store();
    }

    let v = env.step(&["b"], Some("x"), &[], || env.tick("b", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    // x's terminal step, where the parent sweep runs. With --no-cleanup, as
    // the workspace prune below expects of its root: the edge comes first,
    // so the reason is still hierarchy.
    let v = env.step(&["x"], None, &[], || env.tick("x", true));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let kept = v["kept_children"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{v:#}");
    assert_eq!(kept[0]["name"], "b");
    assert!(
        kept[0]["detail"]
            .as_str()
            .unwrap()
            .contains("dispatched-by link names a"),
        "the sweep listing names a: {v:#}"
    );
    // The completion still reached x.
    assert!(env
        .events_text("x")
        .iter()
        .any(|e| e.contains("child_completed")));

    settle();
    let out = env.step(&[], None, &[], || env.prune(&["--apply", "--all"], &[]));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    assert!(kept_reason(&out, "b").contains("a (koto_session)"));
    let out = env.step(&[], None, &[], || env.workspace_prune("x", &[]));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    assert!(
        kept_reason(&out, "b").contains("a (koto_session)"),
        "{out:#}"
    );

    // b's log and keys stay readable from a.
    assert!(env.keys("b").contains(&"notes/b".to_string()));
    let h = env.hierarchy("a", &[]);
    assert!(listed(&h, "members").contains(&"b".to_string()), "{h:#}");
    env.read_from_another_agent_session("b", "notes/b");
    env.read_with_no_anchor("b", "notes/b");
    assert!(env.exists("x"));
}

#[test]
fn a_finished_child_stays_readable_from_its_live_dispatcher() {
    finished_child(false);
}

/// A alive, B dispatched by A finished, C dispatched by B finishes.
fn grandchild(no_request_store: bool) {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.dispatched("c", "b");
    env.add_key("c", "notes/c", "c's notes");
    if no_request_store {
        env.remove_request_store();
    }
    let v = env.step(&["b"], None, &[], || env.tick("b", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let v = env.step(&["c"], None, &[], || env.tick("c", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    assert!(env.keys("c").contains(&"notes/c".to_string()));
    settle();
    let out = env.step(&[], None, &[], || env.prune(&["--apply", "--all"], &[]));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    for name in ["b", "c"] {
        assert!(
            kept_reason(&out, name).contains("a (koto_session)"),
            "{name}"
        );
    }
    assert!(env.exists("b") && env.exists("c"));
    env.read_from_another_agent_session("c", "notes/c");
    env.read_with_no_anchor("c", "notes/c");
}

#[test]
fn a_grandchild_stays_readable_from_the_live_root() {
    grandchild(false);
}

/// C dispatched by both A and B; A finishes, B stays alive, D (dispatched by
/// A only) finishes.
fn co_parent(no_request_store: bool) {
    let env = Env::new();
    env.init("a");
    env.init("b");
    env.dispatched("c", "a");
    env.ok(&["session", "link", "c", "--dispatched-by", "b"]);
    env.dispatched("d", "a");
    if no_request_store {
        env.remove_request_store();
    }
    for name in ["c", "d", "a"] {
        let v = env.step(&[name], None, &[], || env.tick(name, false));
        assert_eq!(
            retention(&v),
            (true, Some("hierarchy".to_string())),
            "{name}"
        );
    }
    settle();
    for args in [&[][..], &["--apply", "--all"][..]] {
        let out = env.step(&[], None, &[], || env.prune(args, &[]));
        for name in ["a", "c", "d"] {
            let why = kept_reason(&out, name);
            assert!(why.contains("b (koto_session)"), "{name}: {why}");
        }
    }
    for name in ["a", "c", "d"] {
        let h = env.hierarchy(name, &[]);
        assert_eq!(keeper_names(&h), vec!["b"], "{name}: {h:#}");
    }
}

#[test]
fn a_co_parent_keeps_what_both_dispatched() {
    co_parent(false);
}

#[test]
fn no_lineage_or_request_store_is_needed() {
    finished_child(true);
    grandchild(true);
    co_parent(true);
}

// ===== Release instant, absent peer, observed anchors =====

fn release_instant(cancel_later: bool) {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.tick("a", false);
    if cancel_later {
        env.ok(&["cancel", "b"]);
    } else {
        env.tick("b", false);
    }
    let keep = [("KOTO_RETENTION_KEEP", "20s")];
    let now = SystemTime::now();
    // 22 seconds after the first finish, the later 12 seconds after it.
    env.set_all_event_times("a", now - Duration::from_secs(22));
    env.set_all_event_times("b", now - Duration::from_secs(10));
    let out = env.step(&[], None, &[], || env.prune(&["--apply", "--all"], &keep));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    let kind = if cancel_later {
        "cancelled"
    } else {
        "terminal"
    };
    for name in ["a", "b"] {
        let why = kept_reason(&out, name);
        assert!(why.contains(&format!("when b stopped ({kind})")), "{why}");
        assert!(why.contains("the retention passes at"), "{why}");
    }
    // 40 seconds after the first finish.
    env.set_all_event_times("a", now - Duration::from_secs(40));
    env.set_all_event_times("b", now - Duration::from_secs(28));
    let out = env.step(&[], None, &["a", "b"], || {
        env.prune(&["--apply", "--all"], &keep)
    });
    assert_eq!(listed(&out, "removed"), vec!["a", "b"], "{out:#}");
}

#[test]
fn the_release_instant_is_the_later_stop_finished_or_cancelled() {
    release_instant(false);
    release_instant(true);
}

#[test]
fn an_absent_peer_keeps_nothing_past_the_retention() {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.ok(&["session", "cleanup", "a"]);
    let v = env.step(&["b"], None, &[], || env.tick("b", false));
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let out = env.step(&[], None, &[], || env.prune(&["--apply", "--all"], &LONG));
    assert!(out["removed"].as_array().unwrap().is_empty());
    settle();
    let out = env.step(&[], None, &["b"], || env.prune(&["--apply", "--all"], &[]));
    assert_eq!(listed(&out, "removed"), vec!["b"]);
}

#[test]
fn an_observed_anchor_holds_until_its_close_command_runs() {
    let env = Env::new();
    let short = [
        ("KOTO_RETENTION_KEEP", "1s"),
        ("KOTO_RETENTION_IDLE_PERIOD", "1s"),
        ("KOTO_RETENTION_STALE_AFTER", "2s"),
    ];
    let open = |harness: &str, key: &str| {
        env.ok(&[
            "session",
            "anchor",
            "open",
            "--harness-session",
            harness,
            "--source",
            "startup",
            "--observed-key",
            key,
        ]);
        format!("anchor-{harness}")
    };
    let holder = open("H1", "KH");
    let dispatcher_anchor = open("D1", "KD");
    env.init_with("d", &[], &[("KOTO_ANCHOR", dispatcher_anchor.as_str())]);
    env.init_with(
        "b",
        &["--dispatched-by", "d"],
        &[("KOTO_ANCHOR", holder.as_str())],
    );
    env.tick("d", false);
    env.tick("b", false);
    // b's dispatcher's anchor is closed by an observed close.
    env.ok(&[
        "session",
        "anchor",
        "close",
        "--key=KD",
        "--harness-session=D1",
    ]);
    // Past the idle period and the stale threshold since the holder's last
    // activity: it stays open, and b is kept.
    std::thread::sleep(Duration::from_millis(2300));
    let command = "koto session anchor close --key='KH' --harness-session='H1'";
    for args in [&[][..], &["--apply", "--all"][..]] {
        let out = env.step(&[], None, &[], || env.prune(args, &short));
        let why = kept_reason(&out, "b");
        assert!(why.contains(&holder), "{why}");
        let stale = &out["stale_anchors"];
        assert!(stale.to_string().contains(command), "{out:#}");
    }
    let h = env.hierarchy("b", &short);
    let keeper = &h["keepers"][0];
    assert_eq!(keeper["name"], holder.as_str());
    assert_eq!(keeper["stale"], true);
    assert_eq!(keeper["close_command"], command);

    // Running the printed command closes it as observed.
    env.ok(&[
        "session",
        "anchor",
        "close",
        "--key=KH",
        "--harness-session=H1",
    ]);
    let h = env.hierarchy("b", &LONG);
    assert_eq!(h["retention"]["release"]["close_kind"], "observed", "{h:#}");
    let out = env.step(&[], None, &[], || env.prune(&["--apply", "--all"], &LONG));
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    assert!(env.exists("b"));
    settle();
    let out = env.prune(&["--apply", "--all"], &[]);
    assert!(
        listed(&out, "removed").contains(&"b".to_string()),
        "{out:#}"
    );
}

// ===== Delivery =====

#[test]
fn a_kept_sessions_leg_still_gets_its_result() {
    let env = Env::new();
    let req = env.ok(&[
        "request",
        "create",
        "--with-data",
        &format!(
            r#"{{"legs":[{{"name":"work","role":"w","template":{:?},"inputs":{{}}}}],"inputs":{{}}}}"#,
            env.template.file_name().unwrap().to_str().unwrap()
        ),
        "--requested-by",
        "coord",
        "--coordinator-of-record",
        "coord",
    ]);
    let id = req["request_id"].as_str().unwrap().to_string();
    env.init("coord");
    env.init_with(
        "worker",
        &[
            "--attach-live",
            "--replace-terminal",
            "--koto-leg",
            &format!("{id}:work"),
        ],
        &[],
    );
    let v = env.tick("worker", false);
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let view = env.ok(&["request", "get", &id]);
    assert_eq!(
        view["legs"]["work"]["result_source"], "promoted",
        "{view:#}"
    );
    assert!(env.exists("worker"));
}

// ===== The re-run =====

#[test]
fn a_re_run_moves_a_held_predecessor_aside() {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.add_key("b", "notes/b", "first run");
    env.tick("b", false);
    let links = env.hashes("b");
    let events = env.events_text("b");

    // The subtree moves; a and every session outside it hash the same.
    let out = env.step(&["b", "b_r1"], None, &["b"], || env.replace("b", &[]));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "replaced");
    assert_eq!(v["moved_to"], "b_r1", "{v:#}");
    assert_eq!(v["replaced_state"], "done");
    assert!(v["moved_reason"]
        .as_str()
        .unwrap()
        .contains("a (koto_session)"));

    // The earlier run reads from a under its new name: events, keys and
    // links unchanged, only the header's name rewritten.
    assert_eq!(env.events_text("b_r1"), events);
    assert!(env.keys("b_r1").contains(&"notes/b".to_string()));
    let moved = env.hashes("b_r1");
    for (path, hash) in &links {
        if path.to_string_lossy().starts_with("ctx") {
            assert_eq!(moved.get(path), Some(hash), "{}", path.display());
        }
    }
    let h = env.hierarchy("a", &[]);
    assert!(listed(&h, "members").contains(&"b_r1".to_string()), "{h:#}");
    // The new run holds the name, with no children and a fresh log.
    assert_eq!(env.header("b")["workflow"], "b");
    assert!(env.keys("b").iter().all(|k| k != "notes/b"));

    // A second re-run gives a third, distinct name. The new run is linked
    // to a again, as its dispatcher would link it, so its tick keeps it.
    env.ok(&["session", "link", "b", "--dispatched-by", "a"]);
    env.tick("b", false);
    let out = env.replace("b", &[]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["moved_to"], "b_r2", "{v:#}");

    // With nothing alive and the retention passed, a re-run replaces as
    // before.
    env.ok(&["session", "link", "b", "--dispatched-by", "a"]);
    env.tick("a", false);
    env.tick("b", false);
    settle();
    let out = env.step(&["b"], None, &["b"], || env.replace("b", &[]));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "replaced");
    assert!(v.get("moved_to").is_none(), "{v:#}");
    assert!(!env.exists("b_r3"));
}

#[test]
fn a_re_run_moves_the_predecessors_koto_subtree_with_it() {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.child("b.task", "b");
    env.child("helper", "b");
    env.tick("b.task", false);
    env.tick("helper", false);
    env.tick("b", false);
    let out = env.step(
        &["b", "b.task", "helper", "b_r1", "b_r1.task"],
        None,
        &["b", "b.task"],
        || env.replace("b", &[]),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["moved_to"], "b_r1");
    assert!(env.exists("b_r1.task") && !env.exists("b.task"));
    assert_eq!(env.header("b_r1.task")["parent_workflow"], "b_r1");
    assert_eq!(env.header("helper")["parent_workflow"], "b_r1");
    assert!(env.header("b_r1")["parent_workflow"].is_null());
    // The new run starts with no children.
    let children = env.ok(&["workflows", "--children", "b"]);
    assert_eq!(children.as_array().map(Vec::len), Some(0), "{children:#}");
}

#[test]
fn a_live_descendant_refuses_the_re_run() {
    let env = Env::new();
    env.init("a");
    env.dispatched("b", "a");
    env.child("b.task", "b");
    env.tick("b", false);
    let before = env.snapshot_except(&[]);
    let out = env.replace("b", &[]);
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "replace_held_live", "{v:#}");
    assert!(v["error"].as_str().unwrap().contains("b.task"));
    assert_eq!(env.snapshot_except(&[]), before);
}

#[test]
fn a_malformed_retention_setting_at_a_replace_moves_aside_with_a_warning() {
    let env = Env::new();
    env.init("b");
    env.tick("b", true);
    let out = env.replace("b", &[("KOTO_RETENTION_KEEP", "7x")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["moved_to"], "b_r1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("retention.keep"), "{stderr}");
    assert!(stderr.contains("moved aside"), "{stderr}");
}

#[test]
fn a_released_replace_still_sweeps_the_predecessors_children() {
    let env = Env::new();
    env.init("b");
    env.child("b.task", "b");
    env.tick("b.task", false);
    env.tick("b", false);
    settle();
    let out = env.replace("b", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.get("moved_to").is_none(), "{v:#}");
    assert!(!env.exists("b.task"));
    assert!(!env.exists("b_r1"));
}

// ===== The re-run under the cloud backend =====

fn object_key(env: &Env, name: &str, file: &str) -> String {
    format!("{}/{}/{}", env.prefix(), name, file)
}

/// The cross-session read steps under the cloud backend: a finished child
/// read from another agent session, with the reader's anchor named and with
/// none, keeps its files and its objects in the bucket as they were; the
/// anchored read goes into the reader's anchor and the anchorless one into
/// the spool on this host.
#[test]
fn under_the_cloud_backend_reads_of_another_session_leave_it_byte_identical() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("a");
    // b's own agent session: its anchor holds b.
    let out = env.run(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "ownb",
            "--source",
            "startup",
        ],
        &[],
    );
    assert!(out.status.success(), "anchor open: {out:?}");
    let own = [("KOTO_ANCHOR", "anchor-ownb")];
    env.init_with("b", &["--dispatched-by", "a"], &own);
    env.add_key("b", "notes/b", "b's notes");
    let v = env.tick("b", false);
    assert_eq!(retention(&v), (true, Some("hierarchy".to_string())));
    let objects = |name: &str| -> Vec<(String, Vec<u8>)> {
        s3.keys_under(&format!("{}/{name}/", env.prefix()))
            .into_iter()
            .map(|k| {
                let v = s3.object(&k).unwrap();
                (k, v)
            })
            .collect()
    };
    let before = objects("b");
    env.read_from_another_agent_session("b", "notes/b");
    env.read_with_no_anchor("b", "notes/b");
    assert_eq!(objects("b"), before, "b's objects in the bucket changed");
    assert!(
        objects("b").iter().all(|(k, _)| !k.ends_with(SPOOL)),
        "the spool reached the bucket"
    );

    // An own read, from b's own agent session outside any tick, goes into
    // b's log under the cloud backend too, not to the spool.
    let spooled = env.spooled("b").len();
    let events = env.events_text("b").len();
    let out = env.run(&["context", "get", "b", "notes/b"], &own);
    assert!(out.status.success(), "own read: {out:?}");
    let added = &env.events_text("b")[events..];
    assert_eq!(added.len(), 1, "{added:?}");
    let read: Value = serde_json::from_str(&added[0]).unwrap();
    assert_eq!(read["type"], "context_read");
    assert!(read["payload"].get("session").is_none());
    assert_eq!(env.spooled("b").len(), spooled, "an own read was spooled");
}

#[test]
fn under_the_cloud_backend_a_re_run_copies_then_deletes_the_old_prefix() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("a");
    env.dispatched("b", "a");
    env.tick("b", false);
    // Something under b_r10 says nothing about b_r1.
    s3.put(&object_key(&env, "b_r10", "x"), b"x");
    let out = env.replace("b", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["moved_to"], "b_r1", "{v:#}");
    // The moved session's objects are under its new prefix, its log under
    // its new name; the old prefix holds only the new run.
    let moved = s3.keys_under(&format!("{}/b_r1/", env.prefix()));
    assert!(
        moved.contains(&object_key(&env, "b_r1", "koto-b_r1.state.jsonl")),
        "{moved:?}"
    );
    assert!(moved.iter().any(|k| k.contains("/ctx/")), "{moved:?}");
    let old = s3.keys_under(&format!("{}/b/", env.prefix()));
    assert!(
        old.iter().all(|k| !k.contains("koto/links")),
        "the old prefix still holds the moved run: {old:?}"
    );
    // The listing that checked the name ended in `/`.
    assert!(s3
        .requests()
        .iter()
        .any(|r| r.list_prefix.as_deref() == Some(&format!("{}/b_r1/", env.prefix()))));
}

#[test]
fn under_the_cloud_backend_a_failed_listing_refuses_the_replace() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("a");
    env.dispatched("b", "a");
    env.tick("b", false);
    s3.fail("LIST", &format!("{}/b_r1/", env.prefix()), 500);
    let before = env.snapshot_except(&[]);
    let out = env.replace("b", &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "replace_failed", "{v:#}");
    assert_eq!(env.snapshot_except(&[]), before);
}

#[test]
fn under_the_cloud_backend_a_failed_copy_moves_everything_back() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("a");
    env.dispatched("b", "a");
    env.tick("b", false);
    let link = s3
        .keys_under(&format!("{}/b/", env.prefix()))
        .into_iter()
        .find(|k| k.contains("koto/links"))
        .expect("b's link was pushed");
    let target = link.replacen("/b/", "/b_r1/", 1);
    s3.fail("PUT", &target, 500);
    let before = env.snapshot_except(&[]);
    let out = env.replace("b", &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "replace_failed", "{v:#}");
    assert_eq!(env.snapshot_except(&[]), before);
    assert!(env.exists("b") && !env.exists("b_r1"));
    // The old prefix still holds everything; the new one was emptied.
    assert!(s3.object(&link).is_some());
    assert!(s3.keys_under(&format!("{}/b_r1/", env.prefix())).is_empty());
}

#[test]
fn under_the_cloud_backend_a_failed_old_prefix_delete_rolls_forward() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("a");
    env.dispatched("b", "a");
    env.tick("b", false);
    let link = s3
        .keys_under(&format!("{}/b/", env.prefix()))
        .into_iter()
        .find(|k| k.contains("koto/links"))
        .expect("b's link was pushed");
    s3.fail("DELETE", &link, 500);
    let out = env.replace("b", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["moved_to"], "b_r1");
    assert_eq!(v["outcome"], "replaced");
    assert_eq!(v["undeleted_remote_prefixes"][0]["name"], "b", "{v:#}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("old remote prefix"));
    assert!(env.exists("b_r1") && env.exists("b"));
}
