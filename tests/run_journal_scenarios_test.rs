//! Scenarios a reader of the run journal relies on, restated as koto
//! commands (see `docs/reference/run-journal.md`).
//!
//! Each test runs the koto binary with `HOME` in its own temporary
//! directory, `KOTO_SESSIONS_BASE` unset, and `CLAUDE_CODE_SESSION_ID` set
//! or cleared explicitly on every process, then reads that home's journal
//! and checks the fields a reader joins on: the run id, the session id, the
//! state, the driver, the template name and hash, and the fixture flag.

#![cfg(unix)]

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const DRIVER_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// `start` waits for `go`, then `middle` chains to `waiting`, which waits
/// for `finish`.
const STEPS: &str = r#"---
name: scenario-steps
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
      - target: middle
        when:
          go: "yes"
  middle:
    transitions:
      - target: waiting
  waiting:
    accepts:
      finish:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: done
        when:
          finish: "yes"
  done:
    terminal: true
---

## start

Start.

## middle

Middle.

## waiting

Waiting.

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        Env {
            _tmp: tmp,
            home,
            work,
        }
    }

    /// A koto command in this home, under `driver` (or none), from `cwd`.
    fn cmd(&self, driver: Option<&str>, cwd: &Path) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.home.join("cache"));
        for unset in [
            "KOTO_SESSIONS_BASE",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_DECIDER",
            "KOTO_DECIDER_API_KEY",
            "KOTO_DECIDER_ENDPOINT",
        ] {
            cmd.env_remove(unset);
        }
        match driver {
            Some(d) => cmd.env(DRIVER_ENV, d),
            None => cmd.env_remove(DRIVER_ENV),
        };
        cmd
    }

    fn run(&self, mut cmd: Command, args: &[&str]) {
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("run journal"), "{stderr}");
    }

    fn koto(&self, driver: Option<&str>, args: &[&str]) {
        self.run(self.cmd(driver, &self.work), args);
    }

    fn init(&self, driver: Option<&str>, name: &str, template: &Path) {
        self.koto(
            driver,
            &["init", name, "--template", template.to_str().unwrap()],
        );
    }

    fn template(&self, dir: &Path, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("template.md");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.home.join(".koto").join("_run_journal.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn started(&self, session: &str) -> Value {
        self.journal()
            .into_iter()
            .find(|r| r["kind"] == "session_started" && r["session"] == session)
            .unwrap_or_else(|| panic!("no session_started for {session}"))
    }
}

/// A directory outside every temporary directory, or `None` when the
/// build's scratch space is itself under one (the scenario can't run).
fn plain_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "run-journal-scenarios-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let resolved = std::fs::canonicalize(&dir).unwrap();
    let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let temporary = resolved.starts_with(&temp)
        || [
            "/tmp",
            "/var/folders",
            "/private/tmp",
            "/private/var/folders",
        ]
        .iter()
        .any(|root| resolved.starts_with(root) || dir.starts_with(root));
    if temporary {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    Some(dir)
}

#[test]
fn fixture_classification_and_its_near_misses() {
    let env = Env::new();
    let Some(plain) = plain_dir() else {
        eprintln!("skipped: the build's scratch directory is itself temporary");
        return;
    };
    // (session, directory under `plain`, expected flag)
    let cases: &[(&str, &str, bool)] = &[
        ("plain", "plain", false),
        ("mktemp", "tmp.Ab12Cd", true),
        ("mktemp-short", "tmp.Ab12", false),
        ("mktemp-no-dot", "tmpAb12Cd34", false),
        ("ablation", "shirabe-ablation.run7", true),
        ("ablation-no-dot", "shirabe-ablation", false),
        ("tool-test", "niwa/tests", true),
        ("tool-test-singular", "shirabe/test", true),
        ("tool-test-near", "niwa/testing", false),
    ];
    for (session, dir, _) in cases {
        let t = env.template(&plain.join(dir), STEPS);
        env.init(None, session, &t);
    }
    // In a temporary directory.
    let temp = TempDir::new().unwrap();
    let t = env.template(temp.path(), STEPS);
    env.init(None, "in-temp", &t);
    // With a relative TMPDIR the process's temporary directory counts for
    // nothing, even when the template sits under the directory it names.
    let scratch_t = env.template(&plain.join("scratch"), STEPS);
    let mut cmd = env.cmd(None, &plain);
    cmd.env("TMPDIR", "scratch");
    env.run(
        cmd,
        &[
            "init",
            "relative-tmpdir",
            "--template",
            scratch_t.to_str().unwrap(),
        ],
    );
    // An absolute TMPDIR counts wherever it is.
    let mut cmd = env.cmd(None, &env.work);
    cmd.env("TMPDIR", plain.join("scratch"));
    env.run(
        cmd,
        &[
            "init",
            "absolute-tmpdir",
            "--template",
            scratch_t.to_str().unwrap(),
        ],
    );

    let mut expected: Vec<(&str, bool)> = cases.iter().map(|(s, _, f)| (*s, *f)).collect();
    expected.extend([
        ("in-temp", true),
        ("relative-tmpdir", false),
        ("absolute-tmpdir", true),
    ]);
    for (session, flag) in expected {
        assert_eq!(env.started(session)["koto.fixture"], flag, "{session}");
    }
    let _ = std::fs::remove_dir_all(&plain);
}

