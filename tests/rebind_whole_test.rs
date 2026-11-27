//! `koto session rebind --whole` on the local backend: a takeover by the
//! session's new holder that moves, in one call, the execution anchor, the
//! held-by links, the `/workflows` publish location and the recorded command
//! environment, under `tick.lock` held exclusive.
//!
//! The move waits for a tick in flight (`--wait`, default 10 seconds, `0`
//! refuses at once with `session_busy`), and a tick that arrives while it
//! runs waits for it and then meets the fence. Each part is logged with an
//! event of its own before it is written, so a move stopped part way
//! finishes when run again and no part happens twice. The caller's
//! `KOTO_ANCHOR` must name an anchor (`no_anchor` otherwise), and a move
//! from inside the session's own tick is refused up front
//! (`nested_invocation`).
//!
//! Two debug-build hooks drive the timing: `KOTO_TEST_REBIND_HOLD` names a
//! file the move creates once it holds `tick.lock` exclusive, just before
//! its commit point, and then waits until the test removes it;
//! `KOTO_TEST_REBIND_FAULT` names a point at which the move exits non-zero.
//!
//! Every command runs with an environment built from scratch, so no
//! `KOTO_ANCHOR`, `KOTO_DISPATCHER_ANCHOR`, backend or tick variable of the
//! shell running the suite reaches it, and under a timeout, so a regression
//! fails here instead of hanging the suite.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const NAME: &str = "wf";
const KEY: &str = "notes.md";
const CONTENT: &[u8] = b"some notes";
const TEMPLATE_FILE: &str = "wf.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const GO: &str = r#"{"choice":"go"}"#;
const OK: &str = r#"{"verdict":"ok"}"#;
const WAIT_ENV: &str = "KOTO_TEST_TICK_WAIT_MS";
const FAULT_ENV: &str = "KOTO_TEST_REBIND_FAULT";
const HOLD_ENV: &str = "KOTO_TEST_REBIND_HOLD";
/// Moves `KOTO_TEST_REBIND_HOLD`'s wait to one of [`FAULT_POINTS`].
const HOLD_AT_ENV: &str = "KOTO_TEST_REBIND_HOLD_AT";
const TICK_LOCK: &str = "tick.lock";
const LOCATION: &str = "workflows/publish-location";
const EXPLICIT: &str = "workflows/publish-explicit";

/// The points at which `KOTO_TEST_REBIND_FAULT` stops a move on the local
/// backend, in the order the move reaches them.
const FAULT_POINTS: &[&str] = &[
    "after-move-event",
    "after-commit",
    "after-held-by-event",
    "after-held-by",
    "after-publish-event",
    "after-publish",
    "after-environment-event",
    "after-environment",
];

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// How long a tick or a move may take to reach the point a test waits for.
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
/// its own under `gate_dir`, then waits for `gate_dir/release`.
fn gated_template(gate_dir: &Path) -> String {
    let dir = gate_dir.display();
    gate_template(&format!(
        "touch {dir}/started.$$; while [ ! -f {dir}/release ]; do sleep 0.05; done"
    ))
}

