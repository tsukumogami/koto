//! What a `koto context` call costs on the cloud backend when a `koto next`
//! action makes it (koto#376), and what still holds across hosts.
//!
//! Each call is its own process, and each used to pull the state file and
//! push the content, the manifest, `version.json` and the state file again:
//! about seven sequential bucket requests a call, so an action making
//! eighteen of them ran past the 30-second action cap on round trips alone.
//! The writes a tick's commands make to its own session now wait in a batch
//! the tick flushes once before it returns.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`, with the backend chosen through
//! `KOTO_SESSION_*` variables alone, which the tick hands its commands. Each
//! simulated host is its own `HOME` and workspace.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BUCKET: &str = "koto-batch";
const NAME: &str = "batch";
const TEMPLATE_FILE: &str = "batch.md";

/// A system `PATH` for the action's shell, after the directory holding the
/// koto under test.
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The action cap a default action runs under when it names none.
const ACTION_CAP: Duration = Duration::from_secs(30);

/// A workflow whose first state runs `script` as its default action and
/// then waits for evidence.
fn template(script: &str) -> String {
    let body: String = script.lines().map(|l| format!("        {}\n", l)).collect();
    format!(
        r#"---
name: batch
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
{body}    transitions:
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

    fn json(&self) -> serde_json::Value {
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line).unwrap_or(serde_json::Value::Null)
    }

    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// One simulated host: its own `HOME`, template cache and workspace, and
/// the shared bucket.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
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
            _root: root,
            endpoint: s3.endpoint(),
        }
    }

    /// A koto with nothing inherited from the test process.
    fn cmd(&self, args: &[&str]) -> Command {
        Command::from_std(self.std_cmd(args))
    }

    /// [`Host::cmd`] as a `std` command, for a test that spawns koto and
    /// stops it part way.
    fn std_cmd(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(&self.ws)
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
        cmd
    }

    fn koto(&self, args: &[&str]) -> Out {
        Self::out(self.cmd(args).output().unwrap())
    }

    fn out(o: std::process::Output) -> Out {
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// `koto context add` from outside any tick.
    fn add(&self, key: &str, content: &[u8]) -> Out {
        Self::out(
            self.cmd(&["context", "add", NAME, key])
                .write_stdin(content.to_vec())
                .output()
                .unwrap(),
        )
    }

    /// Init the session on a workflow whose action is `script`.
    fn init(&self, script: &str) {
        std::fs::write(self.ws.join(TEMPLATE_FILE), template(script)).unwrap();
        let out = self.koto(&["init", NAME, "--template", TEMPLATE_FILE]);
        assert!(out.ok(), "init: {}", out.describe());
    }

    fn next(&self) -> Out {
        self.koto(&["next", NAME])
    }

    fn prefix(&self) -> String {
        prefix_of(&self.ws)
    }

    /// The bucket key of `rest` under this host's copy of the session.
    fn key(&self, rest: &str) -> String {
        format!("{}/{}/{}", self.prefix(), NAME, rest)
    }

    fn session_dir(&self) -> PathBuf {
        self.home.join(".koto").join("sessions").join(NAME)
    }
}

/// A recorded request as `METHOD key`, or `LIST prefix`.
fn describe(r: &fake_s3::Request) -> String {
    match &r.list_prefix {
        Some(prefix) => format!("LIST {}", prefix),
        None => format!("{} {}", r.method, r.key),
    }
}

/// The requests the bucket answered while `f` ran.
fn requests_during(s3: &FakeS3, f: impl FnOnce()) -> Vec<String> {
    s3.clear_requests();
    f();
    s3.requests().iter().map(describe).collect()
}

