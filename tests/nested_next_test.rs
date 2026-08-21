//! `koto next` refuses to run inside a command koto is running (koto#208).
//!
//! The defect these cases pin: a nested tick performed a real transition --
//! in the original reproduction it advanced the session to its terminal
//! state -- while the outer tick, still holding the snapshot it started
//! with, reported the workflow as untouched. The caller's view was wrong
//! rather than absent, so nothing surfaced an error.
//!
//! The refusal is deliberately narrow. It covers `koto next` only: reading
//! and writing context from inside a command is a documented pattern and
//! stays allowed.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

/// The marker a tick exports and a nested one reads. Spelled out rather than
/// imported so the test fails if the name changes -- agents and the reference
/// docs both hard-code it.
const MARKER: &str = "KOTO_TICK_SESSION";

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("koto")
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn init_ok(dir: &Path, name: &str, template: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    let output = koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_next(dir: &Path, name: &str) -> serde_json::Value {
    let output = koto_cmd(dir).args(["next", name]).output().unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "next should print JSON, got stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn run_status(dir: &Path, name: &str) -> serde_json::Value {
    let output = koto_cmd(dir).args(["status", name]).output().unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "status should print JSON, got stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn read_file(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_else(|e| panic!("the command should have written {name}: {e}"))
}

// ---------------------------------------------------------------------------
// templates
// ---------------------------------------------------------------------------

/// A state whose gate runs `inner` once, records the outcome next to the
/// session, and passes regardless. The once-guard matters: without it the
/// nested tick evaluates the same gate and spawns another one, and the
/// original defect turns into unbounded recursion instead of the quiet wrong
/// answer #208 describes.
fn gate_runs_template(inner: &str) -> String {
    let command = format!(
        "if [ ! -f ran.guard ]; then touch ran.guard; \
         {inner} > inner.out 2> inner.err; echo $? > inner.code; fi; true"
    );
    format!(
        r#"---
name: nesting
version: "1.0"
initial_state: s
states:
  s:
    gates:
      probe:
        type: command
        command: "{command}"
    accepts:
      ack:
        type: enum
        values: [ok]
        required: true
    transitions:
      - target: done
        when:
          ack: ok
  done:
    terminal: true
---

## s

Do the thing.

## done

Done.
"#
    )
}

/// The nested call from the issue: a `koto next --with-data` on the very
/// session the outer tick is advancing, with output redirected so this is
/// not an output-capture artifact. `--no-cleanup` keeps the session on disk
/// after the nested tick drives it terminal, which is what makes the stale
/// answer observable rather than a bare "not found".
fn nested_next_on_same_session() -> String {
    format!(
        "{} next nesting --no-cleanup --with-data '{{\\\"ack\\\":\\\"ok\\\"}}'",
        koto_bin().display()
    )
}

// ---------------------------------------------------------------------------
// the defect
// ---------------------------------------------------------------------------

/// koto#208. Before the refusal, the outer tick answered `state: s,
/// advanced: false` while the nested tick had already driven the session to
/// its terminal state -- the answer was wrong, not missing. The assertion is
/// on that agreement: whatever the outer tick reports has to still be true
/// once the tick returns.
#[test]
fn the_outer_tick_does_not_report_a_state_the_session_has_left() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "nesting",
        &gate_runs_template(&nested_next_on_same_session()),
    );

    let outer = run_next(dir.path(), "nesting");
    let after = run_status(dir.path(), "nesting");

    assert_eq!(
        outer["state"], after["current_state"],
        "the tick reported state {} but the session is at {} -- the nested \
         tick moved it and the outer answer went stale (koto#208). \
         outer={outer} status={after}",
        outer["state"], after["current_state"]
    );
    assert_eq!(
        outer["advanced"], false,
        "nothing advanced this tick, so `advanced` must say so: {outer}"
    );
    assert_eq!(
        after["is_terminal"], false,
        "the refused nested tick must not have driven the session terminal: {after}"
    );
}

/// The nested call itself has to say what happened. A bare non-zero exit
/// leaves an agent guessing, so the refusal carries the registered code, an
/// exit code in the caller-error class, and a message naming both the tick
/// that is in flight and the call to take out of the template.
#[test]
fn the_refusal_names_the_condition_and_the_call_to_remove() {
    let dir = TempDir::new().unwrap();
    init_ok(
        dir.path(),
        "nesting",
        &gate_runs_template(&nested_next_on_same_session()),
    );

    run_next(dir.path(), "nesting");

    let code = read_file(dir.path(), "inner.code");
    assert_eq!(
        code.trim(),
        "2",
        "a refusal the caller has to act on exits 2; got {code}"
    );

    let body = read_file(dir.path(), "inner.out");
    let envelope: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("the refusal should be a JSON envelope ({e}): {body}"));
    assert_eq!(
        envelope["error"]["code"], "nested_invocation",
        "the refusal needs its own code, not a reused one: {envelope}"
    );

    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("'nesting'"),
        "the message should name the tick already in flight: {message}"
    );
    assert!(
        message.contains("koto next nesting"),
        "the message should name the call to remove: {message}"
    );
}

