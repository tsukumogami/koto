//! What keeps a coordinator's finished session from `koto session prune`
//! and `koto workspace prune`: never its `coordinate-` name, only what keeps
//! any session, a pin, a live member of its hierarchy, or a later session
//! continuing it (`koto session link --continues`, or `koto init
//! --continues`), which the entry names. Every case runs twice through the
//! real binary, on the local backend and on the cloud backend over the
//! in-process stand-in bucket (`support/fake_s3.rs`).
//!
//! Each test has a scratch `HOME`, `KOTO_HOME` and cache, and every command
//! is built by `support/koto_command.rs` with every inherited `KOTO_*`
//! variable removed, so no command reaches a store or bucket but the
//! test's own. Under `KOTO_HOME` only the root's own config chooses the
//! cloud backend, so the cloud store's root config does; the stand-in's
//! address and keys come from the environment each command sets. Before
//! any prune, each test checks that koto resolves its root, backend and
//! bucket to the test's own, and that every session it prunes lives under
//! the test's temporary directory.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_s3::FakeS3;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `work` waits for a marker; `done` is the success terminal.
const WORK: &str = r#"---
name: prune-successor
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

const BUCKET: &str = "koto-prune-successor";

/// The fields a kept entry carries to name what keeps it.
const KEEPER_FIELDS: [&str; 8] = [
    "rule",
    "detail",
    "hierarchy",
    "kept_by_pin",
    "pin",
    "kept_by_relation",
    "relation_keepers",
    "relation_unreadable",
];

/// Wait out the one-second retention every command runs with.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

struct Store {
    _tmp: TempDir,
    /// The stand-in bucket, under the cloud backend.
    s3: Option<FakeS3>,
    root: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Store {
    fn new(cloud: bool) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for d in ["home", "koto-home", "cache", "work"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        if cloud {
            std::fs::write(
                root.join("koto-home").join("config.toml"),
                "[session]\nbackend = \"cloud\"\n",
            )
            .unwrap();
        }
        let template = root.join("work.md");
        std::fs::write(&template, WORK).unwrap();
        let store = Store {
            _tmp: tmp,
            s3: cloud.then(|| FakeS3::start(BUCKET)),
            work: root.join("work"),
            root,
            template,
        };
        store.assert_scratch_store();
        store
    }

    fn koto_home(&self) -> PathBuf {
        self.root.join("koto-home")
    }

    /// koto resolves the root to this test's `KOTO_HOME`, on the backend
    /// the test chose and, under the cloud backend, the stand-in's bucket.
    fn assert_scratch_store(&self) {
        let report = self.ok(&["session", "dir", "--json"]);
        let resolved = PathBuf::from(report["root"].as_str().unwrap());
        assert_eq!(resolved, self.koto_home(), "{report}");
        assert!(resolved.starts_with(&self.root), "{report}");
        if self.s3.is_some() {
            assert_eq!(report["backend"], "cloud", "{report}");
            assert_eq!(report["bucket"], BUCKET, "{report}");
        } else {
            assert_eq!(report["backend"], "local", "{report}");
        }
    }

    /// A command in the work directory with no inherited `KOTO_*` variable
    /// and no anchor or agent session.
    fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.work)
            .env("HOME", self.root.join("home"))
            .env("KOTO_HOME", self.koto_home())
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        if let Some(s3) = &self.s3 {
            cmd.env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint())
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let out = self.cmd().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn ok(&self, args: &[&str]) -> Value {
        printed_json(&self.run(args).stdout)
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    /// Tick `name` to `done`, keeping it.
    fn done(&self, name: &str) {
        let v = self.ok(&[
            "next",
            name,
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ]);
        assert_eq!(v["action"], "done", "{v:#}");
    }

    /// Where koto keeps `name` on this host.
    fn session_dir(&self, name: &str) -> PathBuf {
        let out = self.run(&["session", "dir", name]);
        PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
    }

    /// The bucket's copy of `name`'s state file, under the cloud backend.
    fn remote_state(&self, name: &str) -> Option<String> {
        self.s3
            .as_ref()
            .map(|_| format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&self.work)))
    }

    /// Check, before a prune, that every one of `names` lives under this
    /// test's temporary directory (and, under the cloud backend, that its
    /// copy is in the stand-in), and return where each lives.
    fn assert_scratch_sessions(&self, names: &[&str]) -> Vec<PathBuf> {
        self.assert_scratch_store();
        names
            .iter()
            .map(|name| {
                let dir = self.session_dir(name);
                assert!(dir.is_dir(), "{name}: {}", dir.display());
                let real = std::fs::canonicalize(&dir).unwrap();
                assert!(
                    real.starts_with(&self.root),
                    "{name} resolves outside the test's root: {}",
                    real.display()
                );
                if let (Some(s3), Some(key)) = (&self.s3, self.remote_state(name)) {
                    assert!(s3.object(&key).is_some(), "{name} isn't in the stand-in");
                }
                dir
            })
            .collect()
    }

    /// `name` is gone from this host and, under the cloud backend, from
    /// the bucket.
    fn assert_removed(&self, name: &str, dir: &Path) {
        assert!(!dir.exists(), "{name} is still at {}", dir.display());
        if let (Some(s3), Some(key)) = (&self.s3, self.remote_state(name)) {
            assert!(
                s3.object(&key).is_none(),
                "{name}'s copy is still in the bucket"
            );
        }
    }

    /// `koto session prune --apply --all`, confirming the one-second
    /// retention under the cloud backend.
    fn prune_apply(&self) -> Value {
        let mut args = vec!["session", "prune", "--apply", "--all"];
        if self.s3.is_some() {
            args.push("--confirm-short-retention");
        }
        let out = self.ok(&args);
        assert_eq!(out["mode"], "apply", "{out:#}");
        out
    }

    /// `koto workspace prune --root <root> --yes`: the text it printed and
    /// the JSON it ended with.
    fn workspace_prune(&self, root: &str) -> (String, Value) {
        let mut args = vec!["workspace", "prune", "--root", root, "--yes"];
        if self.s3.is_some() {
            args.push("--confirm-short-retention");
        }
        let out = self.run(&args);
        let stdout = String::from_utf8(out.stdout).unwrap();
        let json = printed_json(stdout.as_bytes());
        (stdout, json)
    }
}

