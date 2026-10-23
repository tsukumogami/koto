//! What a read costs on the cloud backend (koto#428).
//!
//! A read used to list the session's migration marker, fetch its whole state
//! file and its manifest, and push the whole state file back to log the
//! read: four sequential requests whatever had changed, so an action making
//! dozens of reads ran past its time limit on round trips alone. Now:
//!
//! - one listing of the session's prefix answers the marker and the ETags of
//!   the state file and the manifest, and nothing is fetched when they match
//!   what this host last pulled or pushed;
//! - inside a tick, a session the tick already checked costs no request;
//! - a read of a session by anything but its own tick is spooled beside it
//!   on this host, never pushed.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`. Each simulated host is its own `HOME` and
//! workspace; another host's push is made straight into the bucket.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use assert_cmd::Command;
use fake_s3::FakeS3;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BUCKET: &str = "koto-reads";
const WORKER: &str = "worker";
const HUB: &str = "hub";
const TEMPLATE_FILE: &str = "flow.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// A workflow whose first state runs `script` as its default action and
/// then waits for evidence.
fn template(script: &str) -> String {
    let body: String = script.lines().map(|l| format!("        {}\n", l)).collect();
    format!(
        r#"---
name: flow
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
    PathBuf::from(env!("CARGO_BIN_EXE_koto"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The remote prefix koto derives for a workspace.
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
    fn ok(&self) -> bool {
        self.code == Some(0)
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

/// One simulated host: its own `HOME` and template cache, a workspace, and
/// the backend it runs koto with.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
    endpoint: String,
    cloud: bool,
}

impl Host {
    fn new(s3: &FakeS3, cloud: bool) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let ws = root.path().join("ws");
        Self::at(s3, cloud, root, ws)
    }

    fn at(s3: &FakeS3, cloud: bool, root: tempfile::TempDir, ws: PathBuf) -> Self {
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Host {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            ws: mk(ws),
            _root: root,
            endpoint: s3.endpoint(),
            cloud,
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.env_clear()
            .current_dir(&self.ws)
            .env("PATH", format!("{}:{}", koto_dir().display(), SYSTEM_PATH))
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .args(args);
        if self.cloud {
            cmd.env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        } else {
            cmd.env("KOTO_SESSION_BACKEND", "local");
        }
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
        assert!(out.ok(), "koto {:?}: {}", args, out.describe());
        out
    }

    /// Init `name` on a workflow whose action is `script`.
    fn init(&self, name: &str, script: &str) {
        let file = format!("{name}-{TEMPLATE_FILE}");
        std::fs::write(self.ws.join(&file), template(script)).unwrap();
        self.ok(&["init", name, "--template", &file]);
    }

    fn add(&self, name: &str, key: &str, content: &[u8]) {
        let out = self
            .cmd(&["context", "add", name, key])
            .write_stdin(content.to_vec())
            .output()
            .unwrap();
        assert!(out.status.success(), "context add: {:?}", out);
    }

    fn prefix(&self) -> String {
        prefix_of(&self.ws)
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.home.join(".koto").join("sessions").join(name)
    }
}

/// A recorded request as `METHOD key`, or `LIST prefix`, with the host's
/// prefix cut off.
fn describe(r: &fake_s3::Request, prefix: &str) -> String {
    let cut = |s: &str| {
        s.strip_prefix(&format!("{prefix}/"))
            .unwrap_or(s)
            .to_string()
    };
    match &r.list_prefix {
        Some(p) => format!("LIST {}", cut(p)),
        None => format!("{} {}", r.method, cut(&r.key)),
    }
}

/// The requests the bucket answered while `f` ran.
fn requests_during(s3: &FakeS3, prefix: &str, f: impl FnOnce()) -> Vec<String> {
    s3.clear_requests();
    f();
    s3.requests().iter().map(|r| describe(r, prefix)).collect()
}

/// A worker session holding `k.md`, read once so this host has stamped it.
fn worker_with_a_key(h: &Host) {
    h.init(WORKER, "true");
    h.add(WORKER, "k.md", b"the value");
    h.ok(&["context", "get", WORKER, "k.md"]);
}

/// A read whose local copy matches the bucket lists the session's prefix and
/// fetches nothing, and logs nothing into the session or the bucket.
#[test]
fn a_current_read_costs_one_listing() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    worker_with_a_key(&h);
    let state = h
        .session_dir(WORKER)
        .join(format!("koto-{WORKER}.state.jsonl"));
    let before = std::fs::read(&state).unwrap();

    for args in [
        vec!["context", "get", WORKER, "k.md"],
        vec!["context", "exists", WORKER, "k.md"],
        vec!["context", "list", WORKER],
        vec!["status", WORKER],
    ] {
        let reqs = requests_during(&s3, &h.prefix(), || {
            h.ok(&args);
        });
        let worker_reqs: Vec<&String> = reqs.iter().filter(|r| r.contains(WORKER)).collect();
        assert_eq!(
            worker_reqs,
            vec![&format!("LIST {WORKER}/")],
            "koto {:?} made {:?}",
            args,
            reqs
        );
        assert!(
            !reqs.iter().any(|r| r.starts_with("PUT")),
            "koto {:?} wrote: {:?}",
            args,
            reqs
        );
    }
    assert_eq!(
        std::fs::read(&state).unwrap(),
        before,
        "a read wrote nothing into the session's log"
    );
}

/// A read after another host changed a key fetches the new manifest and the
/// key, and sees the new value. The other host's push is made straight into
/// the bucket: the key's object and its manifest entry, as a push leaves
/// them.
#[test]
fn a_read_after_another_host_wrote_fetches_what_changed() {
    use sha2::{Digest, Sha256};
    let s3 = FakeS3::start(BUCKET);
    let a = Host::new(&s3, true);
    worker_with_a_key(&a);
    let key = |rest: &str| format!("{}/{WORKER}/{rest}", a.prefix());
    let value = b"changed elsewhere";
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&key("ctx/manifest.json")).unwrap()).unwrap();
    manifest["keys"]["k.md"]["hash"] = hex::encode(Sha256::digest(value)).into();
    s3.put(&key("ctx/k.md"), value);
    s3.put(
        &key("ctx/manifest.json"),
        &serde_json::to_vec(&manifest).unwrap(),
    );

    let mut out = None;
    let reqs = requests_during(&s3, &a.prefix(), || {
        out = Some(a.ok(&["context", "get", WORKER, "k.md"]));
    });
    assert_eq!(out.unwrap().stdout, "changed elsewhere");
    for expected in [
        format!("LIST {WORKER}/"),
        format!("GET {WORKER}/ctx/manifest.json"),
        format!("GET {WORKER}/ctx/k.md"),
    ] {
        assert!(reqs.contains(&expected), "missing {expected}: {reqs:?}");
    }
    // The pull wrote the local store, which is logged as a `context_added`
    // by `sync` and pushed with the state file, as before this change; the
    // read itself is spooled. Nothing else is written. That push is a write
    // to a session this host only read, which koto#431 removes; this pins
    // today's behaviour until then.
    let puts: Vec<&String> = reqs.iter().filter(|r| r.starts_with("PUT")).collect();
    assert_eq!(
        puts,
        vec![&format!("PUT {WORKER}/koto-{WORKER}.state.jsonl")],
        "{reqs:?}"
    );
}

/// A read of a session by anything but its own tick is spooled beside it
/// on this host: never in its log, never in the bucket.
#[test]
fn a_read_of_another_session_is_spooled_and_never_pushed() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    worker_with_a_key(&h);
    let remote_state = format!("{}/{WORKER}/koto-{WORKER}.state.jsonl", h.prefix());
    let before = s3.object(&remote_state).unwrap();

    h.ok(&["context", "get", WORKER, "k.md"]);

    let spool = std::fs::read_to_string(h.session_dir(WORKER).join("context-reads.jsonl")).unwrap();
    assert!(
        spool
            .lines()
            .any(|l| l.contains("\"context_read\"") && l.contains("\"k.md\"")),
        "the read is in the spool: {spool}"
    );
    assert_eq!(s3.object(&remote_state).unwrap(), before);
    assert!(s3
        .keys()
        .iter()
        .all(|k| !k.ends_with("context-reads.jsonl")));
}

