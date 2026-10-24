//! Explicit removal against the retention rule: `koto session cleanup` and
//! `koto cancel --cleanup` still remove the named session, and name every
//! other live member of its hierarchy under `live_members`, with a warning
//! on stderr. Nothing they don't remove is written to.
//!
//! Every command runs with a temporary HOME and sessions directory and the
//! cloud backend variables removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::PathBuf;

use assert_fs::TempDir;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TEMPLATE: &str = r#"---
name: removal
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
        when:
          status: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Ws {
    tmp: TempDir,
}

struct Out {
    code: i32,
    json: Value,
    stderr: String,
}

impl Ws {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        for d in ["proj", "home", "sessions"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("proj/removal.md"), TEMPLATE).unwrap();
        Ws { tmp }
    }

    fn sessions(&self) -> PathBuf {
        self.tmp.path().join("sessions")
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = koto_command::koto();
        for var in [
            "CLAUDE_CODE_SESSION_ID",
            "KOTO_SESSION_BACKEND",
            "KOTO_SESSION_CLOUD_ENDPOINT",
            "KOTO_SESSION_CLOUD_BUCKET",
            "KOTO_ANCHOR",
            "KOTO_DISPATCHER_ANCHOR",
            "KOTO_RETENTION_KEEP",
            "KOTO_RETENTION_IDLE_PERIOD",
            "KOTO_RETENTION_STALE_AFTER",
            "KOTO_WORKFLOWS_DIR",
        ] {
            cmd.env_remove(var);
        }
        cmd.current_dir(self.tmp.path().join("proj"));
        cmd.env("HOME", self.tmp.path().join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            json: serde_json::from_slice(&output.stdout).unwrap_or(Value::Null),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn ok(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let out = self.run(args, env);
        assert_eq!(out.code, 0, "{args:?}: {}\n{}", out.json, out.stderr);
        out
    }

    fn init(&self, name: &str, extra: &[&str], env: &[(&str, &str)]) {
        let template = self.tmp.path().join("proj/removal.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args, env);
    }

    fn finish(&self, name: &str) {
        self.ok(
            &[
                "next",
                name,
                "--with-data",
                r#"{"status":"done"}"#,
                "--no-cleanup",
            ],
            &[],
        );
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).exists()
    }

    /// SHA-256 of every file in each named session's directory.
    fn snapshot(&self, names: &[&str]) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        for name in names {
            let mut stack = vec![self.sessions().join(name)];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).unwrap().flatten() {
                    let p = e.path();
                    let meta = std::fs::symlink_metadata(&p).unwrap();
                    if meta.is_dir() {
                        stack.push(p);
                    } else if meta.is_file() {
                        let digest = Sha256::digest(std::fs::read(&p).unwrap());
                        out.insert(p, format!("{digest:x}"));
                    }
                }
            }
        }
        out
    }
}

#[test]
fn cleanup_while_the_dispatcher_lives_removes_and_names_it() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    ws.init("b", &["--dispatched-by", "a"], &[]);
    ws.finish("b");

    let before = ws.snapshot(&["a"]);
    let out = ws.ok(&["session", "cleanup", "b"], &[]);
    assert!(!ws.exists("b"));
    assert_eq!(out.json["cleaned_up"], true);
    assert_eq!(
        out.json["live_members"],
        json!([{"name": "a", "kind": "koto_session", "observed": false}])
    );
    assert!(
        out.stderr
            .contains("removed 'b', which the retention rule would have kept")
            && out.stderr.contains(": a"),
        "{}",
        out.stderr
    );
    assert_eq!(ws.snapshot(&["a"]), before);
}

#[test]
fn cancel_cleanup_of_a_live_session_names_its_live_dispatcher() {
    let ws = Ws::new();
    ws.init("d", &[], &[]);
    ws.init("c", &["--dispatched-by", "d"], &[]);

    let before = ws.snapshot(&["d"]);
    let out = ws.ok(&["cancel", "c", "--cleanup"], &[]);
    assert!(!ws.exists("c"));
    assert_eq!(out.json["cancelled"], true);
    assert_eq!(out.json["cleaned_up"], true);
    assert_eq!(
        out.json["live_members"],
        json!([{"name": "d", "kind": "koto_session", "observed": false}])
    );
    assert!(out.stderr.contains("removed 'c'"), "{}", out.stderr);
    assert_eq!(ws.snapshot(&["d"]), before);
}

#[test]
fn a_live_anchor_is_named_with_its_kind_and_whether_it_is_observed() {
    let ws = Ws::new();
    ws.ok(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "S1",
            "--source",
            "startup",
            "--observed-key",
            "K1",
        ],
        &[],
    );
    ws.init("b", &[], &[("KOTO_ANCHOR", "anchor-S1")]);
    ws.finish("b");

    let before = ws.snapshot(&["anchor-S1"]);
    let out = ws.ok(&["session", "cleanup", "b"], &[]);
    assert!(!ws.exists("b"));
    assert_eq!(
        out.json["live_members"],
        json!([{"name": "anchor-S1", "kind": "anchor", "observed": true}])
    );
    assert_eq!(ws.snapshot(&["anchor-S1"]), before);
}

#[test]
fn a_session_nothing_keeps_names_no_live_member_and_never_itself() {
    let ws = Ws::new();
    // Alone and running: the only live member is the session itself.
    ws.init("alone", &[], &[]);
    let out = ws.ok(&["session", "cleanup", "alone"], &[]);
    assert!(!ws.exists("alone"));
    assert_eq!(out.json["live_members"], json!([]));
    assert!(!out.stderr.contains("warning"), "{}", out.stderr);

    // A finished hierarchy inside its retention: kept, but nothing in it
    // is alive, so nothing is named.
    ws.init("p", &[], &[]);
    ws.init("q", &["--dispatched-by", "p"], &[]);
    ws.finish("q");
    ws.finish("p");
    let before = ws.snapshot(&["p"]);
    let out = ws.ok(&["session", "cleanup", "q"], &[]);
    assert_eq!(out.json["live_members"], json!([]));
    assert_eq!(ws.snapshot(&["p"]), before);

    // A cancel of a session nothing keeps.
    ws.init("lone", &[], &[]);
    let out = ws.ok(&["cancel", "lone", "--cleanup"], &[]);
    assert_eq!(out.json["live_members"], json!([]));

    // A name with no session: still idempotent.
    let out = ws.ok(&["session", "cleanup", "never-was"], &[]);
    assert_eq!(out.json["live_members"], json!([]));
}

#[test]
fn a_cancel_without_cleanup_carries_no_live_members() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    ws.init("b", &["--dispatched-by", "a"], &[]);
    let out = ws.ok(&["cancel", "b"], &[]);
    assert!(ws.exists("b"));
    assert!(out.json.get("live_members").is_none(), "{}", out.json);
}
