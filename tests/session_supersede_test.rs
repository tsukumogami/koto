//! `koto init SUCC --continues PRED --supersede` through the real binary on
//! the local backend: the fence in the predecessor's log, the carry, the
//! refusal of every later write to the predecessor while its reads keep
//! working, the refusals of the command itself, the bounded lock wait, and
//! a re-run that changes nothing.
//!
//! Every koto process runs in its own temporary directory, under a scratch
//! `HOME` and `KOTO_HOME`, with every `KOTO_*` variable inherited from the
//! test's environment removed (`KOTO_SESSIONS_BASE` and every
//! `KOTO_SESSION_*` included) and `CLAUDE_CODE_SESSION_ID` removed. Each
//! test checks that koto resolves its store under that directory before it
//! runs anything else.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: supersede-test
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

/// The prefixes the carry is asked for.
const CARRY: [&str; 4] = ["record/", "legs/", "taken_legs", "leg_children"];

/// The predecessor's keys: two or more under each carried prefix, nested
/// ones included, and one outside them.
const PRED_KEYS: [(&str, &str); 10] = [
    ("record/a", "alpha"),
    ("record/entries/000001", "first entry"),
    ("record/entries/000002", "second entry"),
    ("legs/req-1/leg-a", "leg a"),
    ("legs/req-1/leg-b", "leg b"),
    ("taken_legs/req-1", "[\"leg-a\"]"),
    ("taken_legs.idx", "1"),
    ("leg_children/req-1/leg-a", "child-1"),
    ("leg_children/req-1/leg-b", "child-2"),
    ("coord/x", "coordinator only"),
];

struct Env {
    _tmp: TempDir,
    root: PathBuf,
    home: PathBuf,
    koto_home: PathBuf,
    cache: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let koto_home = root.join("koto-home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &koto_home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("supersede-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let env = Env {
            _tmp: tmp,
            root,
            home,
            koto_home,
            cache,
            work,
            template,
        };
        env.assert_scratch_store();
        env
    }