/// The bucket's copy of the session reads back whole: its manifest is
/// valid JSON, every key it names has an object whose hash matches, and
/// every `context_added` event in its state file names a key the manifest
/// holds or a later event removed. Returns the manifest's keys.
fn assert_remote_consistent(s3: &FakeS3, h: &Host) -> Vec<String> {
    let manifest: serde_json::Value = s3
        .object(&h.key("ctx/manifest.json"))
        .map(|b| serde_json::from_slice(&b).expect("remote manifest parses"))
        .unwrap_or_else(|| serde_json::json!({"keys": {}}));
    let keys = manifest["keys"].as_object().cloned().unwrap_or_default();
    for (key, meta) in &keys {
        let object = s3
            .object(&h.key(&format!("ctx/{}", key)))
            .unwrap_or_else(|| panic!("the manifest names {key}, which the bucket lacks"));
        assert_eq!(
            meta["hash"].as_str().unwrap(),
            sha256_hex(&object),
            "{key}'s object doesn't match the manifest"
        );
    }
    let state = s3
        .object(&h.key(&format!("koto-{}.state.jsonl", NAME)))
        .expect("the bucket holds the state file");
    let mut live = std::collections::BTreeSet::new();
    for line in String::from_utf8(state).unwrap().lines().skip(1) {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        let key = event["payload"]["key"].as_str().unwrap_or("").to_string();
        match event["type"].as_str() {
            Some("context_added") => {
                live.insert(key);
            }
            Some("context_removed") => {
                live.remove(&key);
            }
            _ => {}
        }
    }
    for key in &live {
        assert!(
            keys.contains_key(key),
            "the state file says {key} was added, but the manifest lacks it"
        );
    }
    keys.keys().cloned().collect()
}

/// The script of an action making `adds` adds and `removes` removes of
/// absent keys.
fn script(adds: usize, removes: usize) -> String {
    let mut lines = vec!["set -e".to_string()];
    for i in 0..removes {
        lines.push(format!("koto context remove {NAME} absent-{i}.md"));
    }
    for i in 0..adds {
        lines.push(format!(
            "printf 'value {i}' | koto context add {NAME} key-{i}.md"
        ));
    }
    if adds == 0 && removes == 0 {
        lines.push("true".to_string());
    }
    lines.join("\n")
}

/// Init a session whose action is `script` on a fresh host, tick it once,
/// and return the requests the tick made.
fn tick_requests(s3: &FakeS3, script: &str) -> Vec<String> {
    let h = Host::new(s3);
    h.init(script);
    let mut out = None;
    let requests = requests_during(s3, || out = Some(h.next()));
    let out = out.unwrap();
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());
    assert_remote_consistent(s3, &h);
    requests
}

