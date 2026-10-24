//! Every removal koto makes keeps a pinned session and says why
//! (DESIGN-session-pin.md, the removal guard): the terminal tick, the parent
//! sweep, `koto session prune`, `koto workspace prune`, `koto session
//! cleanup`, `koto cancel --cleanup` and the same-name replace, driven through
//! the real binary in a scratch HOME on the local backend.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// A session that runs until `marker` says how it ends: `done` reaches the
/// success terminal, `fail` the failure terminal.
const WORK: &str = r#"---
name: pin-work
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
      - target: done_blocked
        when:
          marker: fail
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
---

## work

Work.

## done

Done.

## done_blocked

Blocked.
"#;

const REASON: &str = "record of the merged change";

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    template: PathBuf,
    work: PathBuf,
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
        let template = root.join("work.md");
        std::fs::write(&template, WORK).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            template,
            work,
        }
    }

    /// A command in the workspace with a one-second retention and no
    /// anchor, agent session or backend inherited.
    fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        cmd
    }

    /// [`Env::cmd`] as a `std::process::Command`, for a koto run the test
    /// keeps going while it acts.
    fn std_cmd(&self) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// The JSON on stdout of a command expected to exit `code`.
    fn fails(&self, args: &[&str], code: i32) -> Value {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let text = if out.stdout.is_empty() {
            &out.stderr
        } else {
            &out.stdout
        };
        serde_json::from_slice(text).unwrap_or(Value::Null)
    }

    /// A running session, optionally a koto child of `parent`.
    fn running(&self, name: &str, parent: Option<&str>) {
        let template = self.template.to_str().unwrap();
        let mut args = vec!["init", name, "--template", template];
        if let Some(p) = parent {
            args.extend(["--parent", p]);
        }
        self.ok(&args);
    }

    /// Tick `name` to its success (`done`) or failure (`fail`) terminal.
    fn tick(&self, name: &str, marker: &str, no_cleanup: bool) -> Value {
        let data = format!(r#"{{"marker":"{marker}"}}"#);
        let mut args = vec!["next", name, "--with-data", &data];
        if no_cleanup {
            args.push("--no-cleanup");
        }
        self.ok(&args)
    }

    /// A finished session, kept on disk.
    fn done(&self, name: &str, parent: Option<&str>) {
        self.running(name, parent);
        self.tick(name, "done", true);
    }

    fn pin(&self, name: &str) -> Value {
        self.ok(&["session", "pin", name, "--reason", REASON])["pin"].clone()
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).is_dir()
    }

    /// SHA-256 of every file in session `name`'s directory, by relative path.
    fn hashes(&self, name: &str) -> BTreeMap<PathBuf, String> {
        let root = self.sessions().join(name);
        let mut out = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                if meta.is_dir() {
                    stack.push(p);
                } else {
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

    fn state_log(&self, name: &str) -> Vec<u8> {
        std::fs::read(
            self.sessions()
                .join(name)
                .join(format!("koto-{name}.state.jsonl")),
        )
        .unwrap()
    }
}

/// The JSON object a command printed last, after any preview text or
/// prompt on the same line.
fn last_json(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text.lines().last().unwrap_or_default();
    let start = line
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in {text}"));
    serde_json::from_str(&line[start..]).unwrap()
}

fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1300));
}

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|r| match r {
            Value::String(s) => s.clone(),
            r => r["name"].as_str().unwrap().to_string(),
        })
        .collect();
    names.sort();
    names
}

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no entry for {name} in {list}"))
}

/// The entry names `pin` as R15 asks: `kept_by_pin`, and the pin's reason,
/// setter, time and source.
fn assert_names_pin(e: &Value, pin: &Value) {
    assert_eq!(e["kept_by_pin"], true, "{e}");
    assert_eq!(e["pin"]["reason"], REASON, "{e}");
    assert_eq!(e["pin"]["set_by"], Value::Null, "{e}");
    assert_eq!(e["pin"]["set_at"], pin["set_at"], "{e}");
    assert_eq!(e["pin"]["source"], "local", "{e}");
}

fn assert_no_pin_fields(e: &Value) {
    for field in ["kept_by_pin", "pin", "pin_unreadable"] {
        assert!(e.get(field).is_none(), "{field} in {e}");
    }
}

fn pin_text(pin: &Value) -> String {
    format!(
        "pinned: {REASON} (set by hand at {})",
        pin["set_at"].as_str().unwrap()
    )
}