/// A template whose first state's gate runs `command` (no single quotes)
/// and passes when it exits 0, then waits for `GO`.
fn gate_template(command: &str) -> String {
    format!(
        r#"---
name: gated
version: "1.0"
initial_state: hold
states:
  hold:
    gates:
      run:
        type: command
        command: '{command}'
        timeout: 60
    transitions:
      - target: work
        when:
          gates.run.exit_code: 0
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

Run the gate.

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

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The directory holding the koto binary under test, first on every
/// command's `PATH`, so a gate's `koto` is this build.
fn koto_dir() -> PathBuf {
    koto_command::koto_bin().parent().unwrap().to_path_buf()
}

fn path_env() -> String {
    format!("{}:{}", koto_dir().display(), SYSTEM_PATH)
}

/// One scratch host: a koto home of its own, a `HOME` that can hold Claude
/// Code transcripts, and a workspace holding the template. Every path is
/// canonical.
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

    fn template(&self) -> String {
        self.ws.join(TEMPLATE_FILE).to_str().unwrap().to_string()
    }

    /// A directory under the host's root, created.
    fn dir(&self, rel: &str) -> PathBuf {
        mk(self.root.join(rel))
    }

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

    /// koto on the local backend, run in `cwd`, with nothing inherited.
    fn koto_at(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(cwd)
            .env("PATH", path_env())
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("KOTO_HOME", self.root.join("koto"))
            .args(args);
        cmd
    }

    fn koto(&self, args: &[&str]) -> Command {
        self.koto_at(&self.ws, args)
    }

    /// Create session `name` from `cwd`, which becomes its anchor.
    fn init_at(&self, cwd: &Path, name: &str) {
        let template = self.template();
        expect_ok(
            self.koto_at(cwd, &["init", name, "--template", &template]),
            &format!("init {name}"),
        );
    }

    /// Open the anchor for harness session `harness` and return its name.
    fn open_anchor(&self, harness: &str) -> String {
        let out = expect_ok(
            self.koto(&[
                "session",
                "anchor",
                "open",
                "--harness-session",
                harness,
                "--source",
                "startup",
            ]),
            "anchor open",
        );
        out.stdout
            .lines()
            .find_map(|l| l.strip_prefix("anchor="))
            .expect("anchor open printed no anchor line")
            .to_string()
    }

    /// `koto session rebind <name> --whole` from `cwd`, as the holder of
    /// `anchor`.
    fn whole_at(&self, cwd: &Path, name: &str, anchor: &str) -> Command {
        let mut cmd = self.koto_at(cwd, &["session", "rebind", name, "--whole"]);
        cmd.env("KOTO_ANCHOR", anchor);
        cmd
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

    /// The value of context key `key` in `name`, read from `cwd`, or `None`
    /// when the session has no such key.
    fn key(&self, cwd: &Path, name: &str, key: &str) -> Option<String> {
        let out = run(self.koto_at(cwd, &["context", "get", name, key]));
        (out.code == Some(0)).then(|| out.stdout.trim().to_string())
    }

    fn projects(&self) -> PathBuf {
        self.root.join("home").join(".claude").join("projects")
    }

    /// Write the transcript Claude Code keeps for `sid` under `project`.
    fn transcript(&self, project: &str, sid: &str) {
        let dir = mk(self.projects().join(project));
        std::fs::write(dir.join(format!("{sid}.jsonl")), b"").unwrap();
    }

    /// The `/workflows` folder Claude Code reads for `sid` under `project`.
    fn folder(&self, project: &str, sid: &str) -> PathBuf {
        self.projects().join(project).join(sid).join("workflows")
    }
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    /// stdout as JSON: the whole of it, else its last non-empty line.
    fn json(&self) -> Value {
        if let Ok(v) = serde_json::from_str(self.stdout.trim()) {
            return v;
        }
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line).unwrap_or(Value::Null)
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

/// [`run`], asserting exit 0.
fn expect_ok(cmd: Command, what: &str) -> Out {
    let out = run(cmd);
    assert_eq!(out.code, Some(0), "{what}: {}", out.describe());
    out
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

/// Wait until `path` exists, failing after [`START_BOUND`].
fn wait_for_file(path: &Path, what: &str) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < START_BOUND,
            "{what}: {} didn't appear within {START_BOUND:?}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
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

/// Whether a `flock` of kind `op` (`LOCK_EX` or `LOCK_SH`) on `path` can be
/// had at once. The probe's lock is released when its file closes.
fn try_lock(path: &Path, op: libc::c_int) -> bool {
    let file = open_lock_file(path);
    // SAFETY: the fd belongs to `file`, which outlives the call.
    unsafe { libc::flock(file.as_raw_fd(), op | libc::LOCK_NB) == 0 }
}

/// Ticks of session [`NAME`] paused in the gate of [`gated_template`].
/// Dropping it releases the gate and kills any tick still running.
struct Paused {
    gate_dir: PathBuf,
    ticks: Vec<(Child, Instant)>,
}

impl Paused {
    /// Start `n` ticks at once from the workspace and wait until each is
    /// inside the gate.
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

    fn started(&self) -> usize {
        std::fs::read_dir(&self.gate_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("started."))
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

fn header(log: &Path) -> Value {
    let text = std::fs::read_to_string(log).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

/// The events after the header of the log at `log`, oldest first.
fn events(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a state log line isn't JSON"))
        .collect()
}

/// The payloads of the events of type `ty` in the log at `log`.
fn payloads(log: &Path, ty: &str) -> Vec<Value> {
    events(log)
        .into_iter()
        .filter(|e| e["type"] == ty)
        .map(|e| e["payload"].clone())
        .collect()
}

/// The index, among the log's events, of the first event of type `ty`.
fn first_of(log: &Path, ty: &str) -> Option<usize> {
    events(log).iter().position(|e| e["type"] == ty)
}

/// The index, among the log's events, of the last event of type `ty`.
fn last_of(log: &Path, ty: &str) -> Option<usize> {
    events(log).iter().rposition(|e| e["type"] == ty)
}

/// The `execution_anchor_rebound` payloads a whole move wrote.
fn whole_moves(log: &Path) -> Vec<Value> {
    payloads(log, "execution_anchor_rebound")
        .into_iter()
        .filter(|p| p["whole"] == true)
        .collect()
}

/// How many `context_added` events for [`KEY`] the log at `log` holds.
fn context_added(log: &Path) -> usize {
    payloads(log, "context_added")
        .iter()
        .filter(|p| p["key"] == KEY)
        .count()
}

/// Whether the context manifest of the session at `session_dir` lists
/// [`KEY`]; `false` when it has no manifest.
fn indexed(session_dir: &Path) -> bool {
    let Ok(bytes) = std::fs::read(session_dir.join("ctx").join("manifest.json")) else {
        return false;
    };
    let manifest: Value = serde_json::from_slice(&bytes).unwrap();
    manifest["keys"].get(KEY).is_some()
}

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

/// The one `koto-*.json` file in `dir`, if there is exactly one.
fn rendered(dir: &Path) -> Option<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("koto-") && n.ends_with(".json"))
        })
        .collect();
    match files.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The current state the rendered file in `dir` shows.
fn state_in(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(rendered(dir)?).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["koto"]["currentState"].as_str().map(str::to_string)
}

/// Assert that the refusal `out` of `koto session rebind` carries `code`
/// in the error envelope and exits `exit`.
fn assert_refused(out: &Out, exit: i32, code: &str) {
    assert_eq!(out.code, Some(exit), "{}", out.describe());
    let json = out.json();
    assert_eq!(json["error"]["code"], code, "{}", out.describe());
    assert!(
        json["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "the refusal has no message: {}",
        out.describe()
    );
    assert_eq!(json["command"], "session rebind", "{}", out.describe());
}

/// Assert that no whole move was recorded in the session at `log`.
fn assert_not_moved(log: &Path) {
    let h = header(log);
    assert!(
        h["origin"].get("whole_epoch").is_none(),
        "a whole move was recorded: {h}"
    );
    assert!(whole_moves(log).is_empty(), "a whole move was logged");
}

/// A session [`NAME`] started from `old`, held by two anchors and
/// published to the `/workflows` folder of Claude Code session `sess-one`,
/// with `XDG_CONFIG_HOME` at `xdg_one`; and the anchor its new holder runs
/// under in `new`, driven by `sess-two` with `XDG_CONFIG_HOME` at
/// `xdg_two`.
struct Held {
    host: Host,
    old: PathBuf,
    new: PathBuf,
    one: PathBuf,
    two: PathBuf,
    xdg_two: PathBuf,
    old_anchors: Vec<String>,
    new_anchor: String,
}

impl Held {
    fn new() -> Self {
        let host = Host::new();
        host.transcript("proj", "sess-one");
        host.transcript("proj", "sess-two");
        let one = host.folder("proj", "sess-one");
        let two = host.folder("proj", "sess-two");
        let old = host.dir("old");
        let new = host.dir("new");
        let xdg_one = host.dir("xdg-one");
        let xdg_two = host.dir("xdg-two");
        let first = host.open_anchor("old1");
        let second = host.open_anchor("old2");
        let new_anchor = host.open_anchor("new");

        let template = host.template();
        let mut cmd = host.koto_at(&old, &["init", NAME, "--template", &template]);
        cmd.env("KOTO_ANCHOR", &first)
            .env("CLAUDE_CODE_SESSION_ID", "sess-one")
            .env("XDG_CONFIG_HOME", &xdg_one);
        expect_ok(cmd, "setup: init");
        let mut cmd = host.koto_at(&old, &["next", NAME, "--with-data", GO]);
        cmd.env("KOTO_ANCHOR", &second)
            .env("CLAUDE_CODE_SESSION_ID", "sess-one")
            .env("XDG_CONFIG_HOME", &xdg_one);
        expect_ok(cmd, "setup: next");

        let mut old_anchors = vec![first, second];
        old_anchors.sort();
        assert_eq!(held_by(&host, NAME), old_anchors, "setup: held-by links");
        assert_eq!(
            host.key(&old, NAME, LOCATION).as_deref(),
            Some(s(&one)),
            "setup: publish location"
        );
        assert_eq!(
            header(&host.log(NAME))["command_environment"]["xdg_config_home"],
            s(&xdg_one),
            "setup: recorded environment"
        );
        Held {
            host,
            old,
            new,
            one,
            two,
            xdg_two,
            old_anchors,
            new_anchor,
        }
    }

    fn log(&self) -> PathBuf {
        self.host.log(NAME)
    }

    /// The move, run by the new holder from `new`.
    fn mover(&self) -> Command {
        let mut cmd = self.host.whole_at(&self.new, NAME, &self.new_anchor);
        cmd.env("CLAUDE_CODE_SESSION_ID", "sess-two")
            .env("XDG_CONFIG_HOME", &self.xdg_two);
        cmd
    }

    /// Assert that every part of the move happened, each logged once.
    fn assert_moved(&self, what: &str) {
        let log = self.log();
        let h = header(&log);
        assert_eq!(h["execution_dir"], s(&self.new), "{what}: {h}");
        assert_eq!(h["origin"]["anchor"], s(&self.new), "{what}: {h}");
        assert_eq!(h["origin"]["anchor_epoch"], 1, "{what}: {h}");
        assert_eq!(h["origin"]["whole_epoch"], 1, "{what}: {h}");
        assert_eq!(
            h["origin"]["retired_anchors"],
            json!([s(&self.old)]),
            "{what}: {h}"
        );
        assert_eq!(
            h["command_environment"]["xdg_config_home"],
            s(&self.xdg_two),
            "{what}: the environment wasn't re-recorded: {h}"
        );

        let moves = whole_moves(&log);
        assert_eq!(moves.len(), 1, "{what}: whole moves logged: {moves:?}");
        assert_eq!(moves[0]["from"], s(&self.old), "{what}");
        assert_eq!(moves[0]["to"], s(&self.new), "{what}");
        assert_eq!(moves[0]["epoch"], 1, "{what}");
        for ty in [
            "held_by_rebound",
            "publish_location_dropped",
            "environment_rerecorded",
        ] {
            let found = payloads(&log, ty);
            assert_eq!(found.len(), 1, "{what}: {ty} events: {found:?}");
            assert_eq!(found[0]["epoch"], 1, "{what}: {ty}: {found:?}");
        }

        assert_eq!(
            held_by(&self.host, NAME),
            vec![self.new_anchor.clone()],
            "{what}: held-by links"
        );
        assert_ne!(
            self.host.key(&self.new, NAME, LOCATION).as_deref(),
            Some(s(&self.one)),
            "{what}: the old publish location is still stored"
        );
    }
}

#[test]
fn a_whole_rebind_moves_the_anchor_held_by_publish_location_and_environment_in_one_call() {
    let held = Held::new();
    let log = held.log();

    let out = expect_ok(held.mover(), "rebind --whole");

    let json = out.json();
    assert_eq!(json["name"], NAME, "{}", out.describe());
    assert_eq!(json["rebound"], true, "{}", out.describe());
    assert_eq!(json["whole"], true, "{}", out.describe());
    assert_eq!(json["from"], s(&held.old), "{}", out.describe());
    assert_eq!(json["to"], s(&held.new), "{}", out.describe());
    assert_eq!(json["waited"], false, "{}", out.describe());
    assert_eq!(
        json["held_by"],
        json!({"added": held.new_anchor, "removed": held.old_anchors}),
        "{}",
        out.describe()
    );
    assert_eq!(
        json["publish_location"]["dropped"],
        true,
        "{}",
        out.describe()
    );
    assert_eq!(
        json["environment"],
        json!({"rerecorded": true, "changed": ["XDG_CONFIG_HOME"]}),
        "{}",
        out.describe()
    );

    held.assert_moved("after the move");

    // The parts are logged in the order the move makes them, after the move
    // itself.
    let order: Vec<usize> = [
        "execution_anchor_rebound",
        "held_by_rebound",
        "publish_location_dropped",
        "environment_rerecorded",
    ]
    .iter()
    .map(|ty| last_of(&log, ty).unwrap_or_else(|| panic!("no {ty} event")))
    .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "the parts were logged out of order");

    let dropped = payloads(&log, "publish_location_dropped");
    assert_eq!(dropped[0]["was"], s(&held.one));
    assert_eq!(dropped[0]["explicit"], false);

    // The next tick, from the new anchor and driven by the new Claude Code
    // session, renders into that session's folder, and the old folder's
    // file stops changing.
    let old_file = rendered(&held.one).expect("no rendered file in the old folder");
    let old_bytes = std::fs::read(&old_file).unwrap();
    let mut cmd = held.host.koto_at(
        &held.new,
        &["next", NAME, "--with-data", OK, "--no-cleanup"],
    );
    cmd.env("KOTO_ANCHOR", &held.new_anchor)
        .env("CLAUDE_CODE_SESSION_ID", "sess-two")
        .env("XDG_CONFIG_HOME", &held.xdg_two);
    expect_ok(cmd, "next after the move");
    assert_eq!(state_in(&held.two).as_deref(), Some("done"));
    assert_eq!(
        held.host.key(&held.new, NAME, LOCATION).as_deref(),
        Some(s(&held.two))
    );
    assert_eq!(
        std::fs::read(&old_file).unwrap(),
        old_bytes,
        "the old folder's file changed after the move"
    );
}

#[test]
fn a_whole_rebind_waits_for_a_tick_in_flight_and_lands_after_it() {
    let host = Host::new();
    let gate = host.dir("gate");
    let new = host.dir("new");
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let anchor = host.open_anchor("new");
    let log = host.log(NAME);

    let paused = Paused::start(&host, &gate, 1);
    let mut cmd = host.koto_at(
        &new,
        &["session", "rebind", NAME, "--whole", "--wait", "60"],
    );
    cmd.env("KOTO_ANCHOR", &anchor);
    let started = Instant::now();
    let mut mover = spawn(cmd);
    std::thread::sleep(Duration::from_millis(500));
    if mover.try_wait().unwrap().is_some() {
        let out = finish(mover, started);
        panic!(
            "the move finished while a tick held tick.lock: {}",
            out.describe()
        );
    }
    assert_not_moved(&log);

    for tick in paused.release() {
        assert_eq!(tick.code, Some(0), "the tick: {}", tick.describe());
        assert_eq!(tick.json()["state"], "work", "{}", tick.describe());
    }
    let out = finish(mover, started);
    assert_eq!(out.code, Some(0), "rebind --whole: {}", out.describe());
    assert_eq!(out.json()["whole"], true, "{}", out.describe());
    assert_eq!(out.json()["waited"], true, "{}", out.describe());

    let moved = first_of(&log, "execution_anchor_rebound").expect("no move logged");
    let ticked = last_of(&log, "transitioned").expect("the tick logged no transition");
    assert!(
        ticked < moved,
        "the move landed before the tick it waited for ({moved} <= {ticked})"
    );
    assert_eq!(header(&log)["execution_dir"], s(&new));
}

#[test]
fn a_whole_rebind_with_wait_zero_refuses_session_busy_while_a_tick_is_in_flight_and_writes_nothing()
{
    let host = Host::new();
    let gate = host.dir("gate");
    let new = host.dir("new");
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let anchor = host.open_anchor("new");
    let dir = host.session_dir(NAME);

    let paused = Paused::start(&host, &gate, 1);
    let before = files(&dir);
    let mut cmd = host.koto_at(&new, &["session", "rebind", NAME, "--whole", "--wait", "0"]);
    cmd.env("KOTO_ANCHOR", &anchor);
    let started = Instant::now();
    let out = run(cmd);
    let took = started.elapsed();

    assert_refused(&out, 1, "session_busy");
    assert!(
        took < Duration::from_secs(5),
        "--wait 0 waited {took:?} before refusing"
    );
    assert_eq!(files(&dir), before, "the refused move wrote a file");
    assert_not_moved(&host.log(NAME));
    assert_eq!(header(&host.log(NAME))["execution_dir"], s(&host.ws));

    for tick in paused.release() {
        assert_eq!(tick.code, Some(0), "the tick: {}", tick.describe());
    }
}

#[test]
fn a_tick_arriving_during_a_whole_rebind_is_refused_after_it_without_writing() {
    // The move goes up from a/b to a. A tick from a/b passes the anchor
    // check after the move, and the retired anchor, the deeper one, fences
    // it.
    let host = Host::new();
    let a = host.dir("a");
    let b = mk(a.join("b"));
    host.init_at(&b, NAME);
    let anchor = host.open_anchor("new");
    let log = host.log(NAME);
    let hold = host.root.join("hold");

    let mut cmd = host.whole_at(&a, NAME, &anchor);
    cmd.env(HOLD_ENV, &hold);
    let started = Instant::now();
    let mut mover = spawn(cmd);
    let held_at = Instant::now();
    while !hold.exists() {
        if mover.try_wait().unwrap().is_some() {
            let out = finish(mover, started);
            panic!(
                "the move exited before it reached its hold point: {}",
                out.describe()
            );
        }
        assert!(
            held_at.elapsed() < START_BOUND,
            "the move didn't reach its hold point within {START_BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !try_lock(&host.tick_lock(NAME), libc::LOCK_SH),
        "the move holds no exclusive tick.lock at its hold point"
    );
    assert_not_moved(&log);
    let transitions_before = payloads(&log, "transitioned").len();

    let mut cmd = host.koto_at(&b, &["next", NAME, "--with-data", GO]);
    cmd.env(WAIT_ENV, "30000");
    let tick_started = Instant::now();
    let mut tick = spawn(cmd);
    std::thread::sleep(Duration::from_millis(500));
    if tick.try_wait().unwrap().is_some() {
        let out = finish(tick, tick_started);
        panic!(
            "the tick finished while the move held tick.lock: {}",
            out.describe()
        );
    }

    std::fs::remove_file(&hold).unwrap();
    let out = finish(mover, started);
    assert_eq!(out.code, Some(0), "rebind --whole: {}", out.describe());
    let tick = finish(tick, tick_started);

    assert_eq!(tick.code, Some(2), "the tick: {}", tick.describe());
    assert_eq!(
        tick.json()["error"]["code"],
        "session_rebound",
        "{}",
        tick.describe()
    );
    assert_eq!(header(&log)["origin"]["whole_epoch"], 1);
    assert_eq!(
        payloads(&log, "transitioned").len(),
        transitions_before,
        "the refused tick logged a transition"
    );
}

#[test]
fn a_whole_rebind_from_inside_its_own_tick_is_refused_up_front() {
    let host = Host::new();
    let gate = host.dir("gate");
    let new = host.dir("new");
    let anchor = host.open_anchor("new");
    let g = gate.display();
    // The gate names a live anchor itself, so the only thing wrong with the
    // move it runs is where it runs from.
    host.write_template(&gate_template(&format!(
        "KOTO_ANCHOR={anchor} koto session rebind {NAME} --whole --wait 0 --to {new} \
         > {g}/out 2> {g}/err; echo $? > {g}/code",
        new = new.display()
    )));
    host.init_at(&host.ws, NAME);
    let log = host.log(NAME);

    let out = expect_ok(host.koto(&["next", NAME]), "the tick");
    assert_eq!(out.json()["state"], "work", "{}", out.describe());

    let code = std::fs::read_to_string(gate.join("code")).unwrap();
    let inner = Out {
        code: code.trim().parse().ok(),
        stdout: std::fs::read_to_string(gate.join("out")).unwrap_or_default(),
        stderr: std::fs::read_to_string(gate.join("err")).unwrap_or_default(),
    };
    assert_refused(&inner, 2, "nested_invocation");
    assert_not_moved(&log);
    assert_eq!(header(&log)["execution_dir"], s(&host.ws));
}

#[test]
fn after_a_whole_rebind_the_callers_anchor_is_the_only_held_by_link_and_the_log_names_the_removed_ones(
) {
    let held = Held::new();

    let out = expect_ok(held.mover(), "rebind --whole");

    assert_eq!(held_by(&held.host, NAME), vec![held.new_anchor.clone()]);
    let logged = payloads(&held.log(), "held_by_rebound");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["epoch"], 1);
    assert_eq!(logged[0]["added"], held.new_anchor.as_str());
    assert_eq!(logged[0]["removed"], json!(held.old_anchors));
    assert_eq!(
        out.json()["held_by"],
        json!({"added": held.new_anchor, "removed": held.old_anchors}),
        "{}",
        out.describe()
    );
}

#[test]
fn a_whole_rebind_without_a_usable_anchor_refuses_no_anchor_and_writes_nothing() {
    let host = Host::new();
    let new = host.dir("new");
    host.init_at(&host.ws, NAME);
    host.init_at(&host.ws, "other");
    let dir = host.session_dir(NAME);
    let before = files(&dir);

    // Unset, naming a workflow session, naming nothing.
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("unset", None),
        ("a workflow session", Some("other")),
        ("a missing session", Some("anchor-missing")),
    ];
    for (what, anchor) in cases {
        let mut cmd = host.koto_at(&new, &["session", "rebind", NAME, "--whole"]);
        if let Some(anchor) = anchor {
            cmd.env("KOTO_ANCHOR", anchor);
        }
        let out = run(cmd);
        assert_eq!(out.code, Some(2), "KOTO_ANCHOR {what}: {}", out.describe());
        assert_eq!(
            out.json()["error"]["code"],
            "no_anchor",
            "KOTO_ANCHOR {what}: {}",
            out.describe()
        );
        assert_eq!(files(&dir), before, "KOTO_ANCHOR {what}: a file changed");
    }
    assert_not_moved(&host.log(NAME));
    assert_eq!(header(&host.log(NAME))["execution_dir"], s(&host.ws));
}

#[test]
fn a_whole_rebind_drops_the_old_publish_location_and_the_next_commit_publishes_to_the_callers_folder(
) {
    // A location set with `koto workflows publish` doesn't follow a new
    // Claude Code session on its own; the move drops it.
    let host = Host::new();
    host.transcript("proj", "sess-one");
    host.transcript("proj", "sess-two");
    let one = host.folder("proj", "sess-one");
    let two = host.folder("proj", "sess-two");
    let old = host.dir("old");
    let new = host.dir("new");
    host.init_at(&old, NAME);
    let mut cmd = host.koto_at(
        &old,
        &["workflows", "publish", "--dir", s(&one), "--session", NAME],
    );
    cmd.env("CLAUDE_CODE_SESSION_ID", "sess-two");
    expect_ok(cmd, "setup: workflows publish");
    let mut cmd = host.koto_at(&old, &["next", NAME, "--with-data", GO]);
    cmd.env("CLAUDE_CODE_SESSION_ID", "sess-two");
    expect_ok(cmd, "setup: next");
    assert_eq!(state_in(&one).as_deref(), Some("more"), "setup");
    assert!(
        host.key(&old, NAME, EXPLICIT).is_some(),
        "setup: workflows publish stored no explicit mark"
    );
    let anchor = host.open_anchor("new");

    let mut cmd = host.whole_at(&new, NAME, &anchor);
    cmd.env("CLAUDE_CODE_SESSION_ID", "sess-two");
    let out = expect_ok(cmd, "rebind --whole");

    assert_eq!(
        out.json()["publish_location"]["dropped"],
        true,
        "{}",
        out.describe()
    );
    let dropped = payloads(&host.log(NAME), "publish_location_dropped");
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0]["epoch"], 1);
    assert_eq!(dropped[0]["was"], s(&one));
    assert_eq!(dropped[0]["explicit"], true);
    assert_eq!(
        host.key(&new, NAME, EXPLICIT),
        None,
        "the explicit mark is kept"
    );

    let one_file = rendered(&one).unwrap();
    let one_bytes = std::fs::read(&one_file).unwrap();
    let mut cmd = host.koto_at(&new, &["next", NAME, "--with-data", OK, "--no-cleanup"]);
    cmd.env("KOTO_ANCHOR", &anchor)
        .env("CLAUDE_CODE_SESSION_ID", "sess-two");
    expect_ok(cmd, "next after the move");

    assert_eq!(state_in(&two).as_deref(), Some("done"));
    assert_eq!(host.key(&new, NAME, LOCATION).as_deref(), Some(s(&two)));
    assert_eq!(
        std::fs::read(&one_file).unwrap(),
        one_bytes,
        "the old folder's file changed after the move"
    );
}

#[test]
fn a_whole_rebind_hands_the_publish_location_to_koto_workflows_dir_when_set() {
    // `KOTO_WORKFLOWS_DIR` stores itself only when no location is stored,
    // so without the drop the old location would stay recorded.
    let host = Host::new();
    host.transcript("proj", "sess-one");
    let one = host.folder("proj", "sess-one");
    let old = host.dir("old");
    let new = host.dir("new");
    let handoff = host.dir("handoff");
    let template = host.template();
    let mut cmd = host.koto_at(&old, &["init", NAME, "--template", &template]);
    cmd.env("CLAUDE_CODE_SESSION_ID", "sess-one");
    expect_ok(cmd, "setup: init");
    let mut cmd = host.koto_at(&old, &["next", NAME, "--with-data", GO]);
    cmd.env("CLAUDE_CODE_SESSION_ID", "sess-one");
    expect_ok(cmd, "setup: next");
    assert_eq!(
        host.key(&old, NAME, LOCATION).as_deref(),
        Some(s(&one)),
        "setup"
    );
    let anchor = host.open_anchor("new");

    let mut cmd = host.whole_at(&new, NAME, &anchor);
    cmd.env("KOTO_WORKFLOWS_DIR", &handoff);
    let out = expect_ok(cmd, "rebind --whole");
    assert_eq!(
        out.json()["publish_location"]["dropped"],
        true,
        "{}",
        out.describe()
    );
    let dropped = payloads(&host.log(NAME), "publish_location_dropped");
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0]["was"], s(&one));
    assert_eq!(dropped[0]["explicit"], false);

    let mut cmd = host.koto_at(&new, &["next", NAME, "--with-data", OK, "--no-cleanup"]);
    cmd.env("KOTO_ANCHOR", &anchor)
        .env("KOTO_WORKFLOWS_DIR", &handoff);
    expect_ok(cmd, "next after the move");

    assert_eq!(state_in(&handoff).as_deref(), Some("done"));
    assert_eq!(host.key(&new, NAME, LOCATION).as_deref(), Some(s(&handoff)));
}

