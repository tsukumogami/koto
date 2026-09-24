//! End-to-end checks for variable constraints (`values:`, `pattern:`) at
//! `koto init` and on the batch child spawn path.
//!
//! A value that fails its declared constraint is a caller error: exit 2, a
//! typed `code` beside the usual `error`/`command` fields, and no session left
//! behind.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

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
    cmd
}

struct Run {
    code: i32,
    stdout: String,
    json: serde_json::Value,
}

fn run_koto(dir: &Path, args: &[&str]) -> Run {
    let out = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null);
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout,
        json,
    }
}

const INTENT_TEMPLATE: &str = r#"---
name: intent
version: "1.0"
initial_state: work
variables:
  INTENT_FLAG:
    description: Caller's intent token, or empty
    pattern: ^(continue|stop)?$
  MERGE:
    values: ["true", "false"]
    default: "false"
    rebind: true
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Intent {{INTENT_FLAG}}, merge {{MERGE}}.

## done

Done.
"#;

fn write_template(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

/// The `workflow_initialized` variables of session `name`.
fn initialized_variables(dir: &Path, name: &str) -> serde_json::Value {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name));
    let content = std::fs::read_to_string(path).unwrap();
    content
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|e| e["type"] == "workflow_initialized")
        .expect("workflow_initialized event")["payload"]["variables"]
        .clone()
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn a_value_failing_its_pattern_is_refused_with_no_session() {
    let tmp = TempDir::new().unwrap();
    let template = write_template(tmp.path(), "intent.md", INTENT_TEMPLATE);

    let run = run_koto(
        tmp.path(),
        &[
            "init",
            "s",
            "--template",
            template.to_str().unwrap(),
            "--var",
            "INTENT_FLAG=maybe",
        ],
    );

    assert_eq!(run.code, 2, "stdout: {}", run.stdout);
    assert_eq!(run.json["command"], "init");
    assert_eq!(run.json["code"], "invalid_var");
    assert_eq!(run.json["var"], "INTENT_FLAG");
    assert_eq!(run.json["value"], "maybe");
    assert_eq!(run.json["constraint"], "pattern:^(continue|stop)?$");
    assert!(run.json["error"].as_str().unwrap().contains("INTENT_FLAG"));
    assert!(run.json.get("name").is_none(), "no session is printed");

    // Nothing under HOME names the session: no directory, no state file.
    assert!(!sessions_base(tmp.path()).join("s").exists());
    let leftovers: Vec<PathBuf> = files_under(tmp.path())
        .into_iter()
        .filter(|p| p.to_string_lossy().contains(".state.jsonl"))
        .collect();
    assert!(leftovers.is_empty(), "state files left: {:?}", leftovers);
}

#[test]
fn a_value_satisfying_its_pattern_initializes_the_session() {
    let tmp = TempDir::new().unwrap();
    let template = write_template(tmp.path(), "intent.md", INTENT_TEMPLATE);

    let run = run_koto(
        tmp.path(),
        &[
            "init",
            "s",
            "--template",
            template.to_str().unwrap(),
            "--var",
            "INTENT_FLAG=continue",
        ],
    );
    assert_eq!(run.code, 0, "stdout: {}", run.stdout);
    let vars = initialized_variables(tmp.path(), "s");
    assert_eq!(vars["INTENT_FLAG"], "continue");
    assert_eq!(vars["MERGE"], "false");
}

#[test]
fn a_constrained_variable_not_passed_resolves_to_its_default() {
    let tmp = TempDir::new().unwrap();
    let template = write_template(tmp.path(), "intent.md", INTENT_TEMPLATE);

    let run = run_koto(
        tmp.path(),
        &["init", "s", "--template", template.to_str().unwrap()],
    );
    assert_eq!(run.code, 0, "stdout: {}", run.stdout);
    let vars = initialized_variables(tmp.path(), "s");
    assert_eq!(vars["INTENT_FLAG"], "");
}

