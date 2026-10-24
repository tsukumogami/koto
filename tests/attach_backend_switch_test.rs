//! `koto init --attach-live` after the host's session backend switches from
//! local to cloud. The cloud backend keeps its local copy in the same
//! sessions directory the local store used, so a session the local store
//! started is still this caller's: attaching uploads it whole and records
//! the cloud origin. Another directory, or a cloud session from another
//! bucket, is still refused with `origin_mismatch`.
//!
//! The backend is switched the way a host does it, through the
//! `KOTO_SESSION_*` environment, against the in-process S3 endpoint.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::path::{Path, PathBuf};

/// Every variable that could carry the shell's own session store or
/// credentials; each command removes them all first.
const CLEARED: &[&str] = &[
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_CLOUD_ENDPOINT",
    "KOTO_SESSION_CLOUD_BUCKET",
    "KOTO_SESSION_CLOUD_REGION",
    "KOTO_SESSION_CLOUD_PATH_STYLE",
    "KOTO_SESSION_CLOUD_ACCESS_KEY",
    "KOTO_SESSION_CLOUD_SECRET_KEY",
    "KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SECURITY_TOKEN",
    "AWS_PROFILE",
    "KOTO_SESSIONS_BASE",
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_WORKFLOWS_DIR",
];

const TEMPLATE: &str = r#"---
name: switch
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
  MERGE:
    values: ["true", "false"]
    default: "false"
    rebind: true
states:
  work:
    accepts:
      status:
        type: enum
        values: [finished]
        required: true
    transitions:
      - target: done
        when:
          status: finished
  done:
    terminal: true
---

## work

Work on {{TOPIC}}.

## done

Done.
"#;

/// One disposable host: `HOME`, cache and a workspace holding the
/// template and the vars file.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
}

impl Host {
    fn new() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let canon = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        let home = canon(root.path().join("home"));
        let cache = canon(root.path().join("cache"));
        let ws = canon(root.path().join("ws"));
        std::fs::write(ws.join("switch.md"), TEMPLATE).unwrap();
        std::fs::write(ws.join("vars.json"), r#"[["TOPIC", "t1"]]"#).unwrap();
        std::fs::write(ws.join("notes.md"), "notes\n").unwrap();
        Host {
            _root: root,
            home,
            cache,
            ws,
        }
    }

    fn koto_in(&self, cwd: &Path, args: &[&str], env: &[(&str, String)]) -> Out {
        let mut cmd = koto_command::koto();
        for k in CLEARED {
            cmd.env_remove(k);
        }
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let o = cmd.args(args).output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn koto(&self, args: &[&str], env: &[(&str, String)]) -> Out {
        self.koto_in(&self.ws, args, env)
    }

    fn attach_in(&self, cwd: &Path, env: &[(&str, String)]) -> Out {
        let template = self.ws.join("switch.md");
        let vars = self.ws.join("vars.json");
        self.koto_in(
            cwd,
            &[
                "init",
                "s",
                "--template",
                template.to_str().unwrap(),
                "--vars-file",
                vars.to_str().unwrap(),
                "--attach-live",
            ],
            env,
        )
    }

    fn attach(&self, env: &[(&str, String)]) -> Out {
        self.attach_in(&self.ws, env)
    }

    /// A session opened under the local store, with one context key.
    fn local_session(&self) {
        let init = self.koto(
            &[
                "init",
                "s",
                "--template",
                "switch.md",
                "--vars-file",
                "vars.json",
            ],
            &[],
        );
        init.ok("local init");
        self.koto(
            &["context", "add", "s", "notes.md", "--from-file", "notes.md"],
            &[],
        )
        .ok("local context add");
        assert_eq!(self.origin()["store"]["kind"], "local");
    }

    /// The session's state file, wherever the store keeps it under `HOME`.
    fn state_file(&self) -> PathBuf {
        fn find(dir: &Path) -> Option<PathBuf> {
            for entry in std::fs::read_dir(dir).ok()? {
                let path = entry.ok()?.path();
                if path.is_dir() {
                    if let Some(found) = find(&path) {
                        return Some(found);
                    }
                } else if path.file_name().is_some_and(|n| n == "koto-s.state.jsonl") {
                    return Some(path);
                }
            }
            None
        }
        find(&self.home.join(".koto")).expect("no state file for session s")
    }

    fn origin(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.state_file()).unwrap();
        let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        header["origin"].clone()
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self, what: &str) -> serde_json::Value {
        assert_eq!(
            self.code, 0,
            "{what}: stdout {} stderr {}",
            self.stdout, self.stderr
        );
        serde_json::from_str(self.stdout.trim()).unwrap_or(serde_json::Value::Null)
    }

