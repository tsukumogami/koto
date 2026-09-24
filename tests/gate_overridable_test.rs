//! `overridable: false` on gates.
//!
//! A gate declared `overridable: false` refuses `koto overrides record`, with
//! or without `--with-data`, and leaves the state file untouched. The
//! neighbouring overridable gate in the same state keeps today's behaviour.
//! The compile-side checks (unknown keys, non-boolean values, a pointless
//! `override_default`) live in `src/template/compile.rs`; the evaluation-time
//! defense lives in `src/engine/advance.rs`. This file covers the CLI surface
//! end to end, plus a snapshot proving templates that don't use the field
//! compile byte-identical to before it existed.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    // Keep the user's ~/.koto/config.toml out of the test.
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

fn init_workflow(dir: &Path, name: &str, template: &str) {
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

fn run(dir: &Path, args: &[&str]) -> (i32, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.code().unwrap_or(-1), json, stderr)
}

/// One state, two failing command gates: `locked` refuses overrides, `open`
/// accepts them. The unconditional transition fires only when both pass.
const TWO_GATES: &str = r#"---
name: overridable-test
version: "1.0"
initial_state: check
states:
  check:
    gates:
      locked:
        type: command
        command: "exit 1"
        overridable: false
      open:
        type: command
        command: "exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Both gates must pass.

## done

Done.
"#;

/// Same shape, but the state accepts evidence, so a failing gate yields
/// `evidence_required` rather than `gate_blocked`.
const GATE_WITH_ACCEPTS: &str = r#"---
name: overridable-accepts
version: "1.0"
initial_state: check
states:
  check:
    gates:
      locked:
        type: command
        command: "exit 1"
        overridable: false
    accepts:
      decision:
        type: enum
        values: [retry]
        required: true
    transitions:
      - target: done
        when:
          decision: retry
  done:
    terminal: true
---

## check

The gate must pass.

## done

Done.
"#;

fn assert_refused(json: &serde_json::Value, code: i32, stderr: &str) {
    assert_eq!(
        code, 2,
        "refusal must exit 2: body={} stderr={}",
        json, stderr
    );
    assert_eq!(
        json["error"]["code"], "gate_not_overridable",
        "typed code missing: {}",
        json
    );
    assert_eq!(json["error"]["gate"], "locked", "{}", json);
    assert_eq!(json["error"]["state"], "check", "{}", json);
    assert_eq!(json["command"], "overrides record", "{}", json);
    let msg = json["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("locked") && msg.contains("check"),
        "message must name gate and state: {}",
        msg
    );
}

