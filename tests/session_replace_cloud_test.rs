//! `koto init NEW --replaces OLD` under the cloud backend, through the real
//! binary: every line of OLD's close goes into the bucket in one PUT
//! conditional on the copy pulled, this host's copy takes the same close
//! (worked out against its own bytes when it holds events the bucket's
//! lacks), a conditional PUT the bucket refuses writes nothing of the close
//! in either copy, and a bucket copy another host rebound is refused as a
//! local close of a rebound log is.
//!
//! Each test runs one host over an in-process stand-in bucket
//! (`support/fake_s3.rs`), with a scratch `HOME`, `KOTO_HOME` and cache and
//! one workspace directory. Every inherited `KOTO_*` variable is removed
//! from each command (`KOTO_SESSIONS_BASE` and every `KOTO_SESSION_*`
//! included) before the cloud backend is set on the stand-in, so no command
//! reaches any other bucket or store.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: replace-cloud
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

const BUCKET: &str = "koto-replace-cloud";

struct Host {
    _tmp: TempDir,
    s3: FakeS3,
    root: PathBuf,
    ws: PathBuf,
    template: PathBuf,
}

impl Host {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        for d in ["home", "koto-home", "cache"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        // A koto root takes its backend from its own config, not the
        // environment; the bucket's settings still come from it.
        std::fs::write(
            root.join("koto-home/config.toml"),
            "[session]\nbackend = \"cloud\"\n",
        )
        .unwrap();
        let template = root.join("work.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let host = Host {
            _tmp: tmp,
            s3: FakeS3::start(BUCKET),
            root,
            ws,
            template,
        };
        host.assert_scratch_store();
        host
    }

    /// koto resolves the root to the scratch `KOTO_HOME`, inside this
    /// test's temporary directory, so no command reaches a real store.
    fn assert_scratch_store(&self) {
        let out = self.cmd().args(["session", "dir"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let resolved = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        assert_eq!(resolved, self.root.join("koto-home"));
    }

    /// A command in the workspace, under the cloud backend on the
    /// stand-in.
    fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.ws)
            .env("HOME", self.root.join("home"))
            .env("KOTO_HOME", self.root.join("koto-home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", self.s3.endpoint())
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    }

    /// Create `p` in the workspace.
    fn old(&self) {
        self.ok(&["init", "p", "--template", self.template.to_str().unwrap()]);
    }

    /// `koto init s --template <work.md> --replaces p`.
    fn replace(&self) -> std::process::Output {
        let template = self.template.to_str().unwrap();
        self.run(&["init", "s", "--template", template, "--replaces", "p"])
    }

    /// The bucket key of `name`'s state.
    fn state_key(&self, name: &str) -> String {
        format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&self.ws))
    }

    /// The bucket's copy of `name`'s state.
    fn bucket_log(&self, name: &str) -> Vec<u8> {
        self.s3.object(&self.state_key(name)).unwrap()
    }

    /// This host's copy of `name`'s state.
    fn local_path(&self, name: &str) -> PathBuf {
        self.root
            .join("koto-home")
            .join("sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn local_log(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.local_path(name)).unwrap()
    }

    /// The PUTs of `p`'s state the stand-in received.
    fn state_puts(&self) -> Vec<fake_s3::Request> {
        let key = self.state_key("p");
        self.s3
            .requests()
            .into_iter()
            .filter(|r| r.method == "PUT" && r.key == key)
            .collect()
    }

    /// `s`'s session id, from the header of the bucket's copy of its state.
    fn new_id(&self) -> Value {
        lines(&self.bucket_log("s"))[0]["session_id"].clone()
    }
}

fn prefix_of(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(digest)[..16].to_string()
}

fn printed_json(stdout: &[u8]) -> Value {
    if let Ok(v) = serde_json::from_slice(stdout) {
        return v;
    }
    let text = String::from_utf8_lossy(stdout);
    let line = text.lines().last().unwrap_or_default();
    line.find('{')
        .and_then(|start| serde_json::from_str(&line[start..]).ok())
        .unwrap_or(Value::Null)
}

/// Every line of a state log, parsed, its header first.
fn lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The last event's seq in a state log.
fn last_seq(bytes: &[u8]) -> u64 {
    lines(bytes).last().unwrap()["seq"].as_u64().unwrap()
}