/// A context write inside an action costs the bucket a fixed, small number
/// of requests, and the batch is flushed once: one manifest PUT, one
/// `version.json` PUT and one batch-open marker for the whole action,
/// however many keys it wrote.
#[test]
fn context_writes_in_an_action_cost_a_fixed_number_of_requests() {
    let s3 = FakeS3::start(BUCKET);
    let none = tick_requests(&s3, &script(0, 0));
    let one_add = tick_requests(&s3, &script(1, 0));
    let one_remove = tick_requests(&s3, &script(0, 1));
    let ten_adds = tick_requests(&s3, &script(10, 0));
    eprintln!(
        "requests per tick: no context call {}, one add {}, one remove {}, ten adds {}",
        none.len(),
        one_add.len(),
        one_remove.len(),
        ten_adds.len()
    );

    // Each add past the first: its process's migration-marker listing, and
    // its content's PUT in the flush.
    let per_add = (ten_adds.len() - one_add.len()) as f64 / 9.0;
    assert!(
        per_add <= 2.0,
        "each further add cost {per_add} requests; ten adds made {:#?}",
        ten_adds
    );
    // The first write also opens the batch (the version check and the
    // marker) and the flush closes it.
    for (label, requests) in [("one add", &one_add), ("one remove", &one_remove)] {
        let extra = requests.len() - none.len();
        assert!(
            extra <= 10,
            "{label} cost {extra} requests over a tick with no context call: {:#?}",
            requests
        );
    }

    for (label, requests) in [("one add", &one_add), ("ten adds", &ten_adds)] {
        let put = |rest: &str| {
            requests
                .iter()
                .filter(|r| r.starts_with("PUT ") && r.ends_with(rest))
                .count()
        };
        assert_eq!(put("/ctx/manifest.json"), 1, "{label}: {:#?}", requests);
        assert_eq!(put("/version.json"), 1, "{label}: {:#?}", requests);
        assert_eq!(put("/batch-open.json"), 1, "{label}: {:#?}", requests);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.starts_with("DELETE ") && r.ends_with("/batch-open.json"))
                .count(),
            1,
            "{label}: {:#?}",
            requests
        );
    }
    // The flush writes in an order a stop between any two leaves readable:
    // content, the manifest naming it, the state file naming both, the
    // version, and last the marker that says a batch is open.
    let at = |pred: &dyn Fn(&String) -> bool| {
        one_add
            .iter()
            .position(pred)
            .unwrap_or_else(|| panic!("missing from {:#?}", one_add))
    };
    let content = at(&|r| r.starts_with("PUT ") && r.ends_with("/ctx/key-0.md"));
    let manifest = at(&|r| r.starts_with("PUT ") && r.ends_with("/ctx/manifest.json"));
    let version = at(&|r| r.starts_with("PUT ") && r.ends_with("/version.json"));
    let closed = at(&|r| r.starts_with("DELETE ") && r.ends_with("/batch-open.json"));
    let state = one_add[manifest..version]
        .iter()
        .position(|r| r.starts_with("PUT ") && r.ends_with(".state.jsonl"));
    assert!(
        content < manifest && state.is_some() && version < closed,
        "flush out of order: {:#?}",
        one_add
    );

    // A removal costs its flush at most one DELETE, of an object S3 answers
    // for whether or not it is there.
    assert!(
        one_remove
            .iter()
            .filter(|r| r.starts_with("DELETE ") && r.contains("/ctx/"))
            .count()
            <= 1,
        "{:#?}",
        one_remove
    );
}

/// The coordinator's reconcile step that found koto#376 made eighteen koto
/// calls in one action. Against a bucket answering each request a quarter
/// of a second later, an action making eighteen context calls now finishes
/// in well under half the cap.
#[test]
fn eighteen_context_calls_finish_well_inside_the_action_cap() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let mut lines = vec!["set -e".to_string()];
    for i in 0..7 {
        lines.push(format!("koto context remove {NAME} stale-{i}.md"));
    }
    for i in 0..9 {
        lines.push(format!(
            "printf 'value {i}' | koto context add {NAME} key-{i}.md"
        ));
    }
    lines.push(format!("koto context get {NAME} key-0.md > /dev/null"));
    lines.push(format!("koto context exists {NAME} key-8.md"));
    h.init(&lines.join("\n"));

    s3.set_latency(Duration::from_millis(250));
    let mut requests = Vec::new();
    let mut out = None;
    let started = Instant::now();
    requests.extend(requests_during(&s3, || out = Some(h.next())));
    let took = started.elapsed();
    s3.set_latency(Duration::ZERO);
    let out = out.unwrap();
    eprintln!(
        "18 context calls: whole tick {:?}, {} requests, state {}",
        took,
        requests.len(),
        out.json()["state"]
    );
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());

    // The action's own run time, as the tick recorded it.
    let state = std::fs::read_to_string(h.session_dir().join(format!("koto-{}.state.jsonl", NAME)))
        .unwrap();
    let action_ms = state
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| e["type"] == "default_action_executed")
        .and_then(|e| e["payload"]["duration_ms"].as_u64())
        .expect("the tick recorded the action's duration");
    eprintln!(
        "18 context calls: action {} ms, whole tick {:?}, {} requests",
        action_ms,
        took,
        requests.len()
    );
    assert!(
        Duration::from_millis(action_ms) < ACTION_CAP / 3,
        "the action took {} ms, not well inside the {:?} cap",
        action_ms,
        ACTION_CAP
    );
    assert!(
        took < ACTION_CAP / 2,
        "the tick took {:?}, flush included",
        took
    );
    let keys = assert_remote_consistent(&s3, &h);
    assert_eq!(keys.len(), 9, "{:?}", keys);
}

