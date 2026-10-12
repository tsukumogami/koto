//! `koto session list --execution-dir` and `--state`, driven through the real
//! binary against sessions koto itself created, in a scratch HOME. The cloud
//! case runs against the in-process fake S3 endpoint.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `start` waits for `go`; `yes` reaches the success terminal `done`, `fail`
/// the failure terminal `failed`.
const TEMPLATE: &str = r#"---
name: list-state
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

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    root: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        for d in [&home, &cache] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("list-state.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            root,
            template,
        }
    }

    /// A directory under the scratch root, created and canonical.
    fn dir(&self, rel: &str) -> PathBuf {
        let d = self.root.join(rel);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    fn cmd(&self, cwd: &Path) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        cmd
    }

    fn koto(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        self.cmd(cwd).args(args).output().unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> Value {
        let out = self.koto(cwd, args);
        assert!(
            out.status.success(),
            "koto {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn init(&self, cwd: &Path, name: &str) {
        self.ok(
            cwd,
            &["init", name, "--template", self.template.to_str().unwrap()],
        );
    }

    fn next(&self, cwd: &Path, name: &str, go: &str) {
        let data = format!("{{\"go\": \"{go}\"}}");
        self.ok(cwd, &["next", name, "--with-data", &data, "--no-cleanup"]);
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.home
            .join(".koto/sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn list(&self, args: &[&str]) -> Value {
        let mut all = vec!["session", "list"];
        all.extend_from_slice(args);
        self.ok(&self.root, &all)
    }
}

fn names(rows: &Value) -> Vec<String> {
    let mut names: Vec<String> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

fn row<'a>(rows: &'a Value, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row {id} in {rows}"))
}

#[test]
fn execution_dir_matches_whole_path_components() {
    let env = Env::new();
    let b = env.dir("x/a/b");
    let c = env.dir("x/a/b/c");
    let bc = env.dir("x/a/bc");
    env.init(&b, "in-b");
    env.init(&c, "in-c");
    env.init(&bc, "in-bc");

    let arg = b.to_str().unwrap().to_string();
    let out = env.list(&["--execution-dir", &arg]);
    assert_eq!(names(&out["sessions"]), vec!["in-b", "in-c"]);
    assert_eq!(out["excluded_remote_only"], 0);

    // A trailing slash gives the same answer.
    let slashed = format!("{arg}/");
    let out = env.list(&["--execution-dir", &slashed]);
    assert_eq!(names(&out["sessions"]), vec!["in-b", "in-c"]);

    // A symlink to the directory resolves to it.
    let link = env.root.join("link-to-b");
    std::os::unix::fs::symlink(&b, &link).unwrap();
    let out = env.list(&["--execution-dir", link.to_str().unwrap()]);
    assert_eq!(names(&out["sessions"]), vec!["in-b", "in-c"]);

    // A directory that doesn't exist here is matched as given.
    let out = env.list(&["--execution-dir", "/no/such/dir/for/koto"]);
    assert!(out["sessions"].as_array().unwrap().is_empty());

    // `/` matches every session.
    let out = env.list(&["--execution-dir", "/"]);
    assert_eq!(names(&out["sessions"]), vec!["in-b", "in-bc", "in-c"]);
}

#[test]
fn filtered_rows_carry_state_for_every_status() {
    let env = Env::new();
    let w = env.dir("work");
    env.init(&w, "running");
    env.init(&w, "succeeded");
    env.next(&w, "succeeded", "yes");
    env.init(&w, "failed");
    env.next(&w, "failed", "fail");
    env.init(&w, "cancelled");
    env.ok(&w, &["cancel", "cancelled"]);
    env.init(&w, "broken");

    // Break the log after the header: a line that isn't an event, followed
    // by a valid one, so the read refuses the log but the header still parses.
    let path = env.state_path("broken");
    let text = std::fs::read_to_string(&path).unwrap();
    let last = text.lines().last().unwrap().to_string();
    std::fs::write(&path, format!("{text}not an event\n{last}\n")).unwrap();

    let out = env.list(&["--execution-dir", w.to_str().unwrap()]);
    let rows = &out["sessions"];
    let expect = |id: &str, status: &str, state: Value, failure: bool| {
        let r = row(rows, id);
        assert_eq!(r["status"], status, "{id}: {r}");
        assert_eq!(r["state"], state, "{id}: {r}");
        assert_eq!(r["failure"], failure, "{id}: {r}");
        assert_eq!(r["execution_dir"], w.to_str().unwrap(), "{id}: {r}");
        assert!(
            r["session_id"].as_str().is_some_and(|s| !s.is_empty()),
            "{id}: {r}"
        );
    };
    expect("running", "running", Value::from("start"), false);
    expect("succeeded", "terminal", Value::from("done"), false);
    expect("failed", "terminal", Value::from("failed"), true);
    expect("cancelled", "cancelled", Value::from("start"), false);
    expect("broken", "unreadable", Value::Null, false);
    for id in ["running", "succeeded", "failed", "cancelled"] {
        assert!(row(rows, id)["finished_at"].is_string(), "{id}");
    }
}

#[test]
fn state_without_a_filter_keeps_the_array_shape() {
    let env = Env::new();
    let w = env.dir("work");
    env.init(&w, "one");
    let rows = env.list(&["--state"]);
    assert!(rows.is_array());
    assert_eq!(row(&rows, "one")["status"], "running");
}

#[test]
fn plain_list_keeps_every_field_and_adds_two() {
    let env = Env::new();
    let w = env.dir("work");
    env.init(&w, "plain");
    let header: Value = serde_json::from_str(
        std::fs::read_to_string(env.state_path("plain"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();

    let rows = env.list(&[]);
    let r = row(&rows, "plain");
    let mut keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "created_at",
            "execution_dir",
            "id",
            "parent_workflow",
            "session_id",
            "template_hash",
            "template_source_status",
        ]
    );
    assert_eq!(r["created_at"], header["created_at"]);
    assert_eq!(r["template_hash"], header["template_hash"]);
    assert_eq!(r["parent_workflow"], Value::Null);
    assert_eq!(r["session_id"], header["session_id"]);
    assert_eq!(r["execution_dir"], header["execution_dir"]);
}

#[test]
fn a_bucket_only_session_is_left_out_and_counted() {
    const BUCKET: &str = "koto-session-list";
    let s3 = FakeS3::start(BUCKET);
    let env = Env::new();
    let w = env.dir("ws");
    std::fs::create_dir_all(w.join(".koto")).unwrap();
    std::fs::write(
        w.join(".koto/config.toml"),
        format!(
            "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nendpoint = {:?}\n\
             bucket = {:?}\nregion = \"us-east-1\"\npath_style = true\n",
            s3.endpoint(),
            BUCKET
        ),
    )
    .unwrap();
    let prefix = {
        let digest = Sha256::digest(w.to_string_lossy().as_bytes());
        hex::encode(digest)[..16].to_string()
    };
    s3.put(
        &format!("{prefix}/remote-only/koto-remote-only.state.jsonl"),
        b"{}\n",
    );

    let mut cmd = env.cmd(&w);
    cmd.env("AWS_ACCESS_KEY_ID", "test-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "test-secret-key")
        .args([
            "init",
            "local-one",
            "--template",
            env.template.to_str().unwrap(),
        ]);
    assert!(cmd.output().unwrap().status.success());

    let mut cmd = env.cmd(&w);
    cmd.env("AWS_ACCESS_KEY_ID", "test-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "test-secret-key")
        .args(["session", "list", "--execution-dir", "/"]);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&out["sessions"]), vec!["local-one"]);
    assert_eq!(out["excluded_remote_only"], 1);

    // Without the filter the placeholder row is listed, with null state.
    let mut cmd = env.cmd(&w);
    cmd.env("AWS_ACCESS_KEY_ID", "test-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "test-secret-key")
        .args(["session", "list", "--state"]);
    let rows: Value = serde_json::from_slice(&cmd.output().unwrap().stdout).unwrap();
    let r = row(&rows, "remote-only");
    assert_eq!(r["status"], Value::Null);
    assert_eq!(r["execution_dir"], Value::Null);
}

/// One state that loops on evidence, so a session's log can grow without new
/// states.
const LOOP_TEMPLATE: &str = r#"---
name: list-loop
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      again:
        type: enum
        required: true
        values: ["yes", "no"]
    transitions:
      - target: work
        when:
          again: "yes"
      - target: done
        when:
          again: "no"
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// R30's measurement: 1,000 sessions, each a copy of one koto drove through
/// at least 20 events, listed with `--execution-dir`. Run it with
/// `cargo test --release --test session_list_test -- --ignored --nocapture`;
/// it prints the time and fails above 2 seconds.
#[test]
#[ignore]
fn a_thousand_sessions_list_with_state_in_under_two_seconds() {
    let env = Env::new();
    let w = env.dir("work");
    let template = env.root.join("list-loop.md");
    std::fs::write(&template, LOOP_TEMPLATE).unwrap();
    env.ok(
        &w,
        &["init", "seed", "--template", template.to_str().unwrap()],
    );
    for _ in 0..10 {
        env.ok(
            &w,
            &[
                "next",
                "seed",
                "--with-data",
                r#"{"again": "yes"}"#,
                "--no-cleanup",
            ],
        );
    }
    let seed = std::fs::read_to_string(env.state_path("seed")).unwrap();
    let events = seed.lines().count() - 1;
    assert!(events >= 20, "seed log has {events} events");

    let sessions = env.home.join(".koto/sessions");
    let seed_dir = sessions.join("seed");
    for i in 0..1000 {
        let name = format!("copy-{i:04}");
        let dir = sessions.join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines = seed.lines();
        let mut header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        header["workflow"] = Value::from(name.clone());
        let mut body = serde_json::to_string(&header).unwrap();
        body.push('\n');
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        std::fs::write(dir.join(format!("koto-{name}.state.jsonl")), body).unwrap();
        for entry in std::fs::read_dir(&seed_dir).unwrap().flatten() {
            let f = entry.file_name();
            if f.to_string_lossy().starts_with("koto-") || entry.path().is_dir() {
                continue;
            }
            std::fs::copy(entry.path(), dir.join(f)).unwrap();
        }
    }

    let started = std::time::Instant::now();
    let out = env.list(&["--execution-dir", w.to_str().unwrap()]);
    let elapsed = started.elapsed();
    assert_eq!(out["sessions"].as_array().unwrap().len(), 1001);
    println!("listed 1001 sessions with state in {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}
