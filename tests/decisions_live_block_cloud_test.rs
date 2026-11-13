//! The reversal read at `koto next` under the cloud backend (PRD R22,
//! R23), against the in-process S3 endpoint in `support/fake_s3.rs`: a
//! chain session whose bucket listing fails blocks naming it, a chain
//! session moved by `koto session import` is read where it moved, and a
//! person's own reversal and replacement through the deciding session
//! block and clear as they do locally.
//!
//! One host: one `HOME`, so every workspace shares the local store, and
//! two workspaces for the import.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};

const BUCKET: &str = "koto-live-block";
const TEMPLATE_FILE: &str = "work.md";

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// Removed from every command's environment before a test sets its own.
const CLEARED: &[&str] = &[
    "KOTO_SESSIONS_BASE",
    "KOTO_HOME",
    "KOTO_DECIDER",
    "KOTO_DECIDER_API_KEY",
    "KOTO_DECIDER_ENDPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_WORKFLOWS_DIR",
    "KOTO_ANCHOR",
    "KOTO_DISPATCHER_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
];

const REVERSED: &str = "__decision_reversed__";
const UNREADABLE: &str = "__decision_unreadable__";

struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    /// The workspace every session starts in.
    a: PathBuf,
    /// A second workspace, for an import.
    b: PathBuf,
    endpoint: String,
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// The remote prefix koto derives for a workspace.
fn prefix_of(dir: &Path) -> String {
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        let a = mk(root.path().join("instance-a"));
        let b = mk(root.path().join("instance-b"));
        for ws in [&a, &b] {
            std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        }
        Host {
            _root: root,
            home,
            cache,
            a,
            b,
            endpoint: s3.endpoint(),
        }
    }

    fn run(&self, ws: &Path, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for k in CLEARED {
            cmd.env_remove(k);
        }
        cmd.current_dir(ws)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .args(args);
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, ws: &Path, args: &[&str]) -> Value {
        let out = self.run(ws, args);
        assert_eq!(
            out.code,
            Some(0),
            "expected success from {args:?}\n{}\n{}",
            out.stdout,
            out.stderr
        );
        serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null)
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", TEMPLATE_FILE];
        args.extend_from_slice(extra);
        self.ok(&self.a, &args);
    }

    /// `a` dispatches `b` and `b` dispatches `c`.
    fn three_levels(&self) {
        self.init("a", &[]);
        self.init("b", &["--dispatched-by", "a"]);
        self.init("c", &["--dispatched-by", "b"]);
    }

    fn judgment(&self, ws: &Path, name: &str, extra: &[&str]) -> (String, String) {
        let mut args = vec![
            "decisions",
            "record",
            name,
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=x",
        ];
        args.extend_from_slice(extra);
        let out = self.ok(ws, &args);
        (
            out["id"].as_str().unwrap().to_string(),
            out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }

    fn reverse(&self, ws: &Path, session: &str, id: &str, by_in: &str, extra: &[&str]) -> String {
        let mut args = vec!["decisions", "reverse", session, id, "--in", by_in];
        args.extend_from_slice(extra);
        self.ok(ws, &args)["decision"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn next(&self, name: &str) -> Value {
        let out = self.run(&self.a, &["next", name]);
        out.json()
    }
}

fn condition<'a>(resp: &'a Value, name: &str) -> Option<&'a Value> {
    resp["blocking_conditions"]
        .as_array()?
        .iter()
        .find(|c| c["name"] == name)
}

fn reversed_ids(resp: &Value) -> Vec<String> {
    condition(resp, REVERSED)
        .map(|c| {
            c["output"]["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn unreadable_sessions(resp: &Value) -> Vec<String> {
    condition(resp, UNREADABLE)
        .map(|c| {
            c["output"]["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["session"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_permission_error_listing_a_chain_session_blocks_naming_it() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.three_levels();
    h.init("other", &[]);
    h.judgment(&h.a, "c", &["--by", "session"]);
    let resp = h.next("c");
    assert!(condition(&resp, UNREADABLE).is_none(), "{resp}");

    // A session off the chain that can't be listed blocks nothing.
    let prefix = prefix_of(&h.a);
    s3.fail("LIST", &format!("{prefix}/other/ctx/koto/aimed/"), 403);
    let resp = h.next("c");
    assert!(condition(&resp, UNREADABLE).is_none(), "{resp}");

    s3.fail("LIST", &format!("{prefix}/b/ctx/koto/aimed/"), 403);
    let resp = h.next("c");
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(unreadable_sessions(&resp), ["b"], "{resp}");
}

#[test]
fn a_reversal_from_above_blocks_under_the_cloud_backend() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.three_levels();
    let (id, _) = h.judgment(&h.a, "c", &["--by", "session"]);
    h.reverse(&h.a, "c", &id, "a", &["--by", "session"]);
    let resp = h.next("c");
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(reversed_ids(&resp), [id.as_str()], "{resp}");
}

#[test]
fn a_person_reversal_through_the_worker_clears_only_with_a_person_replacement_in_the_cloud() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.three_levels();
    let person = ["--by", "person:dana", "--surface", "terminal"];
    let (id, session_id) = h.judgment(&h.a, "c", &person);
    let rid = h.reverse(&h.a, "c", &id, "c", &person);
    assert_eq!(reversed_ids(&h.next("c")), [id.as_str()]);

    let basis = format!("c:{rid}");
    let mut by_session = vec!["--basis", basis.as_str(), "--by", "session"];
    h.judgment(&h.a, "c", &by_session);
    assert_eq!(reversed_ids(&h.next("c")), [id.as_str()]);

    by_session.truncate(2);
    by_session.extend_from_slice(&person);
    let (replacement, _) = h.judgment(&h.a, "c", &by_session);
    let key = format!(
        "{}/c/ctx/koto/aimed/{session_id}/{replacement}",
        prefix_of(&h.a)
    );
    assert!(
        s3.object(&key).is_some(),
        "the replacement is in the bucket"
    );
    let resp = h.next("c");
    assert!(condition(&resp, REVERSED).is_none(), "{resp}");
}

#[test]
fn a_chain_session_migrated_by_import_is_read_where_it_moved() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.three_levels();
    let (id, _) = h.judgment(&h.a, "c", &["--by", "session"]);

    let out = h.run(
        &h.b,
        &["session", "import", "b", "--from", h.a.to_str().unwrap()],
    );
    assert_eq!(out.code, Some(0), "import: {}\n{}", out.stdout, out.stderr);
    // The source copy in the first workspace now refuses as migrated.
    let refused = h.run(&h.a, &["next", "b"]);
    assert!(
        format!("{}{}", refused.stdout, refused.stderr).contains("session_migrated"),
        "{}\n{}",
        refused.stdout,
        refused.stderr
    );

    // The session it moved to reverses c's decision; c's next tick reads it
    // there.
    h.reverse(&h.b, "c", &id, "b", &["--by", "session"]);
    let resp = h.next("c");
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(reversed_ids(&resp), [id.as_str()], "{resp}");
    assert!(condition(&resp, UNREADABLE).is_none(), "{resp}");
}