/// The batch is flushed before `koto next` returns, so another host's
/// import, after the tick, carries every key the action wrote and none it
/// removed.
#[test]
fn an_import_sees_every_key_written_inside_an_action() {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    a.init(&format!(
        "set -e\nprintf one | koto context add {NAME} one.md\nprintf two | koto context add {NAME} \
         two.md\nkoto context remove {NAME} before.md"
    ));
    assert!(a.add("before.md", b"old").ok());
    assert!(a.add("kept.md", b"kept").ok());
    let out = a.next();
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());

    let b = Host::new(&s3);
    let ws = a.ws.to_string_lossy().into_owned();
    let out = b.koto(&["session", "import", NAME, "--from", &ws, "--trust-template"]);
    assert!(out.ok(), "import: {}", out.describe());
    assert_eq!(out.json()["keys"], 3, "{}", out.describe());
    for (key, want) in [("one.md", "one"), ("two.md", "two"), ("kept.md", "kept")] {
        let out = b.koto(&["context", "get", NAME, key]);
        assert!(out.ok(), "get {key}: {}", out.describe());
        assert_eq!(out.stdout, want);
    }
    assert_eq!(
        b.koto(&["context", "exists", NAME, "before.md"]).code,
        Some(1)
    );
}

/// A tick killed in the middle of its action never flushes. The bucket
/// keeps the copy from before the batch, whole; the batch-open marker makes
/// an import refuse rather than carry that copy without the batch's writes;
/// and the next koto command on the source's host pushes the batch, after
/// which the import carries every key.
#[test]
fn a_tick_killed_mid_batch_leaves_the_bucket_consistent_and_an_import_refuses() {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    a.init(&format!(
        "printf one | koto context add {NAME} one.md\nprintf two | koto context add {NAME} \
         two.md\nkoto context remove {NAME} base.md\nkill -9 $PPID"
    ));
    assert!(a.add("base.md", b"base").ok());
    let out = a.next();
    assert_eq!(
        out.code,
        None,
        "the tick should have been killed: {}",
        out.describe()
    );

    // The bucket still reads as it did before the action.
    let keys = assert_remote_consistent(&s3, &a);
    assert_eq!(keys, vec!["base.md".to_string()]);
    assert!(s3.object(&a.key("batch-open.json")).is_some());
    assert!(a.session_dir().join("cloud-batch.jsonl").exists());

    // An import from another host refuses, and leaves the source unmarked.
    let b = Host::new(&s3);
    let ws = a.ws.to_string_lossy().into_owned();
    let import = || b.koto(&["session", "import", NAME, "--from", &ws, "--trust-template"]);
    let out = import();
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "import_source_unpushed",
        "{}",
        out.describe()
    );
    assert!(s3.object(&a.key("migrated.json")).is_none());

    // Any koto command on the source's host pushes the batch.
    let out = a.koto(&["status", NAME]);
    assert!(out.ok(), "status: {}", out.describe());
    assert!(!a.session_dir().join("cloud-batch.jsonl").exists());
    assert!(s3.object(&a.key("batch-open.json")).is_none());
    let keys = assert_remote_consistent(&s3, &a);
    assert_eq!(keys, vec!["one.md".to_string(), "two.md".to_string()]);

    let out = import();
    assert!(out.ok(), "import: {}", out.describe());
    for (key, want) in [("one.md", "one"), ("two.md", "two")] {
        let out = b.koto(&["context", "get", NAME, key]);
        assert!(out.ok(), "get {key}: {}", out.describe());
        assert_eq!(out.stdout, want);
    }
}

