//! `koto dashboard --once` on one machine shows the sessions another
//! machine pushed to the same bucket.
//!
//! Machine A creates a session on a two-state template whose first state is
//! gated until context key `go` exists, ticks it once so it stops at the
//! gate, and its push publishes a `summary.json` beside the state file.
//! Machine B, with its own `HOME`, session store and working directory but
//! the same bucket, lists every workspace prefix with a `/ctx/`-delimited
//! listing and prints A's session with its state, A's machine name and A's
//! workspace id as columns nine and ten.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`; no real bucket is involved.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUCKET: &str = "koto-dashboard";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The fixture template: `check` waits on context key `go`, then `done`.
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

impl Out {
    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }

    /// The `--once` lines, each split into its tab-separated fields.
    fn rows(&self) -> Vec<Vec<String>> {
        self.stdout
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    }
}

/// One machine: its own `HOME` (and so its own session store), template
/// cache and machine name, and a working directory of its own.
struct Machine {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    dir: PathBuf,
    endpoint: String,
    name: &'static str,
    cloud: bool,
}

impl Machine {
    fn new(s3: &FakeS3, name: &'static str) -> Self {
        Self::with_backend(s3, name, true)
    }

    fn with_backend(s3: &FakeS3, name: &'static str, cloud: bool) -> Self {
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
            cloud,
        }
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto").join("sessions")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
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
            // A bucket is configured either way; only the backend differs.
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .env(
                "KOTO_SESSION_BACKEND",
                if self.cloud { "cloud" } else { "local" },
            )
            .args(args);
        cmd
    }

    fn koto(&self, args: &[&str]) -> Out {
        let o = self.cmd(args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert_eq!(out.code, Some(0), "koto {:?}: {}", args, out.describe());
        out
    }

    /// Create `name` on the fixture template and tick it once, so it stops
    /// at the gate.
    fn start_blocked(&self, name: &str) {
        std::fs::write(self.dir.join("gated.md"), TEMPLATE).unwrap();
        self.ok(&["init", name, "--template", "gated.md"]);
        self.ok(&["next", name]);
    }

    fn workspace(&self) -> String {
        prefix_of(&self.dir)
    }
}

/// The row for `name` under `workspace` in `rows`.
fn row_of<'a>(rows: &'a [Vec<String>], name: &str, workspace: &str) -> Option<&'a Vec<String>> {
    rows.iter().find(|r| {
        r.first().map(String::as_str) == Some(name)
            && r.get(9).map(String::as_str) == Some(workspace)
    })
}

/// Machine A with a session stopped at the fixture's gate, and machine B on
/// the same bucket.
fn two_machines() -> (FakeS3, Machine, Machine) {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    a.start_blocked(WF);
    (s3, a, b)
}

