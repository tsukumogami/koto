//! A top-level `koto next` holds its session's `tick.lock` shared until it
//! exits, and writes nothing before the checks that can refuse it.
//!
//! The lock is what a whole-binding move takes exclusive, so a move either
//! waits for a running tick or the tick waits for the move. Plain ticks
//! share it and never wait on each other. A tick that can't get it within
//! its bound refuses `concurrent_access` and leaves the session as it was;
//! `KOTO_TEST_TICK_WAIT_MS` shortens that bound here.
//!
//! A tick refused because it runs outside its session's anchor leaves no
//! held-by link for `KOTO_ANCHOR`. A session whose header records a whole
//! move (`origin.whole_epoch`) refuses writes from under an anchor the move
//! retired, and when the directory is under both a retired and the current
//! anchor, the deeper of the two decides. A session no whole move fenced
//! keeps today's local behaviour after a plain rebind. Nothing in this
//! build performs a whole move, so the tests write that header by hand.
//!
//! The binary is driven directly, since the lock is held across processes.
//! Every command runs under a timeout, so a regression fails here instead
//! of hanging the suite.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const NAME: &str = "wf";
const KEY: &str = "notes.md";
const CONTENT: &[u8] = b"some notes";
const TEMPLATE_FILE: &str = "wf.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const GO: &str = r#"{"choice":"go"}"#;
const WAIT_ENV: &str = "KOTO_TEST_TICK_WAIT_MS";
const TICK_LOCK: &str = "tick.lock";

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// How long a tick may take to reach its gate.
const START_BOUND: Duration = Duration::from_secs(20);

/// Two states that take evidence, so a tick with `GO` advances and stops
/// short of the terminal state.
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
      - target: more
        when:
          choice: go
  more:
    accepts:
      verdict:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          verdict: ok
  done:
    terminal: true
---

## start

Decide.

## more

Decide again.

## done

Done.
"#;