/// A session imported elsewhere while its tick's action runs: the next
/// context call refuses as it always has, and the flush refuses too, so
/// nothing the batch held reaches the source's copy after the marker.
#[test]
fn a_migrated_source_still_refuses_inside_an_action() {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    a.init(&format!(
        "set -e\nprintf one | koto context add {NAME} one.md\nprintf two | koto context add {NAME} \
         two.md"
    ));
    assert!(a.add("base.md", b"base").ok());
    // The marker lands while the batch's first write checks the version.
    s3.put_after(
        "GET",
        &a.key("version.json"),
        &a.key("migrated.json"),
        br#"{"target":{"prefix":"elsewhere","session":"batch","workspace":"/elsewhere"}}"#,
    );
    let out = a.next();
    assert_ne!(out.json()["state"], "wait", "{}", out.describe());
    assert!(
        out.stdout
            .contains("session_migrated: session 'batch' was migrated to 'batch' in /elsewhere"),
        "the second add should have refused: {}",
        out.describe()
    );
    assert!(s3.object(&a.key("ctx/one.md")).is_none());
    assert!(s3.object(&a.key("ctx/two.md")).is_none());
    let keys = assert_remote_consistent(&s3, &a);
    assert_eq!(keys, vec!["base.md".to_string()]);
}

/// A version conflict is a hard error whether it is there when the batch
/// opens (the first write refuses) or appears while the batch is open (the
/// flush refuses, leaves the bucket alone, and the next write outside the
/// tick refuses).
#[test]
fn a_version_conflict_is_still_a_hard_error() {
    let other = br#"{"version":9,"last_sync_base":9,"machine_id":"otherhost"}"#;

    // There before the batch: this host has an unpushed change and the
    // remote moved past its base.
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    a.init(&format!(
        "set -e\nprintf one | koto context add {NAME} one.md"
    ));
    assert!(a.add("base.md", b"base").ok());
    let version_path = a.session_dir().join("version.json");
    let mut local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&version_path).unwrap()).unwrap();
    local["version"] = serde_json::json!(local["version"].as_u64().unwrap() + 1);
    std::fs::write(&version_path, local.to_string()).unwrap();
    s3.put(&a.key("version.json"), other);
    let out = a.next();
    assert_ne!(out.json()["state"], "wait", "{}", out.describe());
    assert!(
        out.stdout.contains("session conflict"),
        "{}",
        out.describe()
    );
    assert!(s3.object(&a.key("ctx/one.md")).is_none());
    assert!(s3.object(&a.key("batch-open.json")).is_none());

    // Appearing mid-batch: another host pushes after the batch opened.
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    a.init(&format!(
        "set -e\nprintf one | koto context add {NAME} one.md"
    ));
    assert!(a.add("base.md", b"base").ok());
    s3.put_after("GET", &a.key("version.json"), &a.key("version.json"), other);
    let out = a.next();
    assert!(
        out.stderr.contains("session conflict"),
        "{}",
        out.describe()
    );
    assert!(s3.object(&a.key("ctx/one.md")).is_none());
    assert_eq!(s3.object(&a.key("version.json")).unwrap(), other);
    assert!(s3.object(&a.key("batch-open.json")).is_some());
    let out = a.add("later.md", b"later");
    assert!(!out.ok(), "{}", out.describe());
    assert!(
        out.stdout.contains("session conflict"),
        "{}",
        out.describe()
    );
}

/// A session that has never pushed a `version.json` is asked about it with
/// a listing, not a GET: a GET of a missing object is a 404, which rust-s3
/// retries after a one-second sleep.
#[test]
fn a_missing_version_file_costs_no_404() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.init("true");
    assert!(s3.object(&h.key("version.json")).is_none());
    let mut out = None;
    let requests = requests_during(&s3, || out = Some(h.add("first.md", b"first")));
    let out = out.unwrap();
    assert!(out.ok(), "add: {}", out.describe());
    let version = h.key("version.json");
    assert!(
        requests.contains(&format!("LIST {}", version)),
        "{:#?}",
        requests
    );
    assert!(
        !requests.contains(&format!("GET {}", version)),
        "{:#?}",
        requests
    );
    assert!(s3.object(&version).is_some());
}

