//! Integration coverage for `koto init <name> --from-stdin` (inline
//! workflow definitions, Issue 2 of the ad-hoc-workflows plan).
//!
//! Drives the real `koto` binary so the full pipe-a-definition →
//! `koto next` → terminal → `koto rewind` flow is exercised end to end,
//! including the clap-layer flag contract (mutual exclusion with
//! `--template`, rejection of `--allow-legacy-gates`) and the strict
//! compile error surface.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    // Override HOME so tests don't read the user's ~/.koto/config.toml.
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir).join(name)
}

fn state_file_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{}.state.jsonl", name))
}

/// A valid, strict-compilable inline definition: two states, the first
/// auto-advances to a terminal `done`. No gates, so it survives strict
/// compilation without any `gates.*` routing requirement.
const VALID_DEFINITION: &str = r#"---
name: inline-stdin-wf
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Do the first task.

## done

All done.
"#;

/// A definition that fails strict validation by referencing a transition
/// target that does not exist. The compiler names the failing element
/// (state + undefined target), not a generic "invalid template".
const DANGLING_TARGET_DEFINITION: &str = r#"---
name: bad-target-wf
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: nowhere
  done:
    terminal: true
---

## start

Go nowhere.

## done

Done.
"#;

/// A legacy-gate definition: a state with a gate but no `gates.*` when-clause
/// routing. Strict compilation rejects this; only `--allow-legacy-gates`
/// (forbidden on the stdin path) would permit it.
const LEGACY_GATE_DEFINITION: &str = r#"---
name: legacy-gate-wf
version: "1.0"
initial_state: check
states:
  check:
    gates:
      ready:
        type: command
        command: "true"
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check readiness.

## done

Done.
"#;

fn run(dir: &Path, args: &[&str], stdin: Option<&str>) -> std::process::Output {
    let mut cmd = koto_cmd(dir);
    cmd.args(args);
    if let Some(s) = stdin {
        cmd.write_stdin(s.to_string());
    }
    cmd.output().unwrap()
}

fn last_json(out: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
}

/// Happy path: pipe a valid definition, the session starts (no template
/// file pre-existing), `koto next` reaches the terminal state, and
/// `koto rewind` returns to the prior state.
#[test]
fn from_stdin_runs_to_terminal_and_rewinds() {
    let dir = TempDir::new().unwrap();

    // Precondition: no template file exists anywhere the agent manages.
    assert!(
        !state_file_path(dir.path(), "wf").exists(),
        "no session should exist before init"
    );

    let init = run(
        dir.path(),
        &["init", "wf", "--from-stdin"],
        Some(VALID_DEFINITION),
    );
    assert!(
        init.status.success(),
        "init --from-stdin should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    let json = last_json(&init);
    assert_eq!(json["name"].as_str(), Some("wf"));
    assert_eq!(
        json["state"].as_str(),
        Some("start"),
        "first directive should be the initial state"
    );

    // State file now exists inside the session dir.
    assert!(
        state_file_path(dir.path(), "wf").exists(),
        "session state file must exist after inline init"
    );

    // The agent never managed a template file: the only authored artifact
    // koto wrote is the source under the session dir. There is no
    // template file pre-existing in the working directory.
    let stray_template = dir.path().join("wf.md");
    assert!(
        !stray_template.exists(),
        "no template file should exist in the working dir"
    );

    // koto next auto-advances the linear workflow to terminal `done`.
    // --no-cleanup keeps the session on disk so the subsequent rewind has
    // a session to roll back (terminal cleanup would otherwise remove it).
    let next = run(dir.path(), &["next", "wf", "--no-cleanup"], None);
    assert!(
        next.status.success(),
        "next should succeed: stderr={}",
        String::from_utf8_lossy(&next.stderr)
    );
    let next_json = last_json(&next);
    assert_eq!(
        next_json["state"].as_str(),
        Some("done"),
        "auto-advancement should reach the terminal state"
    );

    // koto rewind returns the session to a prior state.
    let rewind = run(dir.path(), &["rewind", "wf"], None);
    assert!(
        rewind.status.success(),
        "rewind should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&rewind.stdout),
        String::from_utf8_lossy(&rewind.stderr)
    );
}

/// The compiled artifact lives in the session dir and per-tick hash
/// verification keeps working even with `~/.cache/koto` emptied — proving
/// the inline path does not depend on the global cache. (Issue 1 covers
/// this at the unit level; this is the end-to-end CLI confirmation.)
#[test]
fn from_stdin_artifact_survives_cache_eviction() {
    let dir = TempDir::new().unwrap();
    let init = run(
        dir.path(),
        &["init", "wf", "--from-stdin"],
        Some(VALID_DEFINITION),
    );
    assert!(init.status.success());

    // HOME points at the tempdir, so the XDG cache resolves under it.
    let cache = dir.path().join(".cache").join("koto");
    if cache.exists() {
        std::fs::remove_dir_all(&cache).unwrap();
    }

    let next = run(dir.path(), &["next", "wf"], None);
    assert!(
        next.status.success(),
        "next must succeed with the cache evicted: stderr={}",
        String::from_utf8_lossy(&next.stderr)
    );
}

/// The readable source is persisted under the fixed `source` filename and
/// is byte-for-byte identical to the piped stdin input.
#[test]
fn from_stdin_persists_byte_identical_source_under_fixed_name() {
    let dir = TempDir::new().unwrap();
    let init = run(
        dir.path(),
        &["init", "wf", "--from-stdin"],
        Some(VALID_DEFINITION),
    );
    assert!(init.status.success());

    let source_path = session_dir(dir.path(), "wf").join("source");
    assert!(
        source_path.exists(),
        "readable source must be persisted under the fixed `source` filename"
    );
    let recovered = std::fs::read(&source_path).unwrap();
    assert_eq!(
        recovered,
        VALID_DEFINITION.as_bytes(),
        "recovered source must be byte-for-byte identical to stdin input"
    );
}

/// `--from-stdin` and `--template` are mutually exclusive.
#[test]
fn from_stdin_and_template_are_mutually_exclusive() {
    let dir = TempDir::new().unwrap();
    // Write a template file so --template has a plausible target.
    let tpl = dir.path().join("t.md");
    std::fs::write(&tpl, VALID_DEFINITION).unwrap();

    let out = run(
        dir.path(),
        &[
            "init",
            "wf",
            "--from-stdin",
            "--template",
            tpl.to_str().unwrap(),
        ],
        Some(VALID_DEFINITION),
    );
    assert!(!out.status.success(), "must reject both flags");
    let json = last_json(&out);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("--from-stdin") && err.contains("--template") && err.contains("mutually"),
        "error must name both flags as mutually exclusive, got: {}",
        err
    );
    assert!(
        !state_file_path(dir.path(), "wf").exists(),
        "no session should be created on the mutual-exclusion error"
    );
}