#[test]
fn non_overridable_gate_refuses_every_override_and_leaves_state_untouched() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    init_workflow(d, "wf", TWO_GATES);

    // First tick evaluates both gates and blocks.
    let (code, blocked, stderr) = run(d, &["next", "wf"]);
    assert_eq!(code, 0, "{} {}", blocked, stderr);
    assert_eq!(blocked["action"], "gate_blocked", "{}", blocked);
    let conditions = blocked["blocking_conditions"].as_array().unwrap();
    let actionable = |name: &str| {
        conditions
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no condition {} in {}", name, blocked))["agent_actionable"]
            .clone()
    };
    assert_eq!(actionable("locked"), serde_json::json!(false));
    assert_eq!(actionable("open"), serde_json::json!(true));

    let state_file = state_path(d, "wf");
    let before = std::fs::read(&state_file).unwrap();

    let payload_file = d.join("payload.json");
    std::fs::write(&payload_file, r#"{"exit_code": 0, "error": ""}"#).unwrap();
    let at_file = format!("@{}", payload_file.display());
    let missing_file = format!("@{}", d.join("missing.json").display());

    let attempts: Vec<Vec<&str>> = vec![
        // No --with-data: would fall back to the built-in default.
        vec![],
        // A schema-valid inline payload.
        vec!["--with-data", r#"{"exit_code": 0, "error": ""}"#],
        // A schema-valid payload from a file.
        vec!["--with-data", &at_file],
        // Payloads that would otherwise fail parsing: the refusal wins.
        vec!["--with-data", "not json"],
        vec!["--with-data", &missing_file],
    ];
    for extra in attempts {
        let mut args = vec![
            "overrides",
            "record",
            "wf",
            "--gate",
            "locked",
            "--rationale",
            "try to force it",
        ];
        args.extend(extra.iter().copied());
        let (code, json, stderr) = run(d, &args);
        assert_refused(&json, code, &stderr);
        assert_eq!(
            std::fs::read(&state_file).unwrap(),
            before,
            "state file must be byte-identical after a refused override ({:?})",
            extra
        );
    }

    // The overridable gate in the same state still records as today.
    let (code, json, stderr) = run(
        d,
        &[
            "overrides",
            "record",
            "wf",
            "--gate",
            "open",
            "--rationale",
            "reviewed by hand",
        ],
    );
    assert_eq!(code, 0, "{} {}", json, stderr);
    assert_eq!(json["status"], "recorded", "{}", json);
    let after_open = std::fs::read_to_string(&state_file).unwrap();
    assert!(
        after_open.len() > before.len() && after_open.contains("gate_override_recorded"),
        "the overridable gate's override must be appended"
    );

    // The next tick still evaluates `locked` for real and stays blocked.
    let (code, next, stderr) = run(d, &["next", "wf"]);
    assert_eq!(code, 0, "{} {}", next, stderr);
    assert_eq!(next["action"], "gate_blocked", "{}", next);
    let names: Vec<&str> = next["blocking_conditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["locked"], "only the real gate should block");

    // The override log holds the `open` entry and nothing for `locked`.
    let (code, list, stderr) = run(d, &["overrides", "list", "wf"]);
    assert_eq!(code, 0, "{} {}", list, stderr);
    let gates: Vec<&str> = list["overrides"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["gate"].as_str().unwrap())
        .collect();
    assert_eq!(gates, vec!["open"], "{}", list);
}

#[test]
fn refused_override_on_state_with_accepts_still_requires_evidence() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    init_workflow(d, "acc", GATE_WITH_ACCEPTS);

    let (code, json, stderr) = run(
        d,
        &[
            "overrides",
            "record",
            "acc",
            "--gate",
            "locked",
            "--rationale",
            "force",
            "--with-data",
            r#"{"exit_code": 0, "error": ""}"#,
        ],
    );
    assert_refused(&json, code, &stderr);

    let (code, next, stderr) = run(d, &["next", "acc"]);
    assert_eq!(code, 0, "{} {}", next, stderr);
    assert_eq!(next["action"], "evidence_required", "{}", next);
    assert_eq!(next["state"], "check", "{}", next);

    let (_, list, _) = run(d, &["overrides", "list", "acc"]);
    assert_eq!(list["overrides"]["count"], 0, "{}", list);
}

#[test]
fn misspelled_overridable_key_fails_template_compile() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let src = d.join("typo.md");
    std::fs::write(&src, TWO_GATES.replace("overridable:", "overrideable:")).unwrap();

    let (code, json, stderr) = run(d, &["template", "compile", src.to_str().unwrap()]);
    assert_ne!(code, 0, "a misspelled key must not compile: {}", json);
    let err = json["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("\"check\"") && err.contains("\"locked\"") && err.contains("overrideable"),
        "error must name the state, gate and key: {} (stderr {})",
        err,
        stderr
    );
}

/// Templates that don't declare `overridable` must compile to exactly the
/// JSON they compiled to before the field existed, or every existing
/// session's template hash would stop matching. The snapshots were taken
/// from the compiler before the field was added.
#[test]
fn templates_without_overridable_compile_byte_identical() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        "context-gate",
        "mixed-routing",
        "multi-state",
        "skip-if-gate",
    ] {
        let src = root.join(format!("test/functional/fixtures/templates/{}.md", name));
        let compiled = koto::template::compile::compile(&src, false).unwrap();
        let json = serde_json::to_string_pretty(&compiled).unwrap();
        let expected = std::fs::read_to_string(
            root.join(format!("tests/fixtures/compiled-snapshots/{}.json", name)),
        )
        .unwrap();
        assert_eq!(
            json, expected,
            "compiled {} drifted from its snapshot",
            name
        );
    }
}