/// The local backend has no batch: an action's context calls write the
/// store directly and leave no journal behind.
#[test]
fn the_local_backend_keeps_no_batch() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path().join("home");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join(TEMPLATE_FILE),
        template(&format!(
            "set -e\nprintf one | koto context add {NAME} one.md\nkoto context remove {NAME} \
             gone.md"
        )),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let o = koto_command::koto()
            .env_clear()
            .current_dir(&ws)
            .env("PATH", format!("{}:{}", koto_dir().display(), SYSTEM_PATH))
            .env("HOME", &home)
            .args(args)
            .output()
            .unwrap();
        Host::out(o)
    };
    assert!(run(&["init", NAME, "--template", TEMPLATE_FILE]).ok());
    let out = run(&["next", NAME]);
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());
    let session = home.join(".koto").join("sessions").join(NAME);
    assert_eq!(
        std::fs::read(session.join("ctx").join("one.md")).unwrap(),
        b"one"
    );
    assert!(!session.join("cloud-batch.jsonl").exists());
    assert!(!session.join("cloud-batch.lock").exists());
}

/// Poll `ready` until it holds, for at most `limit`.
fn wait_for(limit: Duration, what: &str, ready: impl Fn() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kill `pid`, a process this test's own action started, and wait until it
/// is gone.
fn kill_and_reap(pid: libc::pid_t) {
    // SAFETY: plain syscalls on one pid, read from the action's pid file.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    wait_for(Duration::from_secs(10), "the killed write to exit", || {
        // A zombie still answers; the action's shell reaps it.
        unsafe { libc::kill(pid, 0) != 0 }
    });
}

/// An action whose one context write runs in the background, with its PID
/// in `pid_file`, so the test can stop it part way.
fn background_write(pid_file: &Path) -> String {
    format!(
        "printf one > one.txt\nkoto context add {NAME} one.md < one.txt &\necho $! > {}\nwait",
        pid_file.display()
    )
}

/// A `koto context` call killed after the batch-open marker reached the
/// bucket and before its journal was opened used to leave the marker with
/// nothing on its host that would push or close it, and every import
/// refused until some later batch came along (koto#381). The call now
/// writes an opening file before the marker, and the next koto command on
/// that host clears both, so the import goes through, without the write
/// that never happened.
#[test]
fn a_write_killed_between_marker_and_journal_is_cleared_by_the_next_command() {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    let pid_file = a.ws.join("write.pid");
    a.init(&background_write(&pid_file));
    assert!(a.add("base.md", b"base").ok());

    // The write stops with the marker stored and its reply held back.
    let marker = a.key("batch-open.json");
    s3.hold("PUT", &marker, Duration::from_secs(120));
    let mut tick = a
        .std_cmd(&["next", NAME])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(Duration::from_secs(60), "the marker PUT", || {
        s3.object(&marker).is_some()
            && std::fs::read_to_string(&pid_file).is_ok_and(|p| p.ends_with('\n'))
    });
    let write: libc::pid_t = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The tick first, so its exit flush can't run; then the write.
    tick.kill().unwrap();
    tick.wait().unwrap();
    kill_and_reap(write);

    assert!(s3.object(&marker).is_some());
    assert!(a.session_dir().join("cloud-batch.opening").exists());
    assert!(!a.session_dir().join("cloud-batch.jsonl").exists());
    let b = Host::new(&s3);
    let ws = a.ws.to_string_lossy().into_owned();
    let import = || b.koto(&["session", "import", NAME, "--from", &ws, "--trust-template"]);
    let out = import();
    assert_eq!(
        out.json()["error"]["code"],
        "import_source_unpushed",
        "{}",
        out.describe()
    );

    let out = a.koto(&["status", NAME]);
    assert!(out.ok(), "status: {}", out.describe());
    assert!(s3.object(&marker).is_none());
    assert!(!a.session_dir().join("cloud-batch.opening").exists());
    assert!(!a.session_dir().join("cloud-batch.jsonl").exists());
    let keys = assert_remote_consistent(&s3, &a);
    assert_eq!(keys, vec!["base.md".to_string()]);

    let out = import();
    assert!(out.ok(), "import: {}", out.describe());
    let out = b.koto(&["context", "get", NAME, "base.md"]);
    assert_eq!(out.stdout, "base", "{}", out.describe());
    assert_eq!(b.koto(&["context", "exists", NAME, "one.md"]).code, Some(1));
}

