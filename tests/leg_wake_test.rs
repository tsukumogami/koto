//! The leg wake, end to end.
//!
//! A coordinator blocks on a `request-leg` gate while `koto request watch`
//! runs for it; the worker attached to the leg reaches its terminal state,
//! and the watch has to exit within the documented bound of that terminal
//! tick returning. The rest of the file pins the properties that make the
//! wake safe to depend on: a lost wake and a duplicate wake are harmless,
//! a cursor closes the gap between two watches, and a harness can watch
//! the file without any koto command.
//!
//! Every test points `HOME` and `KOTO_SESSIONS_BASE` into its own
//! temporary directory, so the request store and the wake files land
//! under `<tmp>/.koto/`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_fs::TempDir;

/// The documented bound: a running watch exits within this long of the
/// command that made the leg change returning.
const WAKE_BOUND: Duration = Duration::from_secs(1);

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

fn run(dir: &Path, args: &[&str]) -> (i32, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.code().unwrap_or(-1), json, stderr)
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, json, stderr) = run(dir, args);
    assert_eq!(code, 0, "expected success from {args:?}\n{json}\n{stderr}");
    json
}

fn init(dir: &Path, name: &str, file: &str, body: &str, vars: &[&str]) {
    let path = dir.join(file);
    std::fs::write(&path, body).unwrap();
    let mut args = vec!["init", name, "--template", path.to_str().unwrap()];
    for v in vars {
        args.push("--var");
        args.push(v);
    }
    run_ok(dir, &args);
}

fn create_request(dir: &Path, requested_by: &str, coordinator: &str) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"scope","role":"scope","template":"scope.md","inputs":"brief"}]}"#,
            "--requested-by",
            requested_by,
            "--coordinator-of-record",
            coordinator,
        ],
    );
    envelope["request_id"].as_str().unwrap().to_string()
}

fn wake_file(dir: &Path, session: &str) -> PathBuf {
    dir.join(".koto").join("wakes").join(session)
}

/// The session's cursor right now, read the way a harness would: a watch
/// with no budget.
fn cursor_now(dir: &Path, session: &str) -> String {
    let out = run_ok(
        dir,
        &[
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            "0",
        ],
    );
    assert_eq!(out["woke"], false, "{out}");
    out["cursor"].as_str().unwrap().to_string()
}