#[test]
fn a_whole_rebind_rerecords_the_callers_command_environment_and_logs_it() {
    let host = Host::new();
    let old = host.dir("old");
    let new = host.dir("new");
    let xdg_one = host.dir("xdg-one");
    let xdg_two = host.dir("xdg-two");
    let template = host.template();
    let mut cmd = host.koto_at(&old, &["init", NAME, "--template", &template]);
    cmd.env("XDG_CONFIG_HOME", &xdg_one);
    expect_ok(cmd, "setup: init");
    let anchor = host.open_anchor("new");

    // The caller's PATH differs from the record only by an entry recording
    // drops, so only XDG_CONFIG_HOME changes.
    let mut cmd = host.whole_at(&new, NAME, &anchor);
    cmd.env("XDG_CONFIG_HOME", &xdg_two)
        .env("PATH", format!("node_modules/.bin:{}", path_env()));
    let out = expect_ok(cmd, "rebind --whole");

    assert_eq!(
        out.json()["environment"],
        json!({"rerecorded": true, "changed": ["XDG_CONFIG_HOME"]}),
        "{}",
        out.describe()
    );
    let h = header(&host.log(NAME));
    let record = &h["command_environment"];
    assert_eq!(record["xdg_config_home"], s(&xdg_two), "{h}");
    assert_eq!(record["path"], path_env().as_str(), "{h}");
    assert_eq!(record["home"], s(&host.root.join("home")), "{h}");

    let logged = payloads(&host.log(NAME), "environment_rerecorded");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["epoch"], 1);
    assert_eq!(&logged[0]["environment"], record);
    assert_eq!(logged[0]["changed"], json!(["XDG_CONFIG_HOME"]));
    assert_eq!(logged[0]["dropped"], json!(["node_modules/.bin"]));
}

