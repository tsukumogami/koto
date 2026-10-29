//! A session written from more than one directory under the cloud backend
//! (koto#477).
//!
//! The cloud backend keeps a session in the bucket under a prefix derived
//! from the directory a command runs in, while every workspace on a host
//! shares one local copy of the session. A session written from two
//! directories therefore has two bucket copies. A pull used to write the
//! fetched copy over the local log without asking whether it held the local
//! events, so the directory that ran the last command decided which history
//! the host kept: a rebind run from a new anchor was undone by the next read
//! from the workspace root, along with every event the root's copy lacked.
//! A push was a plain PUT, so it could overwrite a copy that moved
//! underneath it.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`, from a scratch `HOME` with the environment
//! cleared; no real bucket or session store is involved.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-fork";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

const TEMPLATE: &str = r#"---
name: flow
version: "1.0"
initial_state: wait
states:
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

## wait

Wait.

## done

Done.
"#;

/// The remote prefix koto derives for a directory.
fn prefix_of(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

/// The bucket key of `WF`'s state file under `dir`'s prefix.
fn state_key(dir: &Path) -> String {
    format!("{}/{WF}/koto-{WF}.state.jsonl", prefix_of(dir))
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// One host: its own `HOME` and template cache, and directories to run
/// koto from, all sharing the host's one local session store.
struct Host {
    root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
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
            root,
            endpoint: s3.endpoint(),
        }
    }

    /// A directory on this host, created and canonical.
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(p).unwrap()
    }

    /// This host's local copy of `WF`'s state log.
    fn local_log(&self) -> String {
        let path = self
            .home
            .join(".koto")
            .join("sessions")
            .join(WF)
            .join(format!("koto-{WF}.state.jsonl"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {}", path.display(), e))
    }

    fn cmd(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(dir)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    koto_command::koto_bin().parent().unwrap().display(),
                    SYSTEM_PATH
                ),
            )
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .args(args);
        cmd
    }

    fn koto(&self, dir: &Path, args: &[&str]) -> Out {
        let o = self.cmd(dir, args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> Out {
        let out = self.koto(dir, args);
        assert_eq!(out.code, Some(0), "koto {:?}: {}", args, out.describe());
        out
    }

    /// `koto init WF` from `dir`, with any extra flags.
    fn init(&self, dir: &Path, extra: &[&str]) {
        std::fs::write(dir.join("flow.md"), TEMPLATE).unwrap();
        let mut args = vec!["init", WF, "--template", "flow.md"];
        args.extend_from_slice(extra);
        self.ok(dir, &args);
    }

    fn add(&self, dir: &Path, key: &str) -> Out {
        let o = self
            .cmd(dir, &["context", "add", WF, key])
            .write_stdin(format!("value of {key}").into_bytes())
            .output()
            .unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn add_ok(&self, dir: &Path, key: &str) {
        let out = self.add(dir, key);
        assert_eq!(out.code, Some(0), "context add {key}: {}", out.describe());
    }
}

/// The event lines of a state log, header excluded.
fn events(log: &str) -> Vec<serde_json::Value> {
    log.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn header(log: &str) -> serde_json::Value {
    serde_json::from_str(log.lines().next().unwrap()).unwrap()
}

/// Whether the log records `context_added` for `key`.
fn has_key_event(log: &str, key: &str) -> bool {
    events(log)
        .iter()
        .any(|e| e["type"] == "context_added" && e["payload"]["key"] == key)
}

fn has_rebind(log: &str) -> bool {
    events(log)
        .iter()
        .any(|e| e["type"] == "execution_anchor_rebound")
}

/// The report's shape: a hub started from its workspace root with its
/// anchor elsewhere, written from both, then rebound from a third
/// directory. A read from the root afterwards found the root's older copy
/// in the bucket and wrote it over the local log, dropping the rebind and
/// every event the root's copy lacked.
#[test]
fn a_read_from_the_root_after_a_rebind_keeps_every_event() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let root = host.dir("ws");
    let anchor = host.dir("anchor");
    let moved = host.dir("moved");
    host.init(&root, &["--execution-dir", anchor.to_str().unwrap()]);

    host.add_ok(&root, "k1");
    host.add_ok(&anchor, "k2");
    host.ok(&moved, &["session", "rebind", WF]);

    // A read from the root: its bucket copy ends before k2 and the rebind.
    host.ok(&root, &["status", WF]);

    let log = host.local_log();
    assert!(
        has_rebind(&log),
        "the local log lost its execution_anchor_rebound event after a read from the \
         workspace root:\n{log}"
    );
    assert!(
        has_key_event(&log, "k2"),
        "the local log lost the event k2 wrote:\n{log}"
    );
    assert_eq!(
        header(&log)["execution_dir"],
        serde_json::json!(moved.to_str().unwrap()),
        "the header must still name the new anchor"
    );

    // The new anchor keeps writing, and the session's one bucket copy, under
    // the prefix it was created under, carries everything.
    host.add_ok(&moved, "k3");
    let log = host.local_log();
    for key in ["k1", "k2", "k3"] {
        assert!(has_key_event(&log, key), "{key} missing:\n{log}");
    }
    assert!(has_rebind(&log));
    let remote = String::from_utf8(s3.object(&state_key(&root)).unwrap()).unwrap();
    assert_eq!(remote, log, "the session's bucket copy is the local log");
    for dir in [&anchor, &moved] {
        assert!(
            s3.object(&state_key(dir)).is_none(),
            "a second copy appeared"
        );
    }
}

/// A bucket copy whose events diverge from the local log's is never written
/// over it: the read refuses, naming both sides' lengths and last seqs.
#[test]
fn a_pull_of_a_diverged_copy_refuses_and_names_both_sides() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let root = host.dir("ws");
    let anchor = host.dir("anchor");
    host.init(&root, &["--execution-dir", anchor.to_str().unwrap()]);
    host.add_ok(&root, "k1");
    host.add_ok(&anchor, "k2");

    // The root's copy, as another writer would have left it: the same
    // history up to k1, then an event of its own at the seq k2 took here.
    let local = host.local_log();
    let lines: Vec<&str> = local.lines().collect();
    let mut forked: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
    forked["payload"]["key"] = serde_json::json!("other");
    let mut diverged = lines[..lines.len() - 1].join("\n");
    diverged.push('\n');
    diverged.push_str(&serde_json::to_string(&forked).unwrap());
    diverged.push('\n');
    s3.put(&state_key(&root), diverged.as_bytes());
    let last_seq = events(&local).last().unwrap()["seq"].as_u64().unwrap();

    let out = host.koto(&root, &["status", WF]);
    assert_ne!(
        out.code,
        Some(0),
        "the read must refuse: {}",
        out.describe()
    );
    let text = format!("{}{}", out.stdout, out.stderr);
    assert!(text.contains("session conflict"), "{}", out.describe());
    assert!(
        text.contains(&format!("{} bytes", local.len()))
            && text.contains(&format!("{} bytes", diverged.len())),
        "the refusal names both lengths: {}",
        out.describe()
    );
    assert!(
        text.contains(&format!("last seq {last_seq}")),
        "the refusal names the last seqs: {}",
        out.describe()
    );
    assert_eq!(host.local_log(), local, "the local log is left as it was");
}

/// A push whose bucket copy moved since this host last synced it, to a
/// history the push doesn't contain, refuses instead of overwriting it.
#[test]
fn a_push_over_a_copy_that_moved_refuses_and_keeps_the_copy() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let anchor = host.dir("anchor");
    host.init(&anchor, &[]);
    host.add_ok(&anchor, "k1");

    // Another writer replaces the bucket's copy while `context add k2`
    // runs, after its pull and before its state push.
    let local = host.local_log();
    let mut other = local.clone();
    let mut forked = events(&local).last().unwrap().clone();
    forked["seq"] = serde_json::json!(forked["seq"].as_u64().unwrap() + 1);
    forked["payload"]["key"] = serde_json::json!("theirs");
    other.push_str(&serde_json::to_string(&forked).unwrap());
    other.push('\n');
    let manifest = format!("{}/{WF}/ctx/manifest.json", prefix_of(&anchor));
    s3.put_after("PUT", &manifest, &state_key(&anchor), other.as_bytes());

    let out = host.add(&anchor, "k2");
    assert_ne!(
        out.code,
        Some(0),
        "the push must refuse: {}",
        out.describe()
    );
    assert!(
        format!("{}{}", out.stdout, out.stderr).contains("session conflict"),
        "{}",
        out.describe()
    );
    assert_eq!(
        s3.object(&state_key(&anchor)).unwrap(),
        other.into_bytes(),
        "the other writer's copy is left in the bucket"
    );
}

