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
        self.predecessor_with(&PRED_KEYS);
    }

    /// Create `p` on host 1 in W1, holding `keys`.
    fn predecessor_with(&self, keys: &[(&str, &str)]) {
        self.ok(
            1,
            &self.w1,
            &["init", "p", "--template", self.template.to_str().unwrap()],
        );
        self.add_to_predecessor(keys);
    }

    /// Write `keys` in `p` on host 1 in W1, each pushed to the bucket.
    fn add_to_predecessor(&self, keys: &[(&str, &str)]) {
        for (key, value) in keys {
            let out = self
                .cmd(1, &self.w1)
                .args(["context", "add", "p", key])
                .write_stdin(*value)
                .output()
                .unwrap();
            assert!(out.status.success(), "context add {key}: {out:?}");
        }
    }

    /// The bucket key of `p`'s context key `key`.
    fn pred_ctx(&self, key: &str) -> String {
        format!("{}/p/ctx/{key}", prefix_of(&self.w1))
    }

    /// The bucket key of `s`'s context key `key`.
    fn succ_ctx(&self, key: &str) -> String {
        format!("{}/s/ctx/{key}", prefix_of(&self.w2))
    }

    /// The GETs recorded under `p`'s context, its manifest left out, in
    /// arrival order.
    fn carried_gets(&self) -> Vec<fake_s3::Request> {
        let ctx = self.pred_ctx("");
        let manifest = self.pred_ctx("manifest.json");
        self.s3
            .requests()
            .into_iter()
            .filter(|r| r.method == "GET" && r.key.starts_with(&ctx) && r.key != manifest)
            .collect()
    }

    /// How many GETs of `p`'s context key `key` were recorded.
    fn gets_of(&self, key: &str) -> usize {
        let key = self.pred_ctx(key);
        self.carried_gets().iter().filter(|r| r.key == key).count()
    }

    /// Whether the bucket holds the carry's completion marker in `s`.
    fn has_marker(&self) -> bool {
        self.s3
            .object(&self.succ_ctx("koto/superseded-carry-complete"))
            .is_some()
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

    // The carry goes up as one push: from the first carried key on, s's
    // manifest is put once, after every carried key and the marker. A push
    // per key would put it after each one.
    let requests = h.s3.requests();
    let carried_key = |key: &str| {
        PRED_KEYS
            .iter()
            .any(|(k, _)| key == format!("{w2}/s/ctx/{k}"))
            || key == format!("{w2}/s/ctx/koto/superseded-carry-complete")
    };
    let first = requests
        .iter()
        .position(|r| r.method == "PUT" && carried_key(&r.key))
        .expect("no carried key was put");
    let manifest_key = format!("{w2}/s/ctx/manifest.json");
    let manifest_puts: Vec<usize> = requests
        .iter()
        .enumerate()
        .skip(first)
        .filter(|(_, r)| r.method == "PUT" && r.key == manifest_key)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(manifest_puts.len(), 1, "{requests:?}");
    let last_key = requests
        .iter()
        .rposition(|r| r.method == "PUT" && carried_key(&r.key))
        .unwrap();
    assert!(last_key < manifest_puts[0], "{requests:?}");

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

/// The carried keys of [`PRED_KEYS`], `coord/x` left out.
fn carried_pred_keys() -> impl Iterator<Item = (&'static str, &'static str)> {
    PRED_KEYS.into_iter().filter(|(k, _)| *k != "coord/x")
}

/// A carry whose read of one key keeps failing after the fence leaves the
/// predecessor fenced and pushes the keys it did read, without the marker,
/// and refuses `carry_incomplete` on exit 1. The same command run again
/// creates nothing new, reads only the key that failed (no GET of a key the
/// first run carried), and carries just that one, so the two runs' `carried`
/// add up to every carried key; a third run carries nothing.
#[test]
fn a_re_run_after_the_fence_finishes_the_carry_once() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail("GET", &h.pred_ctx("record/a"), 500);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(1), "{out:?}");
    let refused = printed_json(&out.stdout);
    assert_eq!(refused["code"], "carry_incomplete", "{refused}");
    assert_eq!(refused["failure"], "unreachable", "{refused}");
    let first = refused["carried"].as_u64().unwrap();
    assert_eq!(h.fences().len(), 1);
    assert!(!h.has_marker());
    for (key, value) in carried_pred_keys() {
        let object = h.s3.object(&h.succ_ctx(key));
        if key == "record/a" {
            assert!(object.is_none(), "record/a reached the successor");
        } else {
            assert_eq!(object.as_deref(), Some(value.as_bytes()), "{key}");
        }
    }
    h.s3.clear_faults();
    h.s3.clear_requests();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    let body = printed_json(&out.stdout);
    assert_eq!(body["outcome"], "unchanged");
    assert_eq!(body["carried"], 1);
    assert_eq!(
        first + body["carried"].as_u64().unwrap(),
        carried_pred_keys().count() as u64
    );
    assert!(h.has_marker());
    assert_eq!(
        h.s3.object(&h.succ_ctx("record/a")).as_deref(),
        Some(&b"alpha"[..])
    );
    let read: Vec<String> = h.carried_gets().into_iter().map(|r| r.key).collect();
    assert_eq!(read, vec![h.pred_ctx("record/a")]);

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 0);
    assert_eq!(h.fences().len(), 1);
}