/// `bytes` with one more `intent_updated` line, stamped as its last event.
fn with_intent(bytes: &[u8], intent: &str) -> Vec<u8> {
    let all = lines(bytes);
    let last = all.last().unwrap();
    let line = serde_json::json!({
        "seq": last["seq"].as_u64().unwrap() + 1,
        "timestamp": last["timestamp"],
        "type": "intent_updated",
        "payload": { "intent": intent },
    });
    let mut out = bytes.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(line.to_string().as_bytes());
    out.push(b'\n');
    out
}

/// Assert that `after` is `before` with the close of `p` for `s` added:
/// lines with consecutive seqs from `before`'s last, ending with the cancel
/// naming `s` and then the replaced fence naming `s`.
fn assert_closed_from(before: &[u8], after: &[u8], new_id: &Value, what: &str) {
    let old = lines(before);
    let now = lines(after);
    assert!(now.len() >= old.len() + 2, "{what}: {now:?}");
    assert_eq!(
        now[..old.len()],
        old[..],
        "{what}: the earlier lines changed"
    );
    let added = &now[old.len()..];
    let from = last_seq(before);
    for (i, event) in added.iter().enumerate() {
        assert_eq!(event["seq"], from + 1 + i as u64, "{what}: {added:?}");
    }
    let cancel = &added[added.len() - 2];
    assert_eq!(cancel["type"], "workflow_cancelled", "{what}: {added:?}");
    assert_eq!(cancel["payload"]["replaced_by"], "s", "{what}");
    assert_eq!(
        cancel["payload"]["replaced_by_session_id"], *new_id,
        "{what}"
    );
    assert_eq!(cancel["payload"]["reason"], "replaced by s", "{what}");
    let fence = &added[added.len() - 1];
    assert_eq!(fence["type"], "session_superseded", "{what}: {added:?}");
    assert_eq!(fence["payload"]["by"], "s", "{what}");
    assert_eq!(fence["payload"]["by_session_id"], *new_id, "{what}");
    assert_eq!(fence["payload"]["replaced"], true, "{what}");
}

/// The bucket's copy of OLD gets every close line in one PUT, conditional
/// on the copy pulled, and this host's copy ends up byte for byte the
/// bucket's.
#[test]
fn a_cloud_replace_writes_every_close_line_in_one_conditional_put() {
    let h = Host::new();
    h.old();
    let before = h.bucket_log("p");
    h.s3.clear_requests();

    let out = h.replace();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["outcome"], "replaced", "{out:?}");

    let puts = h.state_puts();
    assert_eq!(puts.len(), 1, "{puts:?}");
    assert!(puts[0].if_match.is_some(), "{puts:?}");
    let after = h.bucket_log("p");
    assert_closed_from(&before, &after, &h.new_id(), "bucket");
    assert_eq!(
        h.local_log("p"),
        after,
        "this host's copy isn't the bucket's"
    );
}

/// A local copy holding an event the bucket's lacks, while the bucket's
/// holds one the local copy lacks, takes the same close as the bucket's
/// copy, each after its own events.
#[test]
fn a_diverged_local_copy_adopts_the_same_close() {
    let h = Host::new();
    h.old();
    let synced = h.bucket_log("p");
    assert_eq!(h.local_log("p"), synced);
    let bucket_before = with_intent(&synced, "bucket");
    let local_before = with_intent(&synced, "local");
    h.s3.put(&h.state_key("p"), &bucket_before);
    std::fs::write(h.local_path("p"), &local_before).unwrap();
    h.s3.clear_requests();

    let out = h.replace();
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let puts = h.state_puts();
    assert_eq!(puts.len(), 1, "{puts:?}");
    assert!(puts[0].if_match.is_some(), "{puts:?}");
    let new_id = h.new_id();
    let bucket_after = h.bucket_log("p");
    let local_after = h.local_log("p");
    assert_closed_from(&bucket_before, &bucket_after, &new_id, "bucket");
    assert_closed_from(&local_before, &local_after, &new_id, "local");
}