#[test]
fn a_whole_rebind_stopped_after_any_write_finishes_when_run_again_with_no_part_twice() {
    for point in FAULT_POINTS {
        let held = Held::new();
        let mut cmd = held.mover();
        cmd.env(FAULT_ENV, point);
        let out = run(cmd);
        assert!(
            out.code.is_some() && out.code != Some(0),
            "{point}: the stopped move exited 0: {}",
            out.describe()
        );

        let out = run(held.mover());
        assert_eq!(out.code, Some(0), "{point}: run again: {}", out.describe());
        held.assert_moved(point);
    }
}

#[test]
fn a_whole_rebind_run_again_after_it_finished_changes_nothing() {
    let held = Held::new();
    expect_ok(held.mover(), "rebind --whole");
    let dir = held.host.session_dir(NAME);
    let before = files(&dir);

    let out = expect_ok(held.mover(), "rebind --whole again");

    let json = out.json();
    assert_eq!(json["rebound"], false, "{}", out.describe());
    assert_eq!(json["whole"], true, "{}", out.describe());
    assert_eq!(
        json["held_by"],
        json!({"added": null, "removed": []}),
        "{}",
        out.describe()
    );
    assert_eq!(
        json["publish_location"]["dropped"],
        false,
        "{}",
        out.describe()
    );
    assert_eq!(
        json["environment"],
        json!({"rerecorded": false, "changed": []}),
        "{}",
        out.describe()
    );
    assert_eq!(files(&dir), before, "the second run wrote to the session");
    held.assert_moved("after the second run");
}