/// The script of an action reading `k.md` from the worker `n` times.
fn reads_script(n: usize) -> String {
    let mut lines = vec!["set -e".to_string()];
    for _ in 0..n {
        lines.push(format!("koto context get {WORKER} k.md > /dev/null"));
    }
    lines.join("\n")
}

/// Inside a tick, reads of another session after the first make no request:
/// the tick checked it once.
#[test]
fn reads_of_another_session_in_a_tick_check_it_once() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    worker_with_a_key(&h);
    h.init(HUB, &reads_script(10));

    let reqs = requests_during(&s3, &h.prefix(), || {
        h.ok(&["next", HUB]);
    });
    let worker_reqs: Vec<&String> = reqs
        .iter()
        .filter(|r| r.contains(&format!("{WORKER}/")))
        .collect();
    assert_eq!(
        worker_reqs,
        vec![&format!("LIST {WORKER}/")],
        "ten reads in one tick: {reqs:?}"
    );
}

/// A tick whose action reads another session 40 times costs at most twice
/// the same tick on the local backend, against a bucket that takes
/// `LATENCY` to answer each request.
#[test]
fn a_forty_read_action_on_the_cloud_backend_is_within_twice_local() {
    const LATENCY: Duration = Duration::from_millis(150);
    const READS: usize = 40;

    let s3 = FakeS3::start(BUCKET);
    let cloud = Host::new(&s3, true);
    let local = Host::new(&s3, false);
    for h in [&cloud, &local] {
        worker_with_a_key(h);
        h.init(HUB, &reads_script(READS));
    }
    s3.set_latency(LATENCY);

    let time = |h: &Host| {
        let start = Instant::now();
        h.ok(&["next", HUB]);
        start.elapsed()
    };
    let local_time = time(&local);
    let cloud_time = time(&cloud);
    s3.set_latency(Duration::ZERO);
    eprintln!(
        "{READS} reads: local {:?}, cloud {:?} at {:?} a request",
        local_time, cloud_time, LATENCY
    );
    assert!(
        cloud_time <= local_time * 2,
        "{READS} reads took {:?} on the cloud backend, over twice the local {:?}",
        cloud_time,
        local_time
    );
}