/// `koto session rebind` from a directory that is neither the session's
/// own nor the one named by `--to` moves a session that records its prefix:
/// the rebind's header goes up to the session's own copy, and nothing lands
/// under the third directory's prefix.
#[test]
fn a_rebind_from_a_third_directory_pushes_to_the_own_copy() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let anchor = host.dir("anchor");
    let target = host.dir("target");
    let elsewhere = host.dir("elsewhere");
    host.init(&anchor, &[]);

    host.ok(
        &elsewhere,
        &["session", "rebind", WF, "--to", target.to_str().unwrap()],
    );
    assert!(has_rebind(&host.local_log()));
    let remote = String::from_utf8(s3.object(&state_key(&anchor)).unwrap()).unwrap();
    assert!(has_rebind(&remote), "the rebind went up to the own copy");
    let third = format!("{}/", prefix_of(&elsewhere));
    assert!(
        s3.keys().iter().all(|k| !k.starts_with(&third)),
        "nothing goes under the third directory's prefix"
    );
}

/// Drop the `origin.store.prefix` of `WF`'s local header, as an older koto
/// left it.
fn forget_recorded_prefix(host: &Host) {
    let path = host
        .home
        .join(".koto/sessions")
        .join(WF)
        .join(format!("koto-{WF}.state.jsonl"));
    let log = host.local_log();
    let (first, rest) = log.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    header["origin"]["store"]
        .as_object_mut()
        .unwrap()
        .remove("prefix")
        .expect("a new session records its prefix");
    std::fs::write(
        &path,
        format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest),
    )
    .unwrap();
}

