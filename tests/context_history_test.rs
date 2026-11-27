//! `koto context history <session> <key>`: one key's write history, read
//! from the session's log, on the local backend and on the cloud backend
//! against the in-process S3 endpoint in `support/fake_s3.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};

const BUCKET: &str = "koto-history";
const NAME: &str = "w";
const TEMPLATE_FILE: &str = "work.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: first
states:
  first:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: second
        when:
          go: go
  second:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
        context_assignments:
          outcome: finished
  done:
    terminal: true
---

## first

First.

## second

Second.

## done

Done.
"#;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
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

/// One simulated host: a disposable `HOME` and template cache, on the local
/// backend or on the cloud backend at `endpoint`.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: Option<String>,
}

impl Host {
    fn new(endpoint: Option<String>) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        Host {
            _root: root,
            home,
            cache,
            endpoint,
        }
    }

    fn run(&self, ws: &Path, args: &[&str]) -> Out {
        let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", format!("{}:{}", koto_dir.display(), SYSTEM_PATH))
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache);
        match &self.endpoint {
            Some(endpoint) => {
                cmd.env("KOTO_SESSION_BACKEND", "cloud")
                    .env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint)
                    .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                    .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                    .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                    .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                    .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
            }
            None => {
                cmd.env("KOTO_SESSION_BACKEND", "local");
            }
        }
        let o = cmd.args(args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, ws: &Path, args: &[&str]) -> Out {
        let out = self.run(ws, args);
        assert_eq!(
            out.code,
            Some(0),
            "expected success from {args:?}\n{}\n{}",
            out.stdout,
            out.stderr
        );
        out
    }

    fn add(&self, ws: &Path, key: &str, content: &str) {
        let file = ws.join("content.txt");
        std::fs::write(&file, content).unwrap();
        self.ok(
            ws,
            &[
                "context",
                "add",
                NAME,
                key,
                "--from-file",
                file.to_str().unwrap(),
            ],
        );
    }

    fn history(&self, ws: &Path, key: &str) -> Value {
        self.ok(ws, &["context", "history", NAME, key]).json()
    }

    /// Init the session, write `k` once in `first` and once in `second`,
    /// then finish, which assigns `outcome` on the way into `done`.
    fn drive(&self, ws: &Path) {
        self.ok(ws, &["init", NAME, "--template", TEMPLATE_FILE]);
        self.add(ws, "k", "one");
        self.ok(
            ws,
            &[
                "next",
                NAME,
                "--with-data",
                r#"{"go":"go"}"#,
                "--no-cleanup",
            ],
        );
        self.add(ws, "k", "two");
        self.ok(
            ws,
            &[
                "next",
                NAME,
                "--with-data",
                r#"{"go":"go"}"#,
                "--no-cleanup",
            ],
        );
    }
}

fn workspace(root: &Path) -> PathBuf {
    let ws = mk(root.join("ws"));
    std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
    ws
}

