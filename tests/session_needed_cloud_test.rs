//! `koto session needed <PATH>` under the cloud backend, through the real
//! binary: it reads the sessions directory the cloud backend keeps on this
//! host, gives the same answers the local backend does, follows a successor
//! only the bucket knows and a copy an import left a migration marker
//! naming, keeps the path when either can't be read, and makes no request
//! but listings and GETs.
//!
//! Each test runs up to two hosts over one in-process stand-in bucket
//! (`support/fake_s3.rs`, which records every request): a scratch `HOME`,
//! `KOTO_HOME` and cache per host, so each has its own local store, and two
//! workspace directories, so each has its own bucket prefix. Every command
//! is built by `support/koto_command.rs` with every inherited
//! `KOTO_SESSION_*` removed first, so none reaches any bucket but the
//! stand-in, and each test checks koto resolves its store under its own
//! temporary directory before anything else runs.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: needed-cloud
version: "1.0"
initial_state: work
states:
  work:
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

## work

Work.

## done

Done.
"#;

const BUCKET: &str = "koto-needed-cloud";

struct Hosts {
    _tmp: TempDir,
    s3: FakeS3,
    root: PathBuf,
    /// Host 1's workspace, where the runs under judgment are anchored.
    w1: PathBuf,
    /// Host 2's workspace, outside `w1`.
    w2: PathBuf,
    template: PathBuf,
}

impl Hosts {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let w1 = root.join("a").join("repo");
        let w2 = root.join("b").join("ws");
        for d in [&w1, &w2, &root.join("a").join("repo-2")] {
            std::fs::create_dir_all(d).unwrap();
        }
        for host in 1..=2 {
            for d in ["home", "koto-home", "cache"] {
                std::fs::create_dir_all(root.join(format!("{d}{host}"))).unwrap();
            }
            // With `KOTO_HOME` set, only the root's own config chooses the
            // cloud backend; the bucket's settings still come from the
            // environment each command sets.
            std::fs::write(
                root.join(format!("koto-home{host}")).join("config.toml"),
                "[session]\nbackend = \"cloud\"\n",
            )
            .unwrap();
        }
        let template = root.join("needed-cloud.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let hosts = Hosts {
            _tmp: tmp,
            s3: FakeS3::start(BUCKET),
            root,
            w1,
            w2,
            template,
        };
        for host in 1..=2 {
            hosts.assert_scratch_store(host);
        }
        hosts
    }

    fn koto_home(&self, host: u8) -> PathBuf {
        self.root.join(format!("koto-home{host}"))
    }

    /// koto resolves `host`'s root to its own `KOTO_HOME`, inside this
    /// test's temporary directory, on the cloud backend and the stand-in's
    /// bucket, so no command here reaches a real store or bucket.
    fn assert_scratch_store(&self, host: u8) {
        let out = self
            .cmd(host, &self.root, true)
            .args(["session", "dir", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let resolved = PathBuf::from(report["root"].as_str().unwrap());
        assert_eq!(resolved, self.koto_home(host));
        assert!(resolved.starts_with(&self.root));
        assert_eq!(report["backend"], "cloud", "{report}");
        assert_eq!(report["bucket"], BUCKET, "{report}");
    }

    /// A command on `host` in `dir`, under the cloud backend on the
    /// stand-in when `cloud`, with no anchor or agent session.
    fn cmd(&self, host: u8, dir: &Path, cloud: bool) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(dir)
            .env("HOME", self.root.join(format!("home{host}")))
            .env("KOTO_HOME", self.koto_home(host))
            .env("XDG_CACHE_HOME", self.root.join(format!("cache{host}")))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY");
        if cloud {
            cmd.env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", self.s3.endpoint())
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        }
        cmd
    }

    fn ok(&self, host: u8, dir: &Path, args: &[&str]) {
        let out = self.cmd(host, dir, true).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} on host {host} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Create session `name` on `host`, anchored at `dir`, with `extra`
    /// init flags.
    fn init(&self, host: u8, dir: &Path, name: &str, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(host, dir, &args);
    }

    /// Run the verb on `host` over `path` and return its exit code and line.
    fn needed(&self, host: u8, path: &Path, cloud: bool) -> (i32, String) {
        let out = self
            .cmd(host, &self.root, cloud)
            .args(["session", "needed", path.to_str().unwrap()])
            .output()
            .unwrap();
        let code = out.status.code().expect("exit code");
        let text = if code == 2 { out.stderr } else { out.stdout };
        let text = String::from_utf8(text).unwrap();
        let line = text.trim_end_matches('\n');
        assert!(!line.contains('\n'), "more than one line: {text:?}");
        assert!(line.len() <= 200, "line longer than 200 bytes: {line:?}");
        (code, line.to_string())
    }

    /// Run the verb on host 1 over `path`, checking it wrote nothing to the
    /// host's koto root and asked the bucket only to list and GET.
    fn needed_read_only(&self, path: &Path) -> (i32, String) {
        let before = store_hash(&self.koto_home(1));
        self.s3.clear_requests();
        let answer = self.needed(1, path, true);
        for r in self.s3.requests() {
            assert_eq!(
                r.method, "GET",
                "the verb made a {} of {:?}",
                r.method, r.key
            );
        }
        assert_eq!(store_hash(&self.koto_home(1)), before, "the verb wrote");
        answer
    }
}

