//! `koto session prune`, driven through the real binary against sessions koto
//! itself created, in a scratch HOME. The cloud cases run against the
//! in-process fake S3 endpoint.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

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

    fn cmd_in(&self, cwd: &Path) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        if self.cloud {
            cmd.env("AWS_ACCESS_KEY_ID", "test-access-key")
                .env("AWS_SECRET_ACCESS_KEY", "test-secret-key");
        }
        cmd
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        self.cmd_in(cwd).args(args).output().unwrap()
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

    BTreeMap::from([
        ("a-running", "a"),
        ("b-parent", "b"),
        ("b-child-running", "a"),
        ("b-chain-parent", "b"),
        ("b-chain-child", "a"),
        ("c-parent-running", "a"),
        ("c-child", "c"),
        ("c-chain-target", "a"),
        ("c-chain-source", "c"),
        ("d-malformed", "d"),
        ("e-waiting", "e"),
        ("e-unreadable", "e"),
        ("coordinate-f", "f"),
        ("coordinate-c", "c"),
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
        assert_eq!(kept[name].0, "b", "{name}: {}", kept[name].1);
        assert!(kept[name].1.contains("'kid'"), "{}", kept[name].1);
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

/// Set the timestamp of the last event in `name`'s log.
fn set_last_event_time(env: &Env, name: &str, at: std::time::SystemTime) {
    let path = env.state_path(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let last = lines.last_mut().unwrap();
    let mut event: Value = serde_json::from_str(last).unwrap();
    event["timestamp"] = Value::from(koto::engine::types::iso8601_at(at));
    *last = serde_json::to_string(&event).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
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
    set_last_event_time(&env, "a-old", now - hour - margin);
    env.init_in(&a, "a-young");
    env.finish_in(&a, "a-young", "yes");
    set_last_event_time(&env, "a-young", now - hour + margin);
    // In ws-a: an old finished parent whose running child lives in ws-b.
    env.init_in(&a, "a-parent");
    env.finish_in(&a, "a-parent", "yes");
    set_last_event_time(&env, "a-parent", now - hour - margin);
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
    set_last_event_time(&env, "b-old", now - hour - margin);

    let a_arg = a.to_str().unwrap();
    let preview = env.prune_ok(&["--execution-dir", a_arg, "--older-than", "1h"]);
    assert_eq!(names(&preview["remove"]), vec!["a-old"]);
    let kept = rules(&preview);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept["a-parent"].0, "b");

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
    let preview = env.prune_ok(&["--execution-dir", a.to_str().unwrap()]);
    assert_eq!(names(&preview["remove"]), vec!["previewed"]);
    let token = preview["plan"].as_str().unwrap().to_string();

    // `late` finishes inside the preview's filter after the preview.
    env.finish_in(&a, "late", "yes");
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
    let prefix = env.prefix();
    // A migration marker beside a session that is pruned stays.
    s3.put(&format!("{prefix}/gone/migrated.json"), b"{}");
    assert!(s3.keys_under(&format!("{prefix}/gone/")).len() > 1);

    // One of `stuck`'s remote objects refuses to be deleted.
    let stuck_state = format!("{prefix}/stuck/koto-stuck.state.jsonl");
    s3.fail("DELETE", &stuck_state, 500);

    let out = env.prune(&["--apply", "--all"]);
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
    let out = env.prune_ok(&["--apply", "--all"]);
    assert_eq!(names(&out["removed"]), vec!["stuck"]);
    assert!(s3.keys_under(&format!("{prefix}/stuck/")).is_empty());
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

    let started = std::time::Instant::now();
    let preview = env.prune_ok(&[]);
    let elapsed = started.elapsed();
    assert_eq!(preview["remove"].as_array().unwrap().len(), 1000);
    println!("previewed 1000 sessions in {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}