#[test]
fn the_removal_set_keeps_a_pinned_session_and_names_its_pin() {
    let env = Env::new();
    env.running("X", None);
    env.running("P", Some("X"));
    env.running("U", None);
    let pin = env.pin("P");

    // S1: P and U reach their success terminal without --no-cleanup.
    let s1 = env.tick("P", "done", false);
    assert_eq!(s1["retention"]["retained"], true, "{s1}");
    assert_eq!(s1["retention"]["reason"], "pinned", "{s1}");
    assert_names_pin(&s1["retention"], &pin);
    let held = env.hashes("P");
    let u = env.tick("U", "done", false);
    assert_eq!(u["retention"], serde_json::json!({"retained": false}));
    assert!(!env.exists("U"));

    // S2: X's terminal; P is listed among the children it keeps.
    let s2 = env.tick("X", "done", false);
    let kept = entry(&s2["kept_children"], "P");
    assert_eq!(kept["reason"], "pinned", "{s2}");
    assert_eq!(kept["detail"], pin_text(&pin));
    assert_names_pin(kept, &pin);
    assert_eq!(env.hashes("P"), held);

    // S3: past the retention, the prune keeps P by its pin.
    settle();
    let preview = env.ok(&["session", "prune", "--all"]);
    assert_names_pin(entry(&preview["kept"], "P"), &pin);
    let s3 = env.ok(&["session", "prune", "--apply", "--all"]);
    let kept = entry(&s3["kept"], "P");
    assert_eq!(kept["rule"], "p");
    assert_names_pin(kept, &pin);
    assert!(env.exists("P"));
    assert_eq!(env.hashes("P"), held);

    // S5: cleanup refuses, naming the pin and saying to unpin first.
    let s5 = env.fails(&["session", "cleanup", "P"], 2);
    assert_eq!(s5["error"]["code"], "pinned", "{s5}");
    let message = s5["error"]["message"].as_str().unwrap();
    assert!(message.contains(REASON), "{message}");
    assert!(message.contains("set by hand"), "{message}");
    assert!(
        message.contains(pin["set_at"].as_str().unwrap()),
        "{message}"
    );
    assert!(message.contains("koto session unpin P"), "{message}");
    assert_names_pin(&s5["error"], &pin);
    assert_eq!(env.hashes("P"), held);

    // S6: the replace moves P aside under the name its answer reports.
    let template = env.template.to_str().unwrap();
    let s6 = env.ok(&["init", "P", "--template", template, "--replace-terminal"]);
    let moved = s6["moved_to"].as_str().unwrap().to_string();
    assert_ne!(moved, "P");
    assert_names_pin(&s6, &pin);
    assert_eq!(s6["moved_reason"], pin_text(&pin));
    let moved_hashes = env.hashes(&moved);
    let pin_key = PathBuf::from("ctx/koto/links/pin");
    assert_eq!(moved_hashes.get(&pin_key), held.get(&pin_key));
    let log = PathBuf::from("koto-P.state.jsonl");
    assert!(held.contains_key(&log));
    // The new run holds the original name, unpinned.
    assert!(!env.sessions().join("P/ctx/koto/links/pin").exists());
    let h = env.ok(&["session", "hierarchy", &moved, "--json"]);
    assert_names_pin(&h, &pin);
}

#[test]
fn a_pin_keeps_a_failure_terminal_and_a_session_pinned_while_it_ran() {
    let env = Env::new();
    // Pinned while running, with --no-cleanup.
    env.running("A", None);
    let pin = env.pin("A");
    let a = env.tick("A", "done", true);
    assert_eq!(a["retention"]["reason"], "pinned", "{a}");
    assert_names_pin(&a["retention"], &pin);

    // A failure terminal carries the pin beside failure_terminal.
    env.running("F", None);
    let pin = env.pin("F");
    let f = env.tick("F", "fail", false);
    assert_eq!(f["retention"]["reason"], "failure_terminal", "{f}");
    assert_names_pin(&f["retention"], &pin);
    assert!(env.exists("F"));

    // An unpinned failure terminal is reported as before.
    env.running("G", None);
    let g = env.tick("G", "fail", false);
    assert_eq!(
        g["retention"],
        serde_json::json!({"retained": true, "reason": "failure_terminal"})
    );
}

#[test]
fn cleanup_and_cancel_cleanup_refuse_a_pinned_session_and_write_nothing() {
    let env = Env::new();
    env.running("R", None);
    env.done("F", None);
    let pin_r = env.pin("R");
    let pin_f = env.pin("F");
    let r_before = env.hashes("R");
    let f_before = env.hashes("F");

    let r = env.fails(&["cancel", "R", "--cleanup"], 2);
    assert_eq!(r["error"]["code"], "pinned", "{r}");
    assert_names_pin(&r["error"], &pin_r);
    assert!(r["error"]["message"].as_str().unwrap().contains("unpin"));
    assert_eq!(env.hashes("R"), r_before);

    let f = env.fails(&["cancel", "F", "--cleanup"], 2);
    assert_eq!(f["error"]["code"], "pinned", "{f}");
    assert_names_pin(&f["error"], &pin_f);
    assert_eq!(env.hashes("F"), f_before);

    let c = env.fails(&["session", "cleanup", "F"], 2);
    assert_eq!(c["error"]["code"], "pinned", "{c}");
    assert_eq!(env.hashes("F"), f_before);

    // Cancel without --cleanup still cancels, and the pin stays.
    let log_before = env.state_log("R");
    let cancelled = env.ok(&["cancel", "R"]);
    assert_eq!(cancelled["cancelled"], true);
    assert_ne!(env.state_log("R"), log_before);
    assert!(env.sessions().join("R/ctx/koto/links/pin").is_file());

    // A name with no session behaves as it always did.
    let gone = env.ok(&["session", "cleanup", "nosuch"]);
    assert_eq!(gone["cleaned_up"], true);
}

