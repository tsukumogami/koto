//! A `summary.json` PUT that fails after a state push is written by the next
//! flush, even one with no state to push.
//!
//! Machine A creates a session on a template gated on context key `go` and
//! ticks it once, with the fake bucket failing that tick's summary PUT. The
//! tick warns and succeeds; the next `koto session flush`, which has no
//! state to push, PUTs the summary and no state file, after which machine B
//! lists the session from its summary. A further flush PUTs nothing.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`; no real bucket is involved.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::{FakeS3, Request};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUCKET: &str = "koto-summary-retry";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// `check` waits on context key `go`, then `done`.
const TEMPLATE: &str = r#"---
name: gated
version: "1.0"
initial_state: check
states:
  check:
    gates:
      go:
        type: context-exists
        key: go
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Wait for go.

## done

Done.
"#;

/// The bucket prefix koto derives for a directory.
fn prefix_of(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// One machine: its own `HOME`, cache, machine name and working directory,
/// on the cloud backend against the fake bucket.
struct Machine {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    dir: PathBuf,
    endpoint: String,
    name: &'static str,
}

impl Machine {
    fn new(s3: &FakeS3, name: &'static str) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Machine {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            dir: mk(root.path().join(format!("work-{name}"))),
            _root: root,
            endpoint: s3.endpoint(),
            name,
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let o = koto_command::koto()
            .env_clear()
            .current_dir(&self.dir)
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
            .env("KOTO_MACHINE_NAME", self.name)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .args(args)
            .output()
            .unwrap();
        let out = Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        assert_eq!(
            out.code,
            Some(0),
            "koto {args:?}: stdout: {}; stderr: {}",
            out.stdout.trim(),
            out.stderr.trim()
        );
        out
    }

    fn workspace(&self) -> String {
        prefix_of(&self.dir)
    }
}

/// The PUTs among `requests`, by key.
fn puts(requests: &[Request]) -> Vec<&str> {
    requests
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.key.as_str())
        .collect()
}

/// The `state_etag` the summary under `key` names.
fn summary_state_etag(s3: &FakeS3, key: &str) -> Option<String> {
    let summary: serde_json::Value = serde_json::from_slice(&s3.object(key)?).ok()?;
    summary["state_etag"].as_str().map(str::to_string)
}

/// A tick that pushes state PUTs the summary within the 30-second action
/// limit. When that PUT fails, the tick warns and succeeds; the next flush,
/// with no state to push, PUTs `summary.json` and no state file, B's
/// `--once` then shows the session from its summary, and a further flush
/// PUTs nothing.
#[test]
fn a_failed_summary_put_is_written_by_the_next_flush() {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    let ws = a.workspace();
    let key = |name: &str, file: &str| format!("{ws}/{name}/{file}");
    let state_key = key(WF, &format!("koto-{WF}.state.jsonl"));
    let summary_key = key(WF, "summary.json");
    std::fs::write(a.dir.join("gated.md"), TEMPLATE).unwrap();

    // A clean tick on `lat`: the state push, then the summary PUT after it.
    a.ok(&["init", "lat", "--template", "gated.md"]);
    s3.clear_requests();
    a.ok(&["next", "lat"]);
    let ticked = s3.requests();
    let state_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == key("lat", "koto-lat.state.jsonl"))
        .unwrap_or_else(|| panic!("the tick pushed no state: {ticked:?}"));
    let summary_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == key("lat", "summary.json"))
        .unwrap_or_else(|| panic!("the tick published no summary: {ticked:?}"));
    assert!(
        summary_put > state_put,
        "the summary follows the state push"
    );
    let latency = ticked[summary_put].at - ticked[state_put].at;
    println!("summary.json PUT latency after the state push: {latency:?}");
    assert!(latency < Duration::from_secs(30), "latency {latency:?}");

    // A tick on `wf` whose summary PUT fails (rust-s3 asks twice, so two
    // failed requests are one failed call), with no summary left in the
    // bucket from the init.
    a.ok(&["init", WF, "--template", "gated.md"]);
    s3.remove(&summary_key);
    s3.fail_times("PUT", &summary_key, 500, 2);
    s3.clear_requests();
    let out = a.ok(&["next", WF]);
    assert!(
        out.stderr.contains(&format!(
            "warning: cloud sync: publishing the summary of session '{WF}' to {summary_key} failed"
        )),
        "{}",
        out.stderr
    );
    assert!(
        puts(&s3.requests()).contains(&state_key.as_str()),
        "the tick pushed state: {:?}",
        s3.requests()
    );
    assert!(s3.object(&summary_key).is_none(), "no summary went up");

    // B lists the session without a summary: unknown.
    let unknown = b.ok(&["dashboard", "--once", "--all"]);
    let row: Vec<&str> = unknown
        .stdout
        .lines()
        .find(|l| l.starts_with(&format!("{WF}\t")))
        .unwrap_or_else(|| panic!("{}", unknown.stdout))
        .split('\t')
        .collect();
    assert_eq!(row[3], "unknown", "{row:?}");

    // The next flush has no state to push: it PUTs the summary only.
    s3.clear_requests();
    a.ok(&["session", "flush", WF]);
    let flushed = s3.requests();
    assert_eq!(
        puts(&flushed),
        vec![summary_key.as_str()],
        "the summary and no state file: {flushed:?}"
    );
    assert!(!flushed.iter().any(|r| r.method == "DELETE"), "{flushed:?}");
    assert_eq!(
        summary_state_etag(&s3, &summary_key),
        s3.etag_of(&state_key),
        "the summary describes the state in the bucket"
    );

    // B now lists the session from its summary.
    let shown = b.ok(&["dashboard", "--once", "--all"]);
    let row: Vec<&str> = shown
        .stdout
        .lines()
        .find(|l| l.starts_with(&format!("{WF}\t")))
        .unwrap_or_else(|| panic!("{}", shown.stdout))
        .split('\t')
        .collect();
    assert_eq!(
        (row[1], row[3], row[8], row[9]),
        ("check", "blocked", "machine-a", ws.as_str()),
        "{row:?}"
    );

    // Nothing is owed any more: a further flush PUTs nothing.
    s3.clear_requests();
    a.ok(&["session", "flush", WF]);
    assert!(
        puts(&s3.requests()).is_empty(),
        "no PUT: {:?}",
        s3.requests()
    );
}
