//! A read never writes into the session it reads (DESIGN-koto-trusted-removal.md,
//! Decision 5).
//!
//! Outside a session's own tick, `koto status`, `koto context get`,
//! `koto context exists` and `koto context list` leave the session's state
//! log, its context key files and, under the cloud backend, its bucket
//! objects byte-identical, whatever `KOTO_ANCHOR` the reader runs with: unset,
//! the anchor that holds the session, or the session's own name. A reader
//! whose `KOTO_ANCHOR` names another existing anchor has its reads logged in
//! that anchor. Only a read inside the session's own tick (a gate script)
//! logs in the session.
//!
//! Every test drives the koto binary in a host of its own (`HOME`, cache and
//! workspace under a temporary directory, the environment cleared); the cloud
//! cases use the in-process endpoint in `tests/support/fake_s3.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-read-purity";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// The anchor of the agent session that starts every session here.
const OWN_ANCHOR: &str = "anchor-own";
/// Another agent session's anchor, which holds no session here.
const OTHER_ANCHOR: &str = "anchor-other";
/// A session waiting for evidence, and one kept at its terminal.
const RUNNING: &str = "running";
const FINISHED: &str = "finished";

/// `wait` takes `go` into the terminal `done`.
const TEMPLATE: &str = r#"---
name: purity
version: "1.0"
initial_state: wait
states:
  wait:
    accepts:
      go:
        type: enum
        values: [yes]
        required: true
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

/// One simulated host: its own `HOME`, template cache and workspace, and the
/// backend it runs koto with (`None` for the local one).
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
    endpoint: Option<String>,
}