/// A key the successor's bucket manifest lists but this host's local store
/// doesn't hold is read and written again, not skipped: only the local
/// store says what is held. A carry that stopped pushed four keys; this
/// host's local store then loses them, as a host without that copy would
/// lack them, and the re-run reads all five.
#[test]
fn a_key_only_the_successor_s_bucket_manifest_lists_is_fetched() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail("GET", &h.pred_ctx("record/a"), 500);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(1), "{out:?}");
    h.s3.clear_faults();
    let pushed: Value =
        serde_json::from_slice(&h.s3.object(&h.succ_ctx("manifest.json")).unwrap()).unwrap();
    let ctx = h.sessions(2).join("s").join("ctx");
    let local_manifest = ctx.join("manifest.json");
    let mut local: Value =
        serde_json::from_slice(&std::fs::read(&local_manifest).unwrap()).unwrap();
    for (key, _) in carried_pred_keys().filter(|(k, _)| *k != "record/a") {
        assert!(pushed["keys"][key].is_object(), "{key}: {pushed}");
        local["keys"].as_object_mut().unwrap().remove(key).unwrap();
        std::fs::remove_file(ctx.join(key)).unwrap();
    }
    std::fs::write(&local_manifest, local.to_string()).unwrap();
    h.s3.clear_requests();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 5);
    for (key, value) in carried_pred_keys() {
        assert_eq!(h.gets_of(key), 1, "{key}");
        assert_eq!(
            h.s3.object(&h.succ_ctx(key)).as_deref(),
            Some(value.as_bytes())
        );
    }
    assert!(h.has_marker());
}

/// The carry's GETs run in parallel. Every carried key's GET is held for
/// `HOLD` before its reply goes out, so a serial carry could send each GET
/// only after the previous one's reply, at least `HOLD` later. Asserting
/// that a full chunk of GETs arrived within `HOLD` of the first proves they
/// were in flight together without timing the whole command, so a slow
/// machine only makes the margin smaller, never the assertion flaky in the
/// other direction.
#[test]
fn a_carry_reads_its_keys_in_parallel() {
    const CHUNK: usize = 8;
    const HOLD: std::time::Duration = std::time::Duration::from_secs(1);
    let h = Hosts::new();
    h.predecessor();
    let extra: Vec<(String, String)> = (0..CHUNK)
        .map(|i| (format!("record/p/{i}"), format!("value {i}")))
        .collect();
    let extra: Vec<(&str, &str)> = extra
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    h.add_to_predecessor(&extra);
    let carried: Vec<&str> = carried_pred_keys()
        .map(|(k, _)| k)
        .chain(extra.iter().map(|(k, _)| *k))
        .collect();
    for key in &carried {
        h.s3.hold("GET", &h.pred_ctx(key), HOLD);
    }
    h.s3.clear_requests();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], carried.len());
    let gets = h.carried_gets();
    assert_eq!(gets.len(), carried.len(), "{gets:?}");
    let first = gets[0].at;
    for get in &gets[..CHUNK] {
        assert!(
            get.at.duration_since(first) < HOLD,
            "{} arrived {:?} after the first GET",
            get.key,
            get.at.duration_since(first)
        );
    }
    for (key, value) in &extra {
        assert_eq!(
            h.s3.object(&h.succ_ctx(key)).as_deref(),
            Some(value.as_bytes())
        );
    }
}

/// A key whose GET fails one ask (two requests, given rust-s3's own retry)
/// is asked again and carried.
#[test]
fn a_key_whose_read_fails_once_is_still_carried() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail_times("GET", &h.pred_ctx("record/a"), 503, 2);
    h.s3.clear_requests();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 5);
    assert_eq!(h.gets_of("record/a"), 3);
    assert_eq!(
        h.s3.object(&h.succ_ctx("record/a")).as_deref(),
        Some(&b"alpha"[..])
    );
    assert!(h.has_marker());
}