#[test]
fn a_whole_rebind_works_on_a_child_session() {
    let host = Host::new();
    let old = host.dir("old");
    let new = host.dir("new");
    host.init_at(&old, NAME);
    let template = host.template();
    expect_ok(
        host.koto_at(
            &old,
            &["init", "child", "--template", &template, "--parent", NAME],
        ),
        "setup: init child",
    );
    let anchor = host.open_anchor("new");

    let out = expect_ok(
        host.whole_at(&new, "child", &anchor),
        "rebind --whole child",
    );

    assert_eq!(out.json()["whole"], true, "{}", out.describe());
    let h = header(&host.log("child"));
    assert_eq!(h["execution_dir"], s(&new), "{h}");
    assert_eq!(h["origin"]["whole_epoch"], 1, "{h}");
    assert_eq!(whole_moves(&host.log("child")).len(), 1);
    // The parent stays where it was.
    assert_eq!(header(&host.log(NAME))["execution_dir"], s(&old));
    assert_not_moved(&host.log(NAME));

    let mut cmd = host.koto_at(&new, &["next", "child", "--with-data", GO]);
    cmd.env("KOTO_ANCHOR", &anchor);
    let out = expect_ok(cmd, "next child after the move");
    assert_eq!(out.json()["state"], "more", "{}", out.describe());
}

