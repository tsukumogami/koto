//! `koto init NEW --template T --replaces OLD` on the local backend: a
//! restart that creates NEW and closes OLD, the run it replaces, with a
//! cancel naming NEW and a fence marked `replaced`, in one write to OLD's
//! log.
//!
//! The close waits for a tick of OLD in flight (`--wait`, default 10
//! seconds, `0` refuses at once with `session_busy`, exit 1) unless
//! `--abandon-tick` closes OLD without the lock, and a call from inside
//! OLD's own tick or background action is refused up front
//! (`nested_invocation`). Every refusal before the lock writes nothing, and
//! a replace stopped after any write finishes when run again.
//!
//! `KOTO_TEST_REPLACE_FAULT` names a debug-build point at which the command
//! stops as a killed process would (exit 86).
//!
//! Every command runs with an environment built from scratch and under a
//! timeout, as tests/rebind_whole_test.rs does, whose helpers these are.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const NAME: &str = "wf";
const NEW: &str = "wf-r2";
const TEMPLATE_FILE: &str = "wf.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const GO: &str = r#"{"choice":"go"}"#;
const OK: &str = r#"{"verdict":"ok"}"#;
const FAULT_ENV: &str = "KOTO_TEST_REPLACE_FAULT";
const REPLACES_KEY: &str = "koto/replaces";

/// The points at which `KOTO_TEST_REPLACE_FAULT` stops a replace, in the
/// order the command reaches them.
const FAULT_POINTS: &[&str] = &[
    "after-lock",
    "after-create",
    "after-key",
    "torn-close",
    "after-close",
];

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// How long a tick may take to reach the point a test waits for.
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

/// The directory holding the koto binary under test, first on every
/// command's `PATH`, so a gate's `koto` is this build.
fn koto_dir() -> PathBuf {
    koto_command::koto_bin().parent().unwrap().to_path_buf()
}

fn path_env() -> String {
    format!("{}:{}", koto_dir().display(), SYSTEM_PATH)
}

