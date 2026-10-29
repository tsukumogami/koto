//! `koto init SUCC --continues PRED --supersede` under the cloud backend,
//! through the real binary: the fence pushed into the predecessor's prefix
//! conditional on the copy it pulled, the successor created and carried
//! under its own prefix, the refusals that write nothing in either prefix,
//! two successors racing for one predecessor, a re-run that finishes an
//! interrupted carry, and the refusal of every later write to the
//! predecessor from either workspace.
//!
//! Each test runs up to two hosts over one in-process stand-in bucket
//! (`support/fake_s3.rs`): a scratch `HOME`, `KOTO_HOME` and cache per
//! host, so each has its own local store, and two workspace directories,
//! W1 and W2, so each has its own bucket prefix. Every inherited `KOTO_*`
//! variable is removed from each command (`KOTO_SESSIONS_BASE` and every
//! `KOTO_SESSION_*` included) before the cloud backend is set on the
//! stand-in, so no command reaches any other bucket or store.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: supersede-cloud
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

const BUCKET: &str = "koto-supersede-cloud";

/// The prefixes every supersede here carries.
const CARRY: [&str; 4] = ["record/", "legs/", "taken_legs", "leg_children"];

/// The predecessor's keys: every carried family, and one outside them.
const PRED_KEYS: [(&str, &str); 6] = [
    ("record/a", "alpha"),
    ("record/entries/000001", "first entry"),
    ("legs/req-1/leg-a", "leg a"),
    ("taken_legs", "[\"leg-a\"]"),
    ("leg_children/req-1/leg-a", "child-1"),
    ("coord/x", "coordinator only"),
];

struct Hosts {
    _tmp: TempDir,
    s3: FakeS3,
    root: PathBuf,
    w1: PathBuf,
    w2: PathBuf,
    template: PathBuf,
}