/// `--from-stdin` rejects `--allow-legacy-gates` (the inline path is
/// strict-only).
#[test]
fn from_stdin_rejects_allow_legacy_gates() {
    let dir = TempDir::new().unwrap();
    let out = run(
        dir.path(),
        &["init", "wf", "--from-stdin", "--allow-legacy-gates"],
        Some(VALID_DEFINITION),
    );
    assert!(!out.status.success(), "must reject --allow-legacy-gates");
    let json = last_json(&out);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("--from-stdin") && err.contains("--allow-legacy-gates"),
        "error must name both flags, got: {}",
        err
    );
    assert!(
        !state_file_path(dir.path(), "wf").exists(),
        "no session should be created on the reject-legacy error"
    );
}

/// A definition failing strict validation creates no session, exits
/// non-zero, and names the failing element (the state and its undefined
/// transition target) rather than a generic "invalid template".
#[test]
fn from_stdin_strict_failure_names_failing_element_and_creates_no_session() {
    let dir = TempDir::new().unwrap();
    let out = run(
        dir.path(),
        &["init", "wf", "--from-stdin"],
        Some(DANGLING_TARGET_DEFINITION),
    );
    assert!(
        !out.status.success(),
        "strict failure must exit non-zero: stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json = last_json(&out);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("start") && err.contains("nowhere"),
        "error must name the failing state and undefined target, got: {}",
        err
    );
    assert!(
        !state_file_path(dir.path(), "wf").exists(),
        "strict failure must create NO session"
    );
    // The bare session directory must not linger either.
    assert!(
        !session_dir(dir.path(), "wf").exists(),
        "strict failure must leave no half-built session directory"
    );
}

/// A legacy-gate definition is rejected on the strict inline path, naming
/// the state and gate.
#[test]
fn from_stdin_rejects_legacy_gate_definition() {
    let dir = TempDir::new().unwrap();
    let out = run(
        dir.path(),
        &["init", "wf", "--from-stdin"],
        Some(LEGACY_GATE_DEFINITION),
    );
    assert!(
        !out.status.success(),
        "legacy-gate definition must be rejected on the strict path"
    );
    let json = last_json(&out);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("check") && err.contains("ready"),
        "error must name the legacy state and gate, got: {}",
        err
    );
    assert!(
        !session_dir(dir.path(), "wf").exists(),
        "legacy-gate rejection must leave no session"
    );
}
