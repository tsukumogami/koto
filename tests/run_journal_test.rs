//! The run journal (`~/.koto/_run_journal.jsonl`) as koto commands write it.
//!
//! Every test runs the koto binary with `HOME` in a temporary directory and
//! `KOTO_SESSIONS_BASE` unset, so the session store is the default one under
//! that home and the journal it reads is that test's own (a store redirected
//! by `KOTO_SESSIONS_BASE` journals inside its own base). Each test also sets or clears
//! `CLAUDE_CODE_SESSION_ID` on each spawned process explicitly, so the
//! driver of the session running the suite never reaches a journal.
//!
//! Every journal a test reads goes through [`Env::journal`], which also
//! checks each record's shape: only the fields its kind allows, no absolute
//! path, no host name, no field whose name mentions cost, and none of the
//! values a test planted.

#![cfg(unix)]

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[path = "support/funnel_backend.rs"]
mod funnel_backend;
use funnel_backend::FunnelBackend;

const JOURNAL: &str = "_run_journal.jsonl";
const DRIVER_ENV: &str = "CLAUDE_CODE_SESSION_ID";
/// The top-level directory macOS keeps the real `/tmp` and `/var/folders`
/// under, as one path segment.
const MACOS_ALIAS_ROOT: &str = "private";

// ----- Templates -----

/// `start` waits on evidence, then `middle` chains to `done`.
const SIMPLE: &str = r#"---
name: journal-simple
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
      - target: middle
        when:
          go: "yes"
      - target: failed
        when:
          go: "fail"
  middle:
    transitions:
      - target: done
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## start

Start.

## middle

Middle.

## done

Done.

## failed

Failed.
"#;

/// Three automatic transitions after one submission, a self transition,
/// and a state to rewind and direct into.
const CHAIN: &str = r#"---
name: journal-chain
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
      - target: one
        when:
          go: "yes"
  one:
    transitions:
      - target: two
  two:
    transitions:
      - target: loop
  loop:
    accepts:
      again:
        type: enum
        required: true
        values: ["yes", "no", "back"]
    transitions:
      - target: loop
        when:
          again: "yes"
      - target: one
        when:
          again: "back"
      - target: done
        when:
          again: "no"
  done:
    terminal: true
---

## start

Start.

## one

One.

## two

Two.

## loop

Loop.

## done

Done.
"#;

const BATCH_CHILD: &str = r#"---
name: journal-batch-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done, fail]
    transitions:
      - target: done
        when:
          marker: done
      - target: failed
        when:
          marker: fail
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
  skipped_via_upstream_failure:
    terminal: true
    skipped_marker: true
---

## work

Do the work.

## done

Done.

## failed

Failed.

## skipped_via_upstream_failure

Skipped.
"#;

const BATCH_PARENT: &str = r#"---
name: journal-batch-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: ["yes"]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          finalize: "yes"
  summarize:
    terminal: true
---

## plan

Plan.

## summarize

Summarize.
"#;

// ----- Harness -----

/// The result of one koto invocation.
struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
    json: Value,
}

impl Out {
    fn journal_warnings(&self) -> usize {
        self.stderr
            .lines()
            .filter(|l| l.contains("run journal"))
            .count()
    }
}

/// One isolated koto home: `HOME`, the session store (the default one,
/// `$HOME/.koto/sessions`), the template cache and the working directory all
/// live in one temporary directory.
struct Env {
    tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
    /// Values planted in context, evidence, gates or variables, which no
    /// journal record may carry.
    planted: Vec<String>,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let work = tmp.path().join("work");
        for d in [&home, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        Env {
            tmp,
            home,
            work,
            planted: Vec::new(),
        }
    }

    fn sessions(&self) -> PathBuf {
        self.koto_home().join("sessions")
    }

    fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    fn journal_path(&self) -> PathBuf {
        self.koto_home().join(JOURNAL)
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.sessions().join(name)
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.session_dir(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    /// The environment every koto process in this home runs with: `Some`
    /// sets a variable, `None` removes it.
    fn envs(&self, driver: Option<&str>) -> Vec<(String, Option<String>)> {
        let path = |p: PathBuf| Some(p.to_string_lossy().into_owned());
        let mut envs = vec![
            ("HOME".to_string(), path(self.home.clone())),
            (
                "XDG_CACHE_HOME".to_string(),
                path(self.tmp.path().join("cache")),
            ),
        ];
        for unset in [
            "KOTO_SESSIONS_BASE",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_DECIDER",
            "KOTO_DECIDER_API_KEY",
            "KOTO_DECIDER_ENDPOINT",
        ] {
            envs.push((unset.to_string(), None));
        }
        envs.push((DRIVER_ENV.to_string(), driver.map(str::to_string)));
        envs
    }

    /// A plain process for `program`, in this home's working directory and
    /// environment.
    fn process(&self, program: &Path, driver: Option<&str>) -> std::process::Command {
        let mut cmd = std::process::Command::new(program);
        cmd.current_dir(&self.work);
        for (k, v) in self.envs(driver) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        cmd
    }

    /// A koto command for this home, run under `driver` (or none).
    fn cmd(&self, driver: Option<&str>) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(&self.work);
        for (k, v) in self.envs(driver) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        cmd
    }

    fn run(mut cmd: Command, args: &[&str]) -> Out {
        let output = cmd.args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        Out {
            ok: output.status.success(),
            json: serde_json::from_str(last).unwrap_or(Value::Null),
            stdout,
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn koto_as(&self, driver: Option<&str>, args: &[&str]) -> Out {
        Self::run(self.cmd(driver), args)
    }

    fn koto(&self, args: &[&str]) -> Out {
        self.koto_as(None, args)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert!(
            out.ok,
            "koto {:?} failed\nstdout: {}\nstderr: {}",
            args, out.stdout, out.stderr
        );
        out
    }

    fn template(&self, file: &str, body: &str) -> PathBuf {
        let path = self.work.join(file);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn init(&self, name: &str, template: &Path) -> Out {
        self.ok(&["init", name, "--template", template.to_str().unwrap()])
    }

    fn raw_journal(&self) -> String {
        std::fs::read_to_string(self.journal_path()).unwrap_or_default()
    }

    /// Every journal record, each one checked for shape.
    fn journal(&self) -> Vec<Value> {
        let raw = self.raw_journal();
        raw.lines()
            .map(|line| {
                let record: Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("journal line is not JSON ({e}): {line}"));
                check_shape(&record, line, &self.planted);
                record
            })
            .collect()
    }

    /// Records for `session`, in journal order.
    fn records(&self, session: &str) -> Vec<Value> {
        self.journal()
            .into_iter()
            .filter(|r| r["session"] == session)
            .collect()
    }

    fn header(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.state_path(name)).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    fn session_id(&self, name: &str) -> String {
        self.header(name)["session_id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect()
}

fn states(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .filter(|r| r["kind"] == "state_entered")
        .map(|r| r["koto.state"].as_str().unwrap())
        .collect()
}

const BASE_FIELDS: &[&str] = &[
    "kind",
    "v",
    "at",
    "session",
    "koto.session.id",
    "koto.run.id",
];

fn allowed_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "session_started" => &[
            "koto.parent.session.id",
            "koto.driver.session.id",
            "koto.template.name",
            "koto.template.hash",
            "koto.fixture",
            "koto.imported_from.session.id",
        ],
        "state_entered" => &["koto.state"],
        "terminal" => &["koto.terminal"],
        "cancelled" => &[],
        other => panic!("unknown record kind {other}"),
    }
}

/// The host's name, when it is distinctive enough to search for: long, and
/// not something an id or a hex hash could contain by chance.
fn host_name() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let distinctive = name.len() >= 6
        && name
            .bytes()
            .any(|b| !(b.is_ascii_hexdigit() || b == b'-' || b == b'.'));
    distinctive.then_some(name)
}

/// The record-shape rule every journal line in this suite must meet.
fn check_shape(record: &Value, line: &str, planted: &[String]) {
    let obj = record.as_object().expect("a record is an object");
    let kind = obj["kind"].as_str().expect("kind is a string");
    let allowed: BTreeSet<&str> = BASE_FIELDS
        .iter()
        .chain(allowed_fields(kind))
        .copied()
        .collect();
    for (key, value) in obj {
        assert!(
            allowed.contains(key.as_str()),
            "{kind} carries {key}: {line}"
        );
        assert!(!key.contains("cost"), "{line}");
        assert!(
            !value.is_null() && value != "",
            "{key} is null or empty: {line}"
        );
        if let Some(s) = value.as_str() {
            assert!(
                !s.starts_with('/') && !s.starts_with('~'),
                "{key} holds a path: {line}"
            );
        }
    }
    assert_eq!(obj["v"], 1, "{line}");
    let at = obj["at"].as_str().unwrap();
    assert_eq!(at.len(), 24, "{line}");
    assert!(at.ends_with('Z') && at.as_bytes()[19] == b'.', "{line}");
    assert!(obj.contains_key("koto.session.id"), "no session id: {line}");
    assert!(!line.contains("/tmp") && !line.contains("/var/"), "{line}");
    if let Some(host) = host_name() {
        assert!(!line.contains(&host), "host name in {line}");
    }
    for value in planted {
        assert!(!line.contains(value.as_str()), "planted {value} in {line}");
    }
}

/// A directory outside every temporary directory, under the build's own
/// scratch space, for templates that must not count as test fixtures.
fn non_temp_dir(label: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "run-journal-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ----- Whole runs -----

#[test]
fn a_run_to_a_success_terminal_leaves_its_records_after_the_session_is_removed() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("ok", &t);
    let out = env.ok(&["next", "ok", "--with-data", r#"{"go": "yes"}"#]);
    assert_eq!(out.json["state"], "done");
    assert!(!env.session_dir("ok").exists(), "the session was removed");

    let records = env.records("ok");
    assert_eq!(
        kinds(&records),
        vec![
            "session_started",
            "state_entered",
            "state_entered",
            "state_entered",
            "terminal"
        ]
    );
    assert_eq!(states(&records), vec!["start", "middle", "done"]);
    assert_eq!(records[4]["koto.terminal"], "done");
    let id = records[0]["koto.session.id"].as_str().unwrap();
    for r in &records {
        assert_eq!(r["koto.session.id"], id);
        assert_eq!(r["koto.run.id"], id, "a root's run is its own id");
    }
    let started = &records[0];
    assert_eq!(started["koto.template.name"], "journal-simple");
    assert_eq!(started["koto.template.hash"].as_str().unwrap().len(), 64);
    assert_eq!(
        started["koto.fixture"], true,
        "the template is in a tempdir"
    );
    assert!(started.get("koto.driver.session.id").is_none());
    assert!(started.get("koto.parent.session.id").is_none());
}

#[test]
fn a_run_to_a_failure_terminal_writes_the_same_set_and_keeps_the_session() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("bad", &t);
    env.ok(&["next", "bad", "--with-data", r#"{"go": "fail"}"#]);
    assert!(
        env.session_dir("bad").exists(),
        "a failure terminal is kept"
    );
    let records = env.records("bad");
    assert_eq!(
        kinds(&records),
        vec![
            "session_started",
            "state_entered",
            "state_entered",
            "terminal"
        ]
    );
    assert_eq!(states(&records), vec!["start", "failed"]);
    assert_eq!(records[3]["koto.terminal"], "failed");
}

#[test]
fn a_chained_tick_journals_each_state_in_order() {
    let env = Env::new();
    let t = env.template("chain.md", CHAIN);
    env.init("chain", &t);
    let before = env.records("chain").len();
    env.ok(&["next", "chain", "--with-data", r#"{"go": "yes"}"#]);
    let records = env.records("chain");
    assert_eq!(states(&records[before..]), vec!["one", "two", "loop"]);
}

#[test]
fn self_directed_and_rewound_transitions_each_journal_the_state_entered() {
    let env = Env::new();
    let t = env.template("chain.md", CHAIN);
    env.init("moves", &t);
    env.ok(&["next", "moves", "--with-data", r#"{"go": "yes"}"#]);
    let mark = env.records("moves").len();

    // A transition back into the current state.
    env.ok(&["next", "moves", "--with-data", r#"{"again": "yes"}"#]);
    // A directed transition.
    env.ok(&["next", "moves", "--to", "one", "--rationale", "redo"]);
    // A rewind back to where the directed transition left.
    env.ok(&["rewind", "moves"]);

    let records = env.records("moves");
    let tail = &records[mark..];
    assert_eq!(kinds(tail), vec!["state_entered"; 3], "{tail:?}");
    assert_eq!(states(tail), vec!["loop", "one", "loop"]);
}

#[test]
fn a_rewind_out_of_a_terminal_and_a_second_arrival_write_a_second_terminal() {
    let env = Env::new();
    let t = env.template("chain.md", CHAIN);
    env.init("twice", &t);
    env.ok(&["next", "twice", "--with-data", r#"{"go": "yes"}"#]);
    env.ok(&[
        "next",
        "twice",
        "--no-cleanup",
        "--with-data",
        r#"{"again": "no"}"#,
    ]);
    env.ok(&["rewind", "twice"]);
    env.ok(&[
        "next",
        "twice",
        "--no-cleanup",
        "--with-data",
        r#"{"again": "no"}"#,
    ]);
    let records = env.records("twice");
    let terminals: Vec<&Value> = records.iter().filter(|r| r["kind"] == "terminal").collect();
    assert_eq!(terminals.len(), 2, "{records:?}");
    let tail: Vec<&str> = kinds(&records).into_iter().rev().take(4).collect();
    assert_eq!(
        tail,
        vec!["terminal", "state_entered", "state_entered", "terminal"]
    );
}

#[test]
fn cancel_writes_cancelled_and_no_terminal_with_and_without_cleanup() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("keep", &t);
    env.init("gone", &t);
    env.ok(&["cancel", "keep"]);
    env.ok(&["cancel", "gone", "--cleanup"]);
    assert!(env.session_dir("keep").exists());
    assert!(!env.session_dir("gone").exists());
    for name in ["keep", "gone"] {
        let records = env.records(name);
        assert_eq!(
            kinds(&records),
            vec!["session_started", "state_entered", "cancelled"],
            "{name}"
        );
        assert_eq!(records[2]["koto.run.id"], records[0]["koto.run.id"]);
    }
}

#[test]
fn removing_sessions_never_changes_lines_already_written() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    let assert_kept = |before: &str, what: &str| {
        let now = env.raw_journal();
        assert!(
            now.starts_with(before),
            "{what} changed lines already written"
        );
    };

    // koto session cleanup
    env.init("a", &t);
    let before = env.raw_journal();
    env.ok(&["session", "cleanup", "a"]);
    assert_kept(&before, "session cleanup");
    assert_eq!(env.raw_journal(), before);

    // koto cancel --cleanup
    env.init("b", &t);
    let before = env.raw_journal();
    env.ok(&["cancel", "b", "--cleanup"]);
    assert_kept(&before, "cancel --cleanup");

    // koto workspace prune
    env.init("c", &t);
    env.ok(&[
        "next",
        "c",
        "--no-cleanup",
        "--with-data",
        r#"{"go": "yes"}"#,
    ]);
    let before = env.raw_journal();
    env.ok(&["workspace", "prune", "--root", "c", "--yes"]);
    assert!(!env.session_dir("c").exists());
    assert_eq!(env.raw_journal(), before, "workspace prune");

    // koto init --replace-terminal
    env.init("d", &t);
    env.ok(&[
        "next",
        "d",
        "--no-cleanup",
        "--with-data",
        r#"{"go": "yes"}"#,
    ]);
    let old_id = env.session_id("d");
    let before = env.raw_journal();
    env.ok(&[
        "init",
        "d",
        "--template",
        t.to_str().unwrap(),
        "--replace-terminal",
    ]);
    assert_kept(&before, "--replace-terminal");
    let new_id = env.session_id("d");
    assert_ne!(old_id, new_id);
    let started: Vec<Value> = env
        .records("d")
        .into_iter()
        .filter(|r| r["kind"] == "session_started")
        .collect();
    assert_eq!(started.len(), 2);
    assert_eq!(started[1]["koto.session.id"], new_id.as_str());

    // A parent sweep: a removed parent takes its kept children with it.
    let parent = env.template("parent.md", BATCH_PARENT);
    env.template("child.md", BATCH_CHILD);
    env.init("p", &parent);
    let tasks = serde_json::json!({"tasks": [{"name": "k", "waits_on": [], "vars": {}}]});
    env.ok(&["next", "p", "--with-data", &tasks.to_string()]);
    env.ok(&[
        "next",
        "p.k",
        "--no-cleanup",
        "--with-data",
        r#"{"marker": "done"}"#,
    ]);
    assert!(env.session_dir("p.k").exists());
    let before = env.raw_journal();
    let finalize = serde_json::json!({
        "tasks": [{"name": "k", "waits_on": [], "vars": {}}],
        "finalize": "yes",
    });
    env.ok(&["next", "p", "--with-data", &finalize.to_string()]);
    assert!(!env.session_dir("p").exists());
    assert!(
        !env.session_dir("p.k").exists(),
        "the sweep removed the child"
    );
    assert_kept(&before, "parent sweep");
    assert_eq!(kinds(&env.records("p")).last(), Some(&"terminal"));
}

// ----- Ids across a hierarchy -----

#[test]
fn batch_children_and_grandchildren_keep_the_roots_run_id_after_it_is_removed() {
    let env = Env::new();
    let parent = env.template("parent.md", BATCH_PARENT);
    env.template("child.md", BATCH_CHILD);
    let simple = env.template("simple.md", SIMPLE);
    env.init("root", &parent);
    let root_id = env.session_id("root");
    let tasks = serde_json::json!({"tasks": [{"name": "a", "waits_on": [], "vars": {}}]});
    env.ok(&["next", "root", "--with-data", &tasks.to_string()]);
    let child_id = env.session_id("root.a");
    env.ok(&[
        "init",
        "grand",
        "--template",
        simple.to_str().unwrap(),
        "--parent",
        "root.a",
    ]);
    let grand_id = env.session_id("grand");
    assert_eq!(env.header("grand")["root_session_id"], root_id.as_str());
    assert_eq!(env.header("grand")["parent_session_id"], child_id.as_str());
    assert_eq!(env.header("root.a")["root_session_id"], root_id.as_str());

    env.ok(&["session", "cleanup", "root"]);
    assert!(!env.session_dir("root").exists());
    // Drop the caches too: the ids must come from the headers.
    std::fs::remove_file(env.session_dir("root.a").join("run-journal.json")).unwrap();
    std::fs::remove_file(env.session_dir("grand").join("run-journal.json")).unwrap();

    env.ok(&[
        "next",
        "root.a",
        "--no-cleanup",
        "--with-data",
        r#"{"marker": "done"}"#,
    ]);
    env.ok(&["next", "grand", "--with-data", r#"{"go": "yes"}"#]);

    let child = env.records("root.a");
    assert_eq!(child[0]["koto.parent.session.id"], root_id.as_str());
    let grand = env.records("grand");
    assert_eq!(grand[0]["koto.parent.session.id"], child_id.as_str());
    for r in child.iter().chain(grand.iter()) {
        assert_eq!(r["koto.run.id"], root_id.as_str(), "{r}");
    }
    assert_eq!(grand[0]["koto.session.id"], grand_id.as_str());
    assert_eq!(kinds(&grand).last(), Some(&"terminal"));
}

/// Strip what this koto adds to a child, as if an older koto created it.
fn make_older(env: &Env, name: &str) {
    let path = env.state_path(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: Value = serde_json::from_str(&lines[0]).unwrap();
    let obj = header.as_object_mut().unwrap();
    obj.remove("root_session_id");
    obj.remove("parent_session_id");
    lines[0] = serde_json::to_string(&header).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let _ = std::fs::remove_file(env.session_dir(name).join("run-journal.json"));
}

#[test]
fn a_child_from_an_older_koto_gets_its_roots_run_id_only_while_the_root_exists() {
    let env = Env::new();
    let t = env.template("chain.md", CHAIN);
    let simple = env.template("simple.md", SIMPLE);
    env.init("root", &t);
    let root_id = env.session_id("root");
    for child in ["root.x", "root.y"] {
        env.ok(&[
            "init",
            child,
            "--template",
            t.to_str().unwrap(),
            "--parent",
            "root",
        ]);
        make_older(&env, child);
    }

    // With its root present, an older child's records carry the root's id.
    env.ok(&["next", "root.x", "--with-data", r#"{"go": "yes"}"#]);
    let x = env.records("root.x");
    for r in &x[2..] {
        assert_eq!(r["koto.run.id"], root_id.as_str(), "{r}");
    }
    // A grandchild spawned now under the older child records the true root.
    env.ok(&[
        "init",
        "gx",
        "--template",
        simple.to_str().unwrap(),
        "--parent",
        "root.x",
    ]);
    assert_eq!(env.header("gx")["root_session_id"], root_id.as_str());
    assert_eq!(env.records("gx")[0]["koto.run.id"], root_id.as_str());

    // With the root gone, nothing resolves: no run id, and never the
    // child's own id in its place.
    env.ok(&["session", "cleanup", "root"]);
    let y_id = env.session_id("root.y");
    env.ok(&["next", "root.y", "--with-data", r#"{"go": "yes"}"#]);
    let y = env.records("root.y");
    for r in &y[2..] {
        assert!(r.get("koto.run.id").is_none(), "{r}");
    }
    env.ok(&[
        "init",
        "gy",
        "--template",
        simple.to_str().unwrap(),
        "--parent",
        "root.y",
    ]);
    assert!(env.header("gy").get("root_session_id").is_none());
    assert_eq!(env.header("gy")["parent_session_id"], y_id.as_str());
    let gy = env.records("gy");
    assert!(gy[0].get("koto.run.id").is_none(), "{:?}", gy[0]);
    assert_eq!(gy[0]["koto.parent.session.id"], y_id.as_str());
    for r in env.journal() {
        if r["session"] == "gy" || r["session"] == "root.y" {
            assert_ne!(r.get("koto.run.id"), Some(&Value::from(y_id.as_str())));
        }
    }
}

#[test]
fn skip_markers_and_a_retry_respawn_each_write_session_started() {
    let env = Env::new();
    let parent = env.template("parent.md", BATCH_PARENT);
    env.template("child.md", BATCH_CHILD);
    env.init("parent", &parent);
    let tasks = serde_json::json!({"tasks": [
        {"name": "A", "waits_on": [], "vars": {}},
        {"name": "B", "waits_on": ["A"], "vars": {}},
    ]});
    env.ok(&["next", "parent", "--with-data", &tasks.to_string()]);
    env.ok(&[
        "next",
        "parent.A",
        "--no-cleanup",
        "--with-data",
        r#"{"marker": "fail"}"#,
    ]);
    // The scheduler sees A failed and writes B as a skip marker.
    env.ok(&["next", "parent"]);
    let b = env.records("parent.B");
    assert_eq!(kinds(&b)[0], "session_started", "{b:?}");
    assert_eq!(states(&b), vec!["skipped_via_upstream_failure"]);
    let first_b = b[0]["koto.session.id"].clone();

    // Retry A: a rewind of the failed child.
    let retry = serde_json::json!({"retry_failed": {"children": ["A"]}});
    env.ok(&["next", "parent", "--with-data", &retry.to_string()]);
    let a = env.records("parent.A");
    assert_eq!(states(&a), vec!["work", "failed", "work"]);
    env.ok(&[
        "next",
        "parent.A",
        "--no-cleanup",
        "--with-data",
        r#"{"marker": "done"}"#,
    ]);
    // B is respawned as a real child: a new session with its own start.
    env.ok(&["next", "parent"]);
    let started: Vec<Value> = env
        .records("parent.B")
        .into_iter()
        .filter(|r| r["kind"] == "session_started")
        .collect();
    assert_eq!(started.len(), 2, "{started:?}");
    assert_ne!(started[1]["koto.session.id"], first_b);
    let parent_id = env.session_id("parent");
    for r in &started {
        assert_eq!(r["koto.run.id"], parent_id.as_str());
        assert_eq!(r["koto.parent.session.id"], parent_id.as_str());
    }
}

#[test]
fn stdin_and_session_start_sessions_write_session_started() {
    let env = Env::new();
    let mut cmd = env.cmd(None);
    cmd.write_stdin(SIMPLE);
    let out = Env::run(cmd, &["init", "inline", "--from-stdin"]);
    assert!(out.ok, "{}", out.stderr);
    let inline = env.records("inline");
    assert_eq!(kinds(&inline), vec!["session_started", "state_entered"]);
    assert_eq!(inline[0]["koto.template.name"], "journal-simple");
    assert_eq!(inline[0]["koto.fixture"], false, "no template source dir");
    assert_eq!(inline[0]["koto.run.id"], inline[0]["koto.session.id"]);

    let parent_id = env.session_id("inline");
    env.ok(&["session", "start", "plain", "--parent", "inline"]);
    env.ok(&[
        "session",
        "start",
        "asked",
        "--parent",
        "inline",
        "--needs-agent",
        "--role",
        "worker",
        "--template",
        "work-on",
        "--inputs",
        "{}",
    ]);
    for name in ["plain", "asked"] {
        let records = env.records(name);
        assert_eq!(kinds(&records), vec!["session_started"], "{name}");
        assert_eq!(records[0]["koto.run.id"], parent_id.as_str());
        assert_eq!(records[0]["koto.parent.session.id"], parent_id.as_str());
        assert!(records[0].get("koto.template.hash").is_none());
        assert_eq!(records[0]["koto.fixture"], false);
    }
    assert_eq!(env.records("asked")[0]["koto.template.name"], "work-on");
}

#[test]
fn rebind_writes_no_session_started() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("moved", &t);
    let before = env.raw_journal();
    let elsewhere = env.tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    env.ok(&[
        "session",
        "rebind",
        "moved",
        "--to",
        elsewhere.to_str().unwrap(),
    ]);
    assert_eq!(env.raw_journal(), before);
}

/// Event types on `name`'s log, in order.
fn event_types(env: &Env, name: &str) -> Vec<String> {
    std::fs::read_to_string(env.state_path(name))
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| {
            let event: Value = serde_json::from_str(l).unwrap();
            event["type"].as_str().unwrap().to_string()
        })
        .collect()
}

/// `koto session update` appends only its `intent_updated` event: the
/// header line is byte-identical before and after, and the journal, which
/// has no record for an intent change, is unchanged.
#[test]
fn session_update_appends_only_its_event_and_leaves_the_header_untouched() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("described", &t);
    let header_before = std::fs::read_to_string(env.state_path("described"))
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let events_before = event_types(&env, "described");
    let journal_before = env.raw_journal();

    env.ok(&["session", "update", "described", "--intent", "a new intent"]);

    let text = std::fs::read_to_string(env.state_path("described")).unwrap();
    assert_eq!(text.lines().next().unwrap(), header_before);
    let mut expected = events_before;
    expected.push("intent_updated".to_string());
    assert_eq!(event_types(&env, "described"), expected);
    assert_eq!(env.raw_journal(), journal_before);
}

/// `handle_update` appends through `SessionBackend::append_event`, the
/// store's commit funnel, not to the state file directly.
#[test]
fn session_update_appends_through_the_session_backend() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("described", &t);
    let backend = FunnelBackend::new(&env.sessions());
    koto::cli::session::handle_update(&backend, "described", "a new intent").unwrap();
    assert_eq!(
        backend.appended(),
        vec![("described".to_string(), "intent_updated".to_string())]
    );
    assert_eq!(
        event_types(&env, "described").last().map(String::as_str),
        Some("intent_updated")
    );
}

// ----- The driver -----

#[test]
fn session_started_carries_the_creating_driver_only_when_it_is_id_shaped() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    let path = t.to_str().unwrap();
    let cases: [(&str, Option<String>, Option<&str>); 5] = [
        ("with-a", Some("driver-a".into()), Some("driver-a")),
        ("unset", None, None),
        ("empty", Some(String::new()), None),
        ("newline", Some("a\nb".into()), None),
        ("long", Some("d".repeat(200)), None),
    ];
    for (name, value, expected) in &cases {
        let out = env.koto_as(value.as_deref(), &["init", name, "--template", path]);
        assert!(out.ok, "{name}: {}", out.stderr);
        let records = env.records(name);
        assert_eq!(
            records[0]
                .get("koto.driver.session.id")
                .and_then(Value::as_str),
            *expected,
            "{name}"
        );
        // Only session_started carries a driver.
        for r in &records[1..] {
            assert!(r.get("koto.driver.session.id").is_none());
        }
    }
    // A later command under another driver doesn't change the creation
    // driver on record.
    env.koto_as(Some("driver-b"), &["next", "with-a"]);
    let started = &env.records("with-a")[0];
    assert_eq!(started["koto.driver.session.id"], "driver-a");
}

#[test]
fn the_harness_keeps_an_inherited_driver_out_of_the_journal() {
    // A suite run inside a Claude Code session inherits that session's
    // driver. The harness must clear it on every spawn: start from a
    // command that carries a sentinel, as an inherited variable would, and
    // apply the harness environment over it.
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    let sentinel = "sentinel-driver-0b1d";
    let spawn = |driver: Option<&str>, name: &str| {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.env(DRIVER_ENV, sentinel).current_dir(&env.work);
        for (k, v) in env.envs(driver) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        let out = Env::run(cmd, &["init", name, "--template", t.to_str().unwrap()]);
        assert!(out.ok, "{}", out.stderr);
    };

    // The control: left in place, the sentinel is recorded, so this test
    // can fail.
    spawn(Some(sentinel), "control");
    assert_eq!(
        env.records("control")[0]["koto.driver.session.id"],
        sentinel
    );

    // Cleared by the harness, it is not.
    spawn(None, "quiet");
    assert!(env.records("quiet")[0]
        .get("koto.driver.session.id")
        .is_none());
    let quiet: Vec<String> = env.records("quiet").iter().map(|r| r.to_string()).collect();
    assert!(quiet.iter().all(|line| !line.contains(sentinel)));
}

// ----- Fixture rules through koto -----

#[test]
fn the_fixture_flag_follows_the_template_source_directory() {
    let env = Env::new();
    // In a temporary directory: a fixture.
    let temp_t = env.template("simple.md", SIMPLE);
    env.init("in-temp", &temp_t);
    assert_eq!(env.records("in-temp")[0]["koto.fixture"], true);

    // Outside every temporary directory: not one. The roots are /tmp and
    // /var/folders, plus their real locations on macOS, which keeps them
    // below one top-level directory (MACOS_ALIAS_ROOT).
    let plain = non_temp_dir("plain");
    let resolved = std::fs::canonicalize(&plain).unwrap();
    let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let roots: Vec<PathBuf> = ["tmp", "var/folders"]
        .iter()
        .flat_map(|root| {
            [
                Path::new("/").join(root),
                Path::new("/").join(MACOS_ALIAS_ROOT).join(root),
            ]
        })
        .collect();
    if resolved.starts_with(&temp) || roots.iter().any(|root| resolved.starts_with(root)) {
        eprintln!("skipped: the build's scratch directory is itself temporary");
        let _ = std::fs::remove_dir_all(&plain);
        return;
    }
    let plain_t = plain.join("simple.md");
    std::fs::write(&plain_t, SIMPLE).unwrap();
    env.init("plain", &plain_t);
    assert_eq!(env.records("plain")[0]["koto.fixture"], false);

    // A near miss of the mktemp rule, outside the temp dir: not one.
    let near = plain.join("tmp.ab12");
    std::fs::create_dir_all(&near).unwrap();
    std::fs::write(near.join("simple.md"), SIMPLE).unwrap();
    env.init("near-miss", &near.join("simple.md"));
    assert_eq!(env.records("near-miss")[0]["koto.fixture"], false);

    // A mktemp-shaped segment outside the temp dir: one.
    let mk = plain.join("tmp.Ab12Cd");
    std::fs::create_dir_all(&mk).unwrap();
    std::fs::write(mk.join("simple.md"), SIMPLE).unwrap();
    env.init("mktemp", &mk.join("simple.md"));
    assert_eq!(env.records("mktemp")[0]["koto.fixture"], true);

    // A symlink from outside into a temporary directory: not one. The rule
    // reads the directory as the header records it, and an absolute
    // template path is recorded as given, without resolving the link.
    let link = plain.join("linked");
    std::os::unix::fs::symlink(&env.work, &link).unwrap();
    env.init("linked", &link.join("simple.md"));
    assert_eq!(env.records("linked")[0]["koto.fixture"], false);

    // The same plain template with TMPDIR set to its directory: one, since
    // the process's temporary directory counts wherever it is.
    let mut cmd = env.cmd(None);
    cmd.env("TMPDIR", &plain);
    let out = Env::run(
        cmd,
        &["init", "tmpdir", "--template", plain_t.to_str().unwrap()],
    );
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(env.records("tmpdir")[0]["koto.fixture"], true);

    let _ = std::fs::remove_dir_all(&plain);
}

// ----- Names and shapes -----

const NAMED: &str = r#"---
name: NAME
version: "1.0"
initial_state: STATE
states:
  STATE:
    transitions:
      - target: done
  done:
    terminal: true
---

## STATE

First.

## done

Done.
"#;

#[test]
fn names_of_128_characters_are_kept_and_129_are_left_out() {
    let env = Env::new();
    for (label, len) in [("n128", 128usize), ("n129", 129)] {
        let name = format!("t{}", "n".repeat(len - 1));
        let state = format!("s{}", "x".repeat(len - 1));
        let body = NAMED.replace("NAME", &name).replace("STATE", &state);
        let t = env.template(&format!("{label}.md"), &body);
        let out = env.koto(&["init", label, "--template", t.to_str().unwrap()]);
        assert!(out.ok, "{label}: {}", out.stderr);
        let records = env.records(label);
        assert_eq!(kinds(&records), vec!["session_started", "state_entered"]);
        if len == 128 {
            assert_eq!(records[0]["koto.template.name"], name.as_str());
            assert_eq!(records[1]["koto.state"], state.as_str());
        } else {
            assert!(records[0].get("koto.template.name").is_none());
            assert!(records[1].get("koto.state").is_none());
        }
    }
}

#[test]
fn nothing_planted_in_context_evidence_gates_or_variables_reaches_the_journal() {
    let mut env = Env::new();
    let marks = [
        "planted-var-5c1e",
        "planted-gate-output-5c1e",
        "planted-evidence-5c1e",
        "planted-context-5c1e",
    ];
    env.planted = marks.iter().map(|s| s.to_string()).collect();
    let body = r#"---
name: journal-planted
version: "1.0"
initial_state: gather
variables:
  MARK:
    description: a planted value
    required: true
states:
  gather:
    accepts:
      note:
        type: string
        required: true
    gates:
      echo:
        type: command
        command: "echo planted-gate-output-5c1e {{MARK}}"
    transitions:
      - target: done
        when:
          gates.echo.exit_code: 0
  done:
    terminal: true
---

## gather

Gather.

## done

Done.
"#;
    let t = env.template("planted.md", body);
    env.ok(&[
        "init",
        "planted",
        "--template",
        t.to_str().unwrap(),
        "--var",
        "MARK=planted-var-5c1e",
    ]);
    let ctx = env.work.join("ctx.txt");
    std::fs::write(&ctx, "planted-context-5c1e").unwrap();
    env.ok(&[
        "context",
        "add",
        "planted",
        "notes.md",
        "--from-file",
        ctx.to_str().unwrap(),
    ]);
    env.ok(&[
        "next",
        "planted",
        "--with-data",
        r#"{"note": "planted-evidence-5c1e"}"#,
    ]);
    let records = env.records("planted");
    assert_eq!(kinds(&records).last(), Some(&"terminal"), "{records:?}");
    for mark in marks {
        assert!(!env.raw_journal().contains(mark));
    }
}

// ----- Failure never changes behavior -----

/// Run the same short sequence in `env` and return each step's exit status
/// and stdout.
fn sequence(env: &Env) -> Vec<(bool, String)> {
    let t = env.template("simple.md", SIMPLE);
    let steps: Vec<Vec<&str>> = vec![
        vec!["init", "seq", "--template", t.to_str().unwrap()],
        vec!["next", "seq"],
        vec!["next", "seq", "--with-data", r#"{"go": "yes"}"#],
    ];
    let mut outs = Vec::new();
    for step in steps {
        let out = env.koto(&step);
        assert!(
            out.journal_warnings() <= 1,
            "one warning per command at most: {}",
            out.stderr
        );
        outs.push((out.ok, out.stdout));
    }
    outs
}

fn running_as_root() -> bool {
    let probe = TempDir::new().unwrap();
    let ro = probe.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
    let writable = std::fs::write(ro.join("x"), "x").is_ok();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
    writable
}

#[test]
fn a_read_only_koto_home_costs_one_warning_per_command_and_nothing_else() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let good = Env::new();
    let expected = sequence(&good);
    assert!(good.journal_path().exists());

    // A read-only koto home, its session store already in place and still
    // writable: only the journal can't be created.
    let ro = Env::new();
    std::fs::create_dir_all(ro.sessions()).unwrap();
    std::fs::set_permissions(ro.koto_home(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let t = ro.template("simple.md", SIMPLE);
    let out = ro.koto(&["init", "seq", "--template", t.to_str().unwrap()]);
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(out.journal_warnings(), 1, "{}", out.stderr);
    let warning = out
        .stderr
        .lines()
        .find(|l| l.contains("run journal"))
        .unwrap();
    assert!(warning.starts_with("warning: run journal write failed ("));
    ro.ok(&["session", "cleanup", "seq"]);
    let got = sequence(&ro);
    std::fs::set_permissions(ro.koto_home(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(got, expected);
    assert!(!ro.journal_path().exists());
}

/// Whether an append past `ulimit -f` fails here. Linux enforces it; some
/// platforms let the append through.
fn file_size_limit_is_enforced() -> bool {
    let probe = TempDir::new().unwrap();
    let file = probe.path().join("big");
    std::fs::write(&file, vec![b'#'; 8192]).unwrap();
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg("trap '' XFSZ; ulimit -f 1; printf x >> \"$0\"")
        .arg(&file)
        .status();
    let grew = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0) > 8192;
    matches!(status, Ok(s) if !s.success()) && !grew
}

#[test]
fn a_full_journal_file_system_costs_one_warning_and_nothing_else() {
    // A file-size limit stands in for a full disk: an append past it fails
    // with EFBIG, as a full disk fails with ENOSPC, on every platform.
    if !file_size_limit_is_enforced() {
        eprintln!("skipped: this platform doesn't enforce a file-size limit on appends");
        return;
    }
    let good = Env::new();
    let expected = sequence(&good);

    let full = Env::new();
    std::fs::create_dir_all(full.koto_home()).unwrap();
    let filler = format!("{}\n", "#".repeat(1023)).repeat(256);
    std::fs::write(full.journal_path(), &filler).unwrap();
    let t = full.template("simple.md", SIMPLE);
    let koto = assert_cmd::cargo::cargo_bin("koto");
    let steps: Vec<Vec<String>> = vec![
        vec![
            "init".into(),
            "seq".into(),
            "--template".into(),
            t.to_string_lossy().into_owned(),
        ],
        vec!["next".into(), "seq".into()],
        vec![
            "next".into(),
            "seq".into(),
            "--with-data".into(),
            r#"{"go": "yes"}"#.into(),
        ],
    ];
    let mut outs = Vec::new();
    for step in steps {
        // 64 blocks of 512 bytes: room for the session's own files, none
        // for an append to the 256 KiB journal.
        let mut sh = full.process(Path::new("sh"), None);
        sh.arg("-c")
            .arg("trap '' XFSZ; ulimit -f 64; exec \"$0\" \"$@\"")
            .arg(&koto)
            .args(&step);
        let output = sh.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let warnings = stderr.lines().filter(|l| l.contains("run journal")).count();
        assert_eq!(warnings, 1, "{stderr}");
        outs.push((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
        ));
    }
    assert_eq!(outs, expected);
    assert_eq!(
        std::fs::read_to_string(full.journal_path()).unwrap(),
        filler
    );
}

#[test]
fn a_symlink_at_the_journal_path_is_refused_and_the_command_succeeds() {
    let env = Env::new();
    std::fs::create_dir_all(env.koto_home()).unwrap();
    let target = env.tmp.path().join("not-the-journal.txt");
    std::fs::write(&target, "untouched\n").unwrap();
    std::os::unix::fs::symlink(&target, env.journal_path()).unwrap();
    let t = env.template("simple.md", SIMPLE);
    let out = env.koto(&["init", "linked", "--template", t.to_str().unwrap()]);
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(out.json["state"], "start");
    assert_eq!(out.journal_warnings(), 1, "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
}

#[test]
fn a_failed_commit_writes_no_journal_record() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    env.init("locked", &t);
    let before = env.raw_journal();
    let state = env.state_path("locked");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o400)).unwrap();
    let out = env.koto(&["next", "locked", "--with-data", r#"{"go": "yes"}"#]);
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!out.ok, "the commit should have failed: {}", out.stdout);
    assert_eq!(env.raw_journal(), before);
}

#[test]
fn the_journal_is_written_with_workflows_native_off_and_no_decider() {
    let env = Env::new();
    std::fs::create_dir_all(env.koto_home()).unwrap();
    std::fs::write(
        env.koto_home().join("config.toml"),
        "[workflows]\nnative = false\n",
    )
    .unwrap();
    let t = env.template("simple.md", SIMPLE);
    env.init("native-off", &t);
    env.ok(&["next", "native-off", "--with-data", r#"{"go": "yes"}"#]);
    assert_eq!(
        kinds(&env.records("native-off")),
        vec![
            "session_started",
            "state_entered",
            "state_entered",
            "state_entered",
            "terminal"
        ]
    );
}

#[test]
fn the_journal_is_written_with_network_access_blocked() {
    let env = Env::new();
    let t = env.template("simple.md", SIMPLE);
    // Where an unprivileged network namespace is available, run koto with
    // no network at all; elsewhere point every proxy at a closed port.
    let isolated = std::process::Command::new("unshare")
        .args(["-rn", "true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let koto = assert_cmd::cargo::cargo_bin("koto");
    for args in [
        vec![
            "init".to_string(),
            "offline".into(),
            "--template".into(),
            t.to_string_lossy().into_owned(),
        ],
        vec![
            "next".to_string(),
            "offline".into(),
            "--with-data".into(),
            r#"{"go": "yes"}"#.into(),
        ],
    ] {
        let mut cmd = if isolated {
            let mut c = env.process(Path::new("unshare"), None);
            c.arg("-rn").arg(&koto).args(&args);
            c
        } else {
            let mut c = env.process(&koto, None);
            c.args(&args);
            c
        };
        for proxy in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            cmd.env(proxy, "http://127.0.0.1:9");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(kinds(&env.records("offline")).len(), 5);
}

#[test]
fn eight_concurrent_processes_append_whole_lines() {
    let env = Env::new();
    // s0 .. s97: init enters s0, one `koto next` chains through s97, a
    // terminal. Each worker's session writes exactly 100 records.
    let mut body =
        String::from("---\nname: journal-long\nversion: \"1.0\"\ninitial_state: s0\nstates:\n");
    for i in 0..97 {
        body.push_str(&format!(
            "  s{i}:\n    transitions:\n      - target: s{}\n",
            i + 1
        ));
    }
    body.push_str("  s97:\n    terminal: true\n---\n\n");
    for i in 0..98 {
        body.push_str(&format!("## s{i}\n\nStep {i}.\n\n"));
    }
    let t = env.template("long.md", &body);
    for w in 0..8 {
        env.init(&format!("w{w}"), &t);
    }
    let handles: Vec<_> = (0..8)
        .map(|w| {
            let mut cmd = env.cmd(Some(&format!("driver-{w}")));
            cmd.args(["next", &format!("w{w}")]);
            std::thread::spawn(move || cmd.output().unwrap())
        })
        .collect();
    for h in handles {
        let out = h.join().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let journal = env.journal();
    assert_eq!(journal.len(), 800);
    for w in 0..8 {
        let records = env.records(&format!("w{w}"));
        assert_eq!(records.len(), 100, "w{w}");
        let expected: Vec<String> = (0..98).map(|i| format!("s{i}")).collect();
        assert_eq!(states(&records), expected);
        assert_eq!(kinds(&records).last(), Some(&"terminal"));
    }
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// A store redirected with `KOTO_SESSIONS_BASE` is journaled inside its own
/// base: its records go to `<base>/_run_journal.jsonl`, and `HOME` (here a
/// stand-in for the developer's real home, apart from the store) is left
/// with no journal file anywhere under it. The journal file at the base
/// level is never listed as a session.
#[test]
fn a_store_redirected_by_koto_sessions_base_journals_inside_its_base() {
    let env = Env::new();
    let base = env.tmp.path().join("redirected-sessions");
    let parent = env.template("parent.md", BATCH_PARENT);
    env.template("child.md", BATCH_CHILD);
    let simple = env.template("simple.md", SIMPLE);
    let run = |args: &[&str]| {
        let mut cmd = env.cmd(Some("driver-redirected"));
        cmd.env("KOTO_SESSIONS_BASE", &base);
        let out = Env::run(cmd, args);
        assert!(
            out.ok,
            "koto {:?} failed\nstdout: {}\nstderr: {}",
            args, out.stdout, out.stderr
        );
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
        out
    };

    run(&["init", "seq", "--template", simple.to_str().unwrap()]);
    assert!(
        base.join("seq").is_dir(),
        "the session lives in the redirected store"
    );
    assert!(!env.session_dir("seq").exists());
    run(&["next", "seq", "--with-data", r#"{"go": "yes"}"#]);
    run(&["init", "kept", "--template", simple.to_str().unwrap()]);
    run(&["cancel", "kept"]);
    run(&["init", "parent", "--template", parent.to_str().unwrap()]);
    let tasks = serde_json::json!({"tasks": [{"name": "A", "waits_on": [], "vars": {}}]});
    run(&["next", "parent", "--with-data", &tasks.to_string()]);
    assert!(base.join("parent.A").is_dir(), "the child was spawned");

    // Nothing under HOME: the real home's journal is never written.
    let under_home: Vec<PathBuf> = files_under(&env.home)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n == JOURNAL || n == "run-journal.json")
        })
        .collect();
    assert_eq!(under_home, Vec::<PathBuf>::new());
    assert!(!env.journal_path().exists());

    // The store's own journal, inside the redirected base.
    let journal = base.join(JOURNAL);
    let records: Vec<serde_json::Value> = std::fs::read_to_string(&journal)
        .unwrap_or_else(|e| panic!("{}: {e}", journal.display()))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let started: Vec<&str> = records
        .iter()
        .filter(|r| r["kind"] == "session_started")
        .map(|r| r["session"].as_str().unwrap())
        .collect();
    for name in ["seq", "kept", "parent", "parent.A"] {
        assert!(started.contains(&name), "{name}: {started:?}");
    }
    assert!(records
        .iter()
        .any(|r| r["kind"] == "terminal" && r["session"] == "seq"));
    assert!(records
        .iter()
        .any(|r| r["kind"] == "cancelled" && r["session"] == "kept"));

    // The journal file at the base level is not read as a session.
    let mut cmd = env.cmd(None);
    cmd.env("KOTO_SESSIONS_BASE", &base);
    let listed = Env::run(cmd, &["workflows"]);
    assert!(listed.ok, "{}", listed.stderr);
    assert!(!listed.stdout.contains(JOURNAL), "{}", listed.stdout);
    assert!(!listed.stderr.contains(JOURNAL), "{}", listed.stderr);
}