/// What a record says about the write, without when it was logged: the
/// writer, the hash, the size and the state.
fn writes(history: &Value) -> Vec<(Value, String, u64, String)> {
    history
        .as_array()
        .unwrap_or_else(|| panic!("history is not an array: {history}"))
        .iter()
        .map(|w| {
            (
                w["writer"].clone(),
                w["hash"].as_str().unwrap().to_string(),
                w["size"].as_u64().unwrap(),
                w["state"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn expected_k() -> Vec<(Value, String, u64, String)> {
    vec![
        ("agent".into(), sha256(b"one"), 3, "first".to_string()),
        ("agent".into(), sha256(b"two"), 3, "second".to_string()),
    ]
}

fn expected_outcome() -> Vec<(Value, String, u64, String)> {
    vec![(
        "transition".into(),
        sha256(b"finished"),
        8,
        "done".to_string(),
    )]
}

/// Every record carries the fields the command documents, oldest first.
fn assert_shape(history: &Value) {
    let records = history.as_array().unwrap();
    let mut last_seq = 0;
    for r in records {
        let fields: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            fields,
            vec!["hash", "seq", "size", "state", "timestamp", "writer"],
            "{r}"
        );
        assert!(r["timestamp"].as_str().unwrap().ends_with('Z'), "{r}");
        let seq = r["seq"].as_u64().unwrap();
        assert!(seq > last_seq, "records are oldest first: {history}");
        last_seq = seq;
    }
}

#[test]
fn local_a_key_written_in_two_states_has_two_records_with_their_states_and_hashes() {
    let root = tempfile::TempDir::new().unwrap();
    let ws = workspace(root.path());
    let host = Host::new(None);
    host.drive(&ws);

    let k = host.history(&ws, "k");
    assert_shape(&k);
    assert_eq!(writes(&k), expected_k());

    let outcome = host.history(&ws, "outcome");
    assert_shape(&outcome);
    assert_eq!(writes(&outcome), expected_outcome());
}

#[test]
fn local_a_key_never_written_is_an_empty_list_and_a_missing_session_is_exit_2() {
    let root = tempfile::TempDir::new().unwrap();
    let ws = workspace(root.path());
    let host = Host::new(None);
    host.ok(&ws, &["init", NAME, "--template", TEMPLATE_FILE]);

    // A key the session never wrote: `[]`, exit 0.
    let out = host.ok(&ws, &["context", "history", NAME, "never.md"]);
    assert_eq!(out.stdout.trim(), "[]");

    // A session that doesn't exist: exit 2, naming the session, not the store.
    let out = host.run(&ws, &["context", "history", "nosuch", "k"]);
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    let err = out.json();
    assert_eq!(err["command"], "context history");
    assert_eq!(err["error"], "workflow 'nosuch' not found");

    // A key that isn't a usable key: exit 2.
    let out = host.run(&ws, &["context", "history", NAME, "../escape"]);
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["command"], "context history");
}

#[test]
fn local_history_writes_nothing_into_the_session() {
    let root = tempfile::TempDir::new().unwrap();
    let ws = workspace(root.path());
    let host = Host::new(None);
    host.drive(&ws);
    let log = host
        .home
        .join(".koto/sessions")
        .join(NAME)
        .join(format!("koto-{NAME}.state.jsonl"));
    let before = std::fs::read(&log).unwrap();
    host.history(&ws, "k");
    host.history(&ws, "never.md");
    assert_eq!(std::fs::read(&log).unwrap(), before);
}

#[test]
fn cloud_history_matches_local_and_reads_the_same_on_another_host() {
    // The local run, for the records the cloud run must match.
    let local_root = tempfile::TempDir::new().unwrap();
    let local_ws = workspace(local_root.path());
    let local = Host::new(None);
    local.drive(&local_ws);
    let local_k = local.history(&local_ws, "k");
    let local_outcome = local.history(&local_ws, "outcome");

    let s3 = FakeS3::start(BUCKET);
    let shared = tempfile::TempDir::new().unwrap();
    let ws = workspace(shared.path());
    let a = Host::new(Some(s3.endpoint()));
    a.drive(&ws);

    let a_k = a.history(&ws, "k");
    assert_shape(&a_k);
    assert_eq!(writes(&a_k), expected_k());
    assert_eq!(writes(&a_k), writes(&local_k));
    let a_outcome = a.history(&ws, "outcome");
    assert_eq!(writes(&a_outcome), expected_outcome());
    assert_eq!(writes(&a_outcome), writes(&local_outcome));

    // A second host with no local copy is told how to take one, exit 2,
    // rather than failing on the store; once it has pulled the synced log
    // from the bucket it gets the very same records, timestamps and seqs
    // included.
    let b = Host::new(Some(s3.endpoint()));
    let out = b.run(&ws, &["context", "history", NAME, "k"]);
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["code"], "no_local_copy");
    b.ok(&ws, &["session", "resolve", NAME, "--keep", "remote"]);
    assert_eq!(b.history(&ws, "k"), a_k);
    assert_eq!(b.history(&ws, "outcome"), a_outcome);

    // A key never written is `[]` on the cloud backend too, and a session
    // the bucket doesn't hold is exit 2 naming the session.
    assert_eq!(
        b.ok(&ws, &["context", "history", NAME, "never.md"])
            .stdout
            .trim(),
        "[]"
    );
    let out = b.run(&ws, &["context", "history", "nosuch", "k"]);
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["error"], "workflow 'nosuch' not found");
}
