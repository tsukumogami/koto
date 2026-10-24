//! `koto session prune`, driven through the real binary against sessions koto
//! itself created, in a scratch HOME. The cloud cases run against the
//! in-process fake S3 endpoint.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `start` waits for `go`; `yes` reaches the success terminal `done`, `fail`
/// the failure terminal `failed`.
const TEMPLATE: &str = r#"---
name: prune-test
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes", "fail"]
    transitions:
      - target: done
        when:
          go: "yes"
      - target: failed
        when:
          go: "fail"
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## start

Start.

## done

Done.

## failed

Failed.
"#;

const BUCKET: &str = "koto-session-prune";

/// The retention every command runs with unless it says otherwise: one
/// second, so a test can wait it out.
const FAST: [(&str, &str); 3] = [
    ("KOTO_RETENTION_KEEP", "1s"),
    ("KOTO_RETENTION_IDLE_PERIOD", "1h"),
    ("KOTO_RETENTION_STALE_AFTER", "2h"),
];

/// A retention no test waits out.
const LONG: [(&str, &str); 1] = [("KOTO_RETENTION_KEEP", "1h")];

/// Wait out [`FAST`]'s retention.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1300));
}

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    root: PathBuf,
    template: PathBuf,
    /// The workspace every command runs in.
    work: PathBuf,
    /// Set for the cloud backend: the endpoint and the credentials go to
    /// every command.
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
        let template = root.join("prune-test.md");
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

    /// An environment whose workspace uses the cloud backend on `s3`.
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

    /// A directory under the scratch root, created and canonical.
    fn dir(&self, rel: &str) -> PathBuf {
        let d = self.root.join(rel);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    /// Every command runs with a one-second retention, so a finished
    /// hierarchy is released after [`settle`]; a command can override it.
    fn cmd_in(&self, cwd: &Path) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(cwd)
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
        cmd
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        self.cmd_in(cwd).args(args).output().unwrap()
    }

    /// Run `koto` in the workspace with extra environment.
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = self.cmd_in(&self.work);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.args(args).output().unwrap()
    }

    fn ok_env(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run_env(args, env);
        assert!(
            out.status.success(),
            "koto {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ok_in(&self, cwd: &Path, args: &[&str]) -> Value {
        let out = self.run_in(cwd, args);
        assert!(
            out.status.success(),
            "koto {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_in(&self.work, args)
    }

    /// A running session.
    fn init(&self, name: &str) {
        self.init_in(&self.work, name);
    }

    fn init_in(&self, cwd: &Path, name: &str) {
        self.ok_in(
            cwd,
            &["init", name, "--template", self.template.to_str().unwrap()],
        );
    }

    /// A running child of `parent` by header.
    fn init_child(&self, name: &str, parent: &str) {
        self.ok(&[
            "init",
            name,
            "--template",
            self.template.to_str().unwrap(),
            "--parent",
            parent,
        ]);
    }

    /// Drive `name` to `done` (`yes`) or `failed` (`fail`), keeping it.
    fn finish(&self, name: &str, go: &str) {
        self.finish_in(&self.work, name, go);
    }

    fn finish_in(&self, cwd: &Path, name: &str, go: &str) {
        let data = format!("{{\"go\": \"{go}\"}}");
        self.ok_in(cwd, &["next", name, "--with-data", &data, "--no-cleanup"]);
    }

    fn done(&self, name: &str) {
        self.init(name);
        self.finish(name, "yes");
    }

    /// A running session, with extra `init` arguments and environment.
    fn init_with(&self, name: &str, extra: &[&str], env: &[(&str, &str)]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok_env(&args, env);
    }

    /// An observed anchor for harness session `harness` with marker `key`.
    fn anchor(&self, harness: &str, key: &str) -> String {
        self.ok(&[
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
    }

    fn close_anchor(&self, harness: &str, key: &str) {
        let out = self.run_in(
            &self.work,
            &[
                "session",
                "anchor",
                "close",
                &format!("--key={key}"),
                &format!("--harness-session={harness}"),
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn hierarchy(&self, name: &str, env: &[(&str, &str)]) -> Value {
        self.ok_env(&["session", "hierarchy", name, "--json"], env)
    }

    /// SHA-256 of every file in session `name`'s directory, by relative
    /// path.
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
                } else if meta.is_file() {
                    let digest = Sha256::digest(std::fs::read(&p).unwrap());
                    out.insert(
                        p.strip_prefix(&root).unwrap().to_path_buf(),
                        hex::encode(digest),
                    );
                }
            }
        }
        assert!(!out.is_empty(), "{name} has no files");
        out
    }

    /// The hashes of each named session, for a single-writer check.
    fn snapshot(&self, names: &[&str]) -> BTreeMap<String, BTreeMap<PathBuf, String>> {
        names
            .iter()
            .map(|n| (n.to_string(), self.hashes(n)))
            .collect()
    }

    /// Set every event's timestamp in `name`'s log to `at`.
    fn set_all_event_times(&self, name: &str, at: std::time::SystemTime) {
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

    fn set_chain_parent(&self, name: &str, value: &str) {
        let file = self.root.join(format!("chain-{name}"));
        std::fs::write(&file, value).unwrap();
        self.ok(&[
            "context",
            "add",
            name,
            "chain/parent",
            "--from-file",
            file.to_str().unwrap(),
        ]);
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn journal(&self) -> Vec<u8> {
        std::fs::read(self.home.join(".koto/_run_journal.jsonl")).unwrap_or_default()
    }

    fn prune(&self, args: &[&str]) -> std::process::Output {
        let mut all = vec!["session", "prune"];
        all.extend_from_slice(args);
        self.run_in(&self.work, &all)
    }

    fn prune_ok(&self, args: &[&str]) -> Value {
        let out = self.prune(args);
        assert!(
            out.status.success(),
            "prune {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Write a leg pointer into `name`'s directory by hand.
    fn point_at_leg(&self, name: &str, request_id: &str, leg: &str) {
        std::fs::write(
            self.sessions().join(name).join("request-leg.toml"),
            format!(
                "request_id = {request_id:?}\nleg_name = {leg:?}\nbound_at = \"2026-10-12T00:00:00Z\"\n"
            ),
        )
        .unwrap();
    }

    /// Create a request with one open leg, `reviewer`, and return its id.
    fn open_request(&self) -> String {
        let out = self.ok(&[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"reviewer","role":"r","template":"review","inputs":{}}],"inputs":{}}"#,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ]);
        out["request_id"].as_str().unwrap().to_string()
    }
}

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

fn rules(preview: &Value) -> BTreeMap<String, (String, String)> {
    preview["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["name"].as_str().unwrap().to_string(),
                (
                    k["rule"].as_str().unwrap().to_string(),
                    k["detail"].as_str().unwrap().to_string(),
                ),
            )
        })
        .collect()
}

fn error_code(out: &std::process::Output) -> String {
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    v["error"]["code"].as_str().unwrap().to_string()
}

/// One protected session per local rule, each named after the rule, and four
/// sessions nothing protects. Returns the protected names with their rule.
fn build_store(env: &Env) -> BTreeMap<&'static str, &'static str> {
    // a: running. (An unreadable session has a test of its own: one whose
    // header can't be read keeps every finished session.)
    env.init("a-running");

    // b: a finished parent with a running child by header.
    env.init("b-parent");
    env.init_child("b-child-running", "b-parent");
    env.finish("b-parent", "yes");
    // b: a finished parent whose cancelled chain left a running child
    // naming it through chain/parent.
    env.done("b-chain-parent");
    env.init("b-chain-child");
    env.set_chain_parent("b-chain-child", "b-chain-parent");

    // c: a finished child of a running parent by header.
    env.init("c-parent-running");
    env.init_child("c-child", "c-parent-running");
    env.finish("c-child", "yes");
    // c: a finished session whose chain/parent names a running session.
    env.init("c-chain-target");
    env.done("c-chain-source");
    env.set_chain_parent("c-chain-source", "c-chain-target");

    // d: a malformed chain/parent.
    env.done("d-malformed");
    env.set_chain_parent("d-malformed", "c-chain-target\n");

    // e: bound to a waiting leg, and to a leg pointer that can't be read.
    let request = env.open_request();
    env.done("e-waiting");
    env.point_at_leg("e-waiting", &request, "reviewer");
    env.done("e-unreadable");
    std::fs::write(
        env.sessions().join("e-unreadable/request-leg.toml"),
        "this is not toml = = =",
    )
    .unwrap();

    // f: a coordinator's session.
    env.done("coordinate-f");

    // Two rules at once: a coordinator's session that also references a
    // running one reports the first, c.
    env.done("coordinate-c");
    env.set_chain_parent("coordinate-c", "c-chain-target");

    // Nothing protects these: a success, a failure, a cancel, and a session
    // bound to a request that doesn't exist.
    env.done("free-done");
    env.init("free-failed");
    env.finish("free-failed", "fail");
    env.init("free-cancelled");
    env.ok(&["cancel", "free-cancelled"]);
    env.done("free-released");
    env.point_at_leg("free-released", "req-missing-for-prune-test", "reviewer");

    // Wait out the retention, so only the rules keep anything.
    settle();

    // Rules b and c became rule h: a session joined to a running one is
    // kept because its hierarchy is. A malformed chain/parent taints its
    // own hierarchy, so h keeps that one before d is reached.
    BTreeMap::from([
        ("a-running", "a"),
        ("b-parent", "h"),
        ("b-child-running", "a"),
        ("b-chain-parent", "h"),
        ("b-chain-child", "a"),
        ("c-parent-running", "a"),
        ("c-child", "h"),
        ("c-chain-target", "a"),
        ("c-chain-source", "h"),
        ("d-malformed", "h"),
        ("e-waiting", "e"),
        ("e-unreadable", "e"),
        ("coordinate-f", "f"),
        ("coordinate-c", "h"),
    ])
}

const FREE: [&str; 4] = [
    "free-cancelled",
    "free-done",
    "free-failed",
    "free-released",
];

#[test]
fn each_rule_keeps_its_session_and_the_preview_names_it() {
    let env = Env::new();
    let protected = build_store(&env);
    let journal = env.journal();

    let preview = env.prune_ok(&[]);
    assert_eq!(preview["mode"], "preview");
    assert_eq!(names(&preview["remove"]), FREE.to_vec());
    let kept = rules(&preview);
    for (name, rule) in &protected {
        let (got, detail) = kept
            .get(*name)
            .unwrap_or_else(|| panic!("{name} isn't kept: {preview:#}"));
        assert_eq!(got, rule, "{name}: {detail}");
    }
    assert_eq!(kept.len(), protected.len(), "{kept:#?}");
    assert!(kept["b-parent"].1.contains("b-child-running"));
    assert!(kept["b-chain-parent"].1.contains("b-chain-child"));
    assert!(kept["c-child"].1.contains("c-parent-running"));
    assert!(kept["e-waiting"].1.contains("reviewer"));
    assert!(
        kept["d-malformed"].1.contains("chain/parent"),
        "{}",
        kept["d-malformed"].1
    );

    // Rule h's entry carries the JSON `koto session hierarchy` prints for
    // the same session (its `now` aside), and its detail is that verdict's
    // reason.
    let entry = preview["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "b-parent")
        .unwrap();
    let mut from_prune = entry["hierarchy"].clone();
    let mut from_query = env.hierarchy("b-parent", &[]);
    for v in [&mut from_prune, &mut from_query] {
        v.as_object_mut().unwrap().remove("now");
    }
    assert_eq!(from_prune, from_query);
    assert_eq!(
        entry["detail"],
        "kept: alive in its hierarchy: b-child-running (koto_session)"
    );
    // Rule f's doesn't: its hierarchy is released.
    let f = preview["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "coordinate-f")
        .unwrap();
    assert!(f.get("hierarchy").is_none());

    // A removable row carries the listing's fields.
    let row = preview["remove"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "free-failed")
        .unwrap();
    assert_eq!(row["kind"], "session");
    assert_eq!(row["status"], "terminal");
    assert_eq!(row["state"], "failed");
    assert_eq!(row["failure"], true);
    assert!(row["session_id"].is_string());
    assert!(row["finished_at"].is_string());
    assert_eq!(row["execution_dir"], env.work.to_str().unwrap());
    let cancelled = preview["remove"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "free-cancelled")
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");

    // The preview stores its plan privately and removes nothing.
    let token = preview["plan"].as_str().unwrap();
    let plans = env.sessions().join(".prune-plans");
    let stored = plans.join(format!("{token}.json"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&plans).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for name in FREE {
        assert!(env.sessions().join(name).is_dir());
    }
    assert_eq!(env.journal(), journal);

    // The stored plans directory isn't a session to anything that lists.
    let rows = env.ok(&["session", "list"]);
    assert!(!rows
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == ".prune-plans"));
}

fn assert_only_free_removed(env: &Env, protected: &BTreeMap<&str, &str>) {
    for name in protected.keys() {
        assert!(
            env.sessions().join(name).is_dir(),
            "protected {name} was removed"
        );
    }
    for name in FREE {
        assert!(!env.sessions().join(name).exists(), "{name} is still there");
    }
}

/// Write a session directory whose state file has no readable header.
fn header_less(env: &Env, name: &str) {
    let dir = env.sessions().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("koto-{name}.state.jsonl")), "not json\n").unwrap();
}

#[test]
fn a_header_less_session_keeps_every_finished_session_until_it_is_dealt_with() {
    let env = Env::new();
    env.done("parent");
    env.done("other");
    // A child whose header was half-written: nothing can say which session
    // it names as its parent.
    header_less(&env, "kid");

    let preview = env.prune_ok(&[]);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    let kept = rules(&preview);
    assert_eq!(kept["kid"].0, "a");
    for name in ["parent", "other"] {
        assert_eq!(kept[name].0, "h", "{name}: {}", kept[name].1);
        assert!(kept[name].1.contains("kid: "), "{}", kept[name].1);
    }
    let token = preview["plan"].as_str().unwrap().to_string();

    // Neither apply removes the parent.
    let out = env.prune_ok(&["--apply", "--all"]);
    assert!(out["removed"].as_array().unwrap().is_empty());
    let out = env.prune_ok(&["--apply", "--plan", &token]);
    assert!(out["removed"].as_array().unwrap().is_empty());
    assert!(env.sessions().join("parent").is_dir());
    assert!(env.sessions().join("other").is_dir());

    // Once a person removes the unreadable session, the prune goes on.
    env.ok(&["session", "cleanup", "kid"]);
    settle();
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["other", "parent"]);
}

#[test]
fn apply_all_removes_only_what_no_rule_protects() {
    let env = Env::new();
    let protected = build_store(&env);
    let journal = env.journal();

    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(out["mode"], "apply");
    assert_eq!(names(&out["removed"]), FREE.to_vec());
    assert!(out["skipped"].as_array().unwrap().is_empty());
    assert!(out["failed"].as_array().unwrap().is_empty());
    assert_eq!(rules(&out).len(), protected.len());
    assert_only_free_removed(&env, &protected);

    // The journal is untouched, and the pruned directories, sidecars
    // included, are gone.
    assert_eq!(env.journal(), journal);
    assert!(!env.sessions().join("free-done/run-journal.json").exists());
}

#[test]
fn apply_with_the_preview_token_removes_exactly_the_preview() {
    let env = Env::new();
    let protected = build_store(&env);
    let preview = env.prune_ok(&[]);
    let token = preview["plan"].as_str().unwrap().to_string();

    let out = env.prune_ok(&["--apply", "--plan", &token]);
    assert_eq!(out["plan"], token.as_str());
    assert_eq!(names(&out["removed"]), FREE.to_vec());
    assert_only_free_removed(&env, &protected);
    // A successful apply deletes its stored plan.
    assert!(!env
        .sessions()
        .join(format!(".prune-plans/{token}.json"))
        .exists());
}

#[test]
fn a_stale_plan_removes_nothing_and_names_what_changed() {
    let env = Env::new();
    env.done("one");
    env.done("two");
    env.init("three");
    settle();
    let preview = env.prune_ok(&[]);
    let token = preview["plan"].as_str().unwrap().to_string();
    assert_eq!(names(&preview["remove"]), vec!["one", "two"]);

    // A session outside the list changing leaves the token alone.
    env.init("four");
    assert_eq!(env.prune_ok(&[])["plan"], token.as_str());

    // `three` finishes and `two` is recreated under the same name.
    env.finish("three", "yes");
    env.ok(&["session", "cleanup", "two"]);
    env.done("two");
    settle();

    let out = env.prune(&["--apply", "--plan", &token]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(error_code(&out), "prune_plan_stale");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let added: Vec<_> = v["error"]["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    let dropped: Vec<_> = v["error"]["dropped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    let mut added_sorted = added.clone();
    added_sorted.sort();
    assert_eq!(added_sorted, vec!["three", "two"]);
    assert_eq!(dropped, vec!["two"]);
    for name in ["one", "two", "three", "four"] {
        assert!(env.sessions().join(name).is_dir(), "{name} was removed");
    }
}

#[test]
fn tokens_and_arguments_are_checked_before_anything_is_removed() {
    let env = Env::new();
    env.done("one");

    // A well-formed token no preview stored.
    let out = env.prune(&["--apply", "--plan", "0123456789abcdef0123456789abcdef"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(error_code(&out), "prune_plan_unknown");

    for args in [
        &["--apply", "--plan", "not-a-token"][..],
        &["--apply", "--plan", "0123456789ABCDEF0123456789ABCDEF"][..],
        &["--plan", "0123456789abcdef0123456789abcdef"][..],
        &["--apply"][..],
        &["--older-than", "3 days"][..],
        &["--older-than", "1w"][..],
    ] {
        let out = env.prune(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(error_code(&out), "invalid_arguments", "{args:?}");
    }
    assert!(env.sessions().join("one").is_dir());
}

#[test]
fn an_empty_set_previews_and_applies() {
    let env = Env::new();
    env.init("running");
    let preview = env.prune_ok(&[]);
    assert!(preview["remove"].as_array().unwrap().is_empty());
    let token = preview["plan"].as_str().unwrap().to_string();
    let out = env.prune_ok(&["--apply", "--plan", &token]);
    assert!(out["removed"].as_array().unwrap().is_empty());
    let out = env.prune_ok(&["--apply", "--all"]);
    assert!(out["removed"].as_array().unwrap().is_empty());
    assert!(env.sessions().join("running").is_dir());

    // A store with no sessions directory at all previews too.
    let empty = Env::new();
    let preview = empty.prune_ok(&[]);
    assert!(preview["remove"].as_array().unwrap().is_empty());
}

#[test]
fn filters_combine_and_an_excluded_session_still_protects() {
    let env = Env::new();
    let a = env.dir("ws-a");
    let b = env.dir("ws-b");
    let now = std::time::SystemTime::now();
    let hour = std::time::Duration::from_secs(3600);
    let margin = std::time::Duration::from_secs(60);

    // In ws-a: one just past an hour old, one just inside it.
    env.init_in(&a, "a-old");
    env.finish_in(&a, "a-old", "yes");
    env.set_all_event_times("a-old", now - hour - margin);
    env.init_in(&a, "a-young");
    env.finish_in(&a, "a-young", "yes");
    env.set_all_event_times("a-young", now - hour + margin);
    // In ws-a: an old finished parent whose running child lives in ws-b.
    env.init_in(&a, "a-parent");
    env.finish_in(&a, "a-parent", "yes");
    env.set_all_event_times("a-parent", now - hour - margin);
    env.init_in(&b, "b-child");
    let file = env.root.join("chain-b-child");
    std::fs::write(&file, "a-parent").unwrap();
    env.ok_in(
        &b,
        &[
            "context",
            "add",
            "b-child",
            "chain/parent",
            "--from-file",
            file.to_str().unwrap(),
        ],
    );
    // In ws-b: an old finished session the directory filter leaves out.
    env.init_in(&b, "b-old");
    env.finish_in(&b, "b-old", "yes");
    env.set_all_event_times("b-old", now - hour - margin);

    let a_arg = a.to_str().unwrap();
    let preview = env.prune_ok(&["--execution-dir", a_arg, "--older-than", "1h"]);
    assert_eq!(names(&preview["remove"]), vec!["a-old"]);
    let kept = rules(&preview);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept["a-parent"].0, "h");
    assert!(
        kept["a-parent"].1.contains("b-child"),
        "{}",
        kept["a-parent"].1
    );

    // Each filter alone.
    let preview = env.prune_ok(&["--execution-dir", a_arg]);
    assert_eq!(names(&preview["remove"]), vec!["a-old", "a-young"]);
    let preview = env.prune_ok(&["--older-than", "1h"]);
    assert_eq!(names(&preview["remove"]), vec!["a-old", "b-old"]);

    let out = env.prune_ok(&["--apply", "--execution-dir", a_arg, "--older-than", "1h"]);
    assert_eq!(names(&out["removed"]), vec!["a-old"]);
    for name in ["a-young", "a-parent", "b-child", "b-old"] {
        assert!(env.sessions().join(name).is_dir(), "{name} was removed");
    }
}

#[test]
fn a_plan_applies_with_the_filters_its_preview_ran_with() {
    let env = Env::new();
    let a = env.dir("ws-a");
    let b = env.dir("ws-b");
    env.init_in(&a, "in-a");
    env.finish_in(&a, "in-a", "yes");
    env.init_in(&b, "in-b");
    env.finish_in(&b, "in-b", "yes");
    settle();

    let preview = env.prune_ok(&["--execution-dir", a.to_str().unwrap()]);
    assert_eq!(names(&preview["remove"]), vec!["in-a"]);
    let token = preview["plan"].as_str().unwrap().to_string();

    // Filters can't be given again alongside the plan.
    let out = env.prune(&[
        "--apply",
        "--plan",
        &token,
        "--execution-dir",
        a.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_code(&out), "invalid_arguments");

    let out = env.prune_ok(&["--apply", "--plan", &token]);
    assert_eq!(names(&out["removed"]), vec!["in-a"]);
    assert!(env.sessions().join("in-b").is_dir());
}

#[test]
fn a_session_finishing_between_preview_and_apply_survives_the_apply() {
    let env = Env::new();
    let a = env.dir("ws-a");
    env.init_in(&a, "previewed");
    env.finish_in(&a, "previewed", "yes");
    env.init_in(&a, "late");
    settle();
    let preview = env.prune_ok(&["--execution-dir", a.to_str().unwrap()]);
    assert_eq!(names(&preview["remove"]), vec!["previewed"]);
    let token = preview["plan"].as_str().unwrap().to_string();

    // `late` finishes inside the preview's filter after the preview.
    env.finish_in(&a, "late", "yes");
    settle();
    let out = env.prune(&["--apply", "--plan", &token]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(error_code(&out), "prune_plan_stale");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["added"][0]["name"], "late");
    // Nothing was removed: not the session the preview never named, and not
    // the one it did.
    assert!(env.sessions().join("late").is_dir());
    assert!(env.sessions().join("previewed").is_dir());
}

#[test]
fn a_locked_session_is_skipped_and_the_run_still_succeeds() {
    let env = Env::new();
    env.done("held");
    env.done("free");
    settle();
    let file = std::fs::File::open(env.state_path("held")).unwrap();
    use std::os::unix::io::AsRawFd;
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["free"]);
    assert_eq!(names(&out["skipped"]), vec!["held"]);
    assert!(out["skipped"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("lock"));
    assert!(env.sessions().join("held").is_dir());
    drop(file);
}

/// Bump `name`'s remote version past the local one, as another host's push
/// would.
fn advance_remote(env: &Env, s3: &FakeS3, name: &str) {
    let key = format!("{}/{name}/version.json", env.prefix());
    let bytes = s3
        .object(&key)
        .unwrap_or_else(|| panic!("no {key} among {:?}", s3.keys()));
    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
    v["version"] = Value::from(v["version"].as_u64().unwrap() + 5);
    s3.put(&key, serde_json::to_string(&v).unwrap().as_bytes());
}

#[test]
fn under_the_cloud_backend_a_newer_remote_copy_keeps_its_session() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    let prefix = env.prefix();
    // A context write gives a session a version.json, here and remotely.
    for name in ["version-ahead", "version-unreadable", "even-with-version"] {
        env.done(name);
        env.set_chain_parent(name, "nobody-here");
    }
    env.done("log-ahead");
    env.done("even");
    settle();

    advance_remote(&env, &s3, "version-ahead");
    s3.put(
        &format!("{prefix}/version-unreadable/version.json"),
        b"not json",
    );
    // Another host advanced the session: its pushed log is longer.
    let log_key = format!("{prefix}/log-ahead/koto-log-ahead.state.jsonl");
    let mut log = s3.object(&log_key).unwrap();
    log.extend_from_slice(b"{\"seq\":99}\n");
    s3.put(&log_key, &log);

    let preview = env.prune_ok(&[]);
    assert_eq!(names(&preview["remove"]), vec!["even", "even-with-version"]);
    let kept = rules(&preview);
    for name in ["version-ahead", "version-unreadable", "log-ahead"] {
        assert_eq!(kept[name].0, "g", "{name}: {}", kept[name].1);
    }
    assert!(
        kept["version-ahead"].1.contains("ahead"),
        "{}",
        kept["version-ahead"].1
    );
    assert!(
        kept["log-ahead"].1.contains("log"),
        "{}",
        kept["log-ahead"].1
    );

    // A bucket-only session is never considered.
    s3.put(
        &format!("{prefix}/bucket-only/koto-bucket-only.state.jsonl"),
        b"{}\n",
    );
    let preview = env.prune_ok(&[]);
    assert!(!rules(&preview).contains_key("bucket-only"));
    assert_eq!(names(&preview["remove"]), vec!["even", "even-with-version"]);
}

#[test]
fn under_the_cloud_backend_apply_removes_the_remote_copy_first_and_keeps_the_marker() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.done("gone");
    env.done("stuck");
    settle();
    let prefix = env.prefix();
    // A migration marker beside a session that is pruned stays.
    s3.put(&format!("{prefix}/gone/migrated.json"), b"{}");
    assert!(s3.keys_under(&format!("{prefix}/gone/")).len() > 1);

    // One of `stuck`'s remote objects refuses to be deleted.
    let stuck_state = format!("{prefix}/stuck/koto-stuck.state.jsonl");
    s3.fail("DELETE", &stuck_state, 500);

    let out = env.prune(&["--apply", "--all", "--confirm-short-retention"]);
    assert_eq!(out.status.code(), Some(3));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&v["removed"]), vec!["gone"]);
    assert_eq!(names(&v["failed"]), vec!["stuck"]);
    assert!(v["failed"][0]["error"]
        .as_str()
        .unwrap()
        .contains("remote copy"));

    assert_eq!(
        s3.keys_under(&format!("{prefix}/gone/")),
        vec![format!("{prefix}/gone/migrated.json")]
    );
    assert!(!env.sessions().join("gone").exists());
    // The failed session keeps its local copy for a later prune to retry.
    assert!(env.sessions().join("stuck").is_dir());

    s3.clear_faults();
    let out = env.prune_ok(&["--apply", "--all", "--confirm-short-retention"]);
    assert_eq!(names(&out["removed"]), vec!["stuck"]);
    assert!(s3.keys_under(&format!("{prefix}/stuck/")).is_empty());
}

// ===== The short-retention guard =====

/// Every object under the bucket, for a check that nothing was deleted.
fn bucket_keys(s3: &FakeS3) -> Vec<String> {
    let mut keys = s3.keys();
    keys.sort();
    keys
}

/// The refusal a short retention gets under the cloud backend, read from a
/// command that exited 2.
fn short_retention_refusal(out: &std::process::Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let error = if v["error"].is_object() {
        v["error"].clone()
    } else {
        v.clone()
    };
    assert_eq!(error["code"], "prune_short_retention", "{v:#}");
    error
}

#[test]
fn under_the_cloud_backend_a_short_retention_refuses_a_direct_apply_and_removes_nothing() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.done("first");
    env.done("second");
    settle();
    let before = bucket_keys(&s3);
    assert!(!before.is_empty());

    let dir = env.work.to_str().unwrap().to_string();
    for args in [
        vec!["--apply", "--all"],
        vec!["--apply", "--older-than", "1s"],
        vec!["--apply", "--execution-dir", dir.as_str()],
    ] {
        let out = env.prune(&args);
        let error = short_retention_refusal(&out);
        assert_eq!(error["would_remove"], 2, "{args:?}: {error:#}");
        assert_eq!(error["backend"], "cloud");
        assert_eq!(error["bucket"], BUCKET);
        assert_eq!(
            error["store"],
            std::fs::canonicalize(env.sessions())
                .unwrap()
                .to_str()
                .unwrap()
        );
        assert_eq!(
            error["retention"],
            serde_json::json!([{
                "setting": "retention.keep",
                "value": "1s",
                "from": "KOTO_RETENTION_KEEP",
            }])
        );
        let message = error["message"].as_str().unwrap();
        for part in [
            "2 finished sessions",
            "retention.keep is 1s (from KOTO_RETENTION_KEEP)",
            BUCKET,
            "without --apply",
            "koto session prune --apply --plan <token>",
            "--confirm-short-retention",
            "nothing was removed",
        ] {
            assert!(message.contains(part), "{part:?} missing from: {message}");
        }
    }
    assert!(env.sessions().join("first").is_dir());
    assert!(env.sessions().join("second").is_dir());
    assert_eq!(bucket_keys(&s3), before);

    // The preview and its plan are the safe path, and are unchanged.
    let preview = env.prune_ok(&["--all"]);
    assert_eq!(names(&preview["remove"]), vec!["first", "second"]);
    let token = preview["plan"].as_str().unwrap().to_string();
    let out = env.prune(&["--apply", "--plan", &token]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let applied: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&applied["removed"]), vec!["first", "second"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the cloud store at") && stderr.contains(BUCKET),
        "{stderr}"
    );
    assert!(s3
        .keys_under(&format!("{}/first/", env.prefix()))
        .is_empty());
}

#[test]
fn under_the_cloud_backend_the_confirm_flag_removes_under_a_short_retention() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.done("first");
    settle();
    let out = env.prune(&["--apply", "--all", "--confirm-short-retention"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&v["removed"]), vec!["first"]);
    assert!(!env.sessions().join("first").exists());
    assert!(s3
        .keys_under(&format!("{}/first/", env.prefix()))
        .is_empty());
    // The line naming the store comes before the removal starts.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("koto session prune: removing"), "{stderr}");
    assert!(stderr.contains(BUCKET), "{stderr}");
}

#[test]
fn under_the_cloud_backend_the_guard_leaves_long_retentions_and_old_selections_alone() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.done("first");
    settle();

    // A selection of an hour or more runs as it always did: nothing that
    // just finished is old enough, so nothing goes.
    let v = env.prune_ok(&["--apply", "--older-than", "1h"]);
    assert!(names(&v["removed"]).is_empty());

    // An idle period under the floor is named too.
    let out = env.run_env(
        &["session", "prune", "--apply", "--all"],
        &[("KOTO_RETENTION_IDLE_PERIOD", "30m")],
    );
    let error = short_retention_refusal(&out);
    let settings: Vec<_> = error["retention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["setting"].as_str().unwrap())
        .collect();
    assert_eq!(settings, ["retention.keep", "retention.idle_period"]);
    assert_eq!(error["retention"][1]["value"], "30m");

    // With every removal setting at an hour or more, the direct form runs
    // as it always did: the retention hasn't passed, so it keeps the session.
    let v = env.ok_env(&["session", "prune", "--apply", "--all"], &LONG);
    assert!(names(&v["removed"]).is_empty());
    assert!(env.sessions().join("first").is_dir());
}

#[test]
fn the_local_backend_removes_under_a_short_retention_without_confirmation() {
    let env = Env::new();
    env.done("first");
    settle();
    let out = env.prune(&["--apply", "--all"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&v["removed"]), vec!["first"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("the local store at"), "{stderr}");
}

#[test]
fn under_the_cloud_backend_workspace_prune_yes_refuses_a_short_retention() {
    let s3 = FakeS3::start(BUCKET);
    let env = Env::cloud(&s3);
    env.init("root");
    env.init_child("child", "root");
    env.finish("child", "yes");
    env.finish("root", "yes");
    settle();
    let before = bucket_keys(&s3);

    let out = env.run_in(
        &env.work,
        &["workspace", "prune", "--root", "root", "--yes"],
    );
    let error = short_retention_refusal(&out);
    assert_eq!(error["command"], "workspace prune");
    assert_eq!(error["would_remove"], 2);
    assert_eq!(error["backend"], "cloud");
    assert_eq!(error["bucket"], BUCKET);
    let message = error["error"].as_str().unwrap();
    for part in ["without --yes", "--dry-run", "--confirm-short-retention"] {
        assert!(message.contains(part), "{part:?} missing from: {message}");
    }
    assert!(env.sessions().join("root").is_dir());
    assert!(env.sessions().join("child").is_dir());
    assert_eq!(bucket_keys(&s3), before);

    // A dry run still previews, and the confirm flag removes.
    let out = env.run_in(
        &env.work,
        &["workspace", "prune", "--root", "root", "--dry-run"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = env.run_in(
        &env.work,
        &[
            "workspace",
            "prune",
            "--root",
            "root",
            "--yes",
            "--confirm-short-retention",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!env.sessions().join("root").exists());
    assert!(!env.sessions().join("child").exists());
}

/// The store-scale measurement: 1,000 sessions koto created and finished,
/// previewed with no filter. Run it with
/// `cargo test --release --test session_prune_test -- --ignored --nocapture`;
/// it prints the time and fails above 2 seconds.
#[test]
#[ignore]
fn a_thousand_sessions_preview_in_under_two_seconds() {
    let env = Env::new();
    std::thread::scope(|s| {
        for t in 0..8 {
            let env = &env;
            s.spawn(move || {
                for i in (t..1000).step_by(8) {
                    let name = format!("s-{i:04}");
                    env.init(&name);
                    env.finish(&name, if i % 10 == 0 { "fail" } else { "yes" });
                }
            });
        }
    });

    settle();
    let started = std::time::Instant::now();
    let preview = env.prune_ok(&[]);
    let elapsed = started.elapsed();
    assert_eq!(preview["remove"].as_array().unwrap().len(), 1000);
    println!("previewed 1000 sessions in {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}

// ===== The retention rule: rules a and h =====

fn kept_entry<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {v:#}"))
}

fn skipped(v: &Value) -> BTreeMap<String, String> {
    v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_string(),
                s["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn removed_in_order(v: &Value) -> Vec<String> {
    v["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_released_hierarchy_goes_whole_and_one_with_a_live_member_stays_whole() {
    let env = Env::new();
    // Released: a1 dispatched b1, and both finished.
    env.init("a1");
    env.init_with("b1", &["--dispatched-by", "a1"], &[]);
    env.finish("b1", "yes");
    env.finish("a1", "yes");
    // Live: a2 runs, and b2, which it dispatched, finished.
    env.init("a2");
    env.init_with("b2", &["--dispatched-by", "a2"], &[]);
    env.finish("b2", "yes");
    settle();

    let before = env.snapshot(&["a2", "b2"]);
    let preview = env.prune_ok(&[]);
    assert_eq!(names(&preview["remove"]), vec!["a1", "b1"]);
    let kept = rules(&preview);
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert_eq!(kept["a2"].0, "a");
    assert_eq!(kept["a2"].1, "the session is running");
    assert_eq!(kept["b2"].0, "h");
    assert!(
        kept["b2"].1.contains("a2 (koto_session)"),
        "{}",
        kept["b2"].1
    );
    assert_eq!(env.snapshot(&["a2", "b2"]), before);

    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["a1", "b1"]);
    for name in ["a1", "b1"] {
        assert!(!env.sessions().join(name).exists(), "{name} is still there");
    }
    assert_eq!(env.snapshot(&["a2", "b2"]), before);
}

#[test]
fn two_finished_sessions_are_kept_through_the_retention_then_removed_together() {
    let env = Env::new();
    env.init("a");
    env.init_with("b", &["--dispatched-by", "a"], &[]);
    env.finish("a", "yes");
    std::thread::sleep(std::time::Duration::from_millis(50));
    env.finish("b", "yes");

    // Inside the retention: both kept, naming the member that stopped last
    // and when the retention passes.
    let before = env.snapshot(&["a", "b"]);
    let preview = env.ok_env(&["session", "prune"], &LONG);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    for name in ["a", "b"] {
        let k = kept_entry(&preview, name);
        assert_eq!(k["rule"], "h");
        let detail = k["detail"].as_str().unwrap();
        assert!(detail.contains("when b stopped (terminal)"), "{detail}");
        assert!(detail.contains("the retention passes at"), "{detail}");
        assert_eq!(k["hierarchy"]["retention"]["release"]["member"], "b");
        assert!(k["hierarchy"]["retention"]["passes_at"].is_string());
    }
    let out = env.ok_env(&["session", "prune", "--apply", "--all"], &LONG);
    assert!(out["removed"].as_array().unwrap().is_empty());
    assert_eq!(env.snapshot(&["a", "b"]), before);

    // After it: both go in one apply.
    settle();
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["a", "b"]);
}

#[test]
fn filters_narrow_the_candidates_and_never_the_members() {
    let env = Env::new();
    let x = env.dir("ws-x");
    let y = env.dir("ws-y");
    // a runs in one directory; b, which it dispatched, finished in another,
    // two hours ago.
    env.init_in(&x, "a");
    env.ok_in(
        &y,
        &[
            "init",
            "b",
            "--template",
            env.template.to_str().unwrap(),
            "--dispatched-by",
            "a",
        ],
    );
    env.finish_in(&y, "b", "yes");
    let hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    env.set_all_event_times("b", hours_ago);

    let before = env.snapshot(&["a", "b"]);
    let y_arg = y.to_str().unwrap().to_string();
    for args in [
        vec!["--execution-dir", y_arg.as_str()],
        vec!["--apply", "--execution-dir", y_arg.as_str()],
        vec!["--older-than", "1h"],
        vec!["--apply", "--older-than", "1h"],
    ] {
        let out = env.prune_ok(&args);
        let list = if args[0] == "--apply" {
            "removed"
        } else {
            "remove"
        };
        assert!(out[list].as_array().unwrap().is_empty(), "{out:#}");
        let kept = rules(&out);
        assert_eq!(kept.len(), 1, "{args:?}: {kept:?}");
        assert_eq!(kept["b"].0, "h");
        assert!(kept["b"].1.contains("a (koto_session)"), "{}", kept["b"].1);
        assert_eq!(env.snapshot(&["a", "b"]), before, "{args:?}");
    }
}

#[test]
fn older_than_measures_from_the_stop_instant_not_the_last_event() {
    let env = Env::new();
    env.done("s");
    let stopped = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    env.set_all_event_times("s", stopped);
    // A later write gives the log a newer last event; the stop stays.
    env.set_chain_parent("s", "nobody-here");

    let preview = env.prune_ok(&["--older-than", "1h"]);
    assert_eq!(names(&preview["remove"]), vec!["s"]);
    // koto writes every instant in one fixed-width UTC form, so the strings
    // order as the instants do.
    let row = &preview["remove"][0];
    let hour_ago = koto::engine::types::iso8601_at(
        std::time::SystemTime::now() - std::time::Duration::from_secs(3600),
    );
    assert!(
        row["stopped_at"].as_str().unwrap() < hour_ago.as_str(),
        "{row}"
    );
    assert!(
        row["finished_at"].as_str().unwrap() > hour_ago.as_str(),
        "{row}"
    );
}

#[test]
fn a_session_that_gains_a_live_member_after_the_preview_is_skipped_and_listed() {
    let env = Env::new();
    env.done("b");
    settle();
    let b_before = env.snapshot(&["b"]);
    let preview = env.prune_ok(&[]);
    assert_eq!(names(&preview["remove"]), vec!["b"]);
    let token = preview["plan"].as_str().unwrap().to_string();
    assert_eq!(env.snapshot(&["b"]), b_before);

    // c starts, dispatched by b: b's hierarchy has a live member now. The
    // link lives in c, so b is untouched.
    env.init_with("c", &["--dispatched-by", "b"], &[]);
    let before = env.snapshot(&["b", "c"]);
    assert_eq!(env.snapshot(&["b"]), b_before);
    let out = env.prune(&["--apply", "--plan", &token]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["removed"].as_array().unwrap().is_empty(), "{v:#}");
    let reasons = skipped(&v);
    assert!(
        reasons["b"].contains("now kept by rule h") && reasons["b"].contains("c (koto_session)"),
        "{reasons:?}"
    );
    assert_eq!(rules(&v)["b"].0, "h");
    assert_eq!(env.snapshot(&["b", "c"]), before);
}

#[test]
fn close_records_last_the_close_session_goes_only_after_its_anchor() {
    let env = Env::new();
    let anchor = env.anchor("S1", "K1");
    env.init_with("b", &[], &[("KOTO_ANCHOR", anchor.as_str())]);
    env.finish("b", "yes");
    env.close_anchor("S1", "K1");
    let closed = format!("{anchor}-closed");
    assert!(env.sessions().join(&closed).is_dir());
    let all = ["b", anchor.as_str(), closed.as_str()];

    // Inside the retention nothing goes, the close session included, and
    // the anchor reads closed by its observed close.
    let before = env.snapshot(&all);
    let out = env.ok_env(&["session", "prune"], &LONG);
    assert!(out["remove"].as_array().unwrap().is_empty(), "{out:#}");
    let out = env.ok_env(&["session", "prune", "--apply", "--all"], &LONG);
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    assert_eq!(env.snapshot(&all), before);
    let h = env.hierarchy("b", &LONG);
    let member = h["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == anchor.as_str())
        .unwrap();
    assert_eq!(member["alive"], false, "{h:#}");
    assert_eq!(member["close_kind"], "observed", "{h:#}");

    // After the release, while another process holds the anchor, the
    // close session stays with it.
    settle();
    let file = std::fs::File::open(env.state_path(&anchor)).unwrap();
    use std::os::unix::io::AsRawFd;
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(removed_in_order(&out), vec!["b"]);
    let reasons = skipped(&out);
    assert!(reasons[&anchor].contains("lock"), "{reasons:?}");
    assert!(
        reasons[&closed].contains("a close session goes only after its anchor"),
        "{reasons:?}"
    );
    assert!(env.sessions().join(&anchor).is_dir());
    assert!(env.sessions().join(&closed).is_dir());
    drop(file);

    // Then the anchor and its close session go in the same apply, the
    // anchor first.
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(removed_in_order(&out), vec![anchor.clone(), closed.clone()]);
    assert!(!env.sessions().join(&anchor).exists());
    assert!(!env.sessions().join(&closed).exists());
}

#[test]
fn a_close_session_is_kept_while_a_filter_leaves_its_anchor_out() {
    let env = Env::new();
    let anchor = env.anchor("S2", "K2");
    env.close_anchor("S2", "K2");
    let closed = format!("{anchor}-closed");
    settle();
    // Both stop at the same close record, so `--older-than` selects both
    // or neither; give the close session an execution directory of its
    // own, so `--execution-dir` selects it alone.
    let elsewhere = env.dir("elsewhere");
    let path = env.state_path(&closed);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: Value = serde_json::from_str(&lines[0]).unwrap();
    header["execution_dir"] = Value::from(elsewhere.to_str().unwrap());
    lines[0] = serde_json::to_string(&header).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let before = env.snapshot(&[anchor.as_str(), closed.as_str()]);
    let out = env.prune_ok(&["--apply", "--execution-dir", elsewhere.to_str().unwrap()]);
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    let k = kept_entry(&out, &closed);
    assert_eq!(k["rule"], "h");
    assert!(
        k["detail"]
            .as_str()
            .unwrap()
            .contains("a close session goes only with its anchor"),
        "{k}"
    );
    assert_eq!(env.snapshot(&[anchor.as_str(), closed.as_str()]), before);
}

#[test]
fn a_timestamp_far_in_the_future_keeps_its_hierarchy() {
    let env = Env::new();
    env.done("future");
    env.done("plain");
    let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    env.set_all_event_times("future", ahead);
    settle();

    let before = env.snapshot(&["future"]);
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["plain"]);
    let k = kept_entry(&out, "future");
    assert_eq!(k["rule"], "h");
    let detail = k["detail"].as_str().unwrap();
    assert!(
        detail.contains("future: ") && detail.contains("five minutes in the future"),
        "{detail}"
    );
    assert_eq!(env.snapshot(&["future"]), before);
}

#[test]
fn something_unreadable_keeps_its_whole_hierarchy_and_names_it() {
    let env = Env::new();
    for i in 1..=2 {
        let (a, b) = (format!("a{i}"), format!("b{i}"));
        env.init(&a);
        env.init_with(&b, &["--dispatched-by", &a], &[]);
        env.finish(&b, "yes");
        env.finish(&a, "yes");
    }
    let anchor = env.anchor("S3", "K3");
    env.init_with("b3", &[], &[("KOTO_ANCHOR", anchor.as_str())]);
    env.finish("b3", "yes");
    env.close_anchor("S3", "K3");
    let closed = format!("{anchor}-closed");
    env.done("free");
    settle();

    // b1's log holds a line that can't be read.
    let path = env.state_path("b1");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.insert(2, "{this is not an event");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    // b2's link to its dispatcher is cut short.
    let link = env.sessions().join("b2/ctx/koto/links/dispatched-by/a2");
    let body = std::fs::read(&link).unwrap();
    std::fs::write(&link, &body[..body.len() / 2]).unwrap();
    // The record closing the anchor is malformed.
    std::fs::write(
        env.sessions().join(&closed).join("ctx/koto/anchor-close"),
        "{",
    )
    .unwrap();

    let all = [
        "a1",
        "b1",
        "a2",
        "b2",
        "b3",
        anchor.as_str(),
        closed.as_str(),
    ];
    let before = env.snapshot(&all);
    for args in [&[][..], &["--apply", "--all"][..]] {
        let out = env.prune_ok(args);
        let list = if args.is_empty() { "remove" } else { "removed" };
        assert_eq!(names(&out[list]), vec!["free"], "{out:#}");
        for name in all {
            let k = kept_entry(&out, name);
            assert!(k["rule"] == "a" || k["rule"] == "h", "{k}");
        }
        let detail = |name: &str| {
            kept_entry(&out, name)["detail"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(
            detail("a1").contains("b1: its log can't be read"),
            "{}",
            detail("a1")
        );
        assert!(
            detail("a2").contains("koto/links/dispatched-by/a2 is malformed"),
            "{}",
            detail("a2")
        );
        assert!(
            detail("b3").contains(&format!("{closed}: its close record can't be read")),
            "{}",
            detail("b3")
        );
        assert_eq!(env.snapshot(&all), before);
    }
}

#[test]
fn a_sessions_directory_that_cant_be_read_removes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    env.done("one");
    settle();
    let sessions = env.sessions();
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
    let listable = std::fs::read_dir(&sessions).is_ok();
    let preview = env.prune(&[]);
    let apply = env.prune(&["--apply", "--all"]);
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    if listable {
        return; // root: permissions don't bind
    }
    for out in [&preview, &apply] {
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(error_code(out), "prune_scan_failed");
    }
    assert!(env.sessions().join("one").is_dir());
}

#[test]
fn a_malformed_retention_setting_refuses_naming_it() {
    let env = Env::new();
    env.done("one");
    settle();
    for args in [
        &["session", "prune"][..],
        &["session", "prune", "--apply", "--all"][..],
    ] {
        let out = env.run_env(args, &[("KOTO_RETENTION_KEEP", "7x")]);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(error_code(&out), "invalid_retention_setting");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("retention.keep"),
            "{v}"
        );
    }
    assert!(env.sessions().join("one").is_dir());
}

#[test]
fn a_stale_observed_anchor_is_listed_with_its_close_command_and_closes_nothing() {
    let stale = [
        ("KOTO_RETENTION_KEEP", "1s"),
        ("KOTO_RETENTION_STALE_AFTER", "2s"),
    ];
    let env = Env::new();
    let anchor = env.anchor("S1", "K1");
    env.init_with("b", &[], &[("KOTO_ANCHOR", anchor.as_str())]);
    env.finish("b", "yes");
    std::thread::sleep(std::time::Duration::from_millis(2300));

    let command = "koto session anchor close --key='K1' --harness-session='S1'";
    let before = env.snapshot(&["b", anchor.as_str()]);
    for args in [
        &["session", "prune"][..],
        &["session", "prune", "--apply", "--all"][..],
    ] {
        let v = env.ok_env(args, &stale);
        let list = if args.len() == 2 { "remove" } else { "removed" };
        assert!(v[list].as_array().unwrap().is_empty(), "{v:#}");
        let listed = v["stale_anchors"].as_array().unwrap();
        assert_eq!(listed.len(), 1, "{v:#}");
        assert_eq!(listed[0]["name"], anchor.as_str());
        assert_eq!(listed[0]["stale"], true);
        assert_eq!(listed[0]["close_command"], command);
        let a = kept_entry(&v, &anchor);
        assert_eq!(a["rule"], "a");
        assert!(a["detail"].as_str().unwrap().contains(command), "{a}");
        let b = kept_entry(&v, "b");
        assert_eq!(b["rule"], "h");
        assert_eq!(b["hierarchy"]["keepers"][0]["close_command"], command);
        assert_eq!(env.snapshot(&["b", anchor.as_str()]), before);
        assert!(!env.sessions().join(format!("{anchor}-closed")).exists());
    }
    // The query prints the same command.
    assert_eq!(
        env.hierarchy("b", &stale)["keepers"][0]["close_command"],
        command
    );

    // Running it, then a prune after the retention, removes what it held.
    env.close_anchor("S1", "K1");
    settle();
    let v = env.ok_env(&["session", "prune", "--apply", "--all"], &stale);
    assert_eq!(
        names(&v["removed"]),
        vec![anchor.clone(), format!("{anchor}-closed"), "b".to_string()]
    );
}
