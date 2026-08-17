//! Integration coverage for `koto status`'s phase-retrieval extension
//! (Issue 3 of docs/plans/PLAN-inline-phase-details.md).
//!
//! `handle_status` gains three conditionally-present keys -- `directive`,
//! `details`, `expects` -- substituted through the same pipeline `koto
//! next` uses, plus a `template_hash_mismatch` key that reports (rather
//! than fails) when the compiled template on disk diverges from the hash
//! recorded in the session header. This file exercises the read-only
//! contract: the retrieval never appends an event, never evaluates a
//! gate, never runs a default action, never blocks on a lock, and never
//! cleans up a terminal session.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use koto::session::SessionBackend;

// ---------------------------------------------------------------------------
//  Harness (mirrors tests/instructions_delivery_test.rs)
// ---------------------------------------------------------------------------

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn session_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

/// Run a `koto` invocation and parse its last non-blank stdout line as
/// JSON. Panics if the process did not exit successfully.
fn run_koto(dir: &Path, args: &[&str]) -> serde_json::Value {
    let output = koto_cmd(dir).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "`koto {}` failed: stdout={} stderr={}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    parse_last_json(&output.stdout)
}

/// Like `run_koto`, but does not assert success -- for the error-path
/// tests. Returns (success, json, stderr).
fn run_koto_raw(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let json = parse_last_json(&output.stdout);
    (
        output.status.success(),
        json,
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn parse_last_json(stdout: &[u8]) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(stdout).to_string();
    let trimmed = stdout.trim();
    serde_json::from_str(trimmed).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
    })
}

fn init_workflow(dir: &Path, name: &str, template_content: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();
    koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .assert()
        .success();
}

fn init_workflow_with_vars(dir: &Path, name: &str, template_content: &str, vars: &[&str]) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();
    let mut args = vec!["init", name, "--template", src.to_str().unwrap()];
    for var in vars {
        args.push("--var");
        args.push(var);
    }
    koto_cmd(dir).args(&args).assert().success();
}

// ---------------------------------------------------------------------------
//  Templates
// ---------------------------------------------------------------------------

/// `gather` declares instructions and a single-field accepts schema;
/// `implement` declares instructions and self-loops; `bare` declares no
/// instructions; `done` is terminal. Covers substitution (runtime
/// `{{SESSION_DIR}}` and template `{{ORG}}`), `expects`, terminal
/// absence, and instruction-free absence in one template.
const PHASES_TEMPLATE: &str = r#"---
name: status-fixture
version: "1.0"
initial_state: gather
variables:
  ORG:
    description: "Organization"
    default: "acme"
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: implement
        when:
          route: go
  implement:
    accepts:
      loop_again:
        type: enum
        required: true
        values: [yes, no]
    transitions:
      - target: implement
        when:
          loop_again: yes
      - target: bare
        when:
          loop_again: no
  bare:
    transitions:
      - target: done
  done:
    terminal: true
---

## gather

Collect inputs for {{ORG}}.

<!-- details -->

Gather instructions for {{ORG}} at {{SESSION_DIR}}.

## implement

Make the change for {{ORG}}.

<!-- details -->

Implement instructions for {{ORG}}.

## bare

Nothing to declare here.

## done

Done.
"#;

/// `act` carries both a default action and a gate, each with an
/// observable filesystem side effect. `start` transitions to `act`
/// unconditionally, which lets a directed transition (`--to act`) land
/// the workflow there without running either -- the fixture for
/// asserting `status` doesn't run them either.
const ACTION_TEMPLATE: &str = r#"---
name: action-fixture
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: act
  act:
    default_action:
      command: "touch action-ran.txt"
    gates:
      probe:
        type: command
        command: "touch gate-ran.txt && test -f nonexistent-marker"
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Get going.

## act

Take action.

<!-- details -->

Act instructions.

## done

Done.
"#;