/// A session recorded before koto named the prefix, whose copy is under
/// its anchor's prefix, rebinds from a third directory: the rebind goes up
/// to that copy, and nothing goes under the third directory's prefix.
#[test]
fn a_rebind_of_a_located_unrecorded_session_from_a_third_directory_goes_to_its_copy() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let anchor = host.dir("anchor");
    let target = host.dir("target");
    let elsewhere = host.dir("elsewhere");
    host.init(&anchor, &[]);
    forget_recorded_prefix(&host);

    host.ok(
        &elsewhere,
        &["session", "rebind", WF, "--to", target.to_str().unwrap()],
    );
    assert!(has_rebind(&host.local_log()));
    let remote = String::from_utf8(s3.object(&state_key(&anchor)).unwrap()).unwrap();
    assert!(
        has_rebind(&remote),
        "the rebind went up to the located copy"
    );
    let third = format!("{}/", prefix_of(&elsewhere));
    assert!(
        s3.keys().iter().all(|k| !k.starts_with(&third)),
        "nothing goes under the third directory's prefix"
    );
}

/// A session recorded before koto named the prefix, with no copy under
/// any candidate prefix, refuses a rebind from a third directory, naming
/// where to run it.
#[test]
fn a_rebind_of_an_unrecorded_session_from_a_third_directory_refuses() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let anchor = host.dir("anchor");
    let target = host.dir("target");
    let elsewhere = host.dir("elsewhere");
    host.init(&anchor, &[]);
    forget_recorded_prefix(&host);
    s3.remove(&state_key(&anchor));
    let before = host.local_log();

    let out = host.koto(
        &elsewhere,
        &["session", "rebind", WF, "--to", target.to_str().unwrap()],
    );
    assert_eq!(out.code, Some(2), "{}", out.describe());
    let err: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    let message = err["error"].as_str().unwrap();
    assert!(message.starts_with("session_unlocated"), "{message}");
    assert!(message.contains(anchor.to_str().unwrap()), "{message}");
    assert!(
        message.contains(&format!(
            "koto session rebind {WF} --to {}",
            target.display()
        )),
        "the message shows the exact command: {message}"
    );
    assert_eq!(host.local_log(), before, "nothing was written");
}

/// A rebind run from the `--to` directory moves the session while its old
/// anchor still exists; the old anchor's next write then errors on the
/// rebind's fence instead of forking the log.
#[test]
fn the_old_anchors_next_write_errors_on_the_rebind_fence() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let old = host.dir("old");
    let new = host.dir("new");
    host.init(&old, &[]);
    host.add_ok(&old, "k1");

    let out = host.ok(
        &new,
        &["session", "rebind", WF, "--to", new.to_str().unwrap()],
    );
    let rebound: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(rebound["rebound"], serde_json::json!(true));
    let log = host.local_log();
    assert_eq!(header(&log)["origin"]["anchor_epoch"], serde_json::json!(1));
    assert_eq!(
        header(&log)["origin"]["retired_anchors"],
        serde_json::json!([old.to_str().unwrap()])
    );

    let out = host.add(&old, "k2");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(
        format!("{}{}", out.stdout, out.stderr).contains("session_rebound"),
        "{}",
        out.describe()
    );
    assert_eq!(host.local_log(), log, "the old anchor wrote nothing");
    assert!(
        !s3.keys().iter().any(|k| k.ends_with("/ctx/k2")),
        "nothing of k2 reached the bucket"
    );

    host.add_ok(&new, "k2");
    assert!(has_key_event(&host.local_log(), "k2"));
}

/// When every directory the session records is gone (its instance was torn
/// down), a rebind run from the `--to` directory still works.
#[test]
fn a_rebind_from_the_target_works_when_every_recorded_directory_is_gone() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new(&s3);
    let root = host.dir("ws");
    let anchor = host.dir("anchor");
    let target = host.dir("target");
    host.init(&root, &["--execution-dir", anchor.to_str().unwrap()]);
    host.add_ok(&anchor, "k1");
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&anchor).unwrap();

    let out = host.ok(
        &target,
        &["session", "rebind", WF, "--to", target.to_str().unwrap()],
    );
    let rebound: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(rebound["rebound"], serde_json::json!(true));
    let log = host.local_log();
    assert!(has_rebind(&log));
    assert!(has_key_event(&log, "k1"));
    host.add_ok(&target, "k2");
    assert!(has_key_event(&host.local_log(), "k2"));
}