    /// koto resolves its root to this test's `KOTO_HOME`, inside its own
    /// temporary directory, so no command here reaches a real store.
    fn assert_scratch_store(&self) {
        let out = self.cmd().args(["session", "dir"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let resolved = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        assert_eq!(resolved, self.koto_home);
        assert!(resolved.starts_with(&self.root));
    }

    fn cmd(&self) -> Command {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.work)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env("XDG_CACHE_HOME", &self.cache);
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> std::process::Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn json(out: &std::process::Output) -> Value {
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    /// Run `args`, expect exit `code`, and return the output.
    fn fails(&self, args: &[&str], code: i32) -> std::process::Output {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.koto_home.join("sessions").join(name)
    }

    fn log(&self, name: &str) -> PathBuf {
        self.session_dir(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn init(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
    }

    fn add(&self, session: &str, key: &str, value: &str) {
        let out = self
            .cmd()
            .args(["context", "add", session, key])
            .write_stdin(value)
            .output()
            .unwrap();
        assert!(out.status.success(), "context add {key}: {out:?}");
    }

    fn get(&self, session: &str, key: &str) -> String {
        let out = self.ok(&["context", "get", session, key]);
        String::from_utf8(out.stdout).unwrap()
    }

    fn list(&self, session: &str, prefix: &str) -> Vec<String> {
        let out = self.ok(&["context", "list", session, "--prefix", prefix]);
        serde_json::from_value(Env::json(&out)).unwrap()
    }

    /// Every key under the carried prefixes in `session`, with its value.
    fn carried_view(&self, session: &str) -> BTreeMap<String, String> {
        let mut view = BTreeMap::new();
        for prefix in CARRY {
            for key in self.list(session, prefix) {
                let value = self.get(session, &key);
                view.insert(key, value);
            }
        }
        view
    }

    fn supersede_args<'a>(&'a self, succ: &'a str, pred: &'a str) -> Vec<&'a str> {
        vec![
            "init",
            succ,
            "--template",
            self.template.to_str().unwrap(),
            "--continues",
            pred,
            "--supersede",
        ]
    }
}

/// Every file under `dir` with its bytes, by relative path.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

#[test]
fn supersede_fences_the_predecessor_carries_its_keys_and_reruns_unchanged() {
    let env = Env::new();
    env.init("p");
    // The store is the scratch one: p's log is under this test's KOTO_HOME.
    assert!(
        env.log("p").is_file(),
        "p's log is under {:?}",
        env.koto_home
    );
    for (key, value) in PRED_KEYS {
        env.add("p", key, value);
    }

    let mut args = env.supersede_args("s", "p");
    for prefix in CARRY {
        args.extend(["--carry", prefix]);
    }
    let out = Env::json(&env.ok(&args));
    assert_eq!(out["name"], "s");
    assert_eq!(out["state"], "start");
    assert_eq!(out["outcome"], "superseded");
    assert_eq!(out["superseded"], "p");
    assert_eq!(out["forward"], "s");
    assert_eq!(out["carried"], 9);

    // 1. The four prefixes carry exactly; nothing else does.
    let carried = env.carried_view("s");
    assert_eq!(carried.len(), 9);
    assert_eq!(carried, env.carried_view("p"));
    assert!(env.list("s", "coord/").is_empty());
    env.ok(&["context", "exists", "s", "koto/superseded-carry-complete"]);
    // s continues p, and the fence in p names s by its own session id.
    env.ok(&["context", "exists", "s", "koto/links/continues/p"]);
    let s_header: Value = serde_json::from_str(
        std::fs::read_to_string(env.log("s"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let p_log = std::fs::read_to_string(env.log("p")).unwrap();
    let fence: Value = p_log
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e["type"] == "session_superseded")
        .expect("p's log holds the fence");
    assert_eq!(fence["payload"]["by"], "s");
    assert_eq!(fence["payload"]["by_session_id"], s_header["session_id"]);

    // 2. Writes to p are refused, naming s, and leave p as it was.
    let p_before = std::fs::read(env.log("p")).unwrap();
    let out = env.run(&["next", "p"]);
    assert_ne!(out.status.code(), Some(0));
    let body = Env::json(&out);
    assert_eq!(body["error"]["code"], "session_superseded");
    assert!(body["error"]["message"].as_str().unwrap().contains("'s'"));
    assert_eq!(body["newest"], "s");
    assert_eq!(std::fs::read(env.log("p")).unwrap(), p_before);

    let out = env
        .cmd()
        .args(["context", "add", "p", "record/b"])
        .write_stdin("late")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let body = Env::json(&out);
    assert_eq!(body["code"], "session_superseded");
    let message = body["error"].as_str().unwrap();
    assert!(message.contains("session_superseded"), "{message}");
    assert!(message.contains("'s'"), "{message}");
    assert_eq!(std::fs::read(env.log("p")).unwrap(), p_before);
    assert!(!env.session_dir("p").join("ctx/record/b").exists());
    assert!(!env.list("p", "record/").contains(&"record/b".to_string()));

    // 3. Reads of p keep working.
    assert_eq!(env.get("p", "record/a"), "alpha");

    // 4. A second successor is refused, naming the first, and not created.
    let out = env.fails(&env.supersede_args("t", "p"), 2);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("superseded_by=s"), "{stdout}");
    assert_eq!(Env::json(&out)["superseded_by"], "s");
    assert!(!env.session_dir("t").exists());
    assert_eq!(std::fs::read(env.log("p")).unwrap(), p_before);

    // 6. The same command again changes nothing.
    let s_before = snapshot(&env.session_dir("s"));
    let out = Env::json(&env.ok(&args));
    assert_eq!(out["outcome"], "unchanged");
    assert_eq!(out["carried"], 0);
    assert_eq!(snapshot(&env.session_dir("s")), s_before);
    assert_eq!(std::fs::read(env.log("p")).unwrap(), p_before);
    assert_eq!(env.carried_view("s"), carried);
}

#[test]
fn supersede_refuses_with_nothing_written() {
    let env = Env::new();
    env.init("q");
    assert!(
        env.log("q").is_file(),
        "q's log is under {:?}",
        env.koto_home
    );
    let q_before = std::fs::read(env.log("q")).unwrap();

    // self
    let out = Env::json(&env.fails(&env.supersede_args("q", "q"), 2));
    assert_eq!(out["code"], "self");

    // stale_state
    let mut args = env.supersede_args("u", "q");
    args.extend(["--expect-state", "done"]);
    let out = Env::json(&env.fails(&args, 2));
    assert_eq!(out["code"], "stale_state");
    assert_eq!(out["actual"], "start");
    assert!(!env.session_dir("u").exists());

    // --carry without --supersede is a usage error.
    let out = env.run(&[
        "init",
        "v",
        "--template",
        env.template.to_str().unwrap(),
        "--continues",
        "q",
        "--carry",
        "record/",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!env.session_dir("v").exists());

    assert_eq!(std::fs::read(env.log("q")).unwrap(), q_before);
}

#[test]
fn supersede_gives_up_on_a_lock_held_past_ten_seconds() {
    let env = Env::new();
    env.init("q");
    assert!(
        env.log("q").is_file(),
        "q's log is under {:?}",
        env.koto_home
    );
    let q_before = std::fs::read(env.log("q")).unwrap();

    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(env.session_dir("q").join("append.lock"))
        .unwrap();
    // SAFETY: flock on a descriptor `lock` owns until it is dropped below.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);

    let started = Instant::now();
    let out = Env::json(&env.fails(&env.supersede_args("u", "q"), 3));
    assert!(started.elapsed() >= Duration::from_secs(10));
    assert_eq!(out["code"], "fence_unavailable");
    assert!(!env.session_dir("u").exists());
    assert_eq!(std::fs::read(env.log("q")).unwrap(), q_before);

    drop(lock);
    env.ok(&["next", "q"]);
}