fn prefix_of(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// What a command printed as JSON: the whole of stdout, or else its last
/// line that starts an object.
fn printed_json(stdout: &[u8]) -> Value {
    if let Ok(v) = serde_json::from_slice(stdout) {
        return v;
    }
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| panic!("no JSON in: {text}"))
}

fn entry<'a>(list: &'a Value, name: &str) -> Option<&'a Value> {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list:#}"))
        .iter()
        .find(|e| e["name"] == name)
}

fn kept<'a>(out: &'a Value, name: &str) -> &'a Value {
    entry(&out["kept"], name).unwrap_or_else(|| panic!("{name} isn't kept: {out:#}"))
}

fn keeper(name: &str, standing: &str) -> Value {
    json!([{"keeper": name, "standing": standing, "path": []}])
}

/// A finished `coordinate-` session with nothing keeping it is removed
/// once its retention passes, and nothing in its entry names a keeper.
fn a_coordinator_session_nothing_keeps_is_removed(cloud: bool) {
    let s = Store::new(cloud);
    s.init("coordinate-x", &[]);
    s.done("coordinate-x");
    settle();
    let dirs = s.assert_scratch_sessions(&["coordinate-x"]);

    let preview = s.ok(&["session", "prune", "--all"]);
    assert!(
        entry(&preview["kept"], "coordinate-x").is_none(),
        "{preview:#}"
    );
    let row = entry(&preview["remove"], "coordinate-x")
        .unwrap_or_else(|| panic!("not in the preview's remove list: {preview:#}"));
    for field in KEEPER_FIELDS {
        assert!(row.get(field).is_none(), "{field} in {row:#}");
    }

    let out = s.prune_apply();
    assert!(entry(&out["kept"], "coordinate-x").is_none(), "{out:#}");
    let row =
        entry(&out["removed"], "coordinate-x").unwrap_or_else(|| panic!("not removed: {out:#}"));
    for field in KEEPER_FIELDS {
        assert!(row.get(field).is_none(), "{field} in {row:#}");
    }
    s.assert_removed("coordinate-x", &dirs[0]);
}

#[test]
fn a_coordinator_session_nothing_keeps_is_removed_locally() {
    a_coordinator_session_nothing_keeps_is_removed(false);
}

#[test]
fn a_coordinator_session_nothing_keeps_is_removed_under_the_cloud_backend() {
    a_coordinator_session_nothing_keeps_is_removed(true);
}