/// One scratch host: a koto home of its own, a `HOME`, and a workspace
/// holding the template. Every path is canonical.
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

    /// `koto init NEW --template T --replaces old`, plus `extra`, from the
    /// workspace.
    fn replace(&self, old: &str, extra: &[&str]) -> Command {
        let template = self.template();
        let mut args = vec!["init", NEW, "--template", &template, "--replaces", old];
        args.extend_from_slice(extra);
        self.koto(&args)
    }

    /// The value NEW's `koto/replaces` key holds, from its context file.
    fn replaces_key(&self) -> Option<Value> {
        let path = self.session_dir(NEW).join("ctx").join(REPLACES_KEY);
        let bytes = std::fs::read(path).ok()?;
        Some(serde_json::from_slice(&bytes).unwrap())
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

/// Wait for `child`, killed once [`BOUND`] has passed since `started`.
fn finish(mut child: Child, started: Instant) -> Out {
    loop {
        if child.try_wait().unwrap().is_some() {
            let o = child.wait_with_output().unwrap();
            return Out {
                code: o.status.code(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
        if started.elapsed() > BOUND {
            let _ = child.kill();
            let _ = child.wait();
            return Out {
                code: None,
                stdout: String::new(),
                stderr: format!("did not finish within {BOUND:?}"),
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Ticks of session [`NAME`] paused in the gate of [`gated_template`].
/// Dropping it releases the gate and kills any tick still running.
struct Paused {
    gate_dir: PathBuf,
    ticks: Vec<(Child, Instant)>,
}

impl Paused {
    /// Start one tick from the workspace and wait until it is inside the
    /// gate.
    fn start(host: &Host, gate_dir: &Path) -> Self {
        let mut paused = Paused {
            gate_dir: gate_dir.to_path_buf(),
            ticks: Vec::new(),
        };
        let child = spawn(host.koto(&["next", NAME]));
        paused.ticks.push((child, Instant::now()));
        let started = Instant::now();
        while paused.started() < 1 {
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
                "the tick didn't reach the gate within {START_BOUND:?}"
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

/// The events after the header of the log at `log`, oldest first. Every
/// line must parse.
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

/// Assert that `out` is an init refusal in the flat envelope, with `code`
/// and exit `exit`.
fn assert_refused(out: &Out, exit: i32, code: &str) {
    assert_eq!(out.code, Some(exit), "{}", out.describe());
    let json = out.json();
    assert_eq!(json["code"], code, "{}", out.describe());
    assert!(
        json["error"].as_str().is_some_and(|m| !m.is_empty()),
        "the refusal has no message: {}",
        out.describe()
    );
    assert_eq!(json["command"], "init", "{}", out.describe());
}

/// Assert that `old` is closed for [`NEW`]: its last standing event is a
/// cancel naming NEW (unless `cancelled` is false, for a finished run),
/// its log ends with one fence marked `replaced` naming NEW, and NEW holds
/// `koto/replaces` naming `old`.
fn assert_closed(host: &Host, old: &str, cancelled: bool, what: &str) {
    let log = host.log(old);
    let new_id = header(&host.log(NEW))["session_id"].clone();
    let fences = payloads(&log, "session_superseded");
    assert_eq!(fences.len(), 1, "{what}: {fences:?}");
    assert_eq!(fences[0]["by"], NEW, "{what}");
    assert_eq!(fences[0]["by_session_id"], new_id, "{what}");
    assert_eq!(fences[0]["replaced"], true, "{what}");
    let all = events(&log);
    assert_eq!(
        all.last().unwrap()["type"],
        "session_superseded",
        "{what}: the fence isn't the log's last event"
    );
    let replace_cancels: Vec<Value> = payloads(&log, "workflow_cancelled")
        .into_iter()
        .filter(|p| p.get("replaced_by").is_some())
        .collect();
    if cancelled {
        assert_eq!(replace_cancels.len(), 1, "{what}: {replace_cancels:?}");
        let cancel = &replace_cancels[0];
        assert_eq!(cancel["replaced_by"], NEW, "{what}");
        assert_eq!(cancel["replaced_by_session_id"], new_id, "{what}");
        assert_eq!(cancel["reason"], format!("replaced by {NEW}"), "{what}");
        let standing = all
            .iter()
            .rev()
            .find(|e| {
                [
                    "workflow_cancelled",
                    "transitioned",
                    "directed_transition",
                    "rewound",
                ]
                .contains(&e["type"].as_str().unwrap_or(""))
            })
            .unwrap();
        assert_eq!(standing["type"], "workflow_cancelled", "{what}");
        assert!(
            last_of(&log, "workflow_cancelled") < last_of(&log, "session_superseded"),
            "{what}: the fence came before the cancel"
        );
    } else {
        assert!(replace_cancels.is_empty(), "{what}: {replace_cancels:?}");
    }
    assert_eq!(
        host.replaces_key(),
        Some(json!({"v": 1, "session": old, "session_id": header(&log)["session_id"]})),
        "{what}"
    );
}

#[test]
fn init_replaces_creates_the_new_run_and_closes_the_old_one_in_one_write() {
    let host = Host::new();
    let old = host.dir("old");
    host.init_at(&old, NAME);

    let out = expect_ok(host.replace(NAME, &[]), "init --replaces");

    let json = out.json();
    assert_eq!(json["name"], NEW, "{}", out.describe());
    assert_eq!(json["state"], "start", "{}", out.describe());
    assert_eq!(json["replaces"], NAME, "{}", out.describe());
    assert_eq!(json["outcome"], "replaced", "{}", out.describe());
    assert_eq!(json["waited"], false, "{}", out.describe());
    assert_eq!(json["tick_in_flight"], false, "{}", out.describe());
    assert_eq!(json["children"], json!([]), "{}", out.describe());
    assert_eq!(json["deferred_cleanup"], false, "{}", out.describe());

    assert_closed(&host, NAME, true, "init --replaces");
    let log = host.log(NAME);
    let all = events(&log);
    let n = all.len();
    assert_eq!(all[n - 2]["type"], "workflow_cancelled");
    assert_eq!(all[n - 2]["payload"]["state"], "start");
    assert_eq!(
        all[n - 1]["seq"].as_u64().unwrap(),
        all[n - 2]["seq"].as_u64().unwrap() + 1,
        "the cancel and the fence don't have consecutive seqs"
    );

    // NEW records what it replaces, and neither continues OLD nor holds it.
    let added: Vec<Value> = payloads(&host.log(NEW), "context_added")
        .into_iter()
        .filter(|p| p["key"] == REPLACES_KEY)
        .collect();
    assert_eq!(added.len(), 1, "{added:?}");
    let links = host.session_dir(NEW).join("ctx").join("koto").join("links");
    assert!(
        !links.join("continues").exists(),
        "the new run records a continues relation"
    );
    assert_eq!(
        header(&host.log(NEW))["execution_dir"],
        host.ws.to_str().unwrap()
    );
}

#[test]
fn a_replace_run_again_after_it_finished_changes_nothing() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    expect_ok(host.replace(NAME, &[]), "init --replaces");
    let old_before = files(&host.session_dir(NAME));
    let new_before = files(&host.session_dir(NEW));

    let out = expect_ok(host.replace(NAME, &[]), "init --replaces again");

    assert_eq!(out.json()["outcome"], "unchanged", "{}", out.describe());
    assert_eq!(files(&host.session_dir(NAME)), old_before, "OLD changed");
    assert_eq!(files(&host.session_dir(NEW)), new_before, "NEW changed");
    assert_closed(&host, NAME, true, "after the second run");
}

#[test]
fn replace_refuses_session_busy_while_a_tick_holds_the_old_run_and_writes_nothing() {
    let host = Host::new();
    let gate = host.dir("gate");
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let dir = host.session_dir(NAME);

    let paused = Paused::start(&host, &gate);
    let before = files(&dir);

    let started = Instant::now();
    let out = run(host.replace(NAME, &["--wait", "0"]));
    let took = started.elapsed();
    assert_refused(&out, 1, "session_busy");
    assert!(
        took < Duration::from_secs(5),
        "--wait 0 waited {took:?} before refusing"
    );
    assert!(
        out.json()["details"]["waited_for_ms"].is_u64(),
        "{}",
        out.describe()
    );

    let started = Instant::now();
    let out = run(host.replace(NAME, &["--wait", "1"]));
    assert_refused(&out, 1, "session_busy");
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "--wait 1 refused before a second had passed"
    );

    assert_eq!(files(&dir), before, "the refused replace wrote to OLD");
    assert!(
        !host.session_dir(NEW).exists(),
        "the refused replace created NEW"
    );

    for tick in paused.release() {
        assert_eq!(tick.code, Some(0), "the tick: {}", tick.describe());
        assert_eq!(tick.json()["state"], "work", "{}", tick.describe());
    }
}

#[test]
fn abandon_tick_closes_without_the_lock_and_reports_tick_in_flight() {
    let host = Host::new();
    let gate = host.dir("gate");
    host.write_template(&gated_template(&gate));
    host.init_at(&host.ws, NAME);
    let log = host.log(NAME);

    let paused = Paused::start(&host, &gate);
    let out = expect_ok(
        host.replace(NAME, &["--abandon-tick"]),
        "init --replaces --abandon-tick",
    );
    let json = out.json();
    assert_eq!(json["tick_in_flight"], true, "{}", out.describe());
    assert_eq!(json["waited"], false, "{}", out.describe());
    assert_eq!(json["outcome"], "replaced", "{}", out.describe());
    assert_closed(&host, NAME, true, "--abandon-tick");

    // The tick in flight meets the fence: it records no transition.
    let ticks = paused.release();
    assert!(ticks.iter().all(|t| t.code.is_some()), "a tick hung");
    assert!(
        last_of(&log, "transitioned") < first_of(&log, "session_superseded"),
        "the abandoned tick transitioned past the fence"
    );
    assert_closed(&host, NAME, true, "after the tick returned");
}

#[test]
fn abandon_tick_with_a_free_lock_takes_it_and_reports_no_tick_in_flight() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);

    let out = run(host.replace(NAME, &["--abandon-tick", "--wait", "1"]));
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(!host.session_dir(NEW).exists(), "{}", out.describe());

    let out = expect_ok(
        host.replace(NAME, &["--abandon-tick"]),
        "init --replaces --abandon-tick",
    );
    let json = out.json();
    assert_eq!(json["tick_in_flight"], false, "{}", out.describe());
    assert_eq!(json["waited"], false, "{}", out.describe());
    assert_closed(&host, NAME, true, "--abandon-tick on a free lock");
}

#[test]
fn a_replace_with_a_template_that_wont_compile_touches_neither_run() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let dir = host.session_dir(NAME);
    let before = files(&dir);
    let bad = host.ws.join("bad.md");
    std::fs::write(&bad, "---\nname: [unclosed\n---\n").unwrap();

    let out = run(host.koto(&[
        "init",
        NEW,
        "--template",
        bad.to_str().unwrap(),
        "--replaces",
        NAME,
    ]));
    assert_eq!(out.code, Some(1), "{}", out.describe());
    assert_eq!(out.json()["command"], "init", "{}", out.describe());
    assert!(out.json()["code"].is_string(), "{}", out.describe());

    // A variable the template doesn't declare is refused the same way.
    let out = run(host.replace(NAME, &["--var", "NOPE=x"]));
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert_eq!(out.json()["command"], "init", "{}", out.describe());

    assert_eq!(files(&dir), before, "the refused replace wrote to OLD");
    assert!(
        !host.session_dir(NEW).exists(),
        "the refused replace created NEW"
    );
}

#[test]
fn a_replace_from_inside_the_old_runs_tick_is_refused_up_front() {
    let host = Host::new();
    let gate = host.dir("gate");
    let g = gate.display();
    let template = host.template();
    host.write_template(&gate_template(&format!(
        "koto init {NEW} --template {template} --replaces {NAME} --wait 0 \
         > {g}/out 2> {g}/err; echo $? > {g}/code"
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
    assert!(inner.json()["error"]
        .as_str()
        .unwrap()
        .contains("KOTO_TICK_SESSION"));
    assert!(payloads(&log, "session_superseded").is_empty());
    assert!(!host.session_dir(NEW).exists());

    // From inside one of OLD's background actions.
    let before = files(&host.session_dir(NAME));
    let mut cmd = host.replace(NAME, &[]);
    cmd.env("KOTO_BACKGROUND_OF", NAME);
    let out = run(cmd);
    assert_refused(&out, 2, "nested_invocation");
    assert!(out.json()["error"]
        .as_str()
        .unwrap()
        .contains("KOTO_BACKGROUND_OF"));
    assert_eq!(files(&host.session_dir(NAME)), before);
    assert!(!host.session_dir(NEW).exists());
}

#[test]
fn replace_refusals_before_the_lock_write_nothing() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    host.init_at(&host.ws, "other");
    let template = host.template();
    let snapshot = || files(&host.root.join("koto").join("sessions"));
    let before = snapshot();

    let out = run(host.koto(&["init", NAME, "--template", &template, "--replaces", NAME]));
    assert_refused(&out, 2, "self");
    let out = run(host.replace("missing", &[]));
    assert_refused(&out, 2, "session_not_found");
    let out = run(host.koto(&["init", "other", "--template", &template, "--replaces", NAME]));
    assert_refused(&out, 2, "session_exists");
    let out = run(host.replace(NAME, &["--continues", "other"]));
    assert_refused(&out, 2, "invalid_usage");
    assert_eq!(snapshot(), before, "a refused replace wrote something");

    // OLD superseded by another session.
    expect_ok(
        host.koto(&[
            "init",
            "succ",
            "--template",
            &template,
            "--continues",
            NAME,
            "--supersede",
        ]),
        "setup: supersede",
    );
    let before = snapshot();
    let out = run(host.replace(NAME, &[]));
    assert_refused(&out, 2, "superseded_by");
    assert_eq!(out.json()["superseded_by"], "succ", "{}", out.describe());
    assert_eq!(snapshot(), before, "a refused replace wrote something");
}

#[test]
fn a_replace_stopped_after_any_write_finishes_when_run_again() {
    for point in FAULT_POINTS {
        let host = Host::new();
        let old = host.dir("old");
        host.init_at(&old, NAME);
        let mut cmd = host.replace(NAME, &[]);
        cmd.env(FAULT_ENV, point);
        let out = run(cmd);
        assert_eq!(out.code, Some(86), "{point}: {}", out.describe());

        if *point == "after-create" {
            // NEW exists without its key, which no re-run takes for its
            // own: OLD stays open until NEW is removed.
            let out = run(host.replace(NAME, &[]));
            assert_refused(&out, 2, "session_exists");
            assert!(
                payloads(&host.log(NAME), "session_superseded").is_empty(),
                "{point}"
            );
            assert!(
                payloads(&host.log(NAME), "workflow_cancelled").is_empty(),
                "{point}"
            );
            expect_ok(
                host.koto(&["cancel", NEW, "--cleanup"]),
                "cancel the new run",
            );
        }

        let out = run(host.replace(NAME, &[]));
        assert_eq!(out.code, Some(0), "{point}: run again: {}", out.describe());
        let expected = if *point == "after-close" {
            "unchanged"
        } else {
            "replaced"
        };
        assert_eq!(
            out.json()["outcome"],
            expected,
            "{point}: {}",
            out.describe()
        );
        assert_closed(&host, NAME, true, point);
    }
}

#[test]
fn a_torn_close_is_trimmed_with_a_backup_and_finished() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let log = host.log(NAME);
    let mut cmd = host.replace(NAME, &[]);
    cmd.env(FAULT_ENV, "torn-close");
    let out = run(cmd);
    assert_eq!(out.code, Some(86), "{}", out.describe());
    let torn = std::fs::read(&log).unwrap();
    assert_ne!(
        torn.last(),
        Some(&b'\n'),
        "the stopped close left no torn line"
    );

    let out = expect_ok(host.replace(NAME, &[]), "run again");
    assert_eq!(out.json()["outcome"], "replaced", "{}", out.describe());

    let backups: Vec<String> = std::fs::read_dir(host.session_dir(NAME))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".bak"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    let prefix = format!("koto-{NAME}.state.jsonl.seq");
    assert!(
        backups[0].starts_with(&prefix) && backups[0].ends_with("-torn.bak"),
        "{backups:?}"
    );
    assert_eq!(
        std::fs::read(host.session_dir(NAME).join(&backups[0])).unwrap(),
        torn,
        "the backup isn't the log as the stopped close left it"
    );
    assert_eq!(std::fs::read(&log).unwrap().last(), Some(&b'\n'));
    assert_closed(&host, NAME, true, "after the trim");
}

#[test]
fn a_terminal_old_run_gets_the_fence_only() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    expect_ok(host.koto(&["next", NAME, "--with-data", GO]), "next go");
    let out = expect_ok(
        host.koto(&["next", NAME, "--with-data", OK, "--no-cleanup"]),
        "next ok",
    );
    assert_eq!(out.json()["state"], "done", "{}", out.describe());

    expect_ok(host.replace(NAME, &[]), "init --replaces");

    assert!(payloads(&host.log(NAME), "workflow_cancelled").is_empty());
    assert_closed(&host, NAME, false, "a finished run");
}

#[test]
fn a_cancelled_then_rewound_old_run_gets_a_fresh_cancel_and_the_fence() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    expect_ok(host.koto(&["next", NAME, "--with-data", GO]), "next go");
    expect_ok(host.koto(&["cancel", NAME]), "cancel");
    expect_ok(host.koto(&["rewind", NAME]), "rewind");
    let log = host.log(NAME);
    assert_eq!(payloads(&log, "workflow_cancelled").len(), 1);

    expect_ok(host.replace(NAME, &[]), "init --replaces");

    let cancels = payloads(&log, "workflow_cancelled");
    assert_eq!(cancels.len(), 2, "{cancels:?}");
    assert!(cancels[0].get("replaced_by").is_none(), "{cancels:?}");
    assert!(
        last_of(&log, "rewound") < last_of(&log, "workflow_cancelled"),
        "the fresh cancel didn't follow the rewind"
    );
    assert_closed(&host, NAME, true, "a cancelled then rewound run");
}

#[test]
fn replace_reports_the_old_runs_children_and_neither_cancels_nor_refuses_them() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let template = host.template();
    expect_ok(
        host.koto(&["init", "child", "--template", &template, "--parent", NAME]),
        "setup: init child",
    );

    let out = expect_ok(host.replace(NAME, &[]), "init --replaces");
    let json = out.json();
    assert_eq!(
        json["children"],
        json!([{"name": "child", "state": "start"}]),
        "{}",
        out.describe()
    );
    assert_eq!(json["deferred_cleanup"], true, "{}", out.describe());
    assert_closed(&host, NAME, true, "a run with a child");

    let child = host.log("child");
    assert!(payloads(&child, "workflow_cancelled").is_empty());
    assert!(payloads(&child, "session_superseded").is_empty());
    let out = expect_ok(
        host.koto(&["next", "child", "--with-data", GO]),
        "the child's tick",
    );
    assert_eq!(out.json()["state"], "more", "{}", out.describe());
}

#[test]
fn a_plain_supersede_fence_naming_new_is_refused_not_reported_unchanged() {
    let host = Host::new();
    host.init_at(&host.ws, NAME);
    let template = host.template();
    expect_ok(
        host.koto(&[
            "init",
            NEW,
            "--template",
            &template,
            "--continues",
            NAME,
            "--supersede",
        ]),
        "setup: supersede",
    );
    let old_before = files(&host.session_dir(NAME));
    let new_before = files(&host.session_dir(NEW));

    let out = run(host.replace(NAME, &[]));

    assert_refused(&out, 2, "session_replaced");
    assert_ne!(out.json()["outcome"], "unchanged", "{}", out.describe());
    assert_eq!(files(&host.session_dir(NAME)), old_before);
    assert_eq!(files(&host.session_dir(NEW)), new_before);
    assert!(host.replaces_key().is_none());
}

#[test]
fn a_replace_works_on_a_child_old_run_and_when_its_directories_are_gone() {
    let host = Host::new();
    let anchor = host.dir("parent-dir");
    host.init_at(&anchor, "parent");
    let template = host.template();
    expect_ok(
        host.koto_at(
            &anchor,
            &["init", NAME, "--template", &template, "--parent", "parent"],
        ),
        "setup: init child",
    );
    std::fs::remove_dir_all(&anchor).unwrap();

    let out = expect_ok(host.replace(NAME, &[]), "init --replaces");

    assert_eq!(out.json()["outcome"], "replaced", "{}", out.describe());
    assert_closed(&host, NAME, true, "a child whose anchor is gone");
    assert!(payloads(&host.log("parent"), "workflow_cancelled").is_empty());
}

#[test]
fn needed_on_the_old_directory_stops_holding_it_after_a_replace() {
    let host = Host::new();
    let old = host.dir("old");
    host.init_at(&old, NAME);
    let needed = || run(host.koto(&["session", "needed", old.to_str().unwrap()]));
    let out = needed();
    assert_eq!(out.code, Some(1), "{}", out.describe());

    expect_ok(host.replace(NAME, &[]), "init --replaces");

    let out = needed();
    assert_eq!(out.code, Some(0), "{}", out.describe());
}
