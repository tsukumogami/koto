//! The continues relation under the cloud backend, through the real binary:
//! the back-reference a successor writes beside its predecessor, and the
//! bucket walk every removal makes, so a host that never pulled a later
//! session keeps the same predecessors and names the same keepers.
//!
//! Each test runs up to two hosts over one in-process stand-in bucket
//! (`support/fake_s3.rs`, which pages listings at 1000 keys and records
//! every request): a scratch HOME and cache per host, so each has its own
//! local store, and up to two workspace directories, W1 and W2, so each has
//! its own bucket prefix. The cloud backend is set by environment on each
//! command, which `support/koto_command.rs` builds with every inherited
//! `KOTO_SESSION_*` removed first, so no command reaches any bucket but the
//! stand-in.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_s3::FakeS3;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `work` waits for a marker; `done` is the success terminal.
const WORK: &str = r#"---
name: continues-cloud
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

const BUCKET: &str = "koto-continues-cloud";

/// Wait out the one-second retention most commands run with.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

struct Hosts {
    _tmp: TempDir,
    s3: FakeS3,
    root: PathBuf,
    w1: PathBuf,
    w2: PathBuf,
    template: PathBuf,
    keep: std::cell::Cell<&'static str>,
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
            for d in ["home", "cache"] {
                std::fs::create_dir_all(root.join(format!("{d}{host}"))).unwrap();
            }
        }
        let template = root.join("work.md");
        std::fs::write(&template, WORK).unwrap();
        Hosts {
            _tmp: tmp,
            s3: FakeS3::start(BUCKET),
            root,
            w1,
            w2,
            template,
            keep: std::cell::Cell::new("1s"),
        }
    }

    fn sessions(&self, host: u8) -> PathBuf {
        self.root.join(format!("home{host}")).join(".koto/sessions")
    }

    /// A command on `host` in workspace `dir`, under the cloud backend on
    /// the stand-in, with no anchor or agent session.
    fn cmd(&self, host: u8, dir: &Path, cloud: bool) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(dir)
            .env("HOME", self.root.join(format!("home{host}")))
            .env("XDG_CACHE_HOME", self.root.join(format!("cache{host}")))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env("KOTO_RETENTION_KEEP", self.keep.get())
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
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

    fn run(&self, host: u8, dir: &Path, args: &[&str]) -> std::process::Output {
        self.cmd(host, dir, true).args(args).output().unwrap()
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

    fn init(&self, host: u8, dir: &Path, name: &str, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(host, dir, &args);
    }

    /// Tick `name` to `done`, keeping it.
    fn done(&self, host: u8, dir: &Path, name: &str) {
        let v = self.ok(
            host,
            dir,
            &[
                "next",
                name,
                "--with-data",
                r#"{"marker":"done"}"#,
                "--no-cleanup",
            ],
        );
        assert_eq!(v["action"], "done", "{v:#}");
    }

    /// Take the bucket's copy of `name` onto `host`, whole.
    fn pull(&self, host: u8, dir: &Path, name: &str) {
        self.ok(host, dir, &["session", "resolve", name, "--keep", "remote"]);
        assert!(self.sessions(host).join(name).is_dir(), "{name} on {host}");
    }

    /// Compile the template into `host`'s own cache, as a host that runs
    /// the workflow has it.
    fn compile(&self, host: u8) {
        self.ok(
            host,
            &self.w1,
            &["template", "compile", self.template.to_str().unwrap()],
        );
    }

    fn header(&self, host: u8, name: &str) -> Value {
        let path = self
            .sessions(host)
            .join(name)
            .join(format!("koto-{name}.state.jsonl"));
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    /// The chain across workspaces: v1 and v2 in W1, v3 in W2 continuing
    /// v2, all on host 1; v1 and v2 finished, v3 running, the retention
    /// waited out.
    fn chain(&self) {
        let w1 = self.w1.to_str().unwrap().to_string();
        self.init(1, &self.w1, "v1", &[]);
        self.init(1, &self.w1, "v2", &["--continues", "v1"]);
        self.init(
            1,
            &self.w2,
            "v3",
            &["--continues", "v2", "--continues-from", &w1],
        );
        self.done(1, &self.w1, "v1");
        self.done(1, &self.w1, "v2");
        settle();
    }
}

fn prefix_of(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(digest)[..16].to_string()
}

fn pointer(pred_ws: &Path, pred: &str, holder_ws: &Path, holder: &str) -> String {
    format!(
        "{}/{pred}/ctx/koto/continued-by/{}/{holder}",
        prefix_of(pred_ws),
        prefix_of(holder_ws)
    )
}

fn relation_object(ws: &Path, holder: &str, pred: &str) -> String {
    format!("{}/{holder}/ctx/koto/links/continues/{pred}", prefix_of(ws))
}

/// What a command printed as JSON: the whole of stdout, or else the object
/// on its last line.
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

/// The entry for `name` in a prune's `kept` list.
fn kept<'a>(out: &'a Value, name: &str) -> &'a Value {
    out["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {out:#}"))
}