/// A template whose first state's gate notes that it started, in a file of
/// its own under `gate_dir`, then waits for `gate_dir/release`. A tick of
/// it stays in the gate until the test writes that file.
fn gated_template(gate_dir: &Path) -> String {
    let dir = gate_dir.display();
    format!(
        r#"---
name: paused
version: "1.0"
initial_state: hold
states:
  hold:
    gates:
      release:
        type: command
        command: 'touch {dir}/started.$$; while [ ! -f {dir}/release ]; do sleep 0.05; done'
        timeout: 60
    transitions:
      - target: work
        when:
          gates.release.exit_code: 0
  work:
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

## hold

Wait for the release file.

## work

Decide.

## done

Done.
"#
    )
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// One scratch host: a koto home of its own and a workspace holding the
/// template. Every path is canonical.
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

    fn write_template(&self, template: &str) {
        std::fs::write(self.ws.join(TEMPLATE_FILE), template).unwrap();
    }

    /// The session's directory on the local backend, under `KOTO_HOME`.
    fn session_dir(&self, name: &str) -> PathBuf {
        self.root.join("koto").join("sessions").join(name)
    }

    fn log(&self, name: &str) -> PathBuf {
        self.session_dir(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn tick_lock(&self, name: &str) -> PathBuf {
        self.session_dir(name).join(TICK_LOCK)
    }

    /// koto on the local backend, run in `cwd`, with nothing inherited, so
    /// no `KOTO_ANCHOR`, backend, tick or wait variable of the shell
    /// running the suite reaches it.
    fn koto_at(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(cwd)
            .env("PATH", SYSTEM_PATH)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("KOTO_HOME", self.root.join("koto"))
            .args(args);
        cmd
    }

    /// koto run in the workspace.
    fn koto(&self, args: &[&str]) -> Command {
        self.koto_at(&self.ws, args)
    }

    /// Create session `name` from `cwd`, which becomes its anchor.
    fn init_at(&self, cwd: &Path, name: &str) {
        let template = self.ws.join(TEMPLATE_FILE);
        let out = run(self.koto_at(
            cwd,
            &["init", name, "--template", template.to_str().unwrap()],
        ));
        assert_eq!(out.code, Some(0), "init {name}: {}", out.describe());
    }

    /// `koto context add <name> KEY` from `cwd`, with [`CONTENT`] on stdin.
    fn add_at(&self, cwd: &Path, name: &str) -> Out {
        let mut cmd = self.koto_at(cwd, &["context", "add", name, KEY]);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(CONTENT).unwrap();
        finish(child, Instant::now())
    }
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    /// stdout as JSON: the whole of it, else its last non-empty line.
    fn json(&self) -> serde_json::Value {
        if let Ok(v) = serde_json::from_str(self.stdout.trim()) {
            return v;
        }
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line).unwrap_or(serde_json::Value::Null)
    }

    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
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

/// Run `cmd` to completion, killed after [`BOUND`].
fn run(cmd: Command) -> Out {
    finish(spawn(cmd), Instant::now())
}

fn spawn(mut cmd: Command) -> Child {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(child: Child, started: Instant) -> Out {
    finish_within(child, started, BOUND)
}

/// Wait for `child`, killed once `bound` has passed since `started`.
fn finish_within(mut child: Child, started: Instant, bound: Duration) -> Out {
    loop {
        if child.try_wait().unwrap().is_some() {
            let o = child.wait_with_output().unwrap();
            return Out {
                code: o.status.code(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
        if started.elapsed() > bound {
            let _ = child.kill();
            let _ = child.wait();
            return Out {
                code: None,
                stdout: String::new(),
                stderr: format!("did not finish within {bound:?}"),
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open_lock_file(path: &Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap()
}

/// Take an exclusive `flock` on `path`, as a whole move would. It is held
/// until the returned file closes.
fn hold_lock(path: &Path) -> std::fs::File {
    let file = open_lock_file(path);
    // SAFETY: the fd belongs to `file`, which outlives the call.
    assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
    file
}

/// Whether a `flock` of kind `op` (`LOCK_EX` or `LOCK_SH`) on `path` can be
/// had at once. The probe's lock is released when its file closes.
fn try_lock(path: &Path, op: libc::c_int) -> bool {
    let file = open_lock_file(path);
    // SAFETY: the fd belongs to `file`, which outlives the call.
    unsafe { libc::flock(file.as_raw_fd(), op | libc::LOCK_NB) == 0 }
}

/// Ticks of session [`NAME`] paused in the gate of [`gated_template`].
/// Dropping it releases the gate and kills any tick still running, so a
/// failed assertion leaves nothing behind.
struct Paused {
    gate_dir: PathBuf,
    ticks: Vec<(Child, Instant)>,
}

impl Paused {
    /// Start `n` ticks at once and wait until each is inside the gate.
    fn start(host: &Host, gate_dir: &Path, n: usize) -> Self {
        let mut paused = Paused {
            gate_dir: gate_dir.to_path_buf(),
            ticks: Vec::new(),
        };
        for _ in 0..n {
            let child = spawn(host.koto(&["next", NAME]));
            paused.ticks.push((child, Instant::now()));
        }
        let started = Instant::now();
        while paused.started() < n {
            if let Some(i) = paused
                .ticks
                .iter_mut()
                .position(|(c, _)| c.try_wait().unwrap().is_some())
            {
                let (child, at) = paused.ticks.remove(i);
                let out = finish(child, at);
                panic!("a tick exited before its gate held it: {}", out.describe());
            }
            assert!(
                started.elapsed() < START_BOUND,
                "only {} of {n} ticks reached the gate within {START_BOUND:?}",
                paused.started()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        paused
    }

    /// How many gate runs have started.
    fn started(&self) -> usize {
        std::fs::read_dir(&self.gate_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("started."))
            .count()
    }

    /// How many of the ticks are still running.
    fn running(&mut self) -> usize {
        self.ticks
            .iter_mut()
            .map(|(c, _)| c.try_wait().unwrap().is_none())
            .filter(|&running| running)
            .count()
    }

    /// Let the gate pass and wait for every tick.
    fn release(mut self) -> Vec<Out> {
        std::fs::write(self.gate_dir.join("release"), b"").unwrap();
        std::mem::take(&mut self.ticks)
            .into_iter()
            .map(|(child, at)| finish(child, at))
            .collect()
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        let _ = std::fs::write(self.gate_dir.join("release"), b"");
        for (child, _) in &mut self.ticks {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Every file under `dir` but its lock files, by relative path.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if !path.to_string_lossy().ends_with(".lock") {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn header(log: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(log).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

/// Rewrite the header of the log at `log` as a whole move from `retired`
/// to `current` would leave it: the anchor and `execution_dir` at
/// `current`, `retired` the one retired anchor, and both epochs at 1. The
/// events after the header are kept byte for byte.
fn fence_whole_move(log: &Path, retired: &Path, current: &Path) {
    let text = std::fs::read_to_string(log).unwrap();
    let (first, rest) = text
        .split_once('\n')
        .expect("the state log has no header line");
    let mut header: serde_json::Value =
        serde_json::from_str(first).expect("the header line isn't JSON");
    let current = serde_json::Value::from(current.to_str().unwrap());
    header["execution_dir"] = current.clone();
    let origin = header
        .get_mut("origin")
        .and_then(serde_json::Value::as_object_mut)
        .expect("koto init recorded no origin");
    origin.insert("anchor".into(), current);
    origin.insert("anchor_epoch".into(), 1.into());
    origin.insert("whole_epoch".into(), 1.into());
    origin.insert(
        "retired_anchors".into(),
        serde_json::json!([retired.to_str().unwrap()]),
    );
    let mut out = serde_json::to_string(&header).unwrap();
    out.push('\n');
    out.push_str(rest);
    std::fs::write(log, out).unwrap();
}

/// How many `context_added` events for [`KEY`] the log at `log` holds.
fn context_added(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["type"] == "context_added" && e["payload"]["key"] == KEY)
        .count()
}

/// Whether the context manifest of the session at `session_dir` lists
/// [`KEY`]; `false` when it has no manifest.
fn indexed(session_dir: &Path) -> bool {
    let Ok(bytes) = std::fs::read(session_dir.join("ctx").join("manifest.json")) else {
        return false;
    };
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["keys"].get(KEY).is_some()
}

/// Assert that the context add `out` from `cwd` stored [`KEY`] in `name`.
fn assert_stored(host: &Host, name: &str, out: &Out, cwd: &Path) {
    assert_eq!(
        out.code,
        Some(0),
        "context add from {}: {}",
        cwd.display(),
        out.describe()
    );
    assert_eq!(context_added(&host.log(name)), 1, "{}", out.describe());
    assert!(indexed(&host.session_dir(name)), "{}", out.describe());
}

/// Assert that the context add `out` from `cwd` was refused by the fence
/// and stored nothing in `name`.
fn assert_fenced(host: &Host, name: &str, out: &Out, cwd: &Path) {
    let dir = host.session_dir(name);
    assert!(
        out.code.is_some() && out.code != Some(0),
        "context add from {} wasn't refused: {}",
        cwd.display(),
        out.describe()
    );
    assert!(out.all().contains("session_rebound"), "{}", out.describe());
    assert_eq!(context_added(&host.log(name)), 0, "{}", out.describe());
    assert!(!indexed(&dir), "the key is in the manifest");
    assert!(
        !dir.join("ctx").join(KEY).exists(),
        "the key's content was written"
    );
}

/// The names of the held-by links in `session`.
fn held_by(host: &Host, session: &str) -> Vec<String> {
    let dir = host
        .session_dir(session)
        .join("ctx")
        .join("koto")
        .join("links")
        .join("held-by");
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.ends_with(".lock"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn a_tick_holds_its_sessions_tick_lock_shared_until_it_exits() {
    let host = Host::new();
    let gate = mk(host.root.join("gate"));
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let lock = host.tick_lock(NAME);
    let made_by_init = lock.is_file();

    let paused = Paused::start(&host, &gate, 1);
    let exclusive = try_lock(&lock, libc::LOCK_EX);
    let shared = try_lock(&lock, libc::LOCK_SH);
    let outs = paused.release();
    let tick = &outs[0];
    let after = try_lock(&lock, libc::LOCK_EX);

    assert!(
        !exclusive,
        "an exclusive probe got tick.lock while the tick ran: {}",
        tick.describe()
    );
    assert!(
        shared,
        "a shared probe was refused while the tick ran: {}",
        tick.describe()
    );
    assert_eq!(tick.code, Some(0), "next: {}", tick.describe());
    assert_eq!(tick.json()["state"], "work", "{}", tick.describe());
    assert!(after, "tick.lock is still held after the tick exited");
    assert!(made_by_init, "koto init made no tick.lock in the session");
}

#[test]
fn two_plain_ticks_run_side_by_side_under_the_tick_lock() {
    // A session that isn't batch-scoped takes no state-file lock in its
    // tick, so two ticks of it can both be inside the gate at once. The
    // tick lock is shared and must not change that.
    let host = Host::new();
    let gate = mk(host.root.join("gate"));
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let lock = host.tick_lock(NAME);

    let mut paused = Paused::start(&host, &gate, 2);
    let in_gate = paused.started();
    let running = paused.running();
    let exclusive = try_lock(&lock, libc::LOCK_EX);
    let outs = paused.release();

    assert_eq!(in_gate, 2, "both ticks are not in the gate at once");
    assert_eq!(running, 2, "a tick exited while both were in the gate");
    assert!(
        !exclusive,
        "an exclusive probe got tick.lock while two ticks ran: {} / {}",
        outs[0].describe(),
        outs[1].describe()
    );
    for tick in &outs {
        assert_eq!(tick.code, Some(0), "next: {}", tick.describe());
        assert_ne!(
            tick.json()["error"]["code"],
            "concurrent_access",
            "{}",
            tick.describe()
        );
    }
}

#[test]
fn a_tick_that_meets_an_exclusive_holder_past_its_wait_refuses_concurrent_access_and_writes_nothing(
) {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let dir = host.session_dir(NAME);
    let log_before = std::fs::read(host.log(NAME)).unwrap();
    let files_before = files(&dir);

    let holder = hold_lock(&host.tick_lock(NAME));
    let mut cmd = host.koto(&["next", NAME, "--with-data", GO]);
    cmd.env(WAIT_ENV, "300");
    let started = Instant::now();
    let out = run(cmd);
    let waited = started.elapsed();
    drop(holder);

    assert_eq!(out.code, Some(1), "next: {}", out.describe());
    let json = out.json();
    assert_eq!(
        json["error"]["code"],
        "concurrent_access",
        "{}",
        out.describe()
    );
    assert_eq!(
        json["error"]["details"],
        serde_json::json!([]),
        "{}",
        out.describe()
    );
    assert!(
        waited >= Duration::from_millis(300),
        "the tick refused before its wait ran out, after {waited:?}"
    );
    assert_eq!(
        std::fs::read(host.log(NAME)).unwrap(),
        log_before,
        "the refused tick changed the state log"
    );
    assert_eq!(files(&dir), files_before, "the refused tick wrote a file");
}

#[test]
fn a_tick_whose_exclusive_holder_lets_go_within_the_wait_proceeds() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let log = host.log(NAME);
    let before = std::fs::read(&log).unwrap();

    let holder = hold_lock(&host.tick_lock(NAME));
    let mut cmd = host.koto(&["next", NAME, "--with-data", GO]);
    cmd.env(WAIT_ENV, "10000");
    let started = Instant::now();
    let mut tick = spawn(cmd);
    std::thread::sleep(Duration::from_millis(500));
    let finished_early = tick.try_wait().unwrap().is_some();
    let log_while_held = std::fs::read(&log).unwrap();
    drop(holder);
    let out = finish(tick, started);

    assert!(
        !finished_early,
        "the tick finished while another process held tick.lock exclusive: {}",
        out.describe()
    );
    assert_eq!(
        log_while_held, before,
        "the tick wrote while it waited for tick.lock"
    );
    assert_eq!(out.code, Some(0), "next: {}", out.describe());
    let json = out.json();
    assert_eq!(json["state"], "more", "{}", out.describe());
    assert_eq!(json["advanced"], true, "{}", out.describe());
    assert_ne!(
        std::fs::read(&log).unwrap(),
        before,
        "the tick wrote nothing"
    );
}

#[test]
fn a_tick_refused_outside_its_anchor_writes_no_held_by_link() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let out = run(host.koto(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        "h1",
        "--source",
        "startup",
    ]));
    assert_eq!(out.code, Some(0), "anchor open: {}", out.describe());
    let anchor = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .expect("anchor open printed no anchor line")
        .to_string();
    let outside = mk(host.root.join("outside"));
    let session_before = files(&host.session_dir(NAME));
    let anchor_before = files(&host.session_dir(&anchor));

    let mut cmd = host.koto_at(&outside, &["next", NAME]);
    cmd.env("KOTO_ANCHOR", &anchor);
    let out = run(cmd);

    assert_eq!(out.code, Some(2), "next: {}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "execution_anchor_mismatch",
        "{}",
        out.describe()
    );
    assert!(
        held_by(&host, NAME).is_empty(),
        "the refused tick recorded that {NAME} is held by {anchor}"
    );
    assert_eq!(
        files(&host.session_dir(NAME)),
        session_before,
        "the refused tick wrote to the session"
    );
    assert_eq!(
        files(&host.session_dir(&anchor)),
        anchor_before,
        "the refused tick wrote to the anchor"
    );

    // From inside the anchor the same tick records the link, so the check
    // above looks where a link would be.
    let mut cmd = host.koto(&["next", NAME]);
    cmd.env("KOTO_ANCHOR", &anchor);
    let out = run(cmd);
    assert_eq!(
        out.code,
        Some(0),
        "next from the anchor: {}",
        out.describe()
    );
    assert_eq!(held_by(&host, NAME), vec![anchor]);
}

#[test]
fn a_tick_from_under_a_retired_anchor_is_refused_session_rebound_before_writing() {
    // The move went to a parent: the tick's directory is under the current
    // anchor, so it passes the anchor check, and under the retired one,
    // which is deeper and decides.
    let host = Host::new();
    let parent = mk(host.ws.join("a"));
    let child = mk(parent.join("b"));
    host.init_at(&child, NAME);
    fence_whole_move(&host.log(NAME), &child, &parent);
    let dir = host.session_dir(NAME);
    let log_before = std::fs::read(host.log(NAME)).unwrap();
    let files_before = files(&dir);

    let out = run(host.koto_at(&child, &["next", NAME, "--with-data", GO]));

    assert_eq!(out.code, Some(2), "next: {}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "session_rebound",
        "{}",
        out.describe()
    );
    assert_eq!(
        std::fs::read(host.log(NAME)).unwrap(),
        log_before,
        "the refused tick changed the state log"
    );
    assert_eq!(files(&dir), files_before, "the refused tick wrote a file");

    // From the current anchor the tick goes ahead.
    let out = run(host.koto_at(&parent, &["next", NAME, "--with-data", GO]));
    assert_eq!(
        out.code,
        Some(0),
        "next from the anchor: {}",
        out.describe()
    );
    assert_eq!(out.json()["state"], "more", "{}", out.describe());
}

#[test]
fn a_context_add_from_under_a_retired_anchor_stores_nothing() {
    let host = Host::new();
    let old = mk(host.ws.join("old"));
    let new = mk(host.ws.join("new"));
    host.init_at(&old, NAME);
    fence_whole_move(&host.log(NAME), &old, &new);

    let out = host.add_at(&old, NAME);
    assert_fenced(&host, NAME, &out, &old);

    let out = host.add_at(&new, NAME);
    assert_stored(&host, NAME, &out, &new);
}

#[test]
fn the_deeper_of_a_retired_and_the_current_anchor_decides() {
    let host = Host::new();
    let a = mk(host.ws.join("a"));
    let b = mk(a.join("b"));

    // Moved up from a/b to a: a/b is under both, and the retired anchor is
    // the deeper one, so it refuses; a is under the current anchor only.
    host.init_at(&b, "up");
    fence_whole_move(&host.log("up"), &b, &a);
    let out = host.add_at(&b, "up");
    assert_fenced(&host, "up", &out, &b);
    let out = host.add_at(&a, "up");
    assert_stored(&host, "up", &out, &a);

    // Moved down from a to a/b: a/b is under both, and the current anchor
    // is the deeper one, so it writes; a is under the retired anchor only.
    host.init_at(&a, "down");
    fence_whole_move(&host.log("down"), &a, &b);
    let out = host.add_at(&a, "down");
    assert_fenced(&host, "down", &out, &a);
    let out = host.add_at(&b, "down");
    assert_stored(&host, "down", &out, &b);
}

#[test]
fn a_session_no_whole_move_fenced_keeps_todays_local_behaviour_after_a_plain_rebind() {
    // A pin: this holds before the tick lock and the whole-move fence
    // exist, and must hold after.
    let host = Host::new();
    let old = mk(host.ws.join("old"));
    let new = mk(host.ws.join("new"));
    host.init_at(&old, NAME);

    let out = run(host.koto_at(
        &new,
        &["session", "rebind", NAME, "--to", new.to_str().unwrap()],
    ));
    assert_eq!(out.code, Some(0), "rebind: {}", out.describe());
    assert_eq!(out.json()["rebound"], true, "{}", out.describe());
    let h = header(&host.log(NAME));
    assert_eq!(h["execution_dir"], new.to_str().unwrap());
    assert_eq!(h["origin"]["anchor_epoch"], 1);
    assert_eq!(
        h["origin"]["retired_anchors"],
        serde_json::json!([old.to_str().unwrap()])
    );
    assert!(
        h["origin"].get("whole_epoch").is_none(),
        "a plain rebind recorded a whole move: {h}"
    );

    // The local backend doesn't fence a plain rebind's old anchor.
    let out = host.add_at(&old, NAME);
    assert_stored(&host, NAME, &out, &old);

    // The tick's anchor check refuses it, as it always has.
    let log_before = std::fs::read(host.log(NAME)).unwrap();
    let out = run(host.koto_at(&old, &["next", NAME, "--with-data", GO]));
    assert_eq!(out.code, Some(2), "next: {}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "execution_anchor_mismatch",
        "{}",
        out.describe()
    );
    assert_eq!(std::fs::read(host.log(NAME)).unwrap(), log_before);
}