#[test]
fn a_whole_rebind_works_on_a_finished_child() {
    let host = Host::new();
    let old = host.dir("old");
    let new = host.dir("new");
    host.init_at(&old, NAME);
    let template = host.template();
    expect_ok(
        host.koto_at(
            &old,
            &["init", "child", "--template", &template, "--parent", NAME],
        ),
        "setup: init child",
    );
    expect_ok(
        host.koto_at(&old, &["next", "child", "--with-data", GO]),
        "setup: next child",
    );
    expect_ok(
        host.koto_at(&old, &["next", "child", "--with-data", OK, "--no-cleanup"]),
        "setup: finish child",
    );
    let anchor = host.open_anchor("new");

    let out = expect_ok(
        host.whole_at(&new, "child", &anchor),
        "rebind --whole child",
    );

    assert_eq!(out.json()["whole"], true, "{}", out.describe());
    let h = header(&host.log("child"));
    assert_eq!(h["execution_dir"], s(&new), "{h}");
    assert_eq!(h["origin"]["whole_epoch"], 1, "{h}");
    let status = expect_ok(host.koto_at(&new, &["status", "child"]), "status child");
    assert_eq!(
        status.json()["current_state"],
        "done",
        "{}",
        status.describe()
    );
}

#[test]
fn a_whole_rebind_works_when_every_directory_the_session_recorded_is_gone() {
    let host = Host::new();
    let first = host.dir("first");
    let second = host.dir("second");
    let new = host.dir("new");
    host.init_at(&first, NAME);
    expect_ok(
        host.koto_at(&second, &["session", "rebind", NAME]),
        "setup: plain rebind",
    );
    std::fs::remove_dir_all(&first).unwrap();
    std::fs::remove_dir_all(&second).unwrap();
    let anchor = host.open_anchor("new");

    let out = expect_ok(host.whole_at(&new, NAME, &anchor), "rebind --whole");

    assert_eq!(out.json()["rebound"], true, "{}", out.describe());
    let h = header(&host.log(NAME));
    assert_eq!(h["execution_dir"], s(&new), "{h}");
    assert_eq!(h["origin"]["anchor_epoch"], 2, "{h}");
    assert_eq!(h["origin"]["whole_epoch"], 2, "{h}");
    let retired = h["origin"]["retired_anchors"].as_array().unwrap().clone();
    for gone in [&first, &second] {
        assert!(retired.contains(&json!(s(gone))), "{h}");
    }
    let moves = whole_moves(&host.log(NAME));
    assert_eq!(moves.len(), 1, "{moves:?}");
    assert_eq!(moves[0]["from"], s(&second));
    assert_eq!(moves[0]["epoch"], 2);

    let mut cmd = host.koto_at(&new, &["next", NAME, "--with-data", GO]);
    cmd.env("KOTO_ANCHOR", &anchor);
    expect_ok(cmd, "next after the move");
}