    fn err(&self, what: &str) -> serde_json::Value {
        assert_ne!(self.code, 0, "{what} succeeded: {}", self.stdout);
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {} {}", self.stdout, self.stderr))
    }
}

fn cloud_env(s3: &FakeS3) -> Vec<(&'static str, String)> {
    vec![
        ("KOTO_SESSION_BACKEND", "cloud".to_string()),
        ("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint()),
        ("KOTO_SESSION_CLOUD_BUCKET", s3.bucket().to_string()),
        ("KOTO_SESSION_CLOUD_REGION", "us-east-1".to_string()),
        ("KOTO_SESSION_CLOUD_PATH_STYLE", "true".to_string()),
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY",
            "koto-access-key-id".to_string(),
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY",
            "koto-secret-key-value".to_string(),
        ),
    ]
}

#[test]
fn a_local_session_reattaches_after_the_backend_switches_to_cloud() {
    let host = Host::new();
    host.local_session();

    let s3 = FakeS3::start("koto-switch");
    let cloud = cloud_env(&s3);
    // Ticking keeps working across the switch; it pushes the state file.
    host.koto(&["next", "s"], &cloud).ok("cloud next");

    let attached = host.attach(&cloud).ok("cloud attach");
    assert_eq!(attached["outcome"], "attached", "{attached}");

    // The bucket now holds the whole session, not only its state file.
    let keys = s3.keys();
    assert!(
        keys.iter().any(|k| k.ends_with("/s/koto-s.state.jsonl")),
        "{keys:?}"
    );
    assert!(
        keys.iter().any(|k| k.ends_with("/s/ctx/notes.md")),
        "{keys:?}"
    );
    let origin = host.origin();
    assert_eq!(origin["store"]["kind"], "cloud", "{origin}");
    assert_eq!(origin["store"]["bucket"], "koto-switch", "{origin}");

    // Adopted once: the next attach is cloud to cloud.
    host.attach(&cloud).ok("second cloud attach");
}

#[test]
fn a_local_session_from_another_directory_is_still_refused_under_cloud() {
    let host = Host::new();
    host.local_session();
    let s3 = FakeS3::start("koto-switch");
    let other = host.ws.join("other-worktree");
    std::fs::create_dir(&other).unwrap();

    let err = host
        .attach_in(&other, &cloud_env(&s3))
        .err("attach elsewhere");
    assert_eq!(err["code"], "origin_mismatch", "{err}");
    assert_eq!(host.origin()["store"]["kind"], "local");
    assert!(
        !s3.keys().iter().any(|k| k.ends_with("/s/ctx/notes.md")),
        "a refused attach uploaded the session"
    );
}

#[test]
fn a_cloud_session_is_refused_from_another_bucket() {
    let host = Host::new();
    host.local_session();
    let first = FakeS3::start("koto-first");
    host.attach(&cloud_env(&first)).ok("adopting attach");
    let before = std::fs::read(host.state_file()).unwrap();

    let second = FakeS3::start("koto-second");
    let err = host
        .attach(&cloud_env(&second))
        .err("attach from another bucket");
    assert_eq!(err["code"], "origin_mismatch", "{err}");
    let msg = err["error"].as_str().unwrap();
    assert!(msg.contains("bucket koto-first"), "{msg}");
    assert!(msg.contains("bucket koto-second"), "{msg}");
    assert_eq!(std::fs::read(host.state_file()).unwrap(), before);
}

#[test]
fn a_failed_upload_refuses_the_attach_and_keeps_the_local_origin() {
    let host = Host::new();
    host.local_session();
    let s3 = FakeS3::start("koto-switch");
    let cloud = cloud_env(&s3);
    // A key written under cloud shows the session's prefix in the bucket.
    host.koto(
        &["context", "add", "s", "more.md", "--from-file", "notes.md"],
        &cloud,
    )
    .ok("cloud context add");
    let key = s3
        .keys()
        .into_iter()
        .find(|k| k.ends_with("/s/ctx/more.md"))
        .expect("the context write pushed nothing");
    let version_key = key.replace("/ctx/more.md", "/version.json");
    s3.fail("PUT", &version_key, 500);

    let err = host.attach(&cloud).err("attach with a failing bucket");
    assert_eq!(err["code"], "persistence_error", "{err}");
    assert_eq!(host.origin()["store"]["kind"], "local");

    s3.clear_faults();
    host.attach(&cloud).ok("attach once the bucket answers");
    assert_eq!(host.origin()["store"]["kind"], "cloud");
}
