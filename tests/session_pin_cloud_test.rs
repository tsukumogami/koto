//! A pin under the cloud backend (DESIGN-session-pin.md, Decisions 2 and 3):
//! every host sharing the bucket keeps a pinned session, from the bucket's
//! pin object, and pin and unpin write only their own objects.
//!
//! Each test runs up to three hosts over one in-process stand-in bucket
//! (`support/fake_s3.rs`): one workspace directory, so every host has the
//! same bucket prefix, and a scratch HOME per host, so each has its own local
//! store. The cloud backend is set by environment on each command, which
//! `support/koto_command.rs` builds with every inherited `KOTO_SESSION_*`
//! removed first, so no command reaches any bucket but the stand-in.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use fake_s3::FakeS3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// `work` waits for a marker; `done` is the success terminal.
const WORK: &str = r#"---
name: pin-cloud
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

const BUCKET: &str = "koto-pin-cloud";
const REASON: &str = "record of the merged change";
const PIN: &str = "ctx/koto/links/pin";
const RELEASE: &str = "ctx/koto/pin-release";

/// Wait out the one-second retention every command runs with.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

struct Hosts {
    _tmp: TempDir,
    s3: FakeS3,
    root: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Hosts {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        for host in 1..=3 {
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
            work,
            template,
        }
    }

    fn home(&self, host: u8) -> PathBuf {
        self.root.join(format!("home{host}"))
    }

    /// Host `host`'s local store.
    fn sessions(&self, host: u8) -> PathBuf {
        self.home(host).join(".koto/sessions")
    }

    /// A command on host `host`, in the shared workspace, with a one-second
    /// retention, no anchor or agent session, and the cloud backend on the
    /// stand-in when `cloud`.
    fn cmd_on(&self, host: u8, cloud: bool) -> assert_cmd::Command {
        self.cmd_at(host, cloud, &self.work)
    }

    /// [`Hosts::cmd_on`] in workspace directory `dir`.
    fn cmd_at(&self, host: u8, cloud: bool, dir: &std::path::Path) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(dir)
            .env("HOME", self.home(host))
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
            .env("KOTO_RETENTION_KEEP", "1s")
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

    fn run(&self, host: u8, args: &[&str]) -> std::process::Output {
        self.cmd_on(host, true).args(args).output().unwrap()
    }