/// A single gated state whose gate command sleeps, so a `koto next`
/// against it stays mid-tick for a controllable duration. The gate
/// command touches a marker file before sleeping, which the test polls
/// for to know the tick has genuinely entered the gate before racing
/// `koto status` against it.
const SLOW_GATE_TEMPLATE: &str = r#"---
name: slow-gate-fixture
version: "1.0"
initial_state: start
states:
  start:
    gates:
      slow:
        type: command
        command: "touch gate-started.txt && sleep 3"
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Waiting on a slow gate.

<!-- details -->

Slow gate instructions.

## done

Done.
"#;

// ---------------------------------------------------------------------------
//  directive / details / expects: presence, substitution, terminal/absence
// ---------------------------------------------------------------------------

#[test]
fn status_directive_details_expects_match_what_next_would_return() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    // Retrieval before any `next` call -- R10: instructions are returned
    // whether or not a delivery has happened yet.
    let status = run_koto(root, &["status", "wf"]);
    assert_eq!(
        status["directive"].as_str(),
        Some("Collect inputs for acme."),
        "directive should have runtime/template vars substituted: {status}"
    );
    assert_eq!(
        status["details"].as_str(),
        Some(
            format!(
                "Gather instructions for acme at {}.",
                session_dir_str(root, "wf")
            )
            .as_str()
        ),
        "details should have {{{{SESSION_DIR}}}} substituted: {status}"
    );
    assert_eq!(
        status["expects"]["event_type"].as_str(),
        Some("evidence_submitted")
    );
    assert!(
        status["expects"]["fields"].get("route").is_some(),
        "expects.fields should contain the accepts schema: {status}"
    );

    // The first `koto next` call on the same phase must produce
    // byte-identical directive/details text (DESIGN: "recovered text is
    // identical to what `next` would have produced").
    let next = run_koto(root, &["next", "wf"]);
    assert_eq!(next["directive"], status["directive"]);
    assert_eq!(next["details"], status["details"]);
}

fn session_dir_str(root: &Path, name: &str) -> String {
    sessions_base(root)
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn status_omits_all_three_keys_when_terminal() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    run_koto(root, &["next", "wf", "--with-data", r#"{"route":"go"}"#]);
    run_koto(
        root,
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"loop_again":"no"}"#,
            "--no-cleanup",
        ],
    );

    let status = run_koto(root, &["status", "wf"]);
    assert_eq!(status["current_state"].as_str(), Some("done"));
    assert_eq!(status["is_terminal"], true);
    assert!(status.get("directive").is_none(), "{status}");
    assert!(status.get("details").is_none(), "{status}");
    assert!(status.get("expects").is_none(), "{status}");

    // Retrieval against a terminal phase must not clean up the session.
    assert!(
        session_state_path(root, "wf").exists(),
        "status must not clean up a terminal session"
    );
}

#[test]
fn status_details_and_expects_absent_when_phase_declares_neither() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    run_koto(root, &["next", "wf", "--with-data", r#"{"route":"go"}"#]);
    // Directed transition from `implement` to `bare`: single-shot, does
    // not require the `when` condition to hold, and does not continue
    // the advance loop -- lands the workflow at `bare` without auto-
    // advancing into `done`.
    run_koto(root, &["next", "wf", "--to", "bare"]);

    let status = run_koto(root, &["status", "wf"]);
    assert_eq!(status["current_state"].as_str(), Some("bare"));
    assert_eq!(status["is_terminal"], false);
    assert_eq!(
        status["directive"].as_str(),
        Some("Nothing to declare here.")
    );
    assert!(
        status.get("details").is_none(),
        "bare declares no instructions: {status}"
    );
    assert!(
        status.get("expects").is_none(),
        "bare declares no accepts schema: {status}"
    );
}

// ---------------------------------------------------------------------------
//  No side effects: gate not evaluated, default action not run
// ---------------------------------------------------------------------------

#[test]
fn status_does_not_execute_gate_or_default_action() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow(root, "wf", ACTION_TEMPLATE);

    // Directed transition into `act`: does not run its gate or its
    // default action (that is the directed path's own contract, not
    // something `status` relies on -- confirmed here as a fixture
    // sanity check before the real assertion below).
    run_koto(root, &["next", "wf", "--to", "act"]);
    assert!(!root.join("action-ran.txt").exists());
    assert!(!root.join("gate-ran.txt").exists());

    let status = run_koto(root, &["status", "wf"]);
    assert_eq!(status["current_state"].as_str(), Some("act"));
    assert_eq!(status["directive"].as_str(), Some("Take action."));

    assert!(
        !root.join("action-ran.txt").exists(),
        "status must not execute act's default action"
    );
    assert!(
        !root.join("gate-ran.txt").exists(),
        "status must not evaluate act's gate"
    );
}