#[test]
fn a_whole_rebind_of_a_session_with_no_execution_dir_logs_its_anchor_as_from() {
    let host = Host::new();
    let old = host.dir("old");
    let new = host.dir("new");
    host.init_at(&old, NAME);
    let log = host.log(NAME);
    let text = std::fs::read_to_string(&log).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut h: Value = serde_json::from_str(first).unwrap();
    assert_eq!(h["origin"]["anchor"], s(&old), "setup: {h}");
    assert!(
        h.as_object_mut().unwrap().remove("execution_dir").is_some(),
        "setup: {h}"
    );
    std::fs::write(&log, format!("{h}\n{rest}")).unwrap();
    let anchor = host.open_anchor("new");

    let out = expect_ok(host.whole_at(&new, NAME, &anchor), "rebind --whole");

    let moves = whole_moves(&log);
    assert_eq!(moves.len(), 1, "{moves:?}; {}", out.describe());
    assert_eq!(moves[0]["from"], s(&old), "{moves:?}");
}

#[test]
fn a_writer_that_read_before_a_whole_rebind_is_refused_after_it() {
    // A command a tick started in the background outlives the tick and
    // writes after the move, from the anchor the move retired. The tick
    // has ended, so nothing holds tick.lock and the move doesn't wait.
    let host = Host::new();
    let gate = host.dir("gate");
    let old = host.dir("old");
    let new = host.dir("new");
    std::fs::write(gate.join("content"), CONTENT).unwrap();
    let g = gate.display();
    host.write_template(&gate_template(&format!(
        "( while [ ! -f {g}/go ]; do sleep 0.05; done; koto context add {NAME} {KEY} \
         < {g}/content > {g}/writer.out 2> {g}/writer.err; echo $? > {g}/writer.code ) \
         > /dev/null 2>&1 < /dev/null &"
    )));
    host.init_at(&old, NAME);
    let out = expect_ok(host.koto_at(&old, &["next", NAME]), "the tick");
    assert_eq!(out.json()["state"], "work", "{}", out.describe());
    let anchor = host.open_anchor("new");

    expect_ok(host.whole_at(&new, NAME, &anchor), "rebind --whole");
    std::fs::write(gate.join("go"), b"").unwrap();
    wait_for_file(&gate.join("writer.code"), "the background writer");
    // The code file is written after the command exits; give the write a
    // moment to land in full.
    std::thread::sleep(Duration::from_millis(50));

    let writer = Out {
        code: std::fs::read_to_string(gate.join("writer.code"))
            .unwrap()
            .trim()
            .parse()
            .ok(),
        stdout: std::fs::read_to_string(gate.join("writer.out")).unwrap_or_default(),
        stderr: std::fs::read_to_string(gate.join("writer.err")).unwrap_or_default(),
    };
    assert_fenced(&host, NAME, &writer, &old);

    // A writer from the new anchor stores the key.
    let out = host.add_at(&new, NAME);
    assert_stored(&host, NAME, &out, &new);
}

