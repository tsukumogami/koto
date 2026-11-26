//! `koto session resolve --keep local` on a cloud session whose last
//! `koto next` couldn't finish its exit flush.
//!
//! The flush leaves the batch journal open in the session directory and the
//! batch-open marker in the bucket, and this host's `version.json` ends up
//! one ahead of the bucket's while the bucket's names this same machine.
//! Every later command reports a session conflict, and the resolve it tells
//! the user to run has to clear it, from the session's own workspace and
//! from another directory on the same host alike. A conflict with another
//! machine's push stays a hard error for the ordinary commands.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`, with the backend chosen through
//! `KOTO_SESSION_*` variables alone. Each simulated host is its own `HOME`
//! and workspace.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-resolve";
const NAME: &str = "stuck";
const TEMPLATE_FILE: &str = "stuck.md";

/// A system `PATH` for the action's shell, after the directory holding the
/// koto under test.
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// A workflow whose first state writes one context key from its default
/// action, so the tick opens a batch, and then waits for evidence.
fn template() -> String {
    format!(
        r#"---
name: stuck
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
        set -e
        printf one | koto context add {NAME} one.md
    transitions:
      - target: wait
  wait:
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

## wait

Wait.

## done

Done.
"#
    )
}

fn koto_dir() -> PathBuf {
    koto_command::koto_bin().parent().unwrap().to_path_buf()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The remote prefix koto derives for a workspace.
fn prefix_of(dir: &Path) -> String {
    let canonical = std::fs::canonicalize(dir).unwrap();
    sha256_hex(canonical.to_string_lossy().as_bytes())[..16].to_string()
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn says(&self, needle: &str) -> bool {
        self.stdout.contains(needle) || self.stderr.contains(needle)
    }

    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }

    /// The JSON object on stdout, whole or on its last line.
    fn json(&self) -> serde_json::Value {
        let out = self.stdout.trim();
        serde_json::from_str(out)
            .or_else(|_| serde_json::from_str(out.lines().last().unwrap_or_default()))
            .unwrap_or_else(|e| panic!("no JSON ({e}): {}", self.describe()))
    }
}

/// One simulated host: its own `HOME`, template cache and workspace, a
/// second directory on the same host whose prefix isn't the workspace's,
/// and the shared bucket.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
    elsewhere: PathBuf,
    endpoint: String,
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Host {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            ws: mk(root.path().join("ws")),
            elsewhere: mk(root.path().join("elsewhere")),
            _root: root,
            endpoint: s3.endpoint(),
        }
    }

    /// A koto run in `dir` with nothing inherited from the test process.
    fn cmd_in(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(dir)
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
        Command::from_std(cmd)
    }

    fn koto_in(&self, dir: &Path, args: &[&str]) -> Out {
        let o = self.cmd_in(dir, args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// koto run in the session's own workspace.
    fn koto(&self, args: &[&str]) -> Out {
        self.koto_in(&self.ws, args)
    }

    /// `koto context add` from outside any tick.
    fn add(&self, key: &str, content: &[u8]) -> Out {
        let o = self
            .cmd_in(&self.ws, &["context", "add", NAME, key])
            .write_stdin(content.to_vec())
            .output()
            .unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn init(&self) {
        std::fs::write(self.ws.join(TEMPLATE_FILE), template()).unwrap();
        let out = self.koto(&["init", NAME, "--template", TEMPLATE_FILE]);
        assert!(out.ok(), "init: {}", out.describe());
    }

    fn next(&self) -> Out {
        self.koto(&["next", NAME])
    }

    fn prefix(&self) -> String {
        prefix_of(&self.ws)
    }

    /// The bucket key of `rest` under the session's home copy.
    fn key(&self, rest: &str) -> String {
        format!("{}/{}/{}", self.prefix(), NAME, rest)
    }

    fn session_dir(&self) -> PathBuf {
        self.home.join(".koto").join("sessions").join(NAME)
    }

    fn version_path(&self) -> PathBuf {
        self.session_dir().join("version.json")
    }

    fn journal_path(&self) -> PathBuf {
        self.session_dir().join("cloud-batch.jsonl")
    }

    fn local_version(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.version_path()).unwrap()).unwrap()
    }
}

/// Drive `a`'s session into the state a failed exit flush leaves: a tick
/// whose batch-open marker can't be deleted keeps its journal open and the
/// marker in the bucket. Then set the two version files to what the report
/// shows: this host one ahead with a base behind the bucket's, and the
/// bucket's naming `remote_machine` (this host's own id when `None`).
/// Returns this host's machine id.
fn make_stuck(s3: &FakeS3, a: &Host, remote_machine: Option<&str>) -> String {
    a.init();
    let out = a.add("base.md", b"base");
    assert!(out.ok(), "add: {}", out.describe());

    s3.fail("DELETE", &a.key("batch-open.json"), 500);
    let _ = a.next();
    s3.clear_faults();
    assert!(
        a.journal_path().exists(),
        "the failed flush should leave the batch journal open"
    );
    assert!(
        s3.object(&a.key("batch-open.json")).is_some(),
        "the failed flush should leave the bucket's batch-open marker"
    );

    let local = a.local_version();
    let machine = local["machine_id"].as_str().unwrap().to_string();
    let n = s3
        .object(&a.key("version.json"))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).unwrap()["version"].as_u64())
        .unwrap_or_else(|| local["version"].as_u64().unwrap());
    assert!(n >= 1, "remote version {n}");

    let forged_local = serde_json::json!({
        "version": n + 1,
        "last_sync_base": n - 1,
        "machine_id": machine,
    });
    std::fs::write(a.version_path(), forged_local.to_string()).unwrap();
    let forged_remote = serde_json::json!({
        "version": n,
        "last_sync_base": n,
        "machine_id": remote_machine.unwrap_or(&machine),
    });
    s3.put(&a.key("version.json"), forged_remote.to_string().as_bytes());
    machine
}