// ---------------------------------------------------------------------------
//  No writes: byte-identical state file, no delivery record, next
//  call unaffected by an intervening retrieval
// ---------------------------------------------------------------------------

#[test]
fn status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    // First occupancy of `implement`: carries details.
    run_koto(root, &["next", "wf", "--with-data", r#"{"route":"go"}"#]);
    let first_implement = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert!(
        first_implement.get("details").is_some(),
        "first arrival at implement should carry details: {first_implement}"
    );

    let before = std::fs::read(session_state_path(root, "wf")).unwrap();

    let status = run_koto(root, &["status", "wf"]);
    assert!(
        status.get("details").is_some(),
        "retrieval always returns details regardless of delivery history: {status}"
    );

    let after = std::fs::read(session_state_path(root, "wf")).unwrap();
    assert_eq!(
        before, after,
        "the session state file must be byte-identical before and after a retrieval"
    );

    // A non-advancing `next` right after the retrieval must still
    // suppress details, exactly as it would have without the retrieval
    // in between.
    let repeat = run_koto(root, &["next", "wf"]);
    assert!(
        repeat.get("details").is_none(),
        "an intervening retrieval must not change what the next `koto next` returns: {repeat}"
    );
}

// ---------------------------------------------------------------------------
//  Error path
// ---------------------------------------------------------------------------

#[test]
fn status_unknown_workflow_returns_structured_error() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    sessions_base(root); // ensure the sessions dir exists, no session in it

    let (ok, json, _stderr) = run_koto_raw(root, &["status", "no-such-workflow"]);
    assert!(!ok, "status on an unknown workflow must exit non-zero");
    assert!(
        json["error"].as_str().is_some(),
        "expected a structured error, got: {json}"
    );
}

// ---------------------------------------------------------------------------
//  Template hash mismatch: reported, not fatal
// ---------------------------------------------------------------------------

#[test]
fn status_reports_template_hash_mismatch_without_failing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    let before = run_koto(root, &["status", "wf"]);
    assert!(
        before.get("template_hash_mismatch").is_none(),
        "no mismatch should be reported against an untouched cache: {before}"
    );
    let recorded_hash = before["template_hash"]
        .as_str()
        .expect("template_hash should be present")
        .to_string();
    let template_path = before["template_path"]
        .as_str()
        .expect("template_path should be present")
        .to_string();

    // Tamper with the cached compiled template: still valid JSON, but
    // its content -- and therefore its hash -- differs from what the
    // session header recorded at init time.
    let raw = std::fs::read_to_string(&template_path).unwrap();
    let mut compiled: serde_json::Value = serde_json::from_str(&raw).unwrap();
    compiled["states"]["gather"]["directive"] =
        serde_json::json!("Collect inputs for acme. (tampered)");
    let mut f = std::fs::File::create(&template_path).unwrap();
    f.write_all(serde_json::to_string_pretty(&compiled).unwrap().as_bytes())
        .unwrap();

    let (ok, after, stderr) = run_koto_raw(root, &["status", "wf"]);
    assert!(ok, "a hash mismatch must not fail the retrieval: {stderr}");

    let mismatch = after
        .get("template_hash_mismatch")
        .unwrap_or_else(|| panic!("template_hash_mismatch should be present: {after}"));
    assert_eq!(mismatch["recorded"].as_str(), Some(recorded_hash.as_str()));
    let actual = mismatch["actual"]
        .as_str()
        .expect("actual hash should be present");
    assert_ne!(actual, recorded_hash);

    // Best-effort content is still returned alongside the mismatch flag.
    assert_eq!(
        after["directive"].as_str(),
        Some("Collect inputs for acme. (tampered)")
    );
}