/// Start `koto request watch` in the background from a known cursor, so
/// no ring between reading the cursor and the watch starting is missed.
fn spawn_watch(dir: &Path, session: &str, since: &str) -> Child {
    StdCommand::new(assert_cmd::cargo::cargo_bin("koto"))
        .current_dir(dir)
        .env("HOME", dir)
        .env("KOTO_SESSIONS_BASE", dir.join("sessions"))
        .args([
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            "30",
            "--since",
            since,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Wait for a watch to exit and return its JSON and when it exited.
fn finish_watch(mut child: Child) -> (serde_json::Value, Instant) {
    let deadline = Instant::now() + Duration::from_secs(40);
    let exited_at = loop {
        if child.try_wait().unwrap().is_some() {
            break Instant::now();
        }
        assert!(Instant::now() < deadline, "the watch never exited");
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "watch failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap();
    (json, exited_at)
}

fn state_log(dir: &Path, session: &str) -> String {
    std::fs::read_to_string(
        dir.join("sessions")
            .join(session)
            .join(format!("koto-{session}.state.jsonl")),
    )
    .unwrap()
}

fn transitions(dir: &Path, session: &str) -> usize {
    state_log(dir, session)
        .lines()
        .filter(|l| l.contains("\"type\":\"transitioned\""))
        .count()
}

/// The worker: one evidence step, then a terminal whose result map
/// becomes the leg's payload.
const WORKER: &str = r#"---
name: scope
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
    result:
      outcome: scoped
---

## work

Scope it.

## done

Done.
"#;

/// The coordinator, parked on the worker's leg.
const COORDINATOR: &str = r#"---
name: coordinator
version: "1.0"
initial_state: waiting
variables:
  REQ:
    required: true
states:
  waiting:
    gates:
      leg:
        type: request-leg
        request: "{{REQ}}"
        leg: scope
        overridable: false
    transitions:
      - target: resumed
        when:
          gates.leg.disposition: resolved
      - target: abandoned
        when:
          gates.leg.disposition: abandoned
  resumed:
    accepts:
      next:
        type: string
        required: true
    transitions:
      - target: finished
        when:
          next: go
  abandoned:
    terminal: true
  finished:
    terminal: true
---

## waiting

Wait for the worker.

## resumed

Resumed; waiting for the next instruction.

## finished

Finished.

## abandoned

Abandoned.
"#;

/// A coordinator blocked on its leg, and a worker attached to it and one
/// step from its terminal.
fn parked(dir: &Path) -> String {
    let id = create_request(dir, "coord", "coord");
    init(
        dir,
        "coord",
        "coordinator.md",
        COORDINATOR,
        &[&format!("REQ={id}")],
    );
    let next = run_ok(dir, &["next", "coord", "--no-cleanup"]);
    assert_eq!(next["action"], "gate_blocked", "{next}");
    init(dir, "worker", "scope.md", WORKER, &[]);
    let before = cursor_now(dir, "coord");
    run_ok(
        dir,
        &["request", "attach", &id, "scope", "--session", "worker"],
    );
    assert_eq!(cursor_now(dir, "coord"), before, "attaching must not ring");
    id
}

fn finish_worker(dir: &Path) -> Instant {
    let done = run_ok(
        dir,
        &["next", "worker", "--with-data", r#"{"status":"ok"}"#],
    );
    assert_eq!(done["state"], "done", "{done}");
    Instant::now()
}

#[test]
fn a_parked_coordinator_is_woken_within_the_bound_of_the_workers_terminal_tick() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    parked(d);

    let since = cursor_now(d, "coord");
    let watch = spawn_watch(d, "coord", &since);
    // Let the watch reach its poll loop, so the bound is measured against a
    // watch that was already polling rather than one that happened to start
    // after the ring and returned on its first read.
    std::thread::sleep(Duration::from_millis(300));
    // The ring is a write inside the worker's own `koto next`, so nothing
    // outlives that command; the design records why no process is spawned.
    let returned_at = finish_worker(d);
    let (woke, exited_at) = finish_watch(watch);

    assert_eq!(woke["woke"], true, "{woke}");
    assert_eq!(woke["session"], "coord");
    assert_eq!(woke["cli_contract"]["minor"], 2);
    assert_ne!(woke["cursor"], since.as_str());
    let lag = exited_at.saturating_duration_since(returned_at);
    assert!(
        lag <= WAKE_BOUND,
        "the watch exited {lag:?} after the worker's terminal tick returned"
    );

    let resumed = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(resumed["state"], "resumed", "{resumed}");
}

#[test]
fn an_unread_wake_is_harmless() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    parked(d);

    // The ring succeeds, but nobody is watching when the worker finishes.
    finish_worker(d);
    let resumed = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(resumed["state"], "resumed", "{resumed}");
}

/// A wake that is never delivered at all: the ring fails, the worker's
/// terminal tick still succeeds, and the coordinator's next tick still
/// passes its gate.
#[test]
fn a_lost_wake_is_harmless() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    parked(d);

    let wakes = d.join(".koto").join("wakes");
    std::fs::create_dir_all(&wakes).unwrap();
    std::fs::set_permissions(&wakes, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (code, done, stderr) = run(d, &["next", "worker", "--with-data", r#"{"status":"ok"}"#]);
    std::fs::set_permissions(&wakes, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 0, "{done}\n{stderr}");
    assert_eq!(done["state"], "done", "{done}");
    // Root ignores the mode, so the ring may still succeed there; the
    // gate's behaviour is asserted either way.
    if !stderr.contains("could not deliver a wake") {
        assert!(wake_file(d, "coord").is_file(), "{stderr}");
    }

    let resumed = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(resumed["state"], "resumed", "{resumed}");
}

#[test]
fn a_duplicate_wake_is_harmless() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = parked(d);

    // The first wake: the worker finishes, the coordinator ticks and moves.
    let since = cursor_now(d, "coord");
    finish_worker(d);
    assert_ne!(cursor_now(d, "coord"), since);
    let first = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(first["state"], "resumed", "{first}");
    let after_first = transitions(d, "coord");
    assert!(after_first >= 1, "the first tick recorded no transition");

    // A second, real ring with nothing new for the coordinator: closing the
    // request rings its coordinator again. `resumed` is not terminal and
    // still has a transition, so a tick that did anything with the wake
    // itself could move it.
    let since = cursor_now(d, "coord");
    run_ok(d, &["request", "close", &id]);
    assert_ne!(cursor_now(d, "coord"), since, "the close did not ring");

    let second = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(second["state"], first["state"], "{second}");
    assert_eq!(transitions(d, "coord"), after_first);
}

#[test]
fn an_abandoned_leg_wakes_the_coordinator_too() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "coord", "coord");
    init(
        d,
        "coord",
        "coordinator.md",
        COORDINATOR,
        &[&format!("REQ={id}")],
    );
    run_ok(d, &["next", "coord", "--no-cleanup"]);

    let since = cursor_now(d, "coord");
    let watch = spawn_watch(d, "coord", &since);
    run_ok(
        d,
        &[
            "request",
            "abandon-request",
            &id,
            "--rationale",
            "superseded",
        ],
    );
    let (woke, _) = finish_watch(watch);
    assert_eq!(woke["woke"], true, "{woke}");

    let routed = run_ok(d, &["next", "coord", "--no-cleanup"]);
    assert_eq!(routed["state"], "abandoned", "{routed}");
}

#[test]
fn a_cursor_carries_a_wake_between_two_watches() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "coord", "coord");

    // The first watch times out; a wake lands before the second starts.
    let first = run_ok(
        d,
        &[
            "request",
            "watch",
            "--session",
            "coord",
            "--timeout-secs",
            "1",
        ],
    );
    assert_eq!(first["woke"], false, "{first}");
    run_ok(
        d,
        &[
            "request",
            "resolve",
            &id,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"ok"}"#,
        ],
    );

    let second = run_ok(
        d,
        &[
            "request",
            "watch",
            "--session",
            "coord",
            "--timeout-secs",
            "30",
            "--since",
            first["cursor"].as_str().unwrap(),
        ],
    );
    assert_eq!(second["woke"], true, "{second}");
}