    fn ok(&self, host: u8, args: &[&str]) -> Value {
        let out = self.run(host, args);
        assert!(
            out.status.success(),
            "host {host}: koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    }

    /// Run `args` on `host` and require exit `code`; the JSON it printed.
    fn fails(&self, host: u8, args: &[&str], code: i32) -> Value {
        let out = self.run(host, args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "host {host}: koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    }

    fn init(&self, host: u8, name: &str, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(host, &args);
    }

    /// Tick `name` on `host` to `done`, keeping it.
    fn done(&self, host: u8, name: &str) {
        let v = self.ok(
            host,
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
    fn pull(&self, host: u8, name: &str) {
        self.ok(host, &["session", "resolve", name, "--keep", "remote"]);
        assert!(self.sessions(host).join(name).is_dir(), "{name} on {host}");
    }

    fn pin(&self, host: u8, name: &str) -> Value {
        self.ok(host, &["session", "pin", name, "--reason", REASON])
    }

    /// The workspace's bucket prefix.
    fn prefix(&self) -> String {
        let digest = Sha256::digest(self.work.to_string_lossy().as_bytes());
        hex::encode(digest)[..16].to_string()
    }

    /// The bucket key of `rest` under session `name`.
    fn key(&self, name: &str, rest: &str) -> String {
        format!("{}/{}/{}", self.prefix(), name, rest)
    }

    /// Every object under session `name`'s prefix, with its bytes.
    fn objects(&self, name: &str) -> BTreeMap<String, Vec<u8>> {
        self.s3
            .keys_under(&format!("{}/{}/", self.prefix(), name))
            .into_iter()
            .map(|k| {
                let bytes = self.s3.object(&k).unwrap_or_default();
                (k, bytes)
            })
            .collect()
    }

    /// The writes (PUT and DELETE) the stand-in recorded, by method and key.
    fn writes(&self) -> Vec<(String, String)> {
        self.s3
            .requests()
            .into_iter()
            .filter(|r| r.method == "PUT" || r.method == "DELETE")
            .map(|r| (r.method, r.key))
            .collect()
    }

    fn local_manifest_keys(&self, host: u8, name: &str) -> Vec<String> {
        let path = self.sessions(host).join(name).join("ctx/manifest.json");
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v["keys"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// What a command printed as JSON: the whole of stdout, or else the object
/// on its last line, after any preview text (`koto workspace prune`).
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

/// The state log object and file name of session `name`.
fn log_name(name: &str) -> String {
    format!("koto-{name}.state.jsonl")
}

/// The entry for `name` in a list of entries.
fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .and_then(|a| a.iter().find(|e| e["name"] == name))
        .unwrap_or_else(|| panic!("no entry for {name} in {list:#}"))
}

/// An entry the bucket's pin keeps: `kept_by_pin`, the pin's reason, set by
/// hand, read from the bucket.
fn assert_bucket_pin(e: &Value) {
    assert_eq!(e["kept_by_pin"], true, "{e:#}");
    assert_eq!(e["pin"]["reason"], REASON, "{e:#}");
    assert_eq!(e["pin"]["set_by"], Value::Null, "{e:#}");
    assert_eq!(e["pin"]["source"], "bucket", "{e:#}");
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|e| e["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Host 1 runs X and its koto child P, and U on its own, to `done`; host 2
/// takes a copy of each before host 1 pins P by hand. Every session is past
/// the retention when this returns.
fn stale_copy_fixture() -> Hosts {
    let h = Hosts::new();
    h.init(1, "X", &[]);
    h.init(1, "P", &["--parent", "X"]);
    h.init(1, "U", &[]);
    for name in ["P", "U", "X"] {
        h.done(1, name);
    }
    for name in ["X", "P", "U"] {
        h.pull(2, name);
    }
    h.pin(1, "P");
    settle();
    h
}

#[test]
fn a_host_that_never_pulled_refuses_cleanup_naming_the_buckets_pin() {
    let h = stale_copy_fixture();
    let before = h.objects("P");
    h.s3.clear_requests();
    let refused = h.fails(3, &["session", "cleanup", "P"], 2);
    assert_eq!(refused["error"]["code"], "pinned", "{refused}");
    assert_bucket_pin(&refused["error"]);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("koto session unpin P"),
        "{refused}"
    );
    assert!(
        !h.writes().iter().any(|(m, _)| m == "DELETE"),
        "{:?}",
        h.writes()
    );
    assert_eq!(h.objects("P"), before);
    assert!(!h.sessions(3).join("P").exists());
}

#[test]
fn a_stale_copy_keeps_the_pinned_session_through_both_prunes() {
    let h = stale_copy_fixture();
    let before = h.objects("P");

    // The preview names the bucket's pin, and plans to remove U.
    let preview = h.ok(2, &["session", "prune", "--all"]);
    let kept = entry(&preview["kept"], "P");
    assert_eq!(kept["rule"], "p", "{kept:#}");
    assert_bucket_pin(kept);
    assert!(
        names(&preview["remove"]).contains(&"U".to_string()),
        "{preview:#}"
    );
    assert!(!names(&preview["remove"]).contains(&"P".to_string()));

    // The apply keeps P in the bucket and removes U from it.
    let applied = h.ok(2, &["session", "prune", "--apply", "--all"]);
    assert!(
        !names(&applied["removed"]).contains(&"P".to_string()),
        "{applied:#}"
    );
    assert!(
        names(&applied["removed"]).contains(&"U".to_string()),
        "{applied:#}"
    );
    assert_bucket_pin(entry(&applied["skipped"], "P"));
    assert_eq!(h.objects("P"), before);
    assert!(h.objects("U").is_empty(), "{:?}", h.objects("U").keys());
}

#[test]
fn a_stale_copy_keeps_the_pinned_child_through_a_workspace_prune() {
    let h = stale_copy_fixture();
    let before = h.objects("P");
    let out = h.ok(2, &["workspace", "prune", "--root", "X", "--yes"]);
    assert!(
        !names(&out["removed"]).contains(&"P".to_string()),
        "{out:#}"
    );
    assert_bucket_pin(entry(&out["skipped"], "P"));
    assert_eq!(h.objects("P"), before);
}

#[test]
fn a_stale_copy_moves_a_pinned_predecessor_aside_at_a_replace() {
    let h = stale_copy_fixture();
    let pin_object = h.s3.object(&h.key("P", PIN)).unwrap();
    let out = h.ok(
        2,
        &[
            "init",
            "P",
            "--template",
            h.template.to_str().unwrap(),
            "--replace-terminal",
        ],
    );
    let text = out.to_string();
    assert!(text.contains("\"source\":\"bucket\""), "{out:#}");
    // The moved copy holds the pin object under its new name; the new run
    // holds the original name and no pin.
    let moved: Vec<String> =
        h.s3.keys()
            .into_iter()
            .filter(|k| k.ends_with("/ctx/koto/links/pin"))
            .collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_ne!(moved[0], h.key("P", PIN));
    assert_eq!(h.s3.object(&moved[0]).unwrap(), pin_object);
    assert!(h.s3.object(&h.key("P", PIN)).is_none());
}

#[test]
fn cancel_cleanup_on_a_stale_copy_refuses_before_writing() {
    let h = stale_copy_fixture();
    let before = h.objects("P");
    h.s3.clear_requests();
    let refused = h.fails(2, &["cancel", "P", "--cleanup"], 2);
    assert_eq!(refused["error"]["code"], "pinned", "{refused}");
    assert_bucket_pin(&refused["error"]);
    assert!(
        !h.writes().iter().any(|(m, _)| m == "DELETE"),
        "{:?}",
        h.writes()
    );
    assert_eq!(h.objects("P"), before);
}

#[test]
fn a_parents_terminal_tick_on_a_second_host_keeps_the_pinned_child() {
    let h = Hosts::new();
    h.init(1, "X", &[]);
    h.init(1, "P", &["--parent", "X"]);
    h.done(1, "P");
    h.pull(2, "X");
    h.pull(2, "P");
    h.pin(1, "P");
    settle();
    let before = h.objects("P");
    let v = h.ok(2, &["next", "X", "--with-data", r#"{"marker":"done"}"#]);
    assert_eq!(v["action"], "done", "{v:#}");
    // X has a child, so its hierarchy keeps both, and P is listed as kept
    // with it: a session kept for another reason costs no bucket check.
    assert_eq!(
        entry(&v["kept_children"], "P")["reason"],
        "hierarchy",
        "{v:#}"
    );
    assert_eq!(h.objects("P"), before);
}

#[test]
fn a_pin_keeps_its_session_on_a_host_without_the_anchor_that_holds_it() {
    let h = Hosts::new();
    // The start hook opens anchors with the cloud backend dropped, so the
    // anchor lives in host 1's local store only.
    let out = h
        .cmd_on(1, false)
        .args([
            "session",
            "anchor",
            "open",
            "--harness-session",
            "h1",
            "--source",
            "startup",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let anchor = text
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .unwrap_or_else(|| panic!("no anchor line in {text}"))
        .trim()
        .to_string();
    h.init(1, "P", &["--dispatched-by", &anchor]);
    h.done(1, "P");
    h.pull(2, "P");
    h.pin(1, "P");
    assert!(
        h.objects(&anchor).is_empty(),
        "the anchor is local to host 1"
    );
    settle();
    let before = h.objects("P");
    for args in [
        &["session", "prune", "--apply", "--all"][..],
        &["workspace", "prune", "--root", "P", "--yes"][..],
    ] {
        let out = h.ok(2, args);
        let text = out.to_string();
        assert!(text.contains("\"source\":\"bucket\""), "{args:?}: {out:#}");
        assert!(!text.contains(&format!("\"name\":\"{anchor}\"")), "{out:#}");
        assert_eq!(h.objects("P"), before);
    }
}

#[test]
fn a_context_write_from_a_stale_copy_does_not_unpin() {
    let h = stale_copy_fixture();
    let note = h.root.join("note.md");
    std::fs::write(&note, "from host 2").unwrap();
    h.ok(
        2,
        &[
            "context",
            "add",
            "P",
            "note.md",
            "--from-file",
            note.to_str().unwrap(),
        ],
    );
    // The push replaced the bucket's index with one that has no pin; the
    // pin object is still there, and still keeps.
    let manifest: Value =
        serde_json::from_slice(&h.s3.object(&h.key("P", "ctx/manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["keys"].get("koto/links/pin").is_none(),
        "{manifest:#}"
    );
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
    let applied = h.ok(2, &["session", "prune", "--apply", "--all"]);
    assert_bucket_pin(entry(&applied["skipped"], "P"));
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
}

#[test]
fn a_failed_or_unparseable_bucket_check_keeps_and_says_so() {
    let h = stale_copy_fixture();
    h.init(2, "L", &[]);
    h.init(2, "R", &[]);
    let pin_listing = |name: &str| h.key(name, "ctx/koto/");

    // Cleanup of a name this host never pulled: exit 3, nothing deleted.
    h.s3.fail("LIST", &pin_listing("P"), 500);
    let before = h.objects("P");
    let out = h.fails(3, &["session", "cleanup", "P"], 3);
    assert_eq!(out["error"]["code"], "bucket_unchecked", "{out}");
    assert_eq!(
        out["error"]["bucket_unchecked"]["error"], "unreachable",
        "{out}"
    );
    assert_eq!(h.objects("P"), before);
    h.s3.clear_faults();
    h.s3.garble_listing(&pin_listing("P"));
    let out = h.fails(3, &["session", "cleanup", "P"], 3);
    assert_eq!(
        out["error"]["bucket_unchecked"]["error"], "unparseable",
        "{out}"
    );
    assert_eq!(h.objects("P"), before);
    h.s3.clear_faults();

    // Both prunes keep an unpinned session whose bucket can't be checked.
    h.s3.fail("LIST", &pin_listing("U"), 500);
    let u_before = h.objects("U");
    let applied = h.ok(2, &["session", "prune", "--apply", "--all"]);
    let u = entry(&applied["skipped"], "U");
    assert_eq!(u["bucket_unchecked"]["error"], "unreachable", "{applied:#}");
    assert!(u.get("kept_by_pin").is_none(), "{u:#}");
    assert_eq!(h.objects("U"), u_before);
    h.s3.clear_faults();

    // A cancel --cleanup exits 3 and writes no event.
    h.s3.fail("LIST", &pin_listing("R"), 500);
    let log = h.s3.object(&h.key("R", &log_name("R")));
    assert!(log.is_some());
    let out = h.fails(2, &["cancel", "R", "--cleanup"], 3);
    assert_eq!(out["error"]["code"], "bucket_unchecked", "{out}");
    assert_eq!(h.s3.object(&h.key("R", &log_name("R"))), log);
    let local = std::fs::read_to_string(h.sessions(2).join("R").join(log_name("R"))).unwrap();
    assert!(!local.contains("workflow_cancelled"), "{local}");
    h.s3.clear_faults();

    // A terminal tick that would remove a lone session keeps it instead.
    h.s3.fail("LIST", &pin_listing("L"), 500);
    let v = h.ok(2, &["next", "L", "--with-data", r#"{"marker":"done"}"#]);
    assert_eq!(v["retention"]["reason"], "bucket_unchecked", "{v:#}");
    assert_eq!(
        v["retention"]["bucket_unchecked"]["error"], "unreachable",
        "{v:#}"
    );
    assert!(h.sessions(2).join("L").is_dir());
    assert!(!h.objects("L").is_empty());
}

#[test]
fn pin_and_unpin_write_their_own_objects_or_fail() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");

    // A refused write leaves no pin anywhere.
    h.s3.fail("PUT", &h.key("P", PIN), 500);
    let out = h.fails(1, &["session", "pin", "P", "--reason", REASON], 3);
    assert_eq!(out["error"]["code"], "pin_not_pushed", "{out}");
    assert!(h.s3.object(&h.key("P", PIN)).is_none());
    assert!(!h.sessions(1).join("P").join(PIN).exists());
    h.s3.clear_faults();

    // Accepted, it is listed on another host from the bucket.
    let pinned = h.pin(1, "P");
    assert_eq!(pinned["store"]["cloud"], BUCKET, "{pinned}");
    let listed = h.ok(3, &["session", "list", "--pinned", "--json"]);
    let row = entry(&listed, "P");
    assert_eq!(row["source"], "bucket", "{listed:#}");
    assert_eq!(row["pin"]["reason"], REASON, "{listed:#}");

    // A refused delete leaves the bucket's pin in force.
    h.s3.fail("DELETE", &h.key("P", PIN), 500);
    let out = h.fails(1, &["session", "unpin", "P"], 3);
    assert_eq!(out["error"]["code"], "pin_not_pushed", "{out}");
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
    h.s3.clear_faults();
    let refused = h.fails(3, &["session", "cleanup", "P"], 2);
    assert_bucket_pin(&refused["error"]);
}

#[test]
fn pin_and_unpin_from_a_stale_copy_or_none_touch_only_their_objects() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");
    h.pull(2, "P");
    let pin_key = h.key("P", PIN);
    let release_key = h.key("P", RELEASE);

    // From a stale copy: one PUT, of the pin object.
    h.s3.clear_requests();
    h.pin(2, "P");
    assert_eq!(h.writes(), [("PUT".to_string(), pin_key.clone())]);
    // From no copy: the release object, then the pin object's delete.
    h.s3.clear_requests();
    let released = h.ok(3, &["session", "unpin", "P"]);
    assert_eq!(released["released"]["pin"]["reason"], REASON, "{released}");
    assert_eq!(
        h.writes(),
        [
            ("PUT".to_string(), release_key.clone()),
            ("DELETE".to_string(), pin_key.clone()),
        ]
    );
    assert!(!h.sessions(3).join("P").exists());
    // And from no copy again: the pin only. Host 1, which owns P, lists it,
    // and host 2's prune keeps it. (Host 1's own prune keeps P by rule g
    // here: host 2's whole pull pushed a version host 1's copy lacks.)
    h.s3.clear_requests();
    h.pin(3, "P");
    assert_eq!(h.writes(), [("PUT".to_string(), pin_key)]);
    let listed = h.ok(1, &["session", "list", "--pinned", "--json"]);
    assert_eq!(entry(&listed, "P")["source"], "bucket", "{listed:#}");
    settle();
    let preview = h.ok(2, &["session", "prune", "--all"]);
    assert_bucket_pin(entry(&preview["kept"], "P"));
}

#[test]
fn an_unpin_on_one_host_frees_the_session_on_another() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");
    h.pin(1, "P");
    // Host 1's copy holds the pin as a bare file the sync doesn't track.
    assert!(h.sessions(1).join("P").join(PIN).is_file());
    assert!(!h
        .local_manifest_keys(1, "P")
        .iter()
        .any(|k| k.starts_with("koto/")));

    h.ok(3, &["session", "unpin", "P"]);
    assert!(
        h.sessions(1).join("P").join(PIN).is_file(),
        "the stale file stays"
    );
    settle();
    let listed = h.ok(1, &["session", "list", "--pinned", "--json"]);
    assert_eq!(listed, serde_json::json!([]), "{listed:#}");
    let applied = h.ok(1, &["session", "prune", "--apply", "--all"]);
    assert!(
        names(&applied["removed"]).contains(&"P".to_string()),
        "{applied:#}"
    );
    assert!(h.objects("P").is_empty(), "{:?}", h.objects("P").keys());
}

#[test]
fn a_pin_set_released_and_set_again_at_once_is_told_apart_by_its_id() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");
    h.pin(1, "P");
    let first = std::fs::read(h.sessions(1).join("P").join(PIN)).unwrap();
    h.ok(3, &["session", "unpin", "P"]);
    h.pin(3, "P");
    let second = h.s3.object(&h.key("P", PIN)).unwrap();
    let id = |b: &[u8]| serde_json::from_slice::<Value>(b).unwrap()["id"].clone();
    assert_ne!(id(&first), id(&second));
    settle();
    // Host 1's file is the first pin, which the release record names; the
    // bucket's pin object is the second, which keeps, and the scan's check
    // in the bucket says so before anything is planned.
    let applied = h.ok(1, &["session", "prune", "--apply", "--all"]);
    let kept = entry(&applied["kept"], "P");
    assert_eq!(kept["rule"], "p", "{kept:#}");
    assert_bucket_pin(kept);
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
}

#[test]
fn local_pin_records_stay_out_of_sync() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");
    h.pin(1, "P");
    h.ok(1, &["session", "unpin", "P"]);
    let keys = h.local_manifest_keys(1, "P");
    assert!(
        !keys
            .iter()
            .any(|k| k == "koto/links/pin" || k == "koto/pin-release"),
        "{keys:?}"
    );
    let release = h.s3.object(&h.key("P", RELEASE)).expect("release object");
    let note = h.root.join("note.md");
    std::fs::write(&note, "later").unwrap();
    h.ok(
        1,
        &[
            "context",
            "add",
            "P",
            "note.md",
            "--from-file",
            note.to_str().unwrap(),
        ],
    );
    assert_eq!(h.s3.object(&h.key("P", RELEASE)), Some(release));
    assert!(h.s3.object(&h.key("P", PIN)).is_none());
    let listed = h.ok(3, &["session", "list", "--pinned", "--json"]);
    assert_eq!(listed, serde_json::json!([]), "{listed:#}");
}

#[test]
fn a_pin_set_under_the_local_backend_goes_up_at_the_first_cloud_push() {
    let h = Hosts::new();
    let local = |args: &[&str]| {
        let out = h.cmd_on(1, false).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    };
    local(&["init", "P", "--template", h.template.to_str().unwrap()]);
    local(&[
        "next",
        "P",
        "--with-data",
        r#"{"marker":"done"}"#,
        "--no-cleanup",
    ]);
    local(&["session", "pin", "P", "--reason", REASON]);
    assert!(h
        .local_manifest_keys(1, "P")
        .contains(&"koto/links/pin".to_string()));

    // The switch: the same store, now under the cloud backend.
    h.ok(1, &["session", "resolve", "P", "--keep", "local"]);
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
    let manifest: Value =
        serde_json::from_slice(&h.s3.object(&h.key("P", "ctx/manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["keys"].get("koto/links/pin").is_none(),
        "{manifest:#}"
    );
    assert!(!h
        .local_manifest_keys(1, "P")
        .contains(&"koto/links/pin".to_string()));
    let refused = h.fails(3, &["session", "cleanup", "P"], 2);
    assert_bucket_pin(&refused["error"]);
}

#[test]
fn a_whole_pull_from_an_index_without_the_pin_still_shows_it() {
    let h = Hosts::new();
    h.init(1, "P", &[]);
    h.done(1, "P");
    let note = h.root.join("note.md");
    std::fs::write(&note, "indexed").unwrap();
    h.ok(
        1,
        &[
            "context",
            "add",
            "P",
            "note.md",
            "--from-file",
            note.to_str().unwrap(),
        ],
    );
    h.pin(3, "P");
    let manifest: Value =
        serde_json::from_slice(&h.s3.object(&h.key("P", "ctx/manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["keys"].get("koto/links/pin").is_none(),
        "{manifest:#}"
    );
    h.pull(2, "P");
    assert!(h.sessions(2).join("P").join(PIN).is_file());
    let listed = h.ok(2, &["session", "list", "--pinned", "--json"]);
    assert_eq!(entry(&listed, "P")["pin"]["reason"], REASON, "{listed:#}");
    settle();
    let applied = h.ok(2, &["session", "prune", "--apply", "--all"]);
    assert!(
        !names(&applied["removed"]).contains(&"P".to_string()),
        "{applied:#}"
    );
    assert!(h.s3.object(&h.key("P", PIN)).is_some());
}

#[test]
fn a_prune_asks_the_bucket_once_per_session_it_would_delete() {
    let h = Hosts::new();
    let all: Vec<String> = (0..20).map(|i| format!("S{i:02}")).collect();
    for name in &all {
        h.init(1, name, &[]);
        h.done(1, name);
    }
    for name in all.iter().take(5) {
        h.pin(3, name);
    }
    settle();
    h.s3.clear_requests();
    let applied = h.ok(1, &["session", "prune", "--apply", "--all"]);
    assert_eq!(names(&applied["removed"]).len(), 15, "{applied:#}");
    let requests = h.s3.requests();
    let pin_listings = requests
        .iter()
        .filter(|r| {
            r.list_prefix
                .as_deref()
                .is_some_and(|p| p.ends_with("/ctx/koto/"))
        })
        .count();
    let pin_reads = requests
        .iter()
        .filter(|r| r.method == "GET" && r.key.ends_with("/ctx/koto/links/pin"))
        .count();
    let release_reads = requests
        .iter()
        .filter(|r| r.method == "GET" && r.key.ends_with("/ctx/koto/pin-release"))
        .count();
    assert!(pin_listings <= 20, "{pin_listings} pin listings");
    assert!(pin_reads <= 5, "{pin_reads} pin reads");
    assert_eq!(release_reads, 0);
}

/// The bucket prefix of the workspace at `dir`.
fn prefix_of(dir: &std::path::Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(digest)[..16].to_string()
}

#[test]
fn an_import_carries_the_pin_and_its_release_record_and_leaves_the_source_inert() {
    let h = Hosts::new();
    let dest = h.root.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    // P is pinned with a release record from an earlier pin; Q's pin is
    // unreadable.
    h.init(1, "P", &[]);
    h.done(1, "P");
    h.pin(1, "P");
    h.ok(1, &["session", "unpin", "P"]);
    h.pin(1, "P");
    let pin = h.s3.object(&h.key("P", PIN)).unwrap();
    let release = h.s3.object(&h.key("P", RELEASE)).unwrap();
    h.init(1, "Q", &[]);
    h.done(1, "Q");
    h.s3.put(&h.key("Q", PIN), b"garbage");

    let in_dest =
        |host: u8, args: &[&str]| h.cmd_at(host, true, &dest).args(args).output().unwrap();
    let ok_in_dest = |args: &[&str]| {
        let out = in_dest(2, args);
        assert!(
            out.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        printed_json(&out.stdout)
    };
    ok_in_dest(&["template", "compile", h.template.to_str().unwrap()]);
    let from = h.work.to_str().unwrap().to_string();
    ok_in_dest(&["session", "import", "P", "--from", &from]);
    ok_in_dest(&["session", "import", "Q", "--from", &from]);

    let moved = |name: &str, rest: &str| format!("{}/{}/{}", prefix_of(&dest), name, rest);
    assert_eq!(h.s3.object(&moved("P", PIN)), Some(pin));
    assert_eq!(h.s3.object(&moved("P", RELEASE)), Some(release));
    assert_eq!(
        h.s3.object(&moved("Q", PIN)).as_deref(),
        Some(&b"garbage"[..])
    );
    let manifest: Value =
        serde_json::from_slice(&h.s3.object(&moved("P", "ctx/manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["keys"].get("koto/links/pin").is_none(),
        "{manifest:#}"
    );

    // The moved copies are kept by their pins, read from the bucket.
    let out = in_dest(3, &["session", "cleanup", "P"]);
    assert_eq!(out.status.code(), Some(2));
    assert_bucket_pin(&printed_json(&out.stdout)["error"]);
    let out = in_dest(3, &["session", "cleanup", "Q"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        printed_json(&out.stdout)["error"]["pin_unreadable"],
        "unparseable"
    );

    // The migrated source's pin is inert: unpin refuses it and writes
    // nothing, and a prune of migrated sources removes it while the moved
    // copy stays pinned.
    h.s3.clear_requests();
    let out = h.fails(1, &["session", "unpin", "P"], 2);
    assert_eq!(out["error"]["code"], "not_pinnable", "{out}");
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    let preview = ok_in_dest(&[
        "session",
        "prune",
        "--migrated-from",
        &from,
        "--source-retired",
    ]);
    let token = preview["plan"].as_str().unwrap().to_string();
    let applied = ok_in_dest(&[
        "session",
        "prune",
        "--migrated-from",
        &from,
        "--apply",
        "--plan",
        &token,
    ]);
    assert!(
        names(&applied["removed"]).contains(&"P".to_string()),
        "{applied:#}"
    );
    assert!(h.s3.object(&h.key("P", PIN)).is_none());
    assert!(h.s3.object(&moved("P", PIN)).is_some());
}
