//! A context write takes the session's manifest lock before the log's
//! append lock and the cloud batch lock.
//!
//! A gate-passing command, a decision and `koto next --to` past a gate take
//! the manifest lock and then the append lock. A `koto context add` that
//! held the append lock while it waited on the manifest lock could wait
//! forever on one of them while that one waited on the add. Each test holds
//! the manifest lock itself, starts an add, and checks that the add holds
//! none of the locks that come after the manifest lock while it waits.
//!
//! A flush works the other way round: it holds the batch lock and carries a
//! pin record set under the local backend, whose index entry it drops under
//! the manifest lock. Its wait for the manifest lock is bounded, so a flush
//! never waits forever on a context write that waits on its batch lock.
//!
//! The binary is driven directly, since the deadlock is between processes.
//! Every command runs under a timeout, so a regression fails here instead
//! of hanging the suite.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::io::Write as _;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BUCKET: &str = "koto-context-lock-order";
const NAME: &str = "wf";
const KEY: &str = "notes.md";
const PIN_KEY: &str = "koto/links/pin";
const CONTENT: &[u8] = b"some notes";
const TEMPLATE_FILE: &str = "wf.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// How long the add is watched while it waits on the manifest lock, well
/// inside its own ten-second wait for that lock.
const WATCH: Duration = Duration::from_secs(2);

/// How long a flush may take while the manifest lock is held elsewhere:
/// its own bounded wait to carry a pin record, a couple of seconds, and the
/// requests of the push, with room for a loaded host.
const FLUSH_BOUND: Duration = Duration::from_secs(20);

const TEMPLATE: &str = r#"---
name: plain
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          choice: go
  done:
    terminal: true
---

## start

Decide.

## done

Done.
"#;

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// One scratch host: a koto home of its own and a workspace holding the
/// template.
struct Host {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    ws: PathBuf,
}

impl Host {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = mk(tmp.path().to_path_buf());
        mk(root.join("home"));
        mk(root.join("koto"));
        let ws = mk(root.join("ws"));
        std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        Host {
            _tmp: tmp,
            root,
            ws,
        }
    }

    /// The session's directory on the local backend, under `KOTO_HOME`.
    fn session_dir(&self) -> PathBuf {
        self.root.join("koto").join("sessions").join(NAME)
    }

    /// The session's directory on the cloud backend, which runs without
    /// `KOTO_HOME` and keeps its local copy under `HOME`.
    fn cloud_session_dir(&self) -> PathBuf {
        self.root
            .join("home")
            .join(".koto")
            .join("sessions")
            .join(NAME)
    }

    /// koto in the workspace with nothing inherited, so no `KOTO_ANCHOR`,
    /// `KOTO_DISPATCHER_ANCHOR`, backend, tick or write window variable of
    /// the shell running the suite reaches it.
    fn base(&self, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(&self.ws)
            .env("PATH", SYSTEM_PATH)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .args(args);
        cmd
    }

    /// koto on the local backend.
    fn koto(&self, args: &[&str]) -> Command {
        let mut cmd = self.base(args);
        cmd.env("KOTO_HOME", self.root.join("koto"));
        cmd
    }

    /// koto on the cloud backend against `s3`. `KOTO_HOME` stays unset,
    /// since koto ignores the cloud backend under it.
    fn cloud(&self, s3: &FakeS3, args: &[&str]) -> Command {
        let mut cmd = self.base(args);
        cmd.env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint())
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        cmd
    }
}

/// Run `cmd` to completion, killed after [`BOUND`].
fn run(mut cmd: Command) -> (Option<i32>, String) {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    finish(child, Instant::now())
}

fn finish(child: Child, started: Instant) -> (Option<i32>, String) {
    finish_within(child, started, BOUND)
}

