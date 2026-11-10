//! Integration tests for the decision record under the cloud backend,
//! against the in-process S3 endpoint in `support/fake_s3.rs`.
//!
//! A decision is its own bucket object, written create-only, outside the
//! session's manifest. A host with no local copy, and a `--workspace` read,
//! find decisions by listing the decision areas.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fake_s3::FakeS3;
use sha2::{Digest, Sha256};

const BUCKET: &str = "koto-decisions";
const NAME: &str = "w";
const TEMPLATE_FILE: &str = "work.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

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

fn koto_dir() -> PathBuf {
    koto_command::koto_bin().parent().unwrap().to_path_buf()
}

/// The remote prefix koto derives for a workspace.
fn prefix_of(dir: &Path) -> String {
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// The bucket key of decision `id` in `ws`'s copy of the session.
fn decision_key(ws: &Path, id: &str) -> String {
    format!("{}/{NAME}/ctx/koto/decisions/{id}", prefix_of(ws))
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

/// One simulated host: a disposable `HOME` and template cache.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: String,
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        Host {
            _root: root,
            home,
            cache,
            endpoint: s3.endpoint(),
        }
    }

    fn run(&self, ws: &Path, args: &[&str]) -> Out {
        let mut cmd: Command = koto_command::koto();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", format!("{}:{}", koto_dir().display(), SYSTEM_PATH))
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

    fn ok(&self, ws: &Path, args: &[&str]) -> serde_json::Value {
        let out = self.run(ws, args);
        assert_eq!(
            out.code,
            Some(0),
            "expected success from {args:?}\n{}\n{}",
            out.stdout,
            out.stderr
        );
        out.json()
    }

    fn record_args(what: &str) -> Vec<String> {
        [
            "decisions",
            "record",
            NAME,
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            what,
            "--by",
            "person:dana",
            "--surface",
            "terminal",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn record(&self, ws: &Path, what: &str) -> Out {
        let args = Self::record_args(what);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(ws, &args)
    }

    fn decision_path(&self, id: &str) -> PathBuf {
        self.home
            .join(".koto/sessions")
            .join(NAME)
            .join("ctx/koto/decisions")
            .join(id)
    }

    fn local_decisions(&self) -> Vec<String> {
        let dir = self.decision_path("x");
        let dir = dir.parent().unwrap();
        let mut names: Vec<String> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }
}

fn workspace(root: &Path) -> PathBuf {
    let ws = mk(root.join("ws"));
    std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
    ws
}

fn ids(listing: &serde_json::Value) -> Vec<String> {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_decision_is_its_own_create_only_object_outside_the_manifest() {
    let s3 = FakeS3::start(BUCKET);
    let shared = tempfile::TempDir::new().unwrap();
    let ws = workspace(shared.path());
    let host = Host::new(&s3);
    host.ok(&ws, &["init", NAME, "--template", TEMPLATE_FILE]);

    s3.clear_requests();
    let out = host.record(&ws, "what=x");
    assert_eq!(out.code, Some(0), "{}\n{}", out.stdout, out.stderr);
    let key = decision_key(&ws, "000000000001");
    let stored = s3.object(&key).expect("the decision is its own object");
    let local = std::fs::read(
        host.home
            .join(".koto/sessions")
            .join(NAME)
            .join("ctx/koto/decisions/000000000001"),
    )
    .unwrap();
    assert_eq!(stored, local);
    let put = s3
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT" && r.key == key)
        .expect("a PUT of the decision");
    assert_eq!(put.if_none_match.as_deref(), Some("*"));

    // The decision is outside the manifest.
    for manifest in s3
        .keys_under(&prefix_of(&ws))
        .into_iter()
        .filter(|k| k.ends_with("manifest.json"))
    {
        let text = String::from_utf8(s3.object(&manifest).unwrap()).unwrap();
        assert!(!text.contains("koto/decisions"), "{manifest}: {text}");
    }

    // Another writer's object lands at the next id between the listing
    // that allocated it and the put: the put gets 412 and the record is
    // refused, leaving the other object as it was.
    let next = decision_key(&ws, "000000000002");
    let area = format!("{}/{NAME}/ctx/koto/decisions/", prefix_of(&ws));
    s3.put_after("LIST", &area, &next, b"{\"another\":\"writer\"}");
    s3.clear_requests();
    let out = host.record(&ws, "what=y");
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["code"], "decision_exists", "{}", out.stdout);
    assert!(
        s3.requests()
            .iter()
            .any(|r| r.method == "PUT" && r.key == next && r.if_none_match.as_deref() == Some("*")),
        "no conditional PUT of {next}"
    );
    assert_eq!(s3.object(&next).unwrap(), b"{\"another\":\"writer\"}");
    assert_eq!(host.local_decisions(), ["000000000001"]);
}

#[test]
fn a_bucket_that_fails_the_create_only_probe_refuses_decisions() {
    let s3 = FakeS3::start(BUCKET);
    s3.ignore_if_none_match();
    let shared = tempfile::TempDir::new().unwrap();
    let ws = workspace(shared.path());
    let host = Host::new(&s3);
    host.ok(&ws, &["init", NAME, "--template", TEMPLATE_FILE]);

    let out = host.record(&ws, "what=x");
    assert_eq!(out.code, Some(2), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(
        out.json()["code"],
        "decision_bucket_unsupported",
        "{}",
        out.stdout
    );
    assert!(s3.object(&decision_key(&ws, "000000000001")).is_none());
    assert!(host.local_decisions().is_empty());
    assert!(
        s3.keys_under(&prefix_of(&ws))
            .iter()
            .all(|k| !k.contains(".koto-probe")),
        "the probe object was left behind"
    );
    // Refused again on the next try: a failed probe is never remembered.
    let out = host.record(&ws, "what=x");
    assert_eq!(out.json()["code"], "decision_bucket_unsupported");
}

#[test]
fn a_host_with_no_local_copy_and_a_workspace_read_list_the_decisions() {
    let s3 = FakeS3::start(BUCKET);
    let shared = tempfile::TempDir::new().unwrap();
    let ws = workspace(shared.path());
    let first = Host::new(&s3);
    first.ok(&ws, &["init", NAME, "--template", TEMPLATE_FILE]);
    for what in ["what=a", "what=b"] {
        let out = first.record(&ws, what);
        assert_eq!(out.code, Some(0), "{}\n{}", out.stdout, out.stderr);
    }

    // A second host with no local copy reads them from the bucket.
    let second = Host::new(&s3);
    assert!(second.local_decisions().is_empty());
    let listing = second.ok(&ws, &["decisions", "list", NAME, "--format", "v1"]);
    assert_eq!(ids(&listing), ["000000000001", "000000000002"], "{listing}");
    assert!(listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["status"] == "in-effect"));
    let shown = second.ok(&ws, &["decisions", "show", NAME, "000000000002"]);
    assert_eq!(shown["decision"]["subject"]["what"], "b");

    // A pull onto the second host brings the decision objects down,
    // though the manifest doesn't name them, and its next record takes the
    // next id.
    let pulled = second.run(&ws, &["session", "resolve", NAME, "--keep", "remote"]);
    assert_eq!(pulled.code, Some(0), "{}\n{}", pulled.stdout, pulled.stderr);
    assert_eq!(second.local_decisions(), ["000000000001", "000000000002"]);
    for id in ["000000000001", "000000000002"] {
        assert_eq!(
            std::fs::read(second.decision_path(id)).unwrap(),
            s3.object(&decision_key(&ws, id)).unwrap()
        );
    }
    let listing = second.ok(&ws, &["decisions", "list", NAME, "--format", "v1"]);
    assert_eq!(ids(&listing), ["000000000001", "000000000002"], "{listing}");
    let out = second.record(&ws, "what=c");
    assert_eq!(out.code, Some(0), "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["id"], "000000000003");
    let listing = first.ok(&ws, &["decisions", "list", NAME, "--format", "v1"]);
    assert_eq!(
        ids(&listing),
        ["000000000001", "000000000002", "000000000003"],
        "{listing}"
    );

    // A pull never overwrites a decision file this host already holds.
    let held = std::fs::read(first.decision_path("000000000001")).unwrap();
    s3.put(&decision_key(&ws, "000000000001"), b"{\"changed\":true}");
    let pulled = first.run(&ws, &["session", "resolve", NAME, "--keep", "remote"]);
    assert_eq!(pulled.code, Some(0), "{}\n{}", pulled.stdout, pulled.stderr);
    assert_eq!(
        std::fs::read(first.decision_path("000000000001")).unwrap(),
        held
    );
    assert_eq!(first.local_decisions().len(), 3);
    s3.put(&decision_key(&ws, "000000000001"), &held);

    // A --workspace read from another workspace lists and shows them,
    // without importing the session.
    let elsewhere = workspace(&mk(shared.path().join("other")));
    let target = prefix_of(&ws);
    let reader = Host::new(&s3);
    let listing = reader.ok(
        &elsewhere,
        &[
            "--workspace",
            &target,
            "decisions",
            "list",
            NAME,
            "--format",
            "v1",
        ],
    );
    assert_eq!(
        ids(&listing),
        ["000000000001", "000000000002", "000000000003"],
        "{listing}"
    );
    let shown = reader.ok(
        &elsewhere,
        &[
            "--workspace",
            &target,
            "decisions",
            "show",
            NAME,
            "000000000001",
        ],
    );
    assert_eq!(shown["decision"]["subject"]["what"], "a");
    assert!(reader.local_decisions().is_empty());
    let refused = reader.run(
        &elsewhere,
        &[
            "--workspace",
            &target,
            "decisions",
            "record",
            NAME,
            "--kind",
            "judgment",
        ],
    );
    assert_ne!(refused.code, Some(0));
}