#[test]
fn a_plain_rebind_from_a_gate_after_a_whole_rebind_leaves_the_tick_its_epoch() {
    // After a whole move to a, a tick from a/b runs a gate that moves the
    // session down to a/b with a plain rebind. The tick's directory stays
    // under the current anchor, the deeper one, and only a whole move
    // invalidates the epoch the tick read, so the tick goes on writing.
    let host = Host::new();
    let gate = host.dir("gate");
    let old = host.dir("old");
    let a = host.dir("a");
    let b = mk(a.join("b"));
    let g = gate.display();
    host.write_template(&gate_template(&format!(
        "koto session rebind {NAME} --to {b} > {g}/rebind.out 2> {g}/rebind.err; \
         echo $? > {g}/rebind.code",
        b = b.display()
    )));
    host.init_at(&old, NAME);
    let anchor = host.open_anchor("new");
    expect_ok(host.whole_at(&a, NAME, &anchor), "rebind --whole");
    let log = host.log(NAME);

    let out = run(host.koto_at(&b, &["next", NAME]));

    let rebind = std::fs::read_to_string(gate.join("rebind.code")).unwrap_or_default();
    assert_eq!(
        rebind.trim(),
        "0",
        "the gate's plain rebind: {} {}",
        std::fs::read_to_string(gate.join("rebind.out")).unwrap_or_default(),
        std::fs::read_to_string(gate.join("rebind.err")).unwrap_or_default()
    );
    assert_eq!(out.code, Some(0), "the tick: {}", out.describe());
    assert_eq!(out.json()["state"], "work", "{}", out.describe());

    let h = header(&log);
    assert_eq!(h["execution_dir"], s(&b), "{h}");
    assert_eq!(h["origin"]["anchor_epoch"], 2, "{h}");
    assert_eq!(h["origin"]["whole_epoch"], 1, "{h}");
    let rebound = last_of(&log, "execution_anchor_rebound").unwrap();
    let ticked = last_of(&log, "transitioned").expect("the tick logged no transition");
    assert!(
        rebound < ticked,
        "the tick's transition isn't after the gate's rebind"
    );
}

#[test]
fn a_plain_rebind_after_a_whole_rebind_still_fences_the_retired_anchor() {
    let host = Host::new();
    let old = host.dir("old");
    let a = host.dir("a");
    let b = mk(a.join("b"));
    host.init_at(&old, NAME);
    let anchor = host.open_anchor("new");
    expect_ok(host.whole_at(&a, NAME, &anchor), "rebind --whole");
    expect_ok(
        host.koto_at(&b, &["session", "rebind", NAME]),
        "plain rebind",
    );
    let h = header(&host.log(NAME));
    assert_eq!(h["origin"]["anchor_epoch"], 2, "{h}");
    assert_eq!(h["origin"]["whole_epoch"], 1, "{h}");

    // a is retired and outside the current anchor: refused.
    let out = host.add_at(&a, NAME);
    assert_fenced(&host, NAME, &out, &a);
    // a/b is the current anchor, deeper than the retired a: stored.
    let out = host.add_at(&b, NAME);
    assert_stored(&host, NAME, &out, &b);
}

#[test]
fn a_second_process_in_the_retired_directory_is_refused_while_the_mover_finishes_from_there() {
    // The mover runs from the directory it retires, so its own later parts
    // pass the path rule. Any other process there is still refused, while
    // the move is half done and after it finished.
    let held = Held::new();
    let log = held.log();
    let hold = held.host.root.join("hold");

    let mut cmd = held.host.whole_at(&held.old, NAME, &held.new_anchor);
    cmd.args(["--to", s(&held.new)])
        .env("CLAUDE_CODE_SESSION_ID", "sess-two")
        .env("XDG_CONFIG_HOME", &held.xdg_two)
        .env(HOLD_ENV, &hold)
        .env(HOLD_AT_ENV, "after-held-by");
    let started = Instant::now();
    let mut mover = spawn(cmd);
    while !hold.exists() {
        if mover.try_wait().unwrap().is_some() {
            let out = finish(mover, started);
            panic!(
                "the move exited before it reached its hold point: {}",
                out.describe()
            );
        }
        assert!(
            started.elapsed() < START_BOUND,
            "the move didn't reach its hold point within {START_BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let h = header(&log);
    assert_eq!(
        h["origin"]["whole_epoch"], 1,
        "the move hasn't committed: {h}"
    );
    assert!(
        payloads(&log, "environment_rerecorded").is_empty(),
        "the move ran past its hold point"
    );

    let out = held.host.add_at(&held.old, NAME);
    assert_fenced(&held.host, NAME, &out, &held.old);

    std::fs::remove_file(&hold).unwrap();
    let out = finish(mover, started);
    assert_eq!(out.code, Some(0), "rebind --whole --to: {}", out.describe());
    held.assert_moved("after the move");

    let out = held.host.add_at(&held.old, NAME);
    assert_fenced(&held.host, NAME, &out, &held.old);
}