/// PLAN step 1's check: B's `--once` prints A's blocked session with A's
/// columns, `machine-a` and A's workspace, reading only listings and the
/// summary.
#[test]
fn once_prints_other_machines_blocked_session() {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    let ws = a.workspace();

    std::fs::write(a.dir.join("gated.md"), TEMPLATE).unwrap();
    a.ok(&["init", WF, "--template", "gated.md"]);
    s3.clear_requests();
    a.ok(&["next", WF]);

    // A's tick pushed the state file and then the summary beside it.
    let state_key = format!("{ws}/{WF}/koto-{WF}.state.jsonl");
    let summary_key = format!("{ws}/{WF}/summary.json");
    let ticked = s3.requests();
    let state_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == state_key)
        .unwrap_or_else(|| panic!("the tick pushed no state: {ticked:?}"));
    let summary_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == summary_key)
        .unwrap_or_else(|| panic!("the tick published no summary: {ticked:?}"));
    assert!(
        summary_put > state_put,
        "the summary follows the state push"
    );
    // The summary PUT's latency after the state push, against koto's 30 s
    // action limit.
    let latency = ticked[summary_put].at - ticked[state_put].at;
    eprintln!("summary.json PUT latency after the state push: {latency:?}");
    assert!(latency < Duration::from_secs(30), "latency {latency:?}");

    let summary: serde_json::Value =
        serde_json::from_slice(&s3.object(&summary_key).expect("summary in the bucket")).unwrap();
    assert_eq!(summary["current_state"], "check");
    assert_eq!(summary["is_blocked"], true);
    assert_eq!(summary["machine"], "machine-a");
    assert_eq!(summary["schema"], 1);

    let a_rows = a.ok(&["dashboard", "--once"]).rows();
    let a_row = row_of(&a_rows, WF, &ws).unwrap_or_else(|| panic!("A's own row: {a_rows:?}"));
    assert_eq!(a_row[8], "machine-a", "A's own row names A in column nine");

    s3.clear_requests();
    let out = b.ok(&["dashboard", "--once"]);
    let rows = out.rows();
    let row = row_of(&rows, WF, &ws).unwrap_or_else(|| panic!("A's row on B: {}", out.describe()));
    assert_eq!(row.len(), 10, "ten tab-separated fields: {row:?}");
    for (i, column) in [
        (1, "current_state"),
        (3, "status"),
        (4, "intent"),
        (5, "template_name"),
    ] {
        assert_eq!(row[i], a_row[i], "{column} on B equals A's own");
    }
    assert_eq!(row[1], "check");
    assert_eq!(row[3], "blocked");
    assert_eq!(row[8], "machine-a");
    assert_eq!(row[9], ws);

    // B read the bucket and wrote nothing: only listings and GETs, none of
    // them of a state file, and no directory in B's session store.
    let requests = s3.requests();
    assert!(
        requests.iter().all(|r| r.method == "GET"),
        "only listings and GETs: {requests:?}"
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.list_prefix.is_none() && r.key.ends_with(".state.jsonl")),
        "no state file fetched: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|r| r.list_prefix.as_deref() == Some(&format!("{ws}/"))
                && r.delimiter.as_deref() == Some("/ctx/")),
        "A's workspace listed with the /ctx/ delimiter: {requests:?}"
    );
    let created: Vec<_> = std::fs::read_dir(b.sessions())
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        created.is_empty(),
        "B's session store stays empty: {created:?}"
    );
}

/// A session B created itself shows once, as B's own row with B's machine,
/// though its copy and summary are in the bucket too.
#[test]
fn once_shows_own_session_once_with_own_machine() {
    let (_s3, a, b) = two_machines();
    b.start_blocked("mine");

    let rows = b.ok(&["dashboard", "--once"]).rows();
    let mine: Vec<_> = rows.iter().filter(|r| r[0] == "mine").collect();
    assert_eq!(mine.len(), 1, "B's own session once: {rows:?}");
    assert_eq!(mine[0][8], "machine-b");
    assert_eq!(mine[0][9], b.workspace());
    assert!(
        row_of(&rows, WF, &a.workspace()).is_some(),
        "A's row too: {rows:?}"
    );
}

/// `--needs-you`, `--status` and a session name apply to a remote row as to
/// a local one.
#[test]
fn once_filters_apply_to_remote_rows() {
    let (_s3, a, b) = two_machines();
    let ws = a.workspace();

    let needs_you = b.ok(&["dashboard", "--once", "--needs-you"]).rows();
    assert!(row_of(&needs_you, WF, &ws).is_some(), "{needs_you:?}");

    let running = b.ok(&["dashboard", "--once", "--status", "running"]).rows();
    assert!(row_of(&running, WF, &ws).is_none(), "{running:?}");

    let blocked = b
        .ok(&["dashboard", "--once", "--status", "needs-you-blocked"])
        .rows();
    assert!(row_of(&blocked, WF, &ws).is_some(), "{blocked:?}");

    let named = b.ok(&["dashboard", "--once", WF]).rows();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0][9], ws);
    assert_eq!(
        named[0][7], "needs-you-blocked",
        "column eight keeps its meaning"
    );
}

/// With the local backend selected, even with a bucket configured, the two
/// new columns are empty and the run asks the bucket nothing.
#[test]
fn once_on_the_local_backend_prints_empty_columns_and_makes_no_request() {
    let (s3, _a, _b) = two_machines();
    let local = Machine::with_backend(&s3, "machine-c", false);
    local.start_blocked("here");

    s3.clear_requests();
    let rows = local.ok(&["dashboard", "--once"]).rows();
    assert_eq!(rows.len(), 1, "only the local session: {rows:?}");
    assert_eq!(rows[0].len(), 10);
    assert_eq!(rows[0][0], "here");
    assert_eq!((rows[0][8].as_str(), rows[0][9].as_str()), ("", ""));
    assert!(s3.requests().is_empty(), "no request: {:?}", s3.requests());
}
