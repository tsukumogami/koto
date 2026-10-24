//! `koto session pin` and `koto session unpin` on the local backend, and the
//! prune and hierarchy surfaces that read a pin (DESIGN-session-pin.md),
//! driven through the real binary in a scratch HOME.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// One state, terminal: the first `koto next` finishes the session.
const TEMPLATE: &str = r#"---
name: pin-term
version: "1.0"
initial_state: done
states:
  done:
    terminal: true
---

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    root: PathBuf,
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
        let template = root.join("term.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            root,
            template,
            work,
        }
    }

    /// A command in the workspace with a one-second retention and no
    /// anchor, agent session or backend inherited.
    fn cmd(&self, env: &[(&str, &str)]) -> assert_cmd::Command {
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
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        self.cmd(env).args(args).output().unwrap()
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

    fn text(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "koto {args:?} failed");
        String::from_utf8(out.stdout).unwrap()
    }

    /// A finished session, kept on disk.
    fn done(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
        self.ok(&["next", name, "--no-cleanup"]);
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn pin_file(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/links/pin")
    }

    fn release_file(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/pin-release")
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

    fn listed(&self) -> Vec<String> {
        let list = self.ok(&["session", "list"]);
        let mut names: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
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

fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1300));
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

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no entry for {name} in {list}"))
}

/// A, finished, dispatched B, finished. Both stay on disk.
fn a_dispatched_b(env: &Env) {
    env.done("A");
    env.done("B");
    env.ok(&["session", "link", "B", "--dispatched-by", "A"]);
}