/// A value whose bytes differ from the predecessor's manifest hash on both
/// reads fails the carry and is written nowhere in the successor, neither
/// in the bucket nor in the local store. One that differs only on the first
/// read, as an object caught mid-replace would, is read again and carried.
#[test]
fn a_value_that_does_not_match_its_manifest_hash_is_never_carried() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.put(&h.pred_ctx("record/a"), b"tampered");
    h.s3.clear_requests();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(3), "{out:?}");
    let refused = printed_json(&out.stdout);
    assert_eq!(refused["code"], "carry_incomplete", "{refused}");
    assert_eq!(refused["failure"], "invalid_source", "{refused}");
    assert_eq!(refused["retryable"], false, "{refused}");
    assert_eq!(refused["remaining"], 1, "{refused}");
    assert!(!error_text(&h, &refused).contains("tampered"), "{refused}");
    assert_eq!(h.gets_of("record/a"), 2);
    assert!(!h.has_marker());
    assert!(h.s3.object(&h.succ_ctx("record/a")).is_none());
    let local = h.sessions(2).join("s").join("ctx");
    assert!(
        local.join("taken_legs").exists(),
        "the keys read with record/a reached s locally"
    );
    assert!(!local.join("record").join("a").exists());

    let h = Hosts::new();
    h.predecessor();
    let key = h.pred_ctx("record/a");
    h.s3.put(&key, b"tampered");
    h.s3.put_after("GET", &key, &key, b"alpha");
    h.s3.clear_requests();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 5);
    assert_eq!(h.gets_of("record/a"), 2);
    assert_eq!(
        h.s3.object(&h.succ_ctx("record/a")).as_deref(),
        Some(&b"alpha"[..])
    );
}

/// A predecessor whose bucket holds no context manifest fails the carry
/// when its log records a key under a `--carry` prefix, since the carry
/// would otherwise seal a copy of nothing behind the marker; one whose log
/// records no such key carries nothing and finishes.
#[test]
fn a_missing_predecessor_manifest_fails_only_when_its_log_names_a_carried_key() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.remove(&h.pred_ctx("manifest.json"));
    h.s3.clear_requests();
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(3), "{out:?}");
    let refused = printed_json(&out.stdout);
    assert_eq!(refused["code"], "carry_incomplete", "{refused}");
    assert_eq!(refused["failure"], "invalid_source", "{refused}");
    assert_no_counts(&refused);
    assert_eq!(h.fences().len(), 1);
    assert!(!h.has_marker());
    assert!(h.carried_gets().is_empty());

    let h = Hosts::new();
    h.predecessor_with(&[("coord/x", "coordinator only")]);
    h.s3.remove(&h.pred_ctx("manifest.json"));
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 0);
    assert!(h.has_marker());
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
    // This host keeps no synced hash for the key, as a koto from before it
    // kept one: the copies are ordered by their write times, and the local
    // one is the later. With the hash it pushed on record, the bucket's
    // different copy would read as a later write and be pulled.
    std::fs::remove_file(h.sessions(1).join("p").join("cloud-ctx-synced.json")).unwrap();
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
    let refused = printed_json(&out.stdout);
    assert_eq!(refused["failure"], "invalid_source", "{refused}");
    assert!(!refused.to_string().contains("escape"), "{refused}");
    assert!(h.s3.requests().iter().all(|r| !r.key.contains("escape")));
}

/// A `carry_incomplete` refusal whose plan itself failed: `keys`, `held`
/// and `remaining` are left out, and nothing was carried.
fn assert_no_counts(refused: &Value) {
    for field in ["keys", "held", "remaining"] {
        assert!(refused.get(field).is_none(), "{field}: {refused}");
    }
    assert_eq!(refused["carried"], 0, "{refused}");
}

/// The `error` text of a refusal, checked to hold none of the stand-in's
/// own error text and not the bucket key the failed read asked for.
fn error_text(h: &Hosts, refused: &Value) -> String {
    let error = refused["error"].as_str().unwrap().to_string();
    for shown in [
        "injected failure",
        "InternalError",
        "<Error>",
        "HTTP",
        "127.0.0.1",
    ] {
        assert!(!error.contains(shown), "{shown}: {error}");
    }
    assert!(!error.contains(&prefix_of(&h.w1)), "{error}");
    error
}