#[test]
fn duplicate_and_unknown_keys_carry_typed_codes() {
    let tmp = TempDir::new().unwrap();
    let template = write_template(tmp.path(), "intent.md", INTENT_TEMPLATE);
    let t = template.to_str().unwrap();

    let dup = run_koto(
        tmp.path(),
        &[
            "init",
            "s",
            "--template",
            t,
            "--var",
            "MERGE=true",
            "--var",
            "MERGE=false",
        ],
    );
    assert_eq!(dup.code, 2, "stdout: {}", dup.stdout);
    assert_eq!(dup.json["code"], "duplicate_var");
    assert_eq!(dup.json["var"], "MERGE");
    assert_eq!(dup.json["error"], "duplicate --var key \"MERGE\"");

    let unknown = run_koto(
        tmp.path(),
        &["init", "s", "--template", t, "--var", "NOPE=1"],
    );
    assert_eq!(unknown.code, 2, "stdout: {}", unknown.stdout);
    assert_eq!(unknown.json["code"], "unknown_var");
    assert_eq!(unknown.json["var"], "NOPE");
    assert_eq!(
        unknown.json["error"],
        "unknown variable \"NOPE\": not declared in template"
    );
    assert!(!sessions_base(tmp.path()).join("s").exists());
}

#[test]
fn an_invalid_value_on_the_inline_path_carries_the_same_code() {
    let tmp = TempDir::new().unwrap();
    let out = koto_cmd(tmp.path())
        .args(["init", "s", "--from-stdin", "--var", "MERGE=maybe"])
        .write_stdin(INTENT_TEMPLATE)
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(out.status.code(), Some(2), "json: {}", json);
    assert_eq!(json["code"], "invalid_var");
    assert_eq!(json["var"], "MERGE");
    assert_eq!(json["constraint"], "values:[true,false]");
}

// ---------------------------------------------------------------------------
// Batch child spawn path
// ---------------------------------------------------------------------------

#[cfg(unix)]
const PARENT_TEMPLATE: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: [yes]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize results.
"#;

#[cfg(unix)]
const CHILD_TEMPLATE: &str = r#"---
name: batch-child
version: "1.0"
initial_state: work
variables:
  MODE:
    values: [fast, slow]
    required: true
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work in {{MODE}} mode.

## done

Done.
"#;

#[cfg(unix)]
#[test]
fn a_child_whose_vars_violate_its_values_is_not_spawned() {
    let tmp = TempDir::new().unwrap();
    let parent = write_template(tmp.path(), "parent.md", PARENT_TEMPLATE);
    write_template(tmp.path(), "child.md", CHILD_TEMPLATE);

    let init = run_koto(
        tmp.path(),
        &["init", "parent", "--template", parent.to_str().unwrap()],
    );
    assert_eq!(init.code, 0, "parent init: {}", init.stdout);

    let payload = serde_json::json!({
        "tasks": [{"name": "A", "waits_on": [], "vars": {"MODE": "medium"}}]
    })
    .to_string();
    let run = run_koto(tmp.path(), &["next", "parent", "--with-data", &payload]);
    let sched = &run.json["scheduler"];
    let errored = sched["errored"]
        .as_array()
        .unwrap_or_else(|| panic!("errored list expected: {}", run.stdout));
    let err = errored
        .iter()
        .find(|e| e["task"].as_str().is_some_and(|t| t.ends_with('A')))
        .unwrap_or_else(|| panic!("task A must be errored: {}", run.stdout));
    let message = err["message"].as_str().unwrap();
    assert!(message.contains("MODE"), "message: {}", message);
    assert!(message.contains("medium"), "message: {}", message);

    let spawned = sched["spawned_this_tick"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(spawned.is_empty(), "nothing spawns: {:?}", spawned);
    assert!(!sessions_base(tmp.path()).join("parent.A").exists());
}