/// `resolve --keep local` on a 4 MB session reads the bucket's manifest once
/// and uploads only the keys the bucket lacks, then the manifest, the state
/// file and the version, and reports how long it took.
#[test]
fn resolve_keep_local_uploads_a_four_megabyte_session_in_one_pass() {
    const KEYS: usize = 40;
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    h.init(WORKER, "true");
    for i in 0..KEYS {
        h.add(
            WORKER,
            &format!("k{i}.md"),
            &vec![b'a' + (i % 26) as u8; 100 * 1024],
        );
    }
    let key = |rest: &str| format!("{}/{WORKER}/{rest}", h.prefix());
    // Three keys the bucket lacks, objects and manifest entries both: the
    // ones the pass has to send.
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&key("ctx/manifest.json")).unwrap()).unwrap();
    for i in 0..3 {
        s3.remove(&key(&format!("ctx/k{i}.md")));
        manifest["keys"]
            .as_object_mut()
            .unwrap()
            .remove(&format!("k{i}.md"));
    }
    s3.put(
        &key("ctx/manifest.json"),
        &serde_json::to_vec(&manifest).unwrap(),
    );

    let mut out = None;
    let reqs = requests_during(&s3, &h.prefix(), || {
        out = Some(h.ok(&["session", "resolve", WORKER, "--keep", "local"]));
    });
    let response: serde_json::Value = serde_json::from_str(out.unwrap().stdout.trim()).unwrap();
    assert!(response["elapsed_ms"].is_u64(), "{response}");

    let key_puts: Vec<&String> = reqs
        .iter()
        .filter(|r| r.starts_with("PUT") && r.contains("/ctx/k"))
        .collect();
    assert_eq!(key_puts.len(), 3, "only the lost keys go up: {key_puts:?}");
    assert_eq!(
        reqs.iter()
            .filter(|r| *r == &format!("GET {WORKER}/ctx/manifest.json"))
            .count(),
        1,
        "{reqs:?}"
    );
    for i in 0..3 {
        assert!(s3.object(&key(&format!("ctx/k{i}.md"))).is_some());
    }
    for rest in [
        "ctx/manifest.json",
        &format!("koto-{WORKER}.state.jsonl"),
        "version.json",
    ] {
        assert!(
            reqs.contains(&format!("PUT {WORKER}/{rest}")),
            "missing PUT {rest}: {reqs:?}"
        );
    }
}

/// A key this host removes from another session inside a tick is gone for
/// the tick's later reads: the push forgets the manifest cached beside the
/// session, so the tick's freshness never answers from the manifest before
/// the removal.
#[test]
fn a_removal_inside_a_tick_is_seen_by_the_ticks_later_reads() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    worker_with_a_key(&h);
    let script = format!(
        "set -e\n\
         koto context get {WORKER} k.md > /dev/null\n\
         koto context remove {WORKER} k.md\n\
         if koto context exists {WORKER} k.md; then echo stale > stale.txt; fi"
    );
    h.init(HUB, &script);
    h.ok(&["next", HUB]);
    assert!(
        !h.ws.join("stale.txt").exists(),
        "a read after the removal in the same tick still found the key"
    );
}