/// [`finish`] with the child killed after `bound` instead of [`BOUND`].
fn finish_within(mut child: Child, started: Instant, bound: Duration) -> (Option<i32>, String) {
    loop {
        if child.try_wait().unwrap().is_some() {
            let o = child.wait_with_output().unwrap();
            return (
                o.status.code(),
                format!(
                    "status {:?}; stdout: {}; stderr: {}",
                    o.status,
                    String::from_utf8_lossy(&o.stdout).trim(),
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
            );
        }
        if started.elapsed() > bound {
            let _ = child.kill();
            let _ = child.wait();
            return (None, format!("did not finish within {:?}", bound));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Start `cmd`, a `koto context add`, with [`CONTENT`] on its stdin.
fn spawn_add(mut cmd: Command) -> Child {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(CONTENT).unwrap();
    child
}

/// Take an exclusive `flock` on `path`, as another koto process would. It
/// is held until the returned file closes.
fn hold_lock(path: &Path) -> std::fs::File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    // SAFETY: the fd belongs to `file`, which outlives the call.
    assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
    file
}

/// Whether an exclusive `flock` on `path` can be had at once: no other
/// process holds it. The probe's lock is released when the file closes.
fn lock_is_free(path: &Path) -> bool {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    // SAFETY: the fd belongs to `file`, which outlives the call.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    ret == 0
}

/// Probe each of `locks` while `child` runs, for at most [`WATCH`]: the
/// name of the first one found held, or `None` when every probe found them
/// all free.
fn watch(child: &mut Child, locks: &[(&'static str, PathBuf)]) -> Option<&'static str> {
    let started = Instant::now();
    while started.elapsed() < WATCH && child.try_wait().unwrap().is_none() {
        for (name, path) in locks {
            if !lock_is_free(path) {
                return Some(*name);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// How many `context_added` events for `key` the log at `log` holds.
fn context_added(log: &Path, key: &str) -> usize {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["type"] == "context_added" && e["payload"]["key"] == key)
        .count()
}

/// Whether the context manifest of the session at `session_dir` lists
/// `key`.
fn indexed(session_dir: &Path, key: &str) -> bool {
    let bytes = std::fs::read(session_dir.join("ctx").join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["keys"].get(key).is_some()
}

/// The bytes of session [`NAME`]'s object `rest` (`ctx/<key>`, say) in
/// `s3`, under whatever prefix the workspace has.
fn bucket_object(s3: &FakeS3, rest: &str) -> Option<Vec<u8>> {
    let suffix = format!("/{NAME}/{rest}");
    s3.keys()
        .into_iter()
        .find(|k| k.ends_with(&suffix))
        .and_then(|k| s3.object(&k))
}

#[test]
fn a_context_add_waiting_on_the_manifest_lock_holds_no_append_lock() {
    let host = Host::new();
    let (code, out) = run(host.koto(&["init", NAME, "--template", TEMPLATE_FILE]));
    assert_eq!(code, Some(0), "init: {out}");

    let session_dir = host.session_dir();
    let manifest = hold_lock(&session_dir.join("ctx").join("manifest.lock"));
    let started = Instant::now();
    let mut add = spawn_add(host.koto(&["context", "add", NAME, KEY]));
    let held = watch(
        &mut add,
        &[("the append lock", session_dir.join("append.lock"))],
    );
    drop(manifest);
    let (code, out) = finish(add, started);

    assert!(
        held.is_none(),
        "the add holds {} while it waits on the manifest lock: {out}",
        held.unwrap_or_default()
    );
    assert_eq!(code, Some(0), "context add: {out}");
    assert_eq!(
        context_added(&session_dir.join(format!("koto-{NAME}.state.jsonl")), KEY),
        1,
        "the add's event is not in the log once"
    );
}

#[test]
fn a_cloud_context_write_inside_a_tick_waiting_on_the_manifest_holds_no_batch_lock() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new();
    let (code, out) = run(host.cloud(&s3, &["init", NAME, "--template", TEMPLATE_FILE]));
    assert_eq!(code, Some(0), "init: {out}");

    let session_dir = host.cloud_session_dir();
    assert!(
        session_dir.is_dir(),
        "the cloud session isn't at {}: {out}",
        session_dir.display()
    );
    let manifest = hold_lock(&session_dir.join("ctx").join("manifest.lock"));
    let started = Instant::now();
    // Run as a command of the session's tick, so the write joins its batch.
    let mut cmd = host.cloud(&s3, &["context", "add", NAME, KEY]);
    cmd.env("KOTO_TICK_SESSION", NAME);
    let mut add = spawn_add(cmd);
    let held = watch(
        &mut add,
        &[
            ("the append lock", session_dir.join("append.lock")),
            ("the batch lock", session_dir.join("cloud-batch.lock")),
        ],
    );
    drop(manifest);
    let (code, out) = finish(add, started);

    assert!(
        held.is_none(),
        "the add holds {} while it waits on the manifest lock: {out}",
        held.unwrap_or_default()
    );
    assert_eq!(code, Some(0), "context add: {out}");
    assert_eq!(
        context_added(&session_dir.join(format!("koto-{NAME}.state.jsonl")), KEY),
        1,
        "the add's event is not in the local log once"
    );
}

#[test]
fn a_flush_carrying_a_pin_record_does_not_wait_forever_on_the_manifest() {
    let s3 = FakeS3::start(BUCKET);
    let host = Host::new();
    let (code, out) = run(host.cloud(&s3, &["init", NAME, "--template", TEMPLATE_FILE]));
    assert_eq!(code, Some(0), "init: {out}");
    let session_dir = host.cloud_session_dir();
    let journal = session_dir.join("cloud-batch.jsonl");

    // A write of the session's tick, which waits in its open batch.
    let mut cmd = host.cloud(&s3, &["context", "add", NAME, KEY]);
    cmd.env("KOTO_TICK_SESSION", NAME);
    let (code, out) = finish(spawn_add(cmd), Instant::now());
    assert_eq!(code, Some(0), "context add: {out}");
    assert!(journal.exists(), "the tick's write opened no batch: {out}");
    assert!(
        bucket_object(&s3, &format!("ctx/{KEY}")).is_none(),
        "the tick's write went up before the flush"
    );

    // A pin set under the local backend, on the same store: an indexed
    // record, which the next push carries to the bucket.
    let (code, out) = run(host.base(&["session", "pin", NAME, "--reason", "kept"]));
    assert_eq!(code, Some(0), "session pin: {out}");
    assert!(
        indexed(&session_dir, PIN_KEY),
        "the local pin isn't indexed: {out}"
    );

    // The flush carries the pin holding the batch lock, while another
    // process holds the manifest lock that comes before it.
    let manifest = hold_lock(&session_dir.join("ctx").join("manifest.lock"));
    let started = Instant::now();
    let flush = host
        .cloud(&s3, &["session", "flush", NAME])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (code, out) = finish_within(flush, started, FLUSH_BOUND);
    drop(manifest);

    assert!(
        code.is_some(),
        "the flush waited on the manifest lock with no bound, holding the batch lock: {out}"
    );
    assert_eq!(code, Some(0), "session flush: {out}");
    assert!(out.contains("\"flushed\":true"), "{out}");
    assert!(
        out.contains("stays indexed"),
        "the busy manifest isn't reported: {out}"
    );
    assert!(!journal.exists(), "the batch is still open: {out}");
    assert_eq!(
        bucket_object(&s3, &format!("ctx/{KEY}")).as_deref(),
        Some(CONTENT),
        "the batch's key isn't in the bucket: {out}"
    );
    assert!(
        indexed(&session_dir, PIN_KEY),
        "the pin left the index though the manifest was busy: {out}"
    );

    // With the manifest free, the next push carries it.
    let (code, out) = finish(
        spawn_add(host.cloud(&s3, &["context", "add", NAME, "later.md"])),
        Instant::now(),
    );
    assert_eq!(code, Some(0), "context add: {out}");
    assert!(
        !indexed(&session_dir, PIN_KEY),
        "the next push didn't carry the pin: {out}"
    );
    assert!(
        bucket_object(&s3, &format!("ctx/{PIN_KEY}")).is_some(),
        "the bucket holds no pin object: {out}"
    );
}