fn removed(out: &Value) -> BTreeSet<String> {
    out["removed"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| e["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A keeper as both hosts must name it: who, by what standing, through
/// which members, and, for a retention keeper, when it passes.
fn keeper_view(keepers: &Value) -> Vec<Value> {
    keepers
        .as_array()
        .unwrap_or_else(|| panic!("no keepers: {keepers:#}"))
        .iter()
        .map(|k| {
            json!({
                "keeper": k["keeper"],
                "standing": k["standing"],
                "path": k["path"],
                "retention_passes_at": k.get("retention_passes_at"),
                "pin_reason": k.get("pin").map(|p| p["reason"].clone()),
            })
        })
        .collect()
}

/// The requests the continues relation's check added to a command: every
/// listing of one session (its prefix, with a delimiter or not, or its
/// `ctx/koto/`) and every read under the sessions in `members`, less
/// `baseline`, the session listings the command made before the relation
/// existed (the pin guard's, one per session it checks, and `koto session
/// prune`'s remote-copy check, one per session it would remove).
fn relation_requests(s3: &FakeS3, baseline: usize, members: &[String]) -> usize {
    let requests = s3.requests();
    let session_listings = requests
        .iter()
        .filter(|r| {
            r.list_prefix.as_deref().is_some_and(|p| {
                p.ends_with("/ctx/koto/") || (p.ends_with('/') && p.matches('/').count() == 2)
            })
        })
        .count();
    let member_reads = requests
        .iter()
        .filter(|r| r.method == "GET" && members.iter().any(|m| r.key.starts_with(m.as_str())))
        .count();
    (session_listings + member_reads).saturating_sub(baseline)
}

#[test]
fn a_successor_writes_its_pointer_first_and_records_the_prefix() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    h.s3.clear_requests();
    h.init(1, &h.w1, "v2", &["--continues", "v1"]);

    let p = pointer(&h.w1, "v1", &h.w1, "v2");
    let rel = relation_object(&h.w1, "v2", "v1");
    let writes: Vec<String> =
        h.s3.requests()
            .into_iter()
            .filter(|r| r.method == "PUT")
            .map(|r| r.key)
            .collect();
    let at = |k: &str| writes.iter().position(|w| w == k);
    assert!(
        at(&p).unwrap() < at(&rel).expect("the relation was pushed"),
        "{writes:#?}"
    );

    // The pointer holds the holder's id and nothing else.
    let body: Value = serde_json::from_slice(&h.s3.object(&p).unwrap()).unwrap();
    let v2_id = h.header(1, "v2")["session_id"].clone();
    assert_eq!(body, json!({"v": 1, "holder_session_id": v2_id}));

    // The relation records the predecessor's name, id, workspace and prefix.
    let relation: Value = serde_json::from_slice(&h.s3.object(&rel).unwrap()).unwrap();
    assert_eq!(relation["session"], "v1");
    assert_eq!(relation["session_id"], h.header(1, "v1")["session_id"]);
    assert_eq!(relation["workspace"], h.w1.to_str().unwrap());
    assert_eq!(relation["prefix"], prefix_of(&h.w1));
    assert_eq!(relation["holder_session_id"], v2_id);
    let manifest: Value = serde_json::from_slice(
        &h.s3
            .object(&format!("{}/v2/ctx/manifest.json", prefix_of(&h.w1)))
            .unwrap(),
    )
    .unwrap();
    assert!(
        manifest["keys"]["koto/links/continues/v1"].is_object(),
        "{manifest:#}"
    );
    // The pointer is no context key: no manifest names it.
    let v1_manifest =
        h.s3.object(&format!("{}/v1/ctx/manifest.json", prefix_of(&h.w1)))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
    assert!(!v1_manifest.contains("continued-by"), "{v1_manifest}");
    // The mark records the confirmed pointer.
    let mark = std::fs::read_to_string(h.sessions(1).join("v2/.continues-pointer")).unwrap();
    assert_eq!(mark.trim(), p);
}

#[test]
fn an_unconfirmed_write_leaves_nothing_and_running_it_again_works() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    h.init(1, &h.w1, "v2", &[]);
    let p = pointer(&h.w1, "v1", &h.w1, "v2");
    h.s3.fail("PUT", &p, 403);
    h.fails(1, &h.w1, &["session", "link", "v2", "--continues", "v1"], 3);
    assert!(!h
        .sessions(1)
        .join("v2/ctx/koto/links/continues/v1")
        .exists());
    let left: Vec<String> =
        h.s3.keys()
            .into_iter()
            .filter(|k| k.contains("continued-by") || k.contains("links/continues"))
            .collect();
    assert!(left.is_empty(), "{left:?}");

    // At init too: the session opens, without the relation.
    let v3p = pointer(&h.w1, "v1", &h.w1, "v3");
    h.s3.fail("PUT", &v3p, 403);
    let out = h.run(
        1,
        &h.w1,
        &[
            "init",
            "v3",
            "--template",
            h.template.to_str().unwrap(),
            "--continues",
            "v1",
        ],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(h.s3.object(&relation_object(&h.w1, "v3", "v1")).is_none());

    h.s3.clear_faults();
    let v = h.ok(1, &h.w1, &["session", "link", "v2", "--continues", "v1"]);
    assert_eq!(v["written"], true, "{v:#}");
    assert!(h.s3.object(&p).is_some());
    assert!(h.s3.object(&relation_object(&h.w1, "v2", "v1")).is_some());
    assert!(h
        .sessions(1)
        .join("v2/ctx/koto/links/continues/v1")
        .exists());
}

#[test]
fn another_workspace_writes_its_pointer_under_the_predecessors_prefix() {
    let h = Hosts::new();
    let w1 = h.w1.to_str().unwrap().to_string();
    h.init(1, &h.w1, "v1", &[]);
    // Without write access to W1's prefix, nothing is written.
    let p = pointer(&h.w1, "v1", &h.w2, "c");
    h.s3.fail("PUT", &p, 403);
    h.init(1, &h.w2, "c", &[]);
    h.fails(
        1,
        &h.w2,
        &[
            "session",
            "link",
            "c",
            "--continues",
            "v1",
            "--continues-from",
            &w1,
        ],
        3,
    );
    assert!(h.s3.object(&relation_object(&h.w2, "c", "v1")).is_none());
    h.s3.clear_faults();
    h.ok(
        1,
        &h.w2,
        &[
            "session",
            "link",
            "c",
            "--continues",
            "v1",
            "--continues-from",
            &w1,
        ],
    );
    assert!(h.s3.object(&p).is_some());
    let relation: Value =
        serde_json::from_slice(&h.s3.object(&relation_object(&h.w2, "c", "v1")).unwrap()).unwrap();
    assert_eq!(relation["prefix"], prefix_of(&h.w1));

    // A prune in W1 keeps v1 and names the successor in W2.
    h.done(1, &h.w1, "v1");
    settle();
    let out = h.ok(
        1,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    let e = kept(&out, "v1");
    assert_eq!(e["kept_by_relation"], true, "{e:#}");
    assert_eq!(e["relation_keepers"][0]["keeper"], "c", "{e:#}");
    assert!(h.sessions(1).join("v1").is_dir());

    // A predecessor W1 doesn't hold is refused, writing nothing.
    h.fails(
        1,
        &h.w2,
        &[
            "session",
            "link",
            "c",
            "--continues",
            "nope",
            "--continues-from",
            &w1,
        ],
        2,
    );
}

#[test]
fn a_second_host_keeps_the_predecessor_and_names_the_same_keepers() {
    let h = Hosts::new();
    h.chain();
    h.compile(2);
    h.pull(2, &h.w1, "v1");
    assert!(!h.sessions(2).join("v3").exists());

    // v3 running.
    let on1 = h.ok(1, &h.w1, &["session", "prune", "--all"]);
    let k1 = keeper_view(&kept(&on1, "v1")["relation_keepers"]);
    assert_eq!(k1[0]["keeper"], "v3");
    assert_eq!(k1[0]["standing"], "alive");
    assert_eq!(k1[0]["path"], json!(["v2"]));
    h.s3.clear_requests();
    let preview = h.ok(2, &h.w1, &["session", "prune", "--all"]);
    assert_eq!(keeper_view(&kept(&preview, "v1")["relation_keepers"]), k1);
    // The walk from v1 on a host holding only v1: its listing, and three
    // requests for each of v2 and v3.
    let members = [
        format!("{}/v2/", prefix_of(&h.w1)),
        format!("{}/v3/", prefix_of(&h.w2)),
    ];
    let cost = relation_requests(&h.s3, 0, &members);
    assert!(cost <= 2 + 3 * 2, "{cost} requests: {:#?}", h.s3.requests());
    let applied = h.ok(
        2,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    assert_eq!(keeper_view(&kept(&applied, "v1")["relation_keepers"]), k1);
    assert!(h.sessions(2).join("v1").is_dir());

    // v3 pinned and finished: a pin.
    h.ok(
        1,
        &h.w2,
        &["session", "pin", "v3", "--reason", "the record"],
    );
    h.done(1, &h.w2, "v3");
    settle();
    let on1 = h.ok(1, &h.w1, &["session", "prune", "--all"]);
    let on2 = h.ok(
        2,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    let (k1, k2) = (
        keeper_view(&kept(&on1, "v1")["relation_keepers"]),
        keeper_view(&kept(&on2, "v1")["relation_keepers"]),
    );
    assert_eq!(k2[0]["standing"], "pin", "{k2:#?}");
    assert_eq!(k2[0]["pin_reason"], "the record");
    assert_eq!(k1, k2);
}

#[test]
fn a_finished_member_inside_its_retention_keeps_on_both_hosts_until_the_same_instant() {
    let h = Hosts::new();
    h.keep.set("4s");
    h.chain();
    std::thread::sleep(Duration::from_millis(3000));
    h.compile(2);
    h.pull(2, &h.w1, "v1");
    // v1 and v2 are past their own retention; v3 has just finished.
    h.done(1, &h.w2, "v3");
    let on1 = h.ok(1, &h.w1, &["session", "prune", "--all"]);
    let on2 = h.ok(2, &h.w1, &["session", "prune", "--all"]);
    let (k1, k2) = (
        keeper_view(&kept(&on1, "v1")["relation_keepers"]),
        keeper_view(&kept(&on2, "v1")["relation_keepers"]),
    );
    assert_eq!(k2[0]["standing"], "retention", "{k2:#?}");
    assert!(k2[0]["retention_passes_at"].is_string());
    assert_eq!(k1, k2);

    // Once it passes, the second host removes v1.
    std::thread::sleep(Duration::from_millis(4300));
    let out = h.ok(
        2,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    assert!(removed(&out).contains("v1"), "{out:#}");
}

#[test]
fn a_member_whose_listing_fails_keeps_and_is_named_unchecked() {
    let h = Hosts::new();
    h.chain();
    h.compile(2);
    h.pull(2, &h.w1, "v1");
    h.pull(2, &h.w1, "v2");
    h.s3.fail("LIST", &format!("{}/v3/ctx/koto/", prefix_of(&h.w2)), 403);
    let out = h.ok(
        2,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    for name in ["v1", "v2"] {
        let e = kept(&out, name);
        assert_eq!(e["kept_by_relation"], true, "{e:#}");
        let sessions = &e["bucket_unchecked"]["sessions"];
        assert_eq!(
            sessions,
            &json!([{"name": "v3", "error": "unreachable"}]),
            "{e:#}"
        );
    }
    assert!(h.sessions(2).join("v1").is_dir() && h.sessions(2).join("v2").is_dir());
}

#[test]
fn cleanup_removes_despite_a_pointer_flood_and_a_pin_still_refuses() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    h.init(1, &h.w1, "p", &[]);
    h.done(1, &h.w1, "v1");
    h.done(1, &h.w1, "p");
    h.ok(1, &h.w1, &["session", "pin", "p", "--reason", "kept"]);
    for n in 0..70 {
        for name in ["v1", "p"] {
            h.s3.put(
                &format!(
                    "{}/{name}/ctx/koto/continued-by/{}/s{n}",
                    prefix_of(&h.w1),
                    prefix_of(&h.w2)
                ),
                b"{}",
            );
        }
    }
    settle();
    // A prune keeps the flooded session, unchecked.
    let out = h.ok(1, &h.w1, &["session", "prune", "--all"]);
    let e = kept(&out, "v1");
    assert_eq!(e["bucket_unchecked"]["error"], "too_many_pointers", "{e:#}");
    h.ok(1, &h.w1, &["session", "cleanup", "v1"]);
    assert!(!h.sessions(1).join("v1").exists());
    let refused = h.fails(1, &h.w1, &["session", "cleanup", "p"], 2);
    assert_eq!(refused["error"]["code"], "pinned", "{refused:#}");
}

#[test]
fn pointers_outlive_their_predecessor_and_go_with_their_holder() {
    let h = Hosts::new();
    h.init(1, &h.w1, "v1", &[]);
    h.init(1, &h.w1, "v2", &["--continues", "v1"]);
    let p = pointer(&h.w1, "v1", &h.w1, "v2");
    h.ok(1, &h.w1, &["session", "cleanup", "v1"]);
    assert!(
        h.s3.object(&p).is_some(),
        "the predecessor's removal left it"
    );
    // A new session under the removed predecessor's name is a mismatch on
    // a host that holds no successor.
    h.init(1, &h.w1, "v1", &[]);
    h.done(1, &h.w1, "v1");
    h.compile(2);
    h.pull(2, &h.w1, "v1");
    settle();
    let out = h.ok(2, &h.w1, &["session", "prune", "--all"]);
    assert_eq!(
        kept(&out, "v1")["relation_unreadable"],
        json!([{"holder": "v2", "reason": "mismatch"}])
    );
    h.ok(1, &h.w1, &["session", "cleanup", "v2"]);
    assert!(h.s3.object(&p).is_none(), "the holder took it back");
}

#[test]
fn a_relation_recorded_under_the_local_backend_waits_for_its_pointer() {
    let h = Hosts::new();
    let local = |args: &[&str]| {
        let out = h.cmd(1, &h.w1, false).args(args).output().unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    let t = h.template.to_str().unwrap().to_string();
    local(&["init", "v1", "--template", &t]);
    local(&["init", "v2", "--template", &t, "--continues", "v1"]);
    let p = pointer(&h.w1, "v1", &h.w1, "v2");
    let rel = relation_object(&h.w1, "v2", "v1");

    // Under the cloud backend, with the pointer refused, a push of v2 leaves
    // the relation out of the bucket.
    h.s3.fail("PUT", &p, 403);
    h.ok(1, &h.w1, &["session", "resolve", "v2", "--keep", "local"]);
    h.done(1, &h.w1, "v2");
    assert!(h.s3.object(&p).is_none());
    assert!(h.s3.object(&rel).is_none(), "{:?}", h.s3.keys());
    let manifest =
        h.s3.object(&format!("{}/v2/ctx/manifest.json", prefix_of(&h.w1)))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
    assert!(!manifest.contains("links/continues"), "{manifest}");

    // The next command writes the owed pointer, and the next push carries
    // the relation.
    h.s3.clear_faults();
    h.ok(1, &h.w1, &["status", "v2"]);
    assert!(h.s3.object(&p).is_some());
    h.ok(1, &h.w1, &["session", "resolve", "v2", "--keep", "local"]);
    assert!(h.s3.object(&rel).is_some(), "{:?}", h.s3.keys());
}

#[test]
fn a_prune_asks_the_bucket_little_whatever_its_size() {
    let h = Hosts::new();
    let all: Vec<String> = (0..100).map(|i| format!("S{i:03}")).collect();
    for name in &all {
        h.init(1, &h.w1, name, &[]);
        h.done(1, &h.w1, name);
    }
    for name in all.iter().skip(50) {
        h.ok(1, &h.w1, &["session", "pin", name, "--reason", "kept"]);
    }
    let pad = |n: usize| {
        for i in 0..n {
            h.s3.put(
                &format!("{}/pad{i:05}/koto-pad{i:05}.state.jsonl", prefix_of(&h.w1)),
                b"x",
            );
        }
    };
    // Every prune checks the fifty pins in the bucket: the pin guard's
    // listing of each, and the listing that asks whether it was migrated.
    let pins = 2 * 50;
    for size in [0usize, 10_000] {
        pad(size);
        // Nothing is about to be deleted: no request for relations.
        h.keep.set("1h");
        h.s3.clear_requests();
        h.ok(1, &h.w1, &["session", "prune", "--all"]);
        assert_eq!(relation_requests(&h.s3, pins, &[]), 0, "bucket +{size}");

        // Fifty sessions nothing names about to go: at most two each. The
        // preview checks each in the bucket for a pin and its remote copy.
        h.keep.set("1s");
        settle();
        h.s3.clear_requests();
        let preview = h.ok(1, &h.w1, &["session", "prune", "--all"]);
        assert_eq!(preview["remove"].as_array().unwrap().len(), 50);
        let cost = relation_requests(&h.s3, pins + 50 + 50, &[]);
        assert!(cost <= 100, "bucket +{size}: {cost}");
    }
    h.s3.clear_requests();
    let applied = h.ok(
        1,
        &h.w1,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    assert_eq!(removed(&applied).len(), 50, "{applied:#}");
    // The judgment's remote-copy check, then at each delete the remote-copy
    // check again and the pin guard.
    let cost = relation_requests(&h.s3, pins + 50 + 50 + 50, &[]);
    assert!(cost <= 100, "{cost}");
}

/// Whole-bucket scan: every relation the bucket holds, read by session id,
/// and each later member's standing from its own log, as the oracle the
/// walk's keepers must agree with.
fn oracle_keepers(h: &Hosts, pred_id: &str, keep: Duration) -> BTreeSet<String> {
    let mut by_pred: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut logs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for key in h.s3.keys() {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() == 7 && parts[2] == "ctx" && parts[4] == "links" && parts[5] == "continues" {
            let body: Value = serde_json::from_slice(&h.s3.object(&key).unwrap()).unwrap();
            by_pred
                .entry(body["session_id"].as_str().unwrap().to_string())
                .or_default()
                .push((
                    body["holder_session_id"].as_str().unwrap().to_string(),
                    format!("{}/{}", parts[0], parts[1]),
                ));
        }
        if parts.len() == 3 && parts[2].ends_with(".state.jsonl") {
            logs.insert(
                format!("{}/{}", parts[0], parts[1]),
                h.s3.object(&key).unwrap(),
            );
        }
    }
    let standing = |at: &str| -> bool {
        let Some(log) = logs.get(at) else {
            return false;
        };
        let text = String::from_utf8_lossy(log);
        let last = text
            .lines()
            .filter(|l| l.contains("\"type\":\"transitioned\""))
            .last()
            .map(|l| serde_json::from_str::<Value>(l).unwrap());
        match last {
            Some(e) if e["payload"]["to"] == "done" => {
                let ts = e["timestamp"].as_str().unwrap();
                let stop = humantime_like(ts);
                stop + keep > std::time::SystemTime::now()
            }
            _ => true,
        }
    };
    let mut out = BTreeSet::new();
    let mut queue = vec![pred_id.to_string()];
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop() {
        for (holder, at) in by_pred.get(&id).cloned().unwrap_or_default() {
            if !seen.insert(holder.clone()) {
                continue;
            }
            if standing(&at) {
                out.insert(at.rsplit('/').next().unwrap().to_string());
            }
            queue.push(holder);
        }
    }
    out
}

/// An RFC 3339 UTC instant with milliseconds, as koto writes them.
fn humantime_like(ts: &str) -> std::time::SystemTime {
    let n = |a: usize, b: usize| ts[a..b].parse::<u64>().unwrap();
    let (y, mo, d, hh, mm, ss) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
    let ms = if ts.len() >= 24 { n(20, 23) } else { 0 };
    // Days since the epoch by the civil-from-days inverse.
    let y = if mo <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    std::time::UNIX_EPOCH
        + Duration::from_secs(days as u64 * 86_400 + hh * 3600 + mm * 60 + ss)
        + Duration::from_millis(ms)
}

#[test]
fn the_walk_agrees_with_a_whole_bucket_scan_through_renames() {
    let h = Hosts::new();
    h.chain();
    // v2 is replaced under its name while v3 still continues it: the old
    // run moves aside to v2_r1, and its pointer beside v1 follows it.
    h.ok(
        1,
        &h.w1,
        &[
            "init",
            "v2",
            "--template",
            h.template.to_str().unwrap(),
            "--replace-terminal",
        ],
    );
    assert!(h.sessions(1).join("v2_r1").is_dir());
    assert!(h.s3.object(&pointer(&h.w1, "v1", &h.w1, "v2_r1")).is_some());
    assert!(h.s3.object(&pointer(&h.w1, "v1", &h.w1, "v2")).is_none());
    // v3's pointer moved with the predecessor it names.
    assert!(h.s3.object(&pointer(&h.w1, "v2_r1", &h.w2, "v3")).is_some());

    // v1 replaced too: it moves aside to v1_r1, with its pointers.
    h.ok(
        1,
        &h.w1,
        &[
            "init",
            "v1",
            "--template",
            h.template.to_str().unwrap(),
            "--replace-terminal",
        ],
    );
    assert!(h
        .s3
        .object(&pointer(&h.w1, "v1_r1", &h.w1, "v2_r1"))
        .is_some());

    h.compile(2);
    h.pull(2, &h.w1, "v1_r1");
    settle();
    let v1_id = h.header(1, "v1_r1")["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let oracle = oracle_keepers(&h, &v1_id, Duration::from_secs(1));
    assert_eq!(oracle, BTreeSet::from(["v3".to_string()]));
    let out = h.ok(2, &h.w1, &["session", "prune", "--all"]);
    let walked: BTreeSet<String> = kept(&out, "v1_r1")["relation_keepers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["keeper"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(walked, oracle);
}