#[test]
fn a_pinned_session_survives_prune_until_it_is_unpinned_by_hand() {
    let env = Env::new();
    a_dispatched_b(&env);
    let log_before = env.state_log("A");

    let pinned = env.ok(&["session", "pin", "A", "--reason", "demo"]);
    assert_eq!(pinned["name"], "A");
    assert_eq!(pinned["pin"]["reason"], "demo");
    assert_eq!(pinned["pin"]["set_by"], Value::Null);
    assert!(pinned["pin"]["set_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(pinned["store"], "local");
    // The pin is a standalone record: no event in A's log.
    assert_eq!(env.state_log("A"), log_before);
    let after_pin = env.hashes("A");

    // Pinning again, with another reason, prints the first pin and changes
    // nothing.
    let again = env.ok(&["session", "pin", "A", "--reason", "other"]);
    assert_eq!(again["pin"], pinned["pin"]);
    assert_eq!(env.hashes("A"), after_pin);

    settle();
    let pruned = env.ok(&["session", "prune", "--apply", "--all"]);
    let kept = entry(&pruned["kept"], "A");
    assert_eq!(kept["rule"], "p");
    assert_eq!(kept["kept_by_pin"], true);
    assert_eq!(kept["pin"]["reason"], "demo");
    assert_eq!(kept["pin"]["set_by"], Value::Null);
    assert_eq!(kept["pin"]["set_at"], pinned["pin"]["set_at"]);
    assert_eq!(kept["pin"]["source"], "local");
    assert_eq!(
        kept["detail"],
        format!(
            "pinned: demo (set by hand at {})",
            pinned["pin"]["set_at"].as_str().unwrap()
        )
    );
    // A pin keeps its own session only: B goes, and its entry says nothing
    // of pins.
    assert_eq!(names(&pruned["removed"]), vec!["B"]);
    let removed = entry(&pruned["removed"], "B");
    for field in ["kept_by_pin", "pin", "pin_unreadable"] {
        assert!(removed.get(field).is_none(), "{removed}");
    }
    assert_eq!(env.hashes("A"), after_pin);
    assert_eq!(env.listed(), vec!["A"]);

    let unpinned = env.ok(&["session", "unpin", "A"]);
    assert_eq!(unpinned["name"], "A");
    let released = &unpinned["released"];
    assert_eq!(released["released_by"], Value::Null);
    assert_eq!(released["authority"], Value::Null);
    assert_eq!(released["pin"]["reason"], "demo");
    assert_eq!(released["pin"]["set_by"], Value::Null);
    assert_eq!(released["pin"]["set_at"], pinned["pin"]["set_at"]);
    assert!(!env.pin_file("A").exists());
    assert!(env.release_file("A").is_file());
    assert_eq!(env.state_log("A"), log_before);

    // No clock restarts: the very next prune takes A.
    let pruned = env.ok(&["session", "prune", "--apply", "--all"]);
    assert_eq!(names(&pruned["removed"]), vec!["A"]);
    assert!(env.listed().is_empty());
}

#[test]
fn a_reason_must_be_short_printable_and_not_blank() {
    let env = Env::new();
    env.done("A");
    let long = "é".repeat(501);
    for bad in ["", "   ", "a\tb", "a\nb", "a\u{7}b", long.as_str()] {
        let out = env.run(&["session", "pin", "A", "--reason", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad:?}");
        let err: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(err["error"]["code"], "invalid_reason", "{bad:?}");
        assert!(!env.pin_file("A").exists(), "{bad:?} wrote a pin");
    }
    let exact = "é".repeat(499) + "x";
    let pinned = env.ok(&["session", "pin", "A", "--reason", &exact]);
    assert_eq!(pinned["pin"]["reason"], exact.as_str());
}

#[test]
fn pin_and_unpin_refuse_a_name_with_no_session() {
    let env = Env::new();
    for args in [
        &["session", "pin", "nosuch", "--reason", "r"][..],
        &["session", "unpin", "nosuch"][..],
    ] {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(err["error"]["code"], "no_session");
    }
}

#[test]
fn unpin_with_no_pin_changes_nothing() {
    let env = Env::new();
    env.done("A");
    let before = env.hashes("A");
    let out = env.ok(&["session", "unpin", "A"]);
    assert_eq!(out["released"], Value::Null);
    // Under an anchor too: with no pin there is nothing to authorize.
    let out = env.run_env(&["session", "unpin", "A"], &[("KOTO_ANCHOR", "anchor-x")]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(env.hashes("A"), before);
}

#[test]
fn pinning_changes_no_hierarchy() {
    let env = Env::new();
    a_dispatched_b(&env);
    let a_before = env.ok(&["session", "hierarchy", "A", "--json"]);
    let b_before = env.ok(&["session", "hierarchy", "B", "--json"]);

    env.ok(&["session", "pin", "A", "--reason", "demo"]);
    let a_after = env.ok(&["session", "hierarchy", "A", "--json"]);
    let b_after = env.ok(&["session", "hierarchy", "B", "--json"]);

    assert_eq!(a_after["members"], a_before["members"]);
    assert_eq!(b_after["members"], b_before["members"]);
    let edge = &b_after["members"][1]["joined_by"][0];
    assert_eq!(edge["holder"], "B");
    assert_eq!(edge["via"], "dispatched-by link");
    assert_eq!(edge["target"], "A");
    // B holds no pin, so its output is what it was, `now` aside, but for
    // the line naming A as a pinned member of its hierarchy.
    assert_eq!(b_after["pinned_members"], serde_json::json!(["A"]));
    assert!(b_before.get("pinned_members").is_none());
    let strip_now = |v: &Value| {
        let mut v = v.clone();
        v.as_object_mut().unwrap().remove("now");
        v.as_object_mut().unwrap().remove("pinned_members");
        v
    };
    assert_eq!(strip_now(&b_after), strip_now(&b_before));
    // A's names its pin and says it keeps A only.
    assert_eq!(a_after["kept_by_pin"], true);
    assert_eq!(a_after["pin"]["reason"], "demo");
    assert_eq!(a_after["pin_keeps"], serde_json::json!(["A"]));
    assert!(a_before.get("pin").is_none() && a_before.get("pin_keeps").is_none());
    let text = env.text(&["session", "hierarchy", "A"]);
    assert!(text.contains("pinned: demo (set by hand at "), "{text}");
    assert!(text.contains("the pin keeps A only"), "{text}");
}

#[test]
fn context_add_and_remove_refuse_the_pin() {
    let env = Env::new();
    env.done("A");
    env.ok(&["session", "pin", "A", "--reason", "demo"]);
    let before = env.hashes("A");
    let file = env.root.join("forged");
    std::fs::write(&file, "{}").unwrap();
    for args in [
        &[
            "context",
            "add",
            "A",
            "koto/links/pin",
            "--from-file",
            file.to_str().unwrap(),
        ][..],
        &["context", "remove", "A", "koto/links/pin"][..],
        &[
            "context",
            "add",
            "A",
            "koto/pin-release",
            "--from-file",
            file.to_str().unwrap(),
        ][..],
    ] {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    assert_eq!(env.hashes("A"), before);
}

#[test]
fn unpin_under_an_anchor_needs_release_pin() {
    let env = Env::new();
    env.done("A");
    env.ok(&["session", "pin", "A", "--reason", "demo"]);
    let before = env.hashes("A");
    let out = env.run_env(&["session", "unpin", "A"], &[("KOTO_ANCHOR", "anchor-x")]);
    assert_eq!(out.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["error"]["code"], "authority_required");
    assert_eq!(err["error"]["authority"], "release-pin");
    assert_eq!(err["error"]["anchor"], "anchor-x");
    let message = err["error"]["message"].as_str().unwrap();
    assert!(message.contains("release-pin") && message.contains("start"));
    assert!(!env.release_file("A").exists());
    assert!(env.pin_file("A").is_file());
    assert_eq!(env.hashes("A"), before);
    // An empty KOTO_ANCHOR is a call by hand.
    let out = env.run_env(&["session", "unpin", "A"], &[("KOTO_ANCHOR", "")]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn an_anchor_cannot_be_pinned() {
    let env = Env::new();
    env.ok(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        "h1",
        "--source",
        "startup",
    ]);
    let out = env.run(&["session", "pin", "anchor-h1", "--reason", "demo"]);
    assert_eq!(out.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["error"]["code"], "not_pinnable");
    assert!(!env.pin_file("anchor-h1").exists());
}

#[test]
fn a_pin_set_since_the_preview_keeps_its_session() {
    let env = Env::new();
    env.done("A");
    env.done("C");
    settle();
    let preview = env.ok(&["session", "prune"]);
    assert_eq!(names(&preview["remove"]), vec!["A", "C"]);
    let token = preview["plan"].as_str().unwrap().to_string();

    env.ok(&["session", "pin", "A", "--reason", "late"]);
    let applied = env.ok(&["session", "prune", "--apply", "--plan", &token]);
    assert_eq!(names(&applied["removed"]), vec!["C"]);
    let skipped = entry(&applied["skipped"], "A");
    assert_eq!(skipped["kept_by_pin"], true);
    assert_eq!(skipped["pin"]["reason"], "late");
    assert_eq!(env.listed(), vec!["A"]);
}

#[test]
fn unpinned_entries_carry_no_pin_fields() {
    let env = Env::new();
    env.done("U");
    let preview = env.ok(&["session", "prune"]);
    let kept = entry(&preview["kept"], "U");
    let mut fields: Vec<&str> = kept
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(fields, vec!["detail", "hierarchy", "name", "rule"]);
    let hierarchy = env.ok(&["session", "hierarchy", "U", "--json"]);
    let mut fields: Vec<&str> = hierarchy
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        vec![
            "clamped",
            "keepers",
            "kept",
            "members",
            "now",
            "retention",
            "session",
            "settings",
            "unreadable"
        ]
    );
}

#[test]
fn a_pinned_session_holds_no_problem_and_no_link() {
    // The prune reads the pin as a fact of A, never as one of A's problems:
    // an unreadable record would keep B too.
    let env = Env::new();
    a_dispatched_b(&env);
    env.ok(&["session", "pin", "A", "--reason", "demo"]);
    let h = env.ok(&["session", "hierarchy", "B", "--json"]);
    assert_eq!(h["unreadable"], serde_json::json!([]));
    let path: &Path = &env.pin_file("A");
    assert!(path.is_file());
}