/// A finished `coordinate-` session that is pinned, and one with a running
/// member in its hierarchy, are kept, and each entry names what keeps it.
fn a_pin_or_a_running_member_keeps_a_coordinator_session(cloud: bool) {
    let s = Store::new(cloud);
    s.init("coordinate-y", &[]);
    s.ok(&["session", "pin", "coordinate-y", "--reason", "the record"]);
    s.done("coordinate-y");
    s.init("coordinate-z", &[]);
    s.init("z-worker", &["--parent", "coordinate-z"]);
    s.done("coordinate-z");
    settle();
    let dirs = s.assert_scratch_sessions(&["coordinate-y", "coordinate-z", "z-worker"]);

    let out = s.prune_apply();
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    let y = kept(&out, "coordinate-y");
    assert_eq!(y["rule"], "p", "{y:#}");
    assert_eq!(y["kept_by_pin"], true, "{y:#}");
    assert_eq!(y["pin"]["reason"], "the record", "{y:#}");
    let z = kept(&out, "coordinate-z");
    assert_eq!(z["rule"], "h", "{z:#}");
    assert!(z["detail"].as_str().unwrap().contains("z-worker"), "{z:#}");
    for dir in &dirs {
        assert!(dir.is_dir(), "{}", dir.display());
    }
}

#[test]
fn a_pin_or_a_running_member_keeps_a_coordinator_session_locally() {
    a_pin_or_a_running_member_keeps_a_coordinator_session(false);
}

#[test]
fn a_pin_or_a_running_member_keeps_a_coordinator_session_under_the_cloud_backend() {
    a_pin_or_a_running_member_keeps_a_coordinator_session(true);
}

/// A finished session a running later session continues is kept by both
/// prunes, which name the successor; once the successor finishes and its
/// retention passes, the next prune removes both.
fn a_running_successor_keeps_its_predecessor_until_it_finishes(cloud: bool) {
    let s = Store::new(cloud);
    s.init("coordinate-v1", &[]);
    s.done("coordinate-v1");
    s.init("coordinate-v2", &["--continues", "coordinate-v1"]);
    settle();
    let dirs = s.assert_scratch_sessions(&["coordinate-v1", "coordinate-v2"]);

    // `koto session prune --apply --all` keeps it by the relation: the
    // JSON names the successor, and so does the text.
    let out = s.prune_apply();
    assert!(out["removed"].as_array().unwrap().is_empty(), "{out:#}");
    let e = kept(&out, "coordinate-v1");
    assert_eq!(e["rule"], "r", "{e:#}");
    assert_eq!(e["kept_by_relation"], true, "{e:#}");
    assert_eq!(
        e["relation_keepers"],
        keeper("coordinate-v2", "alive"),
        "{e:#}"
    );
    assert_eq!(
        e["detail"], "kept: continued by coordinate-v2 (alive)",
        "{e:#}"
    );

    // `koto workspace prune` keeps it the same way.
    let (text, out) = s.workspace_prune("coordinate-v1");
    assert!(
        text.contains("coordinate-v1: kept: continued by coordinate-v2 (alive)"),
        "{text}"
    );
    let e = kept(&out, "coordinate-v1");
    assert_eq!(e["kept_by_relation"], true, "{e:#}");
    assert_eq!(
        e["relation_keepers"],
        keeper("coordinate-v2", "alive"),
        "{e:#}"
    );
    for dir in &dirs {
        assert!(dir.is_dir(), "{}", dir.display());
    }

    // The successor finishes; once its retention passes, nothing keeps
    // either, and the next prune removes both.
    s.done("coordinate-v2");
    settle();
    let dirs = s.assert_scratch_sessions(&["coordinate-v1", "coordinate-v2"]);
    let out = s.prune_apply();
    assert!(out["kept"].as_array().unwrap().is_empty(), "{out:#}");
    let mut removed: Vec<&str> = out["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    removed.sort();
    assert_eq!(removed, ["coordinate-v1", "coordinate-v2"], "{out:#}");
    for (name, dir) in ["coordinate-v1", "coordinate-v2"].iter().zip(&dirs) {
        s.assert_removed(name, dir);
    }
}

#[test]
fn a_running_successor_keeps_its_predecessor_until_it_finishes_locally() {
    a_running_successor_keeps_its_predecessor_until_it_finishes(false);
}

#[test]
fn a_running_successor_keeps_its_predecessor_until_it_finishes_under_the_cloud_backend() {
    a_running_successor_keeps_its_predecessor_until_it_finishes(true);
}