// ---------------------------------------------------------------------------
//  No blocking: a held lock, and a slow mid-tick `koto next`, do not
//  delay `koto status`
// ---------------------------------------------------------------------------

#[test]
fn status_returns_promptly_while_another_process_holds_the_state_file_lock() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    let backend = koto::session::local::LocalBackend::with_base_dir(sessions_base(root));
    let _guard = backend
        .lock_state_file("wf")
        .expect("this test process should be able to acquire the lock uncontended");

    let started = Instant::now();
    let status = run_koto(root, &["status", "wf"]);
    let elapsed = started.elapsed();

    assert_eq!(status["current_state"].as_str(), Some("gather"));
    assert!(
        elapsed < Duration::from_secs(2),
        "status must not block on a lock held by another process; took {elapsed:?}"
    );
}

/// The held-lock and mid-tick races above show `status` doesn't wait on
/// contention, but neither discriminates a correct implementation from
/// one that wrongly attempts a non-blocking lock: on a non-batch session
/// no lock exists to contend over, so both would return immediately
/// either way. This test is the one that does discriminate -- it traces
/// the process with `strace` and asserts no `flock` syscall is made at
/// all. Skips (rather than fails) when `strace` isn't on `PATH`, since
/// its presence isn't guaranteed on every dev machine; GitHub's
/// `ubuntu-latest` runner ships it, so CI still gets the real check.
#[test]
fn status_attempts_no_lock_syscall_on_a_non_batch_session() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow_with_vars(root, "wf", PHASES_TEMPLATE, &["ORG=acme"]);

    let trace_path = root.join("status.strace");
    let koto_bin = assert_cmd::cargo::cargo_bin("koto");

    let spawn = std::process::Command::new("strace")
        .args([
            "-f",
            "-e",
            "trace=flock",
            "-o",
            trace_path.to_str().unwrap(),
            "--",
        ])
        .arg(&koto_bin)
        .args(["status", "wf"])
        .current_dir(root)
        .env("KOTO_SESSIONS_BASE", sessions_base(root))
        .env("HOME", root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match spawn {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: `strace` not found on PATH");
            return;
        }
        Err(e) => panic!("failed to spawn strace: {e}"),
    };
    assert!(
        output.status.success(),
        "`strace -- koto status wf` failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let trace = std::fs::read_to_string(&trace_path).unwrap();
    assert!(
        !trace.contains("flock("),
        "status must not attempt any flock syscall on a non-batch session; strace output:\n{trace}"
    );
}

#[test]
fn status_returns_promptly_while_koto_next_is_mid_tick_on_a_slow_gate() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    init_workflow(root, "wf", SLOW_GATE_TEMPLATE);

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"))
        .args(["next", "wf"])
        .current_dir(root)
        .env("KOTO_SESSIONS_BASE", sessions_base(root))
        .env("HOME", root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Poll for the gate's own start marker rather than sleeping a fixed
    // guess, so this isn't racy about exactly when the tick reaches the
    // gate command.
    let marker = root.join("gate-started.txt");
    let poll_deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            Instant::now() < poll_deadline,
            "the slow gate never started within 5s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let started = Instant::now();
    let status = run_koto(root, &["status", "wf"]);
    let elapsed = started.elapsed();

    assert_eq!(status["current_state"].as_str(), Some("start"));
    assert!(
        elapsed < Duration::from_millis(1500),
        "status must return immediately while a sibling `next` is mid-tick \
         on a 3s gate; took {elapsed:?}"
    );

    // Let the background tick finish so the child process (and its
    // sleeping gate command) doesn't outlive the test.
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "the background `next` should still complete: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}