/// A nested tick on some other session is refused too. The marker names one
/// session, but the hazard is re-entrancy rather than a name collision: a
/// chain that ticks back into the outer session through a second one lands
/// on exactly the #208 failure, and nothing known needs koto to tick a
/// second workflow from inside a command.
#[test]
fn a_nested_tick_on_a_different_session_is_refused_as_well() {
    let dir = TempDir::new().unwrap();
    init_ok(dir.path(), "other", &gate_runs_template("true"));
    let inner = format!("{} next other", koto_bin().display());
    init_ok(dir.path(), "nesting", &gate_runs_template(&inner));

    run_next(dir.path(), "nesting");

    assert_eq!(
        read_file(dir.path(), "inner.code").trim(),
        "2",
        "a tick on another session from inside a command is refused too"
    );
    let body = read_file(dir.path(), "inner.out");
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["error"]["code"], "nested_invocation");
}

// ---------------------------------------------------------------------------
// what stays allowed
// ---------------------------------------------------------------------------

/// The refusal covers `koto next` and nothing else. Writing context from a
/// command is a documented pattern -- the loop-back edge that clears a key
/// is authored exactly this way -- and it never showed the stale-answer
/// defect, so it keeps working.
#[test]
fn context_writes_from_inside_a_command_still_work() {
    let dir = TempDir::new().unwrap();
    let inner = format!(
        "echo hello | {} context add nesting note.md",
        koto_bin().display()
    );
    init_ok(dir.path(), "nesting", &gate_runs_template(&inner));

    run_next(dir.path(), "nesting");

    assert_eq!(
        read_file(dir.path(), "inner.code").trim(),
        "0",
        "a context write from inside a command must still succeed: {}",
        read_file(dir.path(), "inner.err")
    );

    let output = koto_cmd(dir.path())
        .args(["context", "get", "nesting", "note.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "hello",
        "the value the command wrote should be readable afterwards"
    );
}

/// The marker has no liveness: a command that detaches itself escapes the
/// process-group kill at timeout and carries `KOTO_TICK_SESSION` for as long
/// as it lives, so a tick that exited long ago can still refuse it. Clearing
/// the variable is the way out, and the refusal message names it -- so it is
/// a contract, not an accident of how the marker is read.
#[test]
fn clearing_the_marker_lets_a_tick_through() {
    let dir = TempDir::new().unwrap();
    init_ok(dir.path(), "nesting", &gate_runs_template("true"));

    let refused = koto_cmd(dir.path())
        .env(MARKER, "some-tick-that-exited")
        .args(["next", "nesting"])
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(2),
        "a stale marker refuses: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("KOTO_TICK_SESSION= koto next nesting"),
        "the refusal must name the way out, since whoever hits it cannot \
         read the source: {message}"
    );

    // Exactly what the message told them to run.
    let cleared = koto_cmd(dir.path())
        .env(MARKER, "")
        .args(["next", "nesting"])
        .output()
        .unwrap();
    assert!(
        cleared.status.success(),
        "clearing the marker must let the tick through: stdout={} stderr={}",
        String::from_utf8_lossy(&cleared.stdout),
        String::from_utf8_lossy(&cleared.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&cleared.stdout).unwrap();
    assert_eq!(body["state"], "s", "the cleared tick should run: {body}");
}

/// A plain `koto next` from a shell is not nested, and the marker an outer
/// tick sets does not outlive it. This is the guard against the refusal
/// firing on ordinary use.
#[test]
fn a_tick_that_follows_another_tick_is_not_nested() {
    let dir = TempDir::new().unwrap();
    init_ok(dir.path(), "nesting", &gate_runs_template("true"));

    let first = run_next(dir.path(), "nesting");
    assert_eq!(first["state"], "s", "first tick should land on s: {first}");

    let second = run_next(dir.path(), "nesting");
    assert_eq!(
        second["action"], "evidence_required",
        "a second tick from the shell is a fresh process and must not be \
         treated as nested: {second}"
    );
}