impl Hosts {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let w1 = root.join("w1");
        let w2 = root.join("w2");
        for d in [&w1, &w2] {
            std::fs::create_dir_all(d).unwrap();
        }
        for host in 1..=2 {
            for d in ["home", "koto-home", "cache"] {
                std::fs::create_dir_all(root.join(format!("{d}{host}"))).unwrap();
            }
            // A koto root takes its backend from its own config, not the
            // environment; the bucket's settings still come from it.
            std::fs::write(
                root.join(format!("koto-home{host}/config.toml")),
                "[session]\nbackend = \"cloud\"\n",
            )
            .unwrap();
        }
        let template = root.join("work.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let hosts = Hosts {
            _tmp: tmp,
            s3: FakeS3::start(BUCKET),
            root,
            w1,
            w2,
            template,
        };
        hosts.assert_scratch_store();
        hosts
    }

    /// koto resolves each host's root to its scratch `KOTO_HOME`, inside
    /// this test's temporary directory, so no command reaches a real store.
    fn assert_scratch_store(&self) {
        for host in 1..=2 {
            let out = self
                .cmd(host, &self.w1)
                .args(["session", "dir"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            let resolved = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
            assert_eq!(resolved, self.root.join(format!("koto-home{host}")));
        }
    }

    fn sessions(&self, host: u8) -> PathBuf {
        self.root.join(format!("koto-home{host}")).join("sessions")
    }

    /// A command on `host` in workspace `dir`, under the cloud backend on
    /// the stand-in.
    fn cmd(&self, host: u8, dir: &Path) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(dir)
            .env("HOME", self.root.join(format!("home{host}")))
            .env("KOTO_HOME", self.root.join(format!("koto-home{host}")))
            .env("XDG_CACHE_HOME", self.root.join(format!("cache{host}")))
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

    fn run(&self, host: u8, dir: &Path, args: &[&str]) -> std::process::Output {
        self.cmd(host, dir).args(args).output().unwrap()
    }

    fn ok(&self, host: u8, dir: &Path, args: &[&str]) -> Value {
        let out = self.run(host, dir, args);
        assert!(
            out.status.success(),
            "host {host}: koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    }

    fn fails(&self, host: u8, dir: &Path, args: &[&str], code: i32) -> Value {
        let out = self.run(host, dir, args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "host {host}: koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    }

    /// Create `p` on host 1 in W1, holding [`PRED_KEYS`].
    fn predecessor(&self) {
        self.ok(
            1,
            &self.w1,
            &["init", "p", "--template", self.template.to_str().unwrap()],
        );
        for (key, value) in PRED_KEYS {
            let out = self
                .cmd(1, &self.w1)
                .args(["context", "add", "p", key])
                .write_stdin(value)
                .output()
                .unwrap();
            assert!(out.status.success(), "context add {key}: {out:?}");
        }
    }

    /// `koto init <succ> --continues p --continues-from W1 --supersede`
    /// with every carried prefix.
    fn supersede_args(&self, succ: &str) -> Vec<String> {
        let mut args: Vec<String> = [
            "init",
            succ,
            "--template",
            self.template.to_str().unwrap(),
            "--continues",
            "p",
            "--continues-from",
            self.w1.to_str().unwrap(),
            "--supersede",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for prefix in CARRY {
            args.push("--carry".to_string());
            args.push(prefix.to_string());
        }
        args
    }

    fn supersede(&self, host: u8, succ: &str) -> std::process::Output {
        let args = self.supersede_args(succ);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(host, &self.w2, &args)
    }

    /// Every object in the bucket with its bytes.
    fn bucket(&self) -> BTreeMap<String, Vec<u8>> {
        self.s3
            .keys()
            .into_iter()
            .map(|k| {
                let bytes = self.s3.object(&k).unwrap_or_default();
                (k, bytes)
            })
            .collect()
    }

    fn pred_state_key(&self) -> String {
        format!("{}/p/koto-p.state.jsonl", prefix_of(&self.w1))
    }

    /// The `session_superseded` events in the bucket's copy of `p`.
    fn fences(&self) -> Vec<Value> {
        let bytes = self.s3.object(&self.pred_state_key()).unwrap();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["type"] == "session_superseded")
            .collect()
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

fn exit_of(out: &std::process::Output) -> Option<i32> {
    out.status.code()
}

/// A successor on another host, in another workspace, fences the
/// predecessor in W1's prefix with one conditional PUT of its state,
/// creates itself in W2's prefix, carries every carried key there and
/// writes the marker. A re-run changes nothing.
#[test]
fn a_cross_prefix_supersede_fences_creates_and_carries() {
    let h = Hosts::new();
    h.predecessor();
    let w1 = prefix_of(&h.w1);
    let w2 = prefix_of(&h.w2);
    let before = h.bucket();
    h.s3.clear_requests();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    let body = printed_json(&out.stdout);
    assert_eq!(body["outcome"], "superseded");
    assert_eq!(body["superseded"], "p");
    assert_eq!(body["forward"], "s");
    assert_eq!(body["carried"], 5);

    // The fence: one conditional PUT of p's state file under W1.
    let state_puts: Vec<_> =
        h.s3.requests()
            .into_iter()
            .filter(|r| r.method == "PUT" && r.key == h.pred_state_key())
            .collect();
    assert_eq!(state_puts.len(), 1, "{state_puts:?}");
    let fences = h.fences();
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0]["payload"]["by"], "s");
    let s_state = h.s3.object(&format!("{w2}/s/koto-s.state.jsonl")).unwrap();
    let s_header: Value =
        serde_json::from_slice(s_state.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(
        fences[0]["payload"]["by_session_id"],
        s_header["session_id"]
    );

    // Under W1 only p's state file changed, besides the back-reference
    // `--continues` puts beside it.
    let after = h.bucket();
    for (key, bytes) in &after {
        if !key.starts_with(&format!("{w1}/")) || key == &h.pred_state_key() {
            continue;
        }
        if key.starts_with(&format!("{w1}/p/ctx/koto/continued-by/")) {
            continue;
        }
        assert_eq!(before.get(key), Some(bytes), "{key} changed under W1");
    }

    // Under W2: every carried key, nothing else of p's, and the marker.
    for (key, value) in PRED_KEYS {
        let object = h.s3.object(&format!("{w2}/s/ctx/{key}"));
        if key == "coord/x" {
            assert!(object.is_none(), "coord/x was carried");
        } else {
            assert_eq!(object.as_deref(), Some(value.as_bytes()), "{key}");
        }
    }
    assert!(h
        .s3
        .object(&format!("{w2}/s/ctx/koto/superseded-carry-complete"))
        .is_some());

    // A re-run changes nothing.
    let again = h.bucket();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    let body = printed_json(&out.stdout);
    assert_eq!(body["outcome"], "unchanged");
    assert_eq!(body["carried"], 0);
    assert_eq!(h.fences().len(), 1);
    assert_eq!(
        again.get(&h.pred_state_key()),
        h.bucket().get(&h.pred_state_key())
    );
}

/// A conditional push the bucket refuses (412), and a bucket that doesn't
/// implement conditional writes (501), exit 3 `fence_unavailable` with
/// nothing written in either prefix or in the successor's local store.
#[test]
fn a_refused_conditional_push_exits_3_with_nothing_written() {
    let h = Hosts::new();
    h.predecessor();
    for status in [412, 501] {
        let before = h.bucket();
        h.s3.fail("PUT", &h.pred_state_key(), status);
        let out = h.supersede(2, "s");
        assert_eq!(exit_of(&out), Some(3), "{status}: {out:?}");
        assert_eq!(printed_json(&out.stdout)["code"], "fence_unavailable");
        h.s3.clear_faults();
        assert_eq!(h.bucket(), before, "{status}: the bucket changed");
        assert!(!h.sessions(2).join("s").exists(), "{status}: s was created");
        assert!(h.fences().is_empty());
    }
}

/// Two successors racing for one predecessor: one fences, the other reads
/// that fence and refuses `superseded_by`, and the bucket's copy holds one
/// fence.
#[test]
fn two_racing_successors_yield_one_fence_and_one_refusal() {
    let h = Hosts::new();
    h.predecessor();
    let w2 = prefix_of(&h.w2);
    let args_s = h.supersede_args("s");
    let args_t = h.supersede_args("t");
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| h.cmd(1, &h.w2).args(&args_s).output().unwrap());
        let b = scope.spawn(|| h.cmd(2, &h.w2).args(&args_t).output().unwrap());
        (a.join().unwrap(), b.join().unwrap())
    });
    let codes = [exit_of(&a), exit_of(&b)];
    assert!(
        codes.contains(&Some(0)) && codes.contains(&Some(2)),
        "{a:?}\n{b:?}"
    );
    let (winner, loser) = if exit_of(&a) == Some(0) {
        ("s", &b)
    } else {
        ("t", &a)
    };
    let refusal = printed_json(&loser.stdout);
    assert_eq!(refusal["code"], "superseded_by", "{refusal}");
    assert_eq!(refusal["superseded_by"], winner);
    let fences = h.fences();
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0]["payload"]["by"], winner);
    let other = if winner == "s" { "t" } else { "s" };
    assert!(h.s3.keys_under(&format!("{w2}/{other}/")).is_empty());
}

/// A carry that fails after the fence leaves the predecessor fenced; the
/// same command run again creates nothing new, carries every key once, and
/// a third run carries nothing.
#[test]
fn a_re_run_after_the_fence_finishes_the_carry_once() {
    let h = Hosts::new();
    h.predecessor();
    let w1 = prefix_of(&h.w1);
    let w2 = prefix_of(&h.w2);
    h.s3.fail("GET", &format!("{w1}/p/ctx/record/a"), 500);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(3), "{out:?}");
    assert_eq!(h.fences().len(), 1);
    assert!(h
        .s3
        .object(&format!("{w2}/s/ctx/koto/superseded-carry-complete"))
        .is_none());
    h.s3.clear_faults();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    let body = printed_json(&out.stdout);
    assert_eq!(body["outcome"], "unchanged");
    assert_eq!(body["carried"], 5);
    assert!(h
        .s3
        .object(&format!("{w2}/s/ctx/koto/superseded-carry-complete"))
        .is_some());
    assert_eq!(
        h.s3.object(&format!("{w2}/s/ctx/record/a")).as_deref(),
        Some(&b"alpha"[..])
    );

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 0);
    assert_eq!(h.fences().len(), 1);
}