/// A conditional PUT the bucket refuses, as a lost race (412) or as a
/// bucket without conditional writes (501), exits 3 `fence_unavailable`
/// with nothing of the close written in either copy of OLD. Run again once
/// the bucket takes the PUT, the close lands.
#[test]
fn a_replace_that_loses_the_conditional_put_writes_nothing() {
    let h = Host::new();
    h.old();
    let bucket_before = h.bucket_log("p");
    let local_before = h.local_log("p");
    for status in [412, 501] {
        h.s3.fail("PUT", &h.state_key("p"), status);
        let out = h.replace();
        h.s3.clear_faults();
        assert_eq!(out.status.code(), Some(3), "{status}: {out:?}");
        assert_eq!(
            printed_json(&out.stdout)["code"],
            "fence_unavailable",
            "{status}: {out:?}"
        );
        assert_eq!(
            h.bucket_log("p"),
            bucket_before,
            "{status}: the bucket's copy changed"
        );
        assert_eq!(
            h.local_log("p"),
            local_before,
            "{status}: this host's copy changed"
        );
    }

    let out = h.replace();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let after = h.bucket_log("p");
    assert_closed_from(&bucket_before, &after, &h.new_id(), "bucket");
    assert_eq!(
        h.local_log("p"),
        after,
        "this host's copy isn't the bucket's"
    );
}

/// `bytes` with its header rewritten as a whole-binding move from `from`
/// to `to` on another host leaves it: the next anchor epoch, `whole_epoch`
/// set to it, `to` the anchor and `from` retired. The events are kept byte
/// for byte.
fn moved_whole(bytes: &[u8], from: &Path, to: &Path) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let mut header: Value = serde_json::from_str(&lines[0]).unwrap();
    let to = to.to_str().unwrap();
    header["execution_dir"] = serde_json::json!(to);
    let origin = header["origin"]
        .as_object_mut()
        .expect("the header records no origin");
    let epoch = origin
        .get("anchor_epoch")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + 1;
    origin.insert("anchor".into(), serde_json::json!(to));
    origin.insert("anchor_epoch".into(), serde_json::json!(epoch));
    origin.insert("whole_epoch".into(), serde_json::json!(epoch));
    origin.insert(
        "retired_anchors".into(),
        serde_json::json!([from.to_str().unwrap()]),
    );
    lines[0] = serde_json::to_string(&header).unwrap();
    (lines.join("\n") + "\n").into_bytes()
}

/// The bucket's copy of OLD records a whole-binding move another host made
/// after this host's copy was read: the replace refuses `session_rebound`,
/// exit 2, as a local close of a rebound log does, and neither copy of OLD
/// changes.
#[test]
fn a_cloud_replace_refuses_a_run_another_host_rebound() {
    let h = Host::new();
    h.old();
    let moved = h.root.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let bucket_before = moved_whole(&h.bucket_log("p"), &h.ws, &moved);
    h.s3.put(&h.state_key("p"), &bucket_before);
    let local_before = h.local_log("p");

    let out = h.replace();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        printed_json(&out.stdout)["code"],
        "session_rebound",
        "{out:?}"
    );
    assert_eq!(
        h.bucket_log("p"),
        bucket_before,
        "the bucket's copy changed"
    );
    assert_eq!(h.local_log("p"), local_before, "this host's copy changed");
}

/// The same rebound run, replaced from the directory the move retired: the
/// refusal comes before anything is done, so `s` is never created, on this
/// host or in the bucket, and neither copy of OLD changes.
#[test]
fn a_cloud_replace_of_a_run_another_host_rebound_creates_nothing() {
    let h = Host::new();
    h.old();
    let moved = h.root.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let bucket_before = moved_whole(&h.bucket_log("p"), &h.ws, &moved);
    h.s3.put(&h.state_key("p"), &bucket_before);
    let local_before = h.local_log("p");

    let out = h.replace();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        printed_json(&out.stdout)["code"],
        "session_rebound",
        "{out:?}"
    );
    assert!(
        !h.root.join("koto-home").join("sessions").join("s").exists(),
        "the refused replace created s on this host: {out:?}"
    );
    assert!(
        h.s3.object(&h.state_key("s")).is_none(),
        "the refused replace put s in the bucket: {out:?}"
    );
    assert_eq!(
        h.bucket_log("p"),
        bucket_before,
        "the bucket's copy changed"
    );
    assert_eq!(h.local_log("p"), local_before, "this host's copy changed");
}