#[test]
fn two_watches_on_one_session_both_see_one_wake() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "coord", "coord");
    let since = cursor_now(d, "coord");
    let a = spawn_watch(d, "coord", &since);
    let b = spawn_watch(d, "coord", &since);
    run_ok(
        d,
        &[
            "request",
            "abandon",
            &id,
            "scope",
            "--rationale",
            "not needed",
        ],
    );
    assert_eq!(finish_watch(a).0["woke"], true);
    assert_eq!(finish_watch(b).0["woke"], true);
}

#[test]
fn a_session_that_was_never_initialised_still_gets_its_wake() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "ghost", "ghost");
    let since = cursor_now(d, "ghost");
    assert_eq!(since, "w1:0:");
    let watch = spawn_watch(d, "ghost", &since);
    run_ok(d, &["request", "close", &id]);
    assert_eq!(finish_watch(watch).0["woke"], true);
    assert!(wake_file(d, "ghost").is_file());
}

#[test]
fn a_harness_can_watch_the_file_without_koto() {
    use std::os::unix::fs::MetadataExt;
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "coord", "coord");
    let stat = |p: &Path| {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), m.modified().unwrap(), m.ino())
    };
    // The first ring creates the file; a harness watching it from then on
    // must see the next ring as a change to the same file.
    run_ok(
        d,
        &[
            "request",
            "abandon",
            &id,
            "scope",
            "--rationale",
            "not needed",
        ],
    );
    let before = stat(&wake_file(d, "coord"));
    run_ok(d, &["request", "close", &id]);
    let after = stat(&wake_file(d, "coord"));
    assert_ne!(
        (before.0, before.1),
        (after.0, after.1),
        "no size or mtime change"
    );
    assert!(after.0 > before.0, "the ring did not append");
    assert_eq!(before.2, after.2, "the file was replaced, not appended to");
}

#[test]
fn a_watch_notices_a_wake_that_truncated_the_file() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d, "coord", "coord");
    // Fill the file past the truncation threshold, as about 1,200 unread
    // wakes would.
    let dir = d.join(".koto").join("wakes");
    std::fs::create_dir_all(&dir).unwrap();
    let filler: String = (0..3000).map(|i| format!("{i}.1.{i}\n")).collect();
    std::fs::write(wake_file(d, "coord"), filler).unwrap();
    assert!(std::fs::metadata(wake_file(d, "coord")).unwrap().len() >= 32 * 1024);

    let since = cursor_now(d, "coord");
    let watch = spawn_watch(d, "coord", &since);
    run_ok(d, &["request", "close", &id]);
    assert_eq!(finish_watch(watch).0["woke"], true);
    assert!(std::fs::metadata(wake_file(d, "coord")).unwrap().len() < 1024);
}

#[test]
fn a_watch_with_no_wake_times_out_cleanly() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let started = Instant::now();
    let out = run_ok(
        d,
        &[
            "request",
            "watch",
            "--session",
            "coord",
            "--timeout-secs",
            "1",
        ],
    );
    assert_eq!(out["woke"], false, "{out}");
    assert_eq!(out["cursor"], "w1:0:");
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[test]
fn watch_refuses_bad_arguments_and_an_unreadable_file() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();

    let (code, _, _) = run(d, &["request", "watch", "--session", "coord"]);
    assert_eq!(code, 2, "a missing --timeout-secs is a usage error");
    let (code, _, _) = run(d, &["request", "watch", "--timeout-secs", "1"]);
    assert_eq!(code, 2, "a missing --session is a usage error");
    let (code, json, _) = run(
        d,
        &[
            "request",
            "watch",
            "--session",
            "coord",
            "--timeout-secs",
            "1",
            "--since",
            "garbage",
        ],
    );
    assert_eq!(code, 2, "{json}");
    assert_eq!(json["error"]["code"], "invalid_submission", "{json}");

    // A directory where the wake file should be cannot be read as one.
    std::fs::create_dir_all(wake_file(d, "coord")).unwrap();
    let (code, json, _) = run(
        d,
        &[
            "request",
            "watch",
            "--session",
            "coord",
            "--timeout-secs",
            "1",
        ],
    );
    assert_eq!(code, 3, "{json}");
    assert_eq!(json["error"]["code"], "persistence_error", "{json}");
}