/// After the fence every write to the predecessor is refused
/// `session_superseded`, naming the newest member: from W1 on the host
/// that created it (whose copy predates the fence), and from W1 and W2 on
/// a host that shares one store between them. Reads keep working.
#[test]
fn writes_to_the_predecessor_after_the_fence_are_refused_from_either_workspace() {
    // Another host fenced it: host 1's copy in W1 predates the fence.
    let h = Hosts::new();
    h.predecessor();
    let w1 = prefix_of(&h.w1);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    let before = h.bucket();
    let refused = h.fails(
        1,
        &h.w1,
        &[
            "context",
            "add",
            "p",
            "record/b",
            "--from-file",
            "/dev/null",
        ],
        2,
    );
    assert_eq!(refused["code"], "session_superseded", "{refused}");
    assert!(refused.to_string().contains("'s'"), "{refused}");
    assert!(h.s3.object(&format!("{w1}/p/ctx/record/b")).is_none());
    let refused = h.fails(
        1,
        &h.w1,
        &["next", "p", "--with-data", r#"{"marker":"done"}"#],
        2,
    );
    assert_eq!(refused["error"]["code"], "session_superseded", "{refused}");
    assert_eq!(h.bucket(), before, "a refused write reached the bucket");
    let out = h.run(1, &h.w1, &["context", "get", "p", "record/a"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, b"alpha");

    // The same host fenced it from W2: both workspaces share the copy.
    let h = Hosts::new();
    h.predecessor();
    let out = h.supersede(1, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    for dir in [&h.w1, &h.w2] {
        let refused = h.fails(
            1,
            dir,
            &[
                "context",
                "add",
                "p",
                "record/b",
                "--from-file",
                "/dev/null",
            ],
            2,
        );
        assert_eq!(refused["code"], "session_superseded", "{refused}");
        assert_eq!(refused["newest"], "s", "{refused}");
        let refused = h.fails(
            1,
            dir,
            &["next", "p", "--with-data", r#"{"marker":"done"}"#],
            2,
        );
        assert_eq!(refused["newest"], "s", "{refused}");
        let out = h.run(1, dir, &["context", "get", "p", "record/a"]);
        assert!(out.status.success(), "{out:?}");
    }
}

/// A read of the fenced predecessor whose local copy of a key is newer
/// than the bucket's still answers from the local copy, as reads of a
/// fenced session do, and pushes nothing: the kept copy would be a write
/// to a superseded session.
#[test]
fn a_read_of_the_fenced_predecessor_keeps_a_newer_local_key_without_pushing() {
    let h = Hosts::new();
    h.predecessor();
    let w1 = prefix_of(&h.w1);
    let out = h.supersede(1, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");

    // The bucket's copy of `record/a` becomes an older, different version.
    let manifest_key = format!("{w1}/p/ctx/manifest.json");
    let mut manifest: Value = serde_json::from_slice(&h.s3.object(&manifest_key).unwrap()).unwrap();
    let hash = manifest["keys"]["record/a"]["hash"]
        .as_str()
        .unwrap()
        .to_string();
    let last = if hash.ends_with('0') { "1" } else { "0" };
    manifest["keys"]["record/a"]["hash"] =
        Value::from(format!("{}{last}", &hash[..hash.len() - 1]));
    manifest["keys"]["record/a"]["created_at"] = Value::from("2026-01-01T00:00:00.000Z");
    h.s3.put(&manifest_key, manifest.to_string().as_bytes());
    let before = h.bucket();

    let out = h.run(1, &h.w1, &["context", "get", "p", "record/a"]);
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"alpha");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("superseded"),
        "{out:?}"
    );
    assert_eq!(
        h.bucket(),
        before,
        "a read of a fenced session wrote the bucket"
    );
}

/// A fence in the bucket's copy that names an invalid successor, and a
/// manifest that names an invalid key, are refused before a key is built
/// from either: nothing is written, and the bad key is never fetched.
#[test]
fn names_from_the_bucket_that_fail_validation_are_refused() {
    let h = Hosts::new();
    h.predecessor();
    let w1 = prefix_of(&h.w1);
    let key = h.pred_state_key();
    let original = h.s3.object(&key).unwrap();
    let text = String::from_utf8(original.clone()).unwrap();
    let last_seq = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|e| e["seq"].as_u64())
        .max()
        .unwrap();
    let forged = format!(
        "{text}{}\n",
        serde_json::json!({
            "seq": last_seq + 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "session_superseded",
            "payload": {
                "by": "../../evil",
                "by_session_id": "x",
                "actor": {},
                "at": "2026-01-01T00:00:00Z",
            },
        })
    );
    h.s3.put(&key, forged.as_bytes());
    let before = h.bucket();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(2), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["code"], "invalid_fence");
    assert_eq!(h.bucket(), before);
    assert!(!h.sessions(2).join("s").exists());
    assert!(h.s3.requests().iter().all(|r| !r.key.contains("evil")));

    // A manifest naming a key `koto context add` would refuse.
    h.s3.put(&key, &original);
    let manifest_key = format!("{w1}/p/ctx/manifest.json");
    let mut manifest: Value = serde_json::from_slice(&h.s3.object(&manifest_key).unwrap()).unwrap();
    let meta = manifest["keys"]["record/a"].clone();
    manifest["keys"]["record/../escape"] = meta;
    h.s3.put(&manifest_key, manifest.to_string().as_bytes());
    h.s3.clear_requests();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(3), "{out:?}");
    assert!(h.s3.requests().iter().all(|r| !r.key.contains("escape")));
}
