//! Clearing a state's `clear_on_entry` keys when the workflow enters it
//! (DESIGN-koto-ci-wait-stale-keys.md, Decisions 1-3).
//!
//! These tests drive the real binary through each kind of entry -- a return
//! from another state, a self-transition, `koto next --to` and `koto rewind`
//! -- and check the two halves of a clearing: the key is gone from the store
//! before anything in the state reads it, and the log holds exactly one
//! `context_cleared` for the entry, naming its persisted sequence number.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;

// ===== Harness =====

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    // The `default_action` shells out to this build's `koto`.
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_koto"));
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin.parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    koto_cmd(dir).args(args).output().unwrap()
}

fn run_ok(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "koto {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn init(dir: &Path, name: &str, template: &str) {
    let src = dir.join(format!("{name}.md"));
    std::fs::write(&src, template).unwrap();
    run_ok(dir, &["init", name, "--template", src.to_str().unwrap()]);
}

fn next(dir: &Path, name: &str, data: Option<&str>) -> Value {
    let mut args = vec!["next", name, "--no-cleanup"];
    if let Some(d) = data {
        args.push("--with-data");
        args.push(d);
    }
    let out = run(dir, &args);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid JSON from next: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn add(dir: &Path, name: &str, key: &str, content: &str) {
    let file = dir.join("add-input.txt");
    std::fs::write(&file, content).unwrap();
    run_ok(
        dir,
        &[
            "context",
            "add",
            name,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ],
    );
}

fn exists(dir: &Path, name: &str, key: &str) -> bool {
    run(dir, &["context", "exists", name, key]).status.success()
}

fn events(dir: &Path, name: &str) -> Vec<Value> {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn of_type(events: &[Value], ty: &str) -> Vec<Value> {
    events.iter().filter(|e| e["type"] == ty).cloned().collect()
}

/// The seq of the latest entry event (`transitioned`, `directed_transition`
/// or `rewound`) into `state`.
fn last_entry_seq(events: &[Value], state: &str) -> u64 {
    events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e["type"].as_str(),
                Some("transitioned" | "directed_transition" | "rewound")
            ) && e["payload"]["to"] == state
        })
        .and_then(|e| e["seq"].as_u64())
        .expect("an entry into the state")
}

// ===== Template =====

/// `review` clears `verdict` on entry and gates on it; `outcome: retry` goes
/// to `fix` and back, `outcome: again` loops on `review` itself, and
/// `outcome: pass` with the verdict present finishes. A `default_action` on
/// `review` records, as `seen`, whether `verdict` existed when it ran.
const TEMPLATE: &str = r#"---
name: clear-on-entry
version: "1.0"
initial_state: review
states:
  review:
    clear_on_entry: [verdict]
    default_action:
      command: "if koto context exists $KOTO_TICK_SESSION verdict >/dev/null 2>&1; then echo present; else echo absent; fi > seen.txt"
    accepts:
      outcome:
        type: enum
        values: [pass, retry, again]
        required: true
    gates:
      verdict:
        type: context-exists
        key: verdict
    transitions:
      - target: done
        when:
          gates.verdict.exists: true
          outcome: pass
      - target: fix
        when:
          outcome: retry
      - target: review
        when:
          outcome: again
  fix:
    accepts:
      fixed:
        type: enum
        values: ["yes"]
        required: true
    transitions:
      - target: review
        when:
          fixed: "yes"
  done:
    terminal: true
---

## review

Review.

## fix

Fix.

## done

Done.
"#;

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    init(&dir, "wf", TEMPLATE);
    (tmp, dir)
}

fn seen(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("seen.txt"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

// ===== Tests =====

#[test]
fn the_first_entry_clears_nothing_and_logs_nothing() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    assert!(of_type(&events(&dir, "wf"), "context_cleared").is_empty());
}

#[test]
fn a_return_from_another_state_clears_the_key_before_the_state_reads_it() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    add(&dir, "wf", "verdict", "stale");
    // Control: the action sees the key while it's there.
    next(&dir, "wf", None);
    assert_eq!(seen(&dir), "present");
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));
    assert!(
        exists(&dir, "wf", "verdict"),
        "leaving review clears nothing"
    );

    let resp = next(&dir, "wf", Some(r#"{"fixed":"yes"}"#));
    assert_eq!(resp["state"], "review");
    assert!(!exists(&dir, "wf", "verdict"), "the stale verdict survived");
    // The action and the gate both ran after the clearing.
    assert_eq!(seen(&dir), "absent");
    let blocked: Vec<&Value> = resp["blocking_conditions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == "verdict")
        .collect();
    assert_eq!(blocked.len(), 1, "the verdict gate should block: {resp}");

    let log = events(&dir, "wf");
    let cleared = of_type(&log, "context_cleared");
    assert_eq!(cleared.len(), 1, "one clearing per entry: {cleared:?}");
    let entry = last_entry_seq(&log, "review");
    assert_eq!(
        cleared[0]["payload"],
        serde_json::json!({"state": "review", "keys": ["verdict"], "entry_seq": entry})
    );
    // Nothing read the key to decide: no context_read between the entry and
    // the clearing.
    let cleared_seq = cleared[0]["seq"].as_u64().unwrap();
    assert!(log.iter().all(|e| {
        let s = e["seq"].as_u64().unwrap();
        !(e["type"] == "context_read" && s > entry && s < cleared_seq)
    }));
}

#[test]
fn later_ticks_in_the_same_epoch_clear_nothing_more() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));
    next(&dir, "wf", Some(r#"{"fixed":"yes"}"#));
    add(&dir, "wf", "verdict", "fresh");
    next(&dir, "wf", None);
    next(&dir, "wf", None);
    assert!(
        exists(&dir, "wf", "verdict"),
        "a write after the entry is kept"
    );
    assert_eq!(of_type(&events(&dir, "wf"), "context_cleared").len(), 1);
}

#[test]
fn a_self_transition_is_an_entry_and_clears() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    add(&dir, "wf", "verdict", "stale");
    let resp = next(&dir, "wf", Some(r#"{"outcome":"again"}"#));
    assert_eq!(resp["state"], "review");
    assert!(!exists(&dir, "wf", "verdict"));
    let log = events(&dir, "wf");
    let cleared = of_type(&log, "context_cleared");
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        cleared[0]["payload"]["entry_seq"],
        last_entry_seq(&log, "review")
    );
}

#[test]
fn a_directed_transition_clears_before_it_returns() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));
    add(&dir, "wf", "verdict", "stale");
    run_ok(&dir, &["next", "wf", "--to", "review", "--no-cleanup"]);
    assert!(!exists(&dir, "wf", "verdict"));
    let log = events(&dir, "wf");
    let cleared = of_type(&log, "context_cleared");
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        cleared[0]["payload"]["entry_seq"],
        last_entry_seq(&log, "review")
    );
    // The next tick finds the entry settled.
    next(&dir, "wf", None);
    assert_eq!(of_type(&events(&dir, "wf"), "context_cleared").len(), 1);
}