/// The clearing waits on the batch lock, which a live process opening the
/// batch holds: a command run while the write still waits on its marker
/// PUT leaves the marker to it, and the batch it opens is flushed whole,
/// its content before the marker's DELETE.
#[test]
fn a_batch_a_live_write_is_opening_is_left_to_it() {
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    let pid_file = a.ws.join("write.pid");
    a.init(&background_write(&pid_file));
    assert!(a.add("base.md", b"base").ok());

    let marker = a.key("batch-open.json");
    // Long enough for `koto status` to start and reach the lock while the
    // write still waits; the test waits it out either way.
    s3.hold("PUT", &marker, Duration::from_secs(5));
    let tick = a
        .std_cmd(&["next", NAME])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for(Duration::from_secs(60), "the marker PUT", || {
        s3.object(&marker).is_some()
    });
    s3.clear_requests();
    let out = a.koto(&["status", NAME]);
    assert!(out.ok(), "status: {}", out.describe());
    // `status` waited on the lock for the write and then flushed (or waited
    // for the tick's flush): a status that cleared the marker without
    // waiting would return while the write was still held.
    assert!(
        s3.object(&a.key("ctx/one.md")).is_some(),
        "status returned before the write it should have waited for"
    );
    let out = Host::out(tick.wait_with_output().unwrap());
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());

    let requests: Vec<String> = s3.requests().iter().map(describe).collect();
    let content = requests
        .iter()
        .position(|r| r.starts_with("PUT ") && r.ends_with("/ctx/one.md"))
        .unwrap_or_else(|| panic!("the write was never pushed: {:#?}", requests));
    let first_delete = requests
        .iter()
        .position(|r| r == &format!("DELETE {}", marker))
        .unwrap_or_else(|| panic!("the marker was never deleted: {:#?}", requests));
    assert!(
        content < first_delete,
        "the marker went before the write it marks: {:#?}",
        requests
    );
    assert!(s3.object(&marker).is_none());
    let keys = assert_remote_consistent(&s3, &a);
    assert_eq!(keys, vec!["base.md".to_string(), "one.md".to_string()]);
}

/// A batch whose first write couldn't save its version increment still
/// counts as a change on this host: when another host pushes while the
/// batch is open, the flush refuses with the conflict and leaves the bucket
/// alone, instead of reading the remote as newer and writing over it
/// (koto#381).
#[test]
fn a_failed_first_version_save_still_flushes_into_a_conflict() {
    // SAFETY: a plain syscall with no arguments.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root writes a read-only version file anyway");
        return;
    }
    let other = br#"{"version":9,"last_sync_base":9,"machine_id":"otherhost"}"#;
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3);
    let version_path = a.session_dir().join("version.json");
    a.init(&format!(
        "set -e\nchmod a-w {v}\nprintf one | koto context add {NAME} one.md\nchmod u+w {v}",
        v = version_path.display()
    ));
    assert!(a.add("base.md", b"base").ok());
    let before = std::fs::read(&version_path).unwrap();

    // Another host pushes right after the batch's first write checked.
    s3.put_after("GET", &a.key("version.json"), &a.key("version.json"), other);
    let out = a.next();
    assert!(
        out.stderr.contains("session conflict"),
        "{}",
        out.describe()
    );
    // The first write's save did fail: the local version never moved.
    assert_eq!(std::fs::read(&version_path).unwrap(), before);
    assert!(s3.object(&a.key("ctx/one.md")).is_none());
    assert_eq!(s3.object(&a.key("version.json")).unwrap(), other);
    assert!(s3.object(&a.key("batch-open.json")).is_some());
    assert!(a.session_dir().join("cloud-batch.jsonl").exists());
}