impl Host {
    fn new(s3: Option<&FakeS3>) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Host {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            ws: mk(root.path().join("ws")),
            _root: root,
            endpoint: s3.map(FakeS3::endpoint),
        }
    }

    fn cmd(&self, args: &[&str], anchor: Option<&str>) -> Command {
        let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(&self.ws)
            .env("PATH", format!("{}:{}", koto_dir.display(), SYSTEM_PATH))
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .args(args);
        if let Some(anchor) = anchor {
            cmd.env("KOTO_ANCHOR", anchor);
        }
        match &self.endpoint {
            Some(endpoint) => {
                cmd.env("KOTO_SESSION_BACKEND", "cloud")
                    .env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint)
                    .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                    .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                    .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                    .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                    .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
                    .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0");
            }
            None => {
                cmd.env("KOTO_SESSION_BACKEND", "local");
            }
        }
        cmd
    }

    fn run(&self, args: &[&str], anchor: Option<&str>) -> std::process::Output {
        self.cmd(args, anchor).output().unwrap()
    }

    fn ok(&self, args: &[&str], anchor: Option<&str>) -> std::process::Output {
        let out = self.run(args, anchor);
        assert!(
            out.status.success(),
            "koto {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.home.join(".koto").join("sessions").join(name)
    }

    /// The remote prefix koto derives for this host's workspace.
    fn prefix(&self) -> String {
        hex::encode(Sha256::digest(self.ws.to_string_lossy().as_bytes()))[..16].to_string()
    }

    /// Open both anchors, then start [`RUNNING`] and [`FINISHED`] under
    /// [`OWN_ANCHOR`], so each holds `koto/links/held-by/anchor-own`, each
    /// with key `k`; [`FINISHED`] is taken to its terminal and kept.
    fn sessions(&self) {
        for harness in ["own", "other"] {
            self.ok(
                &[
                    "session",
                    "anchor",
                    "open",
                    "--harness-session",
                    harness,
                    "--source",
                    "startup",
                ],
                None,
            );
        }
        std::fs::write(self.ws.join("purity.md"), TEMPLATE).unwrap();
        for name in [RUNNING, FINISHED] {
            let own = Some(OWN_ANCHOR);
            self.ok(&["init", name, "--template", "purity.md"], own);
            let out = self
                .cmd(&["context", "add", name, "k"], own)
                .write_stdin("the value")
                .output()
                .unwrap();
            assert!(out.status.success(), "context add: {out:?}");
        }
        let out = self.ok(
            &[
                "next",
                FINISHED,
                "--no-cleanup",
                "--with-data",
                r#"{"go": "yes"}"#,
            ],
            Some(OWN_ANCHOR),
        );
        let resp: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(resp["action"], "done", "{resp}");
        assert!(self
            .session_dir(RUNNING)
            .join("ctx/koto/links/held-by")
            .join(OWN_ANCHOR)
            .exists());
    }

    /// The SHA-256 of `name`'s state log and of every file under its context
    /// store, by path.
    fn session_hashes(&self, name: &str) -> BTreeMap<String, String> {
        let root = self.session_dir(name);
        let mut out = BTreeMap::new();
        let log = root.join(format!("koto-{name}.state.jsonl"));
        out.insert("log".to_string(), sha256_file(&log));
        let mut stack = vec![root.join("ctx")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(&root).unwrap().to_string_lossy();
                    out.insert(rel.into_owned(), sha256_file(&p));
                }
            }
        }
        out
    }

    /// The SHA-256 of every bucket object under `name`'s remote prefix, by key.
    fn bucket_hashes(&self, s3: &FakeS3, name: &str) -> BTreeMap<String, String> {
        s3.keys_under(&format!("{}/{name}/", self.prefix()))
            .into_iter()
            .map(|k| {
                let digest = hex::encode(Sha256::digest(s3.object(&k).unwrap()));
                (k, digest)
            })
            .collect()
    }

    /// The `context_read` events in `name`'s log, as (key, access, session).
    fn logged_reads(&self, name: &str) -> Vec<(String, String, Option<String>)> {
        let log = self
            .session_dir(name)
            .join(format!("koto-{name}.state.jsonl"));
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|e| e["type"] == "context_read")
            .map(|e| {
                let p = &e["payload"];
                (
                    p["key"].as_str().unwrap().to_string(),
                    p["access"].as_str().unwrap_or("content").to_string(),
                    p["session"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    /// Every read verb on `name`, run outside its tick with `anchor`; each
    /// answers as it would with no read logged at all.
    fn read_all(&self, name: &str, anchor: Option<&str>) {
        let status: Value =
            serde_json::from_slice(&self.ok(&["status", name], anchor).stdout).unwrap();
        assert_eq!(status["name"], name, "{status}");
        let got = self.ok(&["context", "get", name, "k"], anchor);
        assert_eq!(String::from_utf8_lossy(&got.stdout), "the value");
        let exists = self.run(&["context", "exists", name, "k"], anchor);
        assert_eq!(exists.status.code(), Some(0), "{exists:?}");
        let listed: Vec<String> =
            serde_json::from_slice(&self.ok(&["context", "list", name], anchor).stdout).unwrap();
        assert!(listed.contains(&"k".to_string()), "{listed:?}");
    }
}

fn sha256_file(p: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(p).unwrap()))
}

/// Read both sessions with every reader that isn't another agent session,
/// then with another agent session's anchor, and check what each wrote.
fn reads_leave_the_session_byte_identical(s3: Option<&FakeS3>) {
    let h = Host::new(s3);
    h.sessions();
    for name in [RUNNING, FINISHED] {
        let files = h.session_hashes(name);
        let objects = s3.map(|s3| h.bucket_hashes(s3, name));
        if let Some(objects) = &objects {
            assert!(!objects.is_empty(), "{name} has no bucket objects");
        }
        let reads_before = h.logged_reads(name);

        for anchor in [None, Some(OWN_ANCHOR), Some(name), Some(OTHER_ANCHOR)] {
            h.read_all(name, anchor);
            assert_eq!(h.session_hashes(name), files, "{name} with {anchor:?}");
            if let Some(s3) = s3 {
                assert_eq!(
                    Some(h.bucket_hashes(s3, name)),
                    objects,
                    "{name}'s bucket objects with {anchor:?}"
                );
            }
        }
        assert_eq!(h.logged_reads(name), reads_before, "{name}");
    }

    // Another agent session's reads are logged in its anchor, naming the
    // session read; the anchor that holds the sessions got none.
    let mut expected = Vec::new();
    for name in [RUNNING, FINISHED] {
        for access in ["content", "presence"] {
            expected.push(("k".to_string(), access.to_string(), Some(name.to_string())));
        }
    }
    assert_eq!(h.logged_reads(OTHER_ANCHOR), expected);
    assert!(h.logged_reads(OWN_ANCHOR).is_empty());
}

#[test]
fn reads_outside_the_tick_leave_a_local_session_byte_identical() {
    reads_leave_the_session_byte_identical(None);
}

#[test]
fn reads_outside_the_tick_leave_a_cloud_session_and_its_bucket_objects_byte_identical() {
    let s3 = FakeS3::start(BUCKET);
    reads_leave_the_session_byte_identical(Some(&s3));
}

/// A gate script in the session's own tick that runs `koto context get` on
/// its session logs the read in the session, with no reader's anchor.
#[test]
fn a_gate_script_in_the_sessions_own_tick_logs_its_read_in_the_session() {
    let h = Host::new(None);
    std::fs::write(
        h.ws.join("gate.md"),
        r#"---
name: gate-read
version: "1.0"
initial_state: check
states:
  check:
    gates:
      peek:
        type: command
        command: "koto context get g k"
    transitions:
      - target: done
        when:
          gates.peek.exit_code: 0
  done:
    terminal: true
---

## check

Check.

## done

Done.
"#,
    )
    .unwrap();
    h.ok(&["init", "g", "--template", "gate.md"], None);
    let out = h
        .cmd(&["context", "add", "g", "k"], None)
        .write_stdin("the value")
        .output()
        .unwrap();
    assert!(out.status.success(), "context add: {out:?}");

    let resp: Value =
        serde_json::from_slice(&h.ok(&["next", "g", "--no-cleanup"], None).stdout).unwrap();
    assert_eq!(resp["action"], "done", "{resp}");
    assert_eq!(
        h.logged_reads("g"),
        vec![("k".to_string(), "content".to_string(), None)]
    );
}