#[test]
fn a_rewind_clears_before_it_returns() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    add(&dir, "wf", "verdict", "stale");
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));
    run_ok(&dir, &["rewind", "wf"]);
    assert!(!exists(&dir, "wf", "verdict"));
    let log = events(&dir, "wf");
    let cleared = of_type(&log, "context_cleared");
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        cleared[0]["payload"]["entry_seq"],
        last_entry_seq(&log, "review")
    );
}

#[test]
fn an_override_and_a_recheck_clear_nothing() {
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    // Leave and come back, so review is in an epoch that did owe a clearing.
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));
    next(&dir, "wf", Some(r#"{"fixed":"yes"}"#));
    assert_eq!(of_type(&events(&dir, "wf"), "context_cleared").len(), 1);

    add(&dir, "wf", "verdict", "kept");
    run_ok(
        &dir,
        &[
            "overrides",
            "record",
            "wf",
            "--gate",
            "verdict",
            "--rationale",
            "checked by hand",
        ],
    );
    next(&dir, "wf", None);
    next(&dir, "wf", None);
    assert!(exists(&dir, "wf", "verdict"), "an override is not an entry");
    assert_eq!(of_type(&events(&dir, "wf"), "context_cleared").len(), 1);
}

#[test]
fn an_entry_another_writer_recorded_is_cleared_on_the_next_tick() {
    // A batch retry rewinds a failed child by appending `rewound` to the
    // child's log directly, with no clearing. The child's next tick owes it.
    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    add(&dir, "wf", "verdict", "stale");
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));

    let log = sessions_base(&dir).join("wf").join("koto-wf.state.jsonl");
    let seq = events(&dir, "wf").last().unwrap()["seq"].as_u64().unwrap() + 1;
    let line = serde_json::json!({
        "seq": seq,
        "timestamp": "2026-01-01T00:00:00.000Z",
        "type": "rewound",
        "payload": {"from": "fix", "to": "review"},
    });
    let mut body = std::fs::read_to_string(&log).unwrap();
    body.push_str(&format!("{line}\n"));
    std::fs::write(&log, body).unwrap();
    assert!(
        exists(&dir, "wf", "verdict"),
        "appending the entry clears nothing"
    );

    let resp = next(&dir, "wf", None);
    assert_eq!(resp["state"], "review");
    assert!(!exists(&dir, "wf", "verdict"));
    assert_eq!(seen(&dir), "absent");
    let cleared = of_type(&events(&dir, "wf"), "context_cleared");
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0]["payload"]["entry_seq"], seq);
}

#[test]
fn a_removal_that_fails_records_nothing_and_the_next_tick_clears() {
    use std::os::unix::fs::PermissionsExt;

    let (_tmp, dir) = setup();
    next(&dir, "wf", None);
    add(&dir, "wf", "verdict", "stale");
    next(&dir, "wf", Some(r#"{"outcome":"retry"}"#));

    let ctx = sessions_base(&dir).join("wf").join("ctx");
    let original = std::fs::metadata(&ctx).unwrap().permissions();
    std::fs::set_permissions(&ctx, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores directory permissions, so the removal can't be made to fail.
    if std::fs::write(ctx.join("probe"), b"x").is_ok() {
        let _ = std::fs::remove_file(ctx.join("probe"));
        std::fs::set_permissions(&ctx, original).unwrap();
        eprintln!("skipping: directory permissions are not enforced for this user");
        return;
    }
    let out = run(
        &dir,
        &[
            "next",
            "wf",
            "--no-cleanup",
            "--with-data",
            r#"{"fixed":"yes"}"#,
        ],
    );
    std::fs::set_permissions(&ctx, original).unwrap();

    assert!(!out.status.success(), "a failed removal must fail the tick");
    assert!(of_type(&events(&dir, "wf"), "context_cleared").is_empty());
    assert!(exists(&dir, "wf", "verdict"));

    next(&dir, "wf", None);
    assert!(!exists(&dir, "wf", "verdict"));
    assert_eq!(of_type(&events(&dir, "wf"), "context_cleared").len(), 1);
}