/// The resolve cleared the stuck state: the journal and the bucket's marker
/// are gone, and the bucket's version file says what the local one does.
fn assert_resolved(s3: &FakeS3, a: &Host) {
    assert!(
        !a.journal_path().exists(),
        "the batch journal is still open after the resolve"
    );
    assert!(
        s3.object(&a.key("batch-open.json")).is_none(),
        "the bucket still holds the batch-open marker after the resolve"
    );
    let remote: serde_json::Value =
        serde_json::from_slice(&s3.object(&a.key("version.json")).unwrap()).unwrap();
    assert_eq!(remote, a.local_version());
}

/// Every bucket object and its bytes.
fn bucket(s3: &FakeS3) -> BTreeMap<String, Vec<u8>> {
    s3.keys()
        .into_iter()
        .map(|k| {
            let bytes = s3.object(&k).unwrap();
            (k, bytes)
        })
        .collect()
}

/// Rewrite the `session_id` in the header of the state copy at `key`, so
/// the bucket holds another session's copy where this one's should be.
fn plant_other_session(s3: &FakeS3, key: &str) {
    let text = String::from_utf8(s3.object(key).unwrap()).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    assert!(
        header.get("session_id").is_some(),
        "the state header names its session: {header}"
    );
    header["session_id"] = "00000000-0000-4000-8000-000000000000".into();
    s3.put(
        key,
        format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest).as_bytes(),
    );
}

/// Run resolve with `args` from `dir` on a stuck session and check that it
/// and the ordinary commands after it succeed.
fn resolve_and_continue(dir_is_elsewhere: bool, args: &[&str]) {
    resolve_and_continue_against(dir_is_elsewhere, None, args);
}

/// The same, with the bucket's version file naming `remote_machine`
/// (this host's own id when `None`).
fn resolve_and_continue_against(
    dir_is_elsewhere: bool,
    remote_machine: Option<&str>,
    args: &[&str],
) {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    assert_ne!(
        prefix_of(&a.elsewhere),
        a.prefix(),
        "the second directory must have a prefix of its own"
    );
    make_stuck(&s3, &a, remote_machine);
    let dir = if dir_is_elsewhere {
        a.elsewhere.clone()
    } else {
        a.ws.clone()
    };

    let out = a.koto_in(&dir, &["status", NAME]);
    assert!(
        !out.ok() && out.says("session conflict"),
        "status before the resolve: {}",
        out.describe()
    );

    let out = a.koto_in(&dir, args);
    assert!(out.ok(), "resolve: {}", out.describe());
    assert_resolved(&s3, &a);

    let out = a.koto_in(&dir, &["status", NAME]);
    assert!(out.ok(), "status after the resolve: {}", out.describe());
    // `koto next` runs only from the session's own directory (its execution
    // anchor), wherever the resolve ran, so the tick goes there.
    let out = a.koto_in(&a.ws, &["next", NAME, "--no-cleanup"]);
    assert!(out.ok(), "next after the resolve: {}", out.describe());
}

/// From the session's own workspace, `koto session resolve <name> --keep
/// local` clears a stuck flush, and status and next work again after it.
#[test]
fn resolve_keep_local_clears_a_stuck_flush_in_its_own_workspace() {
    resolve_and_continue(false, &["session", "resolve", NAME, "--keep", "local"]);
}

/// The same, with the flag before the session name.
#[test]
fn resolve_keep_local_clears_a_stuck_flush_with_the_flag_first() {
    resolve_and_continue(false, &["session", "resolve", "--keep", "local", NAME]);
}

/// From another directory on the same host, whose prefix isn't the one the
/// session was created under, the resolve still clears a stuck flush, and
/// status and next work again from there.
#[test]
fn resolve_keep_local_clears_a_stuck_flush_from_another_directory() {
    let s3 = FakeS3::start(BUCKET);
    let probe = Host::new(&s3);
    assert_ne!(
        prefix_of(&probe.elsewhere),
        probe.prefix(),
        "the second directory must have a prefix of its own"
    );
    probe.init();
    // The header records the workspace's prefix, so a command from the
    // second directory reads the session as homed elsewhere.
    let state = std::fs::read_to_string(
        probe
            .session_dir()
            .join(format!("koto-{}.state.jsonl", NAME)),
    )
    .unwrap();
    let header: serde_json::Value = serde_json::from_str(state.lines().next().unwrap()).unwrap();
    assert_eq!(
        header
            .pointer("/origin/store/prefix")
            .and_then(|v| v.as_str()),
        Some(probe.prefix().as_str()),
        "header: {header}"
    );
    drop(probe);

    resolve_and_continue(true, &["session", "resolve", NAME, "--keep", "local"]);
}