/// A key whose read keeps failing with a 503 stops the carry: the refusal
/// is `carry_incomplete`, `throttled` and retryable, on exit 1, with the
/// counts of the keys the successor holds and the one that remains. Its
/// `error` names the key and none of the bucket's own text.
#[test]
fn a_transient_read_failure_refuses_carry_incomplete_on_exit_1_with_its_counts() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail("GET", &h.pred_ctx("record/a"), 503);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(1), "{out:?}");
    let refused = printed_json(&out.stdout);
    for (field, value) in [
        ("command", Value::from("init")),
        ("code", Value::from("carry_incomplete")),
        ("superseded", Value::from("p")),
        ("name", Value::from("s")),
        ("failure", Value::from("throttled")),
        ("retryable", Value::from(true)),
        ("keys", Value::from(5)),
        ("held", Value::from(4)),
        ("remaining", Value::from(1)),
        ("carried", Value::from(4)),
    ] {
        assert_eq!(refused[field], value, "{field}: {refused}");
    }
    let error = error_text(&h, &refused);
    assert!(error.contains("'record/a'"), "{error}");
    assert!(error.contains("4 of 5 keys carried, 1 remain"), "{error}");
    assert!(!h.has_marker());
}

/// A 403 on a carried key's read is `denied`, not retryable, on exit 3.
#[test]
fn a_denied_read_refuses_carry_incomplete_on_exit_3() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail("GET", &h.pred_ctx("record/a"), 403);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(3), "{out:?}");
    let refused = printed_json(&out.stdout);
    assert_eq!(refused["code"], "carry_incomplete", "{refused}");
    assert_eq!(refused["failure"], "denied", "{refused}");
    assert_eq!(refused["retryable"], false, "{refused}");
    assert_eq!(refused["remaining"], 1, "{refused}");
    error_text(&h, &refused);
}

/// When every key and the marker are written but the push fails, the
/// refusal is `push_failed`, retryable, on exit 1, with nothing remaining.
/// The re-run pushes the open batch and returns at the marker.
#[test]
fn a_failed_final_push_refuses_push_failed_on_exit_1() {
    let h = Hosts::new();
    h.predecessor();
    h.s3.fail("PUT", &h.succ_ctx("koto/superseded-carry-complete"), 500);
    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(1), "{out:?}");
    let refused = printed_json(&out.stdout);
    for (field, value) in [
        ("code", Value::from("carry_incomplete")),
        ("failure", Value::from("push_failed")),
        ("retryable", Value::from(true)),
        ("keys", Value::from(5)),
        ("held", Value::from(5)),
        ("remaining", Value::from(0)),
        ("carried", Value::from(5)),
    ] {
        assert_eq!(refused[field], value, "{field}: {refused}");
    }
    error_text(&h, &refused);
    assert!(!h.has_marker());
    h.s3.clear_faults();

    let out = h.supersede(2, "s");
    assert_eq!(exit_of(&out), Some(0), "{out:?}");
    assert_eq!(printed_json(&out.stdout)["carried"], 0);
    assert!(h.has_marker());
}

/// A manifest key holding control characters, and one far longer than a
/// key may be, each fail the carry as `invalid_source` on exit 3 with no
/// counts, and neither reaches the refusal.
#[test]
fn a_manifest_key_the_carry_refuses_never_reaches_the_refusal() {
    let h = Hosts::new();
    h.predecessor();
    let manifest_key = h.pred_ctx("manifest.json");
    let original: Value = serde_json::from_slice(&h.s3.object(&manifest_key).unwrap()).unwrap();
    let long = format!("record/{}", "L".repeat(400));
    for (bad, shown) in [
        ("record/\u{1b}[31mEVIL\u{7}\r\nX", "EVIL"),
        (long.as_str(), "LLLLLLLL"),
    ] {
        let mut manifest = original.clone();
        let meta = manifest["keys"]["record/a"].clone();
        manifest["keys"][bad] = meta;
        h.s3.put(&manifest_key, manifest.to_string().as_bytes());
        let out = h.supersede(2, "s");
        assert_eq!(exit_of(&out), Some(3), "{out:?}");
        let refused = printed_json(&out.stdout);
        assert_eq!(refused["code"], "carry_incomplete", "{refused}");
        assert_eq!(refused["failure"], "invalid_source", "{refused}");
        assert_no_counts(&refused);
        let body = refused.to_string();
        assert!(!body.contains(shown), "{body}");
        let error = error_text(&h, &refused);
        assert!(!error.chars().any(char::is_control), "{error:?}");
    }
    assert!(!h.has_marker());
}