/// The bucket prefix of the workspace at `dir`.
fn prefix_of(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// SHA-256 of every file under `root`, with its mode, plus every
/// directory, so an added, removed or changed entry shows.
fn store_hash(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if meta.is_dir() {
                out.insert(rel, format!("dir {:o}", meta.permissions().mode()));
                stack.push(p);
            } else {
                let digest = Sha256::digest(std::fs::read(&p).unwrap());
                out.insert(
                    rel,
                    format!("{:o} {}", meta.permissions().mode(), hex::encode(digest)),
                );
            }
        }
    }
    out
}

/// The verb reads the directory the cloud backend stores sessions in, so a
/// session `koto init` just made there is the one it judges, and it gives
/// the local backend's answers.
#[test]
fn the_cloud_backend_gives_the_local_answers_from_its_own_store() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    assert!(h.koto_home(1).join("sessions").join("v1").is_dir());

    assert_eq!(
        h.needed_read_only(&h.w1),
        (1, "needed: session v1 is running".to_string())
    );
    assert_eq!(
        h.needed_read_only(&h.root.join("a").join("repo-2")),
        (0, "not needed".to_string())
    );
    assert_eq!(h.needed_read_only(&h.root.join("missing")).0, 2);

    h.ok(
        1,
        &h.w1,
        &[
            "next",
            "v1",
            "--with-data",
            r#"{"go": "yes"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));
}

/// Under the local backend the verb reads `KOTO_SESSIONS_BASE`, where
/// `koto init` put the session.
#[test]
fn the_local_backend_reads_the_sessions_base_its_init_wrote() {
    let h = Hosts::new();
    std::fs::write(
        h.koto_home(1).join("config.toml"),
        "[session]\nbackend = \"local\"\n",
    )
    .unwrap();
    let base = h.root.join("sessions-base");
    std::fs::create_dir_all(&base).unwrap();
    let out = h
        .cmd(1, &h.w1, false)
        .env("KOTO_SESSIONS_BASE", &base)
        .args(["init", "v1", "--template", h.template.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(base.join("v1").is_dir());
    let out = h
        .cmd(1, &h.root, false)
        .env("KOTO_SESSIONS_BASE", &base)
        .args(["session", "needed", h.w1.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

/// v2 continues v1 from host 2, in another workspace: host 1 never pulled
/// it, so only the bucket knows it, and it releases v1's path.
#[test]
fn a_successor_only_the_bucket_knows_releases_the_path() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    assert_eq!(h.needed_read_only(&h.w1).0, 1);

    let w1 = h.w1.to_str().unwrap().to_string();
    h.init(
        2,
        &h.w2,
        "v2",
        &["--continues", "v1", "--continues-from", &w1],
    );
    assert!(!h.koto_home(1).join("sessions").join("v2").exists());
    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));
}

/// A successor the bucket can't serve keeps the path, named as unchecked.
#[test]
fn a_successor_the_bucket_cannot_serve_keeps_the_path() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    let w1 = h.w1.to_str().unwrap().to_string();
    h.init(
        2,
        &h.w2,
        "v2",
        &["--continues", "v1", "--continues-from", &w1],
    );

    let state = format!("{}/v2/koto-v2.state.jsonl", prefix_of(&h.w2));
    h.s3.fail("GET", &state, 500);
    assert_eq!(
        h.needed_read_only(&h.w1),
        (
            1,
            "needed: session v1 is running, and its successors in the bucket can't be checked"
                .to_string()
        )
    );
    h.s3.clear_faults();
    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));
}

/// v1 imported into host 2's workspace leaves a copy anchored there that
/// continues v1, with its back-reference beside v1, and a migration marker
/// naming the copy. Either releases v1's path; with only the marker there
/// (an earlier release's import), a marker that can't be read keeps it.
#[test]
fn a_session_imported_away_releases_the_path() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    let w1 = h.w1.to_str().unwrap().to_string();
    h.ok(
        2,
        &h.w2,
        &["session", "import", "v1", "--from", &w1, "--trust-template"],
    );
    let marker = format!("{}/v1/migrated.json", prefix_of(&h.w1));
    assert!(h.s3.object(&marker).is_some(), "the import left no marker");

    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));

    // The copy's relation, found through its back-reference, releases the
    // path whatever the marker says.
    h.s3.fail("GET", &marker, 500);
    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));
    h.s3.clear_faults();

    // An import by a release without the relation left the marker alone:
    // it releases the path, and a marker that can't be read keeps it.
    let pointer = format!(
        "{}/v1/ctx/koto/continued-by/{}/v1",
        prefix_of(&h.w1),
        prefix_of(&h.w2)
    );
    assert!(
        h.s3.object(&pointer).is_some(),
        "the import left no pointer"
    );
    h.s3.remove(&pointer);
    assert_eq!(h.needed_read_only(&h.w1), (0, "not needed".to_string()));
    h.s3.fail("GET", &marker, 500);
    assert_eq!(h.needed_read_only(&h.w1).0, 1);
}