#[test]
fn two_roots_under_one_driver_get_distinct_run_ids_and_both_carry_the_driver() {
    let env = Env::new();
    let t = env.template(&env.work.clone(), STEPS);
    env.init(Some("driver-one"), "first", &t);
    env.init(Some("driver-one"), "second", &t);
    let first = env.started("first");
    let second = env.started("second");
    for s in [&first, &second] {
        assert_eq!(s["koto.driver.session.id"], "driver-one");
        assert_eq!(
            s["koto.run.id"], s["koto.session.id"],
            "a root is its own run"
        );
    }
    assert_ne!(first["koto.run.id"], second["koto.run.id"]);
}

#[test]
fn alternating_two_roots_under_one_driver_reconstructs_each_sequence_by_run_id() {
    let env = Env::new();
    let t = env.template(&env.work.clone(), STEPS);
    let d = Some("driver-one");
    env.init(d, "first", &t);
    env.init(d, "second", &t);
    env.koto(d, &["next", "first", "--with-data", r#"{"go": "yes"}"#]);
    env.koto(d, &["next", "second", "--with-data", r#"{"go": "yes"}"#]);
    env.koto(
        d,
        &["next", "second", "--with-data", r#"{"finish": "yes"}"#],
    );
    env.koto(d, &["next", "first", "--with-data", r#"{"finish": "yes"}"#]);

    let journal = env.journal();
    // The two runs' records really are interleaved in the file.
    let order: Vec<&str> = journal
        .iter()
        .filter(|r| r["kind"] == "state_entered")
        .map(|r| r["session"].as_str().unwrap())
        .collect();
    assert_ne!(order, {
        let mut sorted = order.clone();
        sorted.sort();
        sorted
    });

    let first_run = env.started("first")["koto.run.id"].clone();
    let second_run = env.started("second")["koto.run.id"].clone();
    for run in [&first_run, &second_run] {
        let states: Vec<&str> = journal
            .iter()
            .filter(|r| r["kind"] == "state_entered" && &r["koto.run.id"] == run)
            .map(|r| r["koto.state"].as_str().unwrap())
            .collect();
        assert_eq!(states, vec!["start", "middle", "waiting", "done"], "{run}");
    }
}

#[test]
fn one_template_name_compiled_before_and_after_an_edit_differs_only_in_hash() {
    let env = Env::new();
    let dir = env.work.join("edited");
    let t = env.template(&dir, STEPS);
    env.init(Some("driver-one"), "before", &t);
    std::fs::write(&t, STEPS.replace("Middle.", "Middle, edited.")).unwrap();
    env.init(Some("driver-one"), "after", &t);
    let before = env.started("before");
    let after = env.started("after");
    assert_eq!(before["koto.template.name"], "scenario-steps");
    assert_eq!(after["koto.template.name"], "scenario-steps");
    let hash = |r: &Value| r["koto.template.hash"].as_str().unwrap().to_string();
    assert!(!hash(&before).is_empty());
    assert_ne!(hash(&before), hash(&after));
}