/// The flag-first form of the resolve from another directory on the same
/// host clears a stuck flush too, and status and next work from there.
#[test]
fn resolve_keep_local_clears_a_stuck_flush_from_another_directory_with_the_flag_first() {
    resolve_and_continue(true, &["session", "resolve", "--keep", "local", NAME]);
}

/// From the session's own workspace, `koto session resolve <name> --keep
/// remote` clears a stuck flush by taking the bucket's copy, and status and
/// next work again after it.
#[test]
fn resolve_keep_remote_clears_a_stuck_flush_in_its_own_workspace() {
    resolve_and_continue(false, &["session", "resolve", NAME, "--keep", "remote"]);
}

/// The same `--keep remote` resolve run from another directory on the same
/// host clears a stuck flush, and status and next work from there.
#[test]
fn resolve_keep_remote_clears_a_stuck_flush_from_another_directory() {
    resolve_and_continue(true, &["session", "resolve", NAME, "--keep", "remote"]);
}

/// A conflict with another machine's push is refused by status and next
/// run from `dir`, and leaves the journal and the bucket's marker in place.
fn assert_other_machine_refused(dir_is_elsewhere: bool) {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    let machine = make_stuck(&s3, &a, Some("otherhost"));
    assert_ne!(machine, "otherhost");
    let dir = if dir_is_elsewhere {
        a.elsewhere.clone()
    } else {
        a.ws.clone()
    };

    let out = a.koto_in(&dir, &["status", NAME]);
    assert!(
        !out.ok() && out.says("session conflict"),
        "status: {}",
        out.describe()
    );
    let out = a.koto_in(&dir, &["next", NAME, "--no-cleanup"]);
    assert!(
        !out.ok() && out.says("session conflict"),
        "next: {}",
        out.describe()
    );
    assert!(a.journal_path().exists());
    assert!(s3.object(&a.key("batch-open.json")).is_some());
}

/// A conflict with another machine's push is still refused by status and
/// next, batch open or not: only the resolve may settle it.
#[test]
fn a_conflict_with_another_machine_is_still_refused() {
    assert_other_machine_refused(false);
}

/// The same refusal holds when status and next run from another directory
/// on the same host.
#[test]
fn a_conflict_with_another_machine_is_still_refused_from_another_directory() {
    assert_other_machine_refused(true);
}

/// The resolve is the one command that settles a conflict with another
/// machine's push, from another directory on this host as well:
/// `--keep local` clears it, and status and next work from there.
#[test]
fn resolve_keep_local_settles_a_conflict_with_another_machine_from_another_directory() {
    resolve_and_continue_against(
        true,
        Some("otherhost"),
        &["session", "resolve", NAME, "--keep", "local"],
    );
}

/// A resolve from another directory whose recorded home in the bucket holds
/// another session's state copy is refused with `session_unlocated` and the
/// reason `recorded_copy_mismatch`, for either side kept. Nothing in the
/// bucket is written over, and the stuck batch stays where it was.
#[test]
fn resolve_from_another_directory_refuses_a_home_holding_another_session() {
    for keep in ["local", "remote"] {
        let s3 = FakeS3::start(BUCKET);
        let a = Host::new(&s3);
        make_stuck(&s3, &a, None);
        let state_key = a.key(&format!("koto-{NAME}.state.jsonl"));
        plant_other_session(&s3, &state_key);
        let before = bucket(&s3);
        let local_state_before =
            std::fs::read(a.session_dir().join(format!("koto-{}.state.jsonl", NAME))).unwrap();

        let out = a.koto_in(&a.elsewhere, &["session", "resolve", NAME, "--keep", keep]);
        assert_eq!(out.code, Some(2), "--keep {keep}: {}", out.describe());
        let err = out.json();
        assert_eq!(
            err["code"],
            "session_unlocated",
            "--keep {keep}: {}",
            out.describe()
        );
        assert_eq!(
            err["reason"],
            "recorded_copy_mismatch",
            "--keep {keep}: {}",
            out.describe()
        );
        assert_eq!(err["command"], "session resolve", "{}", out.describe());
        assert_eq!(err["session"], NAME, "{}", out.describe());

        assert_eq!(
            bucket(&s3),
            before,
            "--keep {keep}: the refused resolve changed the bucket"
        );
        assert!(
            a.journal_path().exists(),
            "--keep {keep}: the refused resolve dropped the batch journal"
        );
        assert_eq!(
            std::fs::read(a.session_dir().join(format!("koto-{}.state.jsonl", NAME)),).unwrap(),
            local_state_before,
            "--keep {keep}: the refused resolve changed the local state file"
        );
    }
}