#[test]
fn workspace_prune_keeps_a_pinned_child_and_removes_its_released_siblings() {
    let env = Env::new();
    env.done("X", None);
    for c in ["C1", "C2", "C3"] {
        env.done(c, Some("X"));
    }
    let pin = env.pin("C2");
    let c2 = env.hashes("C2");
    settle();

    let out = env.run(&["workspace", "prune", "--root", "X", "--yes"]);
    assert!(out.status.success(), "{out:?}");
    let out = last_json(&out.stdout);
    assert_eq!(names(&out["removed"]), vec!["C1", "C3", "X"], "{out}");
    let kept = entry(&out["kept"], "C2");
    assert_names_pin(kept, &pin);
    assert_eq!(kept["reason"], pin_text(&pin));
    assert_eq!(env.hashes("C2"), c2);

    // The pinned child outlives its parent, read by its own name.
    let status = env.ok(&["status", "C2"]);
    assert_eq!(status["current_state"], "done", "{status}");
    let h = env.ok(&["session", "hierarchy", "C2", "--json"]);
    assert_names_pin(&h, &pin);
    let pruned = env.ok(&["session", "prune", "--apply", "--all"]);
    assert_names_pin(entry(&pruned["kept"], "C2"), &pin);
    assert_eq!(env.hashes("C2"), c2);

    // Unpinned by hand, the next prune takes it.
    env.ok(&["session", "unpin", "C2"]);
    let pruned = env.ok(&["session", "prune", "--apply", "--all"]);
    assert_eq!(names(&pruned["removed"]), vec!["C2"]);
}

#[test]
fn workspace_prune_with_force_keeps_a_pinned_root_and_child() {
    let env = Env::new();
    env.done("X", None);
    for c in ["C1", "C2", "C3"] {
        env.done(c, Some("X"));
    }
    let pin_c2 = env.pin("C2");
    let pin_x = env.pin("X");
    settle();

    let mut cmd = env.cmd();
    cmd.args(["workspace", "prune", "--root", "X", "--yes", "--force"])
        .write_stdin("force-prune\n");
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let json = last_json(&out.stdout);
    assert_eq!(names(&json["removed"]), vec!["C1", "C3"], "{json}");
    assert_names_pin(entry(&json["kept"], "C2"), &pin_c2);
    assert_names_pin(entry(&json["kept"], "X"), &pin_x);
    assert!(env.exists("X") && env.exists("C2"));
}

#[test]
fn a_pin_set_while_workspace_prune_waits_keeps_its_session() {
    let env = Env::new();
    env.done("X", None);
    env.done("P", Some("X"));
    settle();

    let mut child = env
        .std_cmd()
        .args(["workspace", "prune", "--root", "X"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The preview is printed and the prompt waits on stdin.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let pin = env.pin("P");
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("remove"), "{stdout}");
    let json = last_json(&out.stdout);
    let skipped = entry(&json["skipped"], "P");
    assert_names_pin(skipped, &pin);
    assert!(env.exists("P"));
}

#[test]
fn every_session_prune_filter_keeps_a_pinned_session() {
    let env = Env::new();
    env.done("P", None);
    let pin = env.pin("P");
    settle();
    let dir = env.work.to_str().unwrap().to_string();
    for args in [
        vec!["session", "prune", "--apply", "--all"],
        vec!["session", "prune", "--apply", "--execution-dir", &dir],
        vec!["session", "prune", "--apply", "--older-than", "1s"],
    ] {
        let out = env.ok(&args);
        assert_names_pin(entry(&out["kept"], "P"), &pin);
        assert!(env.exists("P"), "{args:?}");
    }
}

#[test]
fn a_released_replace_sweeps_unpinned_children_and_keeps_a_pinned_one() {
    let env = Env::new();
    env.done("X", None);
    for c in ["C1", "C2", "C3"] {
        env.done(c, Some("X"));
    }
    let pin = env.pin("C2");
    let c2 = env.hashes("C2");
    settle();

    let template = env.template.to_str().unwrap();
    let out = env.ok(&["init", "X", "--template", template, "--replace-terminal"]);
    assert_eq!(out["outcome"], "replaced", "{out}");
    assert!(out.get("moved_to").is_none(), "{out}");
    assert_names_pin(entry(&out["kept_children"], "C2"), &pin);
    assert!(!env.exists("C1") && !env.exists("C3"));
    assert_eq!(env.hashes("C2"), c2);
}

#[test]
fn unpinned_entries_are_the_same_beside_a_pinned_sibling() {
    // X's terminal tick lists its kept children; C1's entry is the same with
    // and without a pin on C2.
    let entries = |pin: bool| {
        let env = Env::new();
        env.running("X", None);
        env.done("C1", Some("X"));
        env.done("C2", Some("X"));
        if pin {
            env.pin("C2");
        }
        let out = env.tick("X", "done", false);
        let c1 = entry(&out["kept_children"], "C1").clone();
        assert_no_pin_fields(&c1);
        (c1, out["retention"].clone())
    };
    assert_eq!(entries(true), entries(false));
}
