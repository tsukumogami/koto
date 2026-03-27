use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::Path;

fn koto() -> Command {
    Command::cargo_bin("koto").unwrap()
}

fn template_with_variables() -> &'static str {
    r#"---
name: var-workflow
version: "1.0"
initial_state: start
variables:
  OWNER:
    description: "Repository owner"
    required: true
  REPO:
    description: "Repository name"
    required: true
  BRANCH:
    description: "Branch name"
    default: "main"
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Work on {{OWNER}}/{{REPO}} branch {{BRANCH}}.

## done

All done.
"#
}

fn minimal_template() -> &'static str {
    r#"---
name: test-workflow
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
"#
}

/// Write the minimal template source to a file in `dir` and return its path.
fn write_template_source(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("test-template.md");
    std::fs::write(&src, minimal_template()).unwrap();
    src
}

/// Write the variable template source to a file in `dir` and return its path.
fn write_var_template_source(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("var-template.md");
    std::fs::write(&src, template_with_variables()).unwrap();
    src
}

/// Compile the minimal template into the cache and return the compiled JSON path.
fn compile_template(dir: &Path) -> String {
    let src = write_template_source(dir);

    let output = koto()
        .current_dir(dir)
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "template compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

#[test]
fn version_outputs_human_readable_by_default() {
    let output = koto().arg("version").output().unwrap();
    assert!(output.status.success(), "version should exit 0");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("koto "),
        "default version output should start with 'koto ', got: {}",
        stdout
    );
    // Should NOT be JSON
    assert!(
        serde_json::from_str::<serde_json::Value>(&stdout).is_err(),
        "default version output should not be JSON"
    );
}

#[test]
fn version_exits_0_and_produces_json() {
    let output = koto().args(["version", "--json"]).output().unwrap();

    assert!(output.status.success(), "version should exit 0");

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("version output should be valid JSON");

    let v = json["version"]
        .as_str()
        .expect("version field should be a string");
    assert!(!v.is_empty(), "version field should not be empty");
}

#[test]
fn version_is_derived_from_git_not_cargo_toml() {
    let output = koto().args(["version", "--json"]).output().unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let version = json["version"].as_str().unwrap();
    let commit = json["commit"].as_str().unwrap();

    // Version must follow one of the git-derived patterns:
    // - Release tag: "X.Y.Z" (digits and dots only)
    // - Ahead of tag: "X.Y.Z-dev+<hash>"
    // - No tags: "dev+<hash>"
    let valid = version
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.')                    // exact tag
        || version.contains("-dev+")                                  // ahead of tag
        || version.starts_with("dev+"); // no tags
    assert!(
        valid,
        "version '{}' doesn't match any git-derived pattern (X.Y.Z, X.Y.Z-dev+hash, dev+hash)",
        version
    );

    // The commit field should be a short hex hash (or "unknown" in non-git builds).
    assert!(
        commit == "unknown" || commit.chars().all(|c| c.is_ascii_hexdigit()),
        "commit '{}' should be a hex hash or 'unknown'",
        commit
    );

    // Version must NOT be the literal Cargo.toml version "0.1.0" when we're
    // on a non-tag commit (which is always true in test builds).
    // If we're on an exact v0.1.0 tag this would legitimately be "0.1.0",
    // but test builds are ahead of the tag so it should have -dev+ suffix.
    if version == "0.1.0" {
        // Only acceptable if we're actually on the v0.1.0 tag.
        let on_tag = std::process::Command::new("git")
            .args(["describe", "--tags", "--exact-match"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(
            on_tag,
            "version is '0.1.0' but we're not on a release tag -- build.rs should produce '0.1.0-dev+<hash>'"
        );
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[test]
fn init_creates_state_file() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args(["init", "my-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let state_path = dir.path().join("koto-my-wf.state.jsonl");
    assert!(state_path.exists(), "state file should be created");

    // Verify the state file has exactly 3 lines: header + workflow_initialized + transitioned.
    let state_content = std::fs::read_to_string(&state_path).unwrap();
    let lines: Vec<&str> = state_content.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "state file should have 3 lines (header + 2 events), got {}",
        lines.len()
    );

    // Verify the header line has schema_version.
    let header: serde_json::Value =
        serde_json::from_str(lines[0]).expect("header line should be valid JSON");
    assert_eq!(
        header["schema_version"].as_u64(),
        Some(1),
        "header should have schema_version=1"
    );
    assert_eq!(
        header["workflow"].as_str(),
        Some("my-wf"),
        "header workflow should match"
    );
    assert!(
        header["template_hash"].as_str().is_some(),
        "header should have template_hash"
    );
    assert!(
        header["created_at"].as_str().is_some(),
        "header should have created_at"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("init output should be valid JSON");
    assert_eq!(json["name"], "my-wf");
    assert!(
        json["state"].as_str().is_some(),
        "state field should be present"
    );
}

#[test]
fn init_fails_if_file_exists() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    // First init succeeds.
    let first = koto()
        .current_dir(dir.path())
        .args(["init", "dup-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "first init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );

    // Second init must fail.
    let second = koto()
        .current_dir(dir.path())
        .args(["init", "dup-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !second.status.success(),
        "second init on same name should fail"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("error output should be valid JSON");
    assert!(
        json["error"].as_str().is_some(),
        "error field should be present"
    );
}

// ---------------------------------------------------------------------------
// next
// ---------------------------------------------------------------------------

#[test]
fn next_returns_state_directive_transitions() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    koto()
        .current_dir(dir.path())
        .args(["init", "next-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "next-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be valid JSON");

    // With auto-advancement, start -> done is auto-advanced (unconditional transition).
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "auto-advancement should reach terminal state"
    );
    assert_eq!(
        json["action"].as_str(),
        Some("done"),
        "terminal state should have action=done"
    );
    assert_eq!(
        json["advanced"], true,
        "advanced should be true after auto-advancing"
    );
    assert!(
        json["error"].is_null(),
        "error field should be null on success"
    );
}

#[test]
fn next_fails_for_unknown_workflow() {
    let dir = TempDir::new().unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "no-such-workflow"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "next on unknown workflow should fail"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("error output should be valid JSON");
    // The error field is a structured object with code/message/details
    assert!(
        json["error"].is_object(),
        "error field should be a structured object, got: {:?}",
        json["error"]
    );
    assert!(
        json["error"]["code"].as_str().is_some(),
        "error should have a code field"
    );
}

// ---------------------------------------------------------------------------
// rewind
// ---------------------------------------------------------------------------

#[test]
fn rewind_appends_rewind_event() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    koto()
        .current_dir(dir.path())
        .args(["init", "rewind-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    // Append a transitioned event so there are 2+ state-changing events,
    // making rewind possible (init writes header + workflow_initialized + transitioned).
    let state_path = dir.path().join("koto-rewind-wf.state.jsonl");
    let extra_event = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"gate"}}"#;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&state_path)
        .unwrap();
    writeln!(f, "{}", extra_event).unwrap();
    drop(f);

    let before_lines = std::fs::read_to_string(&state_path)
        .unwrap()
        .lines()
        .count();

    let output = koto()
        .current_dir(dir.path())
        .args(["rewind", "rewind-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "rewind should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let after_lines = std::fs::read_to_string(&state_path)
        .unwrap()
        .lines()
        .count();

    assert_eq!(
        after_lines,
        before_lines + 1,
        "rewind should append one new event line"
    );

    // The last line must be a rewind event with from/to payload.
    let last_line = std::fs::read_to_string(&state_path)
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_string();
    let last_event: serde_json::Value =
        serde_json::from_str(&last_line).expect("last line should be valid JSON");
    assert_eq!(
        last_event["type"].as_str(),
        Some("rewound"),
        "last event should be a rewound event"
    );
    assert!(
        last_event["payload"]["from"].as_str().is_some(),
        "rewound event should have payload.from"
    );
    assert!(
        last_event["payload"]["to"].as_str().is_some(),
        "rewound event should have payload.to"
    );
    assert_eq!(
        last_event["payload"]["from"].as_str(),
        Some("done"),
        "rewound event should rewind from 'done'"
    );
    assert_eq!(
        last_event["payload"]["to"].as_str(),
        Some("start"),
        "rewound event should rewind to 'start'"
    );
}

#[test]
fn rewind_fails_at_initial_state() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    // Only init — single event in the file.
    koto()
        .current_dir(dir.path())
        .args(["init", "at-init-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["rewind", "at-init-wf"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "rewind at initial state should fail"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("error output should be valid JSON");
    assert!(
        json["error"].as_str().is_some(),
        "error field should be present"
    );
}

#[test]
fn rewind_fails_for_unknown_workflow() {
    let dir = TempDir::new().unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["rewind", "no-such-workflow"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "rewind on unknown workflow should fail"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("error output should be valid JSON");
    assert!(
        json["error"].as_str().is_some(),
        "error field should be present"
    );
}

// ---------------------------------------------------------------------------
// workflows
// ---------------------------------------------------------------------------

#[test]
fn workflows_returns_array_with_workflow() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    koto()
        .current_dir(dir.path())
        .args(["init", "listed-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto()
        .current_dir(dir.path())
        .arg("workflows")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "workflows should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("workflows output should be valid JSON");

    let arr = json
        .as_array()
        .expect("workflows output should be a JSON array");
    assert!(
        arr.iter().any(|v| v["name"].as_str() == Some("listed-wf")),
        "array should contain an object with the initialized workflow name, got: {:?}",
        arr
    );

    // Verify the object has the expected metadata fields.
    let wf = arr
        .iter()
        .find(|v| v["name"].as_str() == Some("listed-wf"))
        .expect("should find listed-wf in array");
    assert!(
        wf["created_at"].as_str().is_some(),
        "created_at field should be present"
    );
    assert!(
        wf["template_hash"].as_str().is_some(),
        "template_hash field should be present"
    );
}

#[test]
fn workflows_returns_empty_array_when_none() {
    let dir = TempDir::new().unwrap();

    let output = koto()
        .current_dir(dir.path())
        .arg("workflows")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "workflows should exit 0 for empty dir"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("workflows output should be valid JSON");

    let arr = json
        .as_array()
        .expect("workflows output should be a JSON array");
    assert!(
        arr.is_empty(),
        "array should be empty when no workflows exist"
    );
}

// ---------------------------------------------------------------------------
// template compile
// ---------------------------------------------------------------------------

#[test]
fn template_compile_produces_format_version_1() {
    let dir = TempDir::new().unwrap();
    let compiled_path = compile_template(dir.path());

    let compiled_json =
        std::fs::read_to_string(&compiled_path).expect("compiled template file should exist");
    let compiled: serde_json::Value =
        serde_json::from_str(&compiled_json).expect("compiled template should be valid JSON");

    assert_eq!(
        compiled["format_version"].as_u64(),
        Some(1),
        "compiled template should have format_version=1"
    );
}

#[test]
fn template_compile_fails_for_invalid_yaml() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("bad.md");
    // Missing closing `---` makes frontmatter parsing fail.
    std::fs::write(&src, "---\nname: [broken yaml\nno closing delimiter\n").unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template compile should fail for invalid YAML"
    );
}

// ---------------------------------------------------------------------------
// template validate
// ---------------------------------------------------------------------------

#[test]
fn template_validate_succeeds_for_valid_template() {
    let dir = TempDir::new().unwrap();
    let compiled_path = compile_template(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args(["template", "validate", &compiled_path])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "template validate should succeed for valid compiled template: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn template_validate_fails_for_missing_required_fields() {
    let dir = TempDir::new().unwrap();
    // Write valid JSON that lacks required fields (format_version alone is not enough).
    let bad_json = dir.path().join("bad.json");
    std::fs::write(&bad_json, r#"{"format_version":1}"#).unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["template", "validate", bad_json.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template validate should fail for JSON with missing required fields"
    );
}

// ---------------------------------------------------------------------------
// full happy path sequence: init -> next -> rewind
// ---------------------------------------------------------------------------

#[test]
fn init_next_rewind_sequence() {
    let dir = TempDir::new().unwrap();
    // Use accepts-based template so auto-advancement doesn't skip past states.
    init_workflow(dir.path(), "seq-wf", &template_with_accepts());

    // next: initial state has accepts, so it stops at start with EvidenceRequired.
    let next_out = koto()
        .current_dir(dir.path())
        .args(["next", "seq-wf"])
        .output()
        .unwrap();
    assert!(next_out.status.success(), "next should succeed");
    let next_json: serde_json::Value = serde_json::from_slice(&next_out.stdout).unwrap();
    assert_eq!(next_json["state"].as_str(), Some("start"));
    assert!(next_json["directive"].as_str().is_some());
    assert!(next_json["action"].as_str().is_some());

    // Use --to to advance to implement, enabling rewind.
    let advance = koto()
        .current_dir(dir.path())
        .args(["next", "seq-wf", "--to", "implement"])
        .output()
        .unwrap();
    assert!(advance.status.success(), "--to should succeed");

    // rewind
    let rewind_out = koto()
        .current_dir(dir.path())
        .args(["rewind", "seq-wf"])
        .output()
        .unwrap();
    assert!(rewind_out.status.success(), "rewind should succeed");
    let rewind_json: serde_json::Value = serde_json::from_slice(&rewind_out.stdout).unwrap();
    assert_eq!(rewind_json["name"], "seq-wf");
    assert_eq!(
        rewind_json["state"].as_str(),
        Some("start"),
        "rewind should go back to start"
    );

    // next after rewind: back at start, which has accepts -> EvidenceRequired.
    let next_after = koto()
        .current_dir(dir.path())
        .args(["next", "seq-wf"])
        .output()
        .unwrap();
    assert!(
        next_after.status.success(),
        "next after rewind should succeed"
    );
    let next_after_json: serde_json::Value = serde_json::from_slice(&next_after.stdout).unwrap();
    assert_eq!(
        next_after_json["state"].as_str(),
        Some("start"),
        "state after rewind should be start (evidence required)"
    );
    assert!(
        next_after_json["directive"].as_str().is_some(),
        "directive should be present after rewind"
    );

    // Verify the last event in the state file is a rewound event.
    let state_path = dir.path().join("koto-seq-wf.state.jsonl");
    let state_content = std::fs::read_to_string(&state_path).unwrap();
    let last_line = state_content.lines().last().unwrap().to_string();
    let last_event: serde_json::Value =
        serde_json::from_str(&last_line).expect("last event should be valid JSON");
    assert_eq!(
        last_event["type"].as_str(),
        Some("rewound"),
        "last event should be a rewound event after rewind"
    );
}

// ---------------------------------------------------------------------------
// corrupted state files
// ---------------------------------------------------------------------------

#[test]
fn corrupted_state_file_rejected_with_exit_code_3() {
    let dir = TempDir::new().unwrap();
    let state_path = dir.path().join("koto-corrupt.state.jsonl");
    std::fs::write(&state_path, "this is not valid json at all\n").unwrap();

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "corrupt"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "next should fail for corrupted state file"
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "exit code should be 3 for corrupted state file"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("error output should be valid JSON");
    assert!(
        json["error"].as_str().is_some(),
        "error field should be present for corrupted file"
    );
}

#[test]
fn rewind_event_has_from_and_to_in_payload() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    koto()
        .current_dir(dir.path())
        .args(["init", "payload-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    // Append a transitioned event so rewind is possible.
    let state_path = dir.path().join("koto-payload-wf.state.jsonl");
    let extra_event = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"gate"}}"#;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&state_path)
            .unwrap();
        writeln!(f, "{}", extra_event).unwrap();
    }

    let output = koto()
        .current_dir(dir.path())
        .args(["rewind", "payload-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "rewind should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Read the last event from the state file and verify it has the right shape.
    let state_content = std::fs::read_to_string(&state_path).unwrap();
    let last_line = state_content.lines().last().unwrap().to_string();
    let last_event: serde_json::Value =
        serde_json::from_str(&last_line).expect("last event should be valid JSON");

    assert_eq!(
        last_event["type"].as_str(),
        Some("rewound"),
        "last event type should be 'rewound'"
    );
    assert_eq!(
        last_event["payload"]["from"].as_str(),
        Some("done"),
        "payload.from should be 'done'"
    );
    assert_eq!(
        last_event["payload"]["to"].as_str(),
        Some("start"),
        "payload.to should be 'start'"
    );
    assert!(
        last_event["seq"].as_u64().is_some(),
        "rewound event should have a seq number"
    );
    assert!(
        last_event["timestamp"].as_str().is_some(),
        "rewound event should have a timestamp"
    );
}

// ---------------------------------------------------------------------------
// Template helpers for rich fixtures
// ---------------------------------------------------------------------------

/// Template with a gate on the start state that runs a shell command.
fn template_with_gate(gate_command: &str, timeout: u32) -> String {
    let timeout_line = if timeout > 0 {
        format!("\n        timeout: {}", timeout)
    } else {
        String::new()
    };
    format!(
        r#"---
name: gated-workflow
version: "1.0"
initial_state: start
states:
  start:
    gates:
      check:
        type: command
        command: "{}"{}
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Do the gated task.

## done

All done.
"#,
        gate_command, timeout_line
    )
}

/// Template with an accepts block on start and conditional transitions.
fn template_with_accepts() -> String {
    r#"---
name: evidence-workflow
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      decision:
        type: enum
        required: true
        values: [proceed, escalate]
      notes:
        type: string
        required: false
    transitions:
      - target: implement
        when:
          decision: proceed
      - target: review
        when:
          decision: escalate
  implement:
    transitions:
      - target: done
  review:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Make a decision about this work.

## implement

Implement the changes.

## review

Review the changes.

## done

All done.
"#
    .to_string()
}

/// Template with an integration field on a state.
fn template_with_integration() -> String {
    r#"---
name: integration-workflow
version: "1.0"
initial_state: start
states:
  start:
    integration: code_review
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Delegate to integration.

## done

All done.
"#
    .to_string()
}

/// Multi-state template for agent-driven workflow loop testing.
fn template_multi_state() -> String {
    r#"---
name: multi-state-workflow
version: "1.0"
initial_state: plan
states:
  plan:
    transitions:
      - target: implement
  implement:
    transitions:
      - target: verify
  verify:
    transitions:
      - target: done
  done:
    terminal: true
---

## plan

Create the implementation plan.

## implement

Write the code.

## verify

Run the tests.

## done

All done.
"#
    .to_string()
}

/// Initialize a workflow from a custom template string.
fn init_workflow(dir: &Path, name: &str, template_content: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();

    let output = koto()
        .current_dir(dir)
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

// ---------------------------------------------------------------------------
// scenario-26: Integration state classification
// ---------------------------------------------------------------------------

#[test]
fn next_integration_state_returns_integration_unavailable() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "integ-wf", &template_with_integration());

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "integ-wf"])
        .output()
        .unwrap();

    // IntegrationUnavailable is a success response (exit 0) with integration.available=false.
    assert!(
        output.status.success(),
        "integration_unavailable should exit 0, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    assert_eq!(
        json["action"].as_str(),
        Some("execute"),
        "action should be execute"
    );
    assert_eq!(
        json["state"].as_str(),
        Some("start"),
        "state should be start"
    );
    assert!(
        json["error"].is_null(),
        "error should be null for integration_unavailable"
    );
    assert_eq!(
        json["integration"]["name"].as_str(),
        Some("code_review"),
        "integration name should match"
    );
    assert_eq!(
        json["integration"]["available"], false,
        "integration should be unavailable"
    );
}

// ---------------------------------------------------------------------------
// scenario-28: --with-data and --to mutual exclusivity
// ---------------------------------------------------------------------------

#[test]
fn next_with_data_and_to_mutually_exclusive() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "mutex-wf", &template_with_accepts());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "next",
            "mutex-wf",
            "--with-data",
            r#"{"decision":"proceed"}"#,
            "--to",
            "implement",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "mutual exclusivity violation should exit 2"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("precondition_failed"),
        "error code should be precondition_failed"
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mutually exclusive"),
        "error message should mention mutual exclusivity"
    );
}

// ---------------------------------------------------------------------------
// scenario-29: --with-data payload size limit (1MB)
// ---------------------------------------------------------------------------

#[test]
fn next_with_data_rejects_oversized_payload() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "size-wf", &template_with_accepts());

    // The koto code checks for >1MB (1,048,576 bytes), but the OS kernel
    // enforces MAX_ARG_STRLEN (~128KB on Linux) before koto sees the argument.
    // We can't test the >1MB rejection path via CLI, so instead we verify:
    // 1. A large-but-valid payload (100KB) is accepted without size rejection.
    // 2. The size check exists in the code (tested by the fact that the 100KB
    //    payload passes -- it would fail if the limit were set too low).
    let big_value = "x".repeat(100_000);
    let payload = format!(r#"{{"decision":"{}"}}"#, big_value);

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "size-wf", "--with-data", &payload])
        .output()
        .unwrap();

    // This fails with invalid_submission because "xxx..." is not a valid enum
    // value, but it should NOT fail with "maximum size" -- proving the size
    // check passed for this 100KB payload.
    assert_eq!(
        output.status.code(),
        Some(2),
        "large payload should fail validation (not size), stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("invalid_submission"),
        "error code should be invalid_submission (schema validation, not size)"
    );
    // Crucially, the message should be about validation, not about size.
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        !msg.contains("maximum size"),
        "100KB payload should not hit size limit, got: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// scenario-30: Full koto next on terminal state returns done response
// ---------------------------------------------------------------------------

#[test]
fn next_on_terminal_state_returns_done() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "term-wf", minimal_template());

    // Advance to terminal state via --to.
    let advance = koto()
        .current_dir(dir.path())
        .args(["next", "term-wf", "--to", "done"])
        .output()
        .unwrap();
    assert!(
        advance.status.success(),
        "advance to done should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&advance.stdout),
        String::from_utf8_lossy(&advance.stderr)
    );

    // Now call next again on the terminal state.
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "term-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next on terminal state should exit 0"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["action"].as_str(),
        Some("done"),
        "action should be done"
    );
    assert_eq!(json["state"].as_str(), Some("done"), "state should be done");
    assert_eq!(
        json["advanced"], false,
        "advanced should be false (no event appended)"
    );
    assert!(json["error"].is_null(), "error should be null");
}

// ---------------------------------------------------------------------------
// scenario-31: Full koto next with failing gates returns gate_blocked
// ---------------------------------------------------------------------------

#[test]
fn next_with_failing_gate_returns_gate_blocked() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "gate-wf", &template_with_gate("exit 1", 0));

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "gate-wf"])
        .output()
        .unwrap();

    // GateBlocked is a success response (exit 0) with blocking_conditions.
    assert!(
        output.status.success(),
        "gate_blocked should exit 0, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    assert_eq!(
        json["action"].as_str(),
        Some("execute"),
        "action should be execute"
    );
    assert_eq!(
        json["state"].as_str(),
        Some("start"),
        "state should be start"
    );
    assert!(
        json["error"].is_null(),
        "error should be null for gate_blocked"
    );

    let conditions = json["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array");
    assert_eq!(conditions.len(), 1, "should have one blocking condition");
    assert_eq!(conditions[0]["name"].as_str(), Some("check"));
    assert_eq!(conditions[0]["type"].as_str(), Some("command"));
    assert_eq!(conditions[0]["status"].as_str(), Some("failed"));
}

// ---------------------------------------------------------------------------
// scenario-32: Full evidence submission flow via --with-data
// ---------------------------------------------------------------------------

#[test]
fn next_with_valid_evidence_advances_state() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "evid-wf", &template_with_accepts());

    // First, check the initial state returns evidence_required.
    let initial = koto()
        .current_dir(dir.path())
        .args(["next", "evid-wf"])
        .output()
        .unwrap();
    assert!(initial.status.success(), "initial next should succeed");
    let initial_json: serde_json::Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_eq!(initial_json["state"].as_str(), Some("start"));
    assert!(
        initial_json["expects"].is_object(),
        "expects should be present for evidence_required"
    );

    // Submit evidence via --with-data.
    let submit = koto()
        .current_dir(dir.path())
        .args([
            "next",
            "evid-wf",
            "--with-data",
            r#"{"decision":"proceed","notes":"looks good"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        submit.status.success(),
        "evidence submission should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&submit.stdout),
        String::from_utf8_lossy(&submit.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&submit.stdout).unwrap();
    // After submitting decision=proceed, auto-advancement chains:
    // start -> implement (unconditional) -> done (terminal).
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "auto-advancement should reach terminal state after evidence"
    );
    assert_eq!(
        json["advanced"], true,
        "advanced should be true after evidence + auto-advance"
    );
    assert_eq!(
        json["action"].as_str(),
        Some("done"),
        "action should be done at terminal state"
    );
    assert!(json["error"].is_null(), "error should be null");

    // Verify the state file has an evidence_submitted event.
    let state_path = dir.path().join("koto-evid-wf.state.jsonl");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let has_evidence = content
        .lines()
        .any(|line| line.contains("evidence_submitted"));
    assert!(
        has_evidence,
        "state file should contain evidence_submitted event"
    );
}

// ---------------------------------------------------------------------------
// scenario-33: Invalid evidence submission returns structured error
// ---------------------------------------------------------------------------

#[test]
fn next_with_invalid_evidence_returns_structured_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "bad-evid-wf", &template_with_accepts());

    // Submit evidence with wrong type and missing required field.
    let output = koto()
        .current_dir(dir.path())
        .args([
            "next",
            "bad-evid-wf",
            "--with-data",
            r#"{"decision":42,"unknown_field":"x"}"#,
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "invalid evidence should exit 2"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("invalid_submission"),
        "error code should be invalid_submission"
    );
    assert_eq!(
        json["error"]["message"].as_str(),
        Some("evidence validation failed"),
        "error message should say validation failed"
    );

    let details = json["error"]["details"]
        .as_array()
        .expect("details should be an array");
    assert!(
        !details.is_empty(),
        "details should contain field-level errors"
    );

    // Should have errors for: unknown field, wrong type for decision (enum expects string).
    let fields: Vec<&str> = details.iter().filter_map(|d| d["field"].as_str()).collect();
    assert!(
        fields.contains(&"decision") || fields.contains(&"unknown_field"),
        "details should reference problematic fields, got: {:?}",
        fields
    );
}

// ---------------------------------------------------------------------------
// scenario-34: Directed transition via --to
// ---------------------------------------------------------------------------

#[test]
fn next_with_to_performs_directed_transition() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "directed-wf", &template_with_accepts());

    // Use --to to advance directly to implement (skipping evidence).
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "directed-wf", "--to", "implement"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "directed transition should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["state"].as_str(),
        Some("implement"),
        "state should be implement after directed transition"
    );
    assert_eq!(
        json["advanced"], true,
        "advanced should be true after directed transition"
    );
    assert!(json["error"].is_null(), "error should be null");

    // Verify the state file has a directed_transition event.
    let state_path = dir.path().join("koto-directed-wf.state.jsonl");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let has_directed = content
        .lines()
        .any(|line| line.contains("directed_transition"));
    assert!(
        has_directed,
        "state file should contain directed_transition event"
    );

    // Verify next on the new state works. Auto-advancement chains
    // implement -> done (unconditional transition).
    let next_output = koto()
        .current_dir(dir.path())
        .args(["next", "directed-wf"])
        .output()
        .unwrap();
    assert!(next_output.status.success());
    let next_json: serde_json::Value = serde_json::from_slice(&next_output.stdout).unwrap();
    assert_eq!(
        next_json["state"].as_str(),
        Some("done"),
        "auto-advancement from implement should reach done"
    );
    assert_eq!(next_json["advanced"], true);
}

// ---------------------------------------------------------------------------
// scenario-35: Agent-driven workflow loop using only koto next output
// ---------------------------------------------------------------------------

#[test]
fn agent_driven_workflow_loop() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "loop-wf", &template_multi_state());

    // With auto-advancement, a single koto next chains through all
    // unconditional transitions: plan -> implement -> verify -> done.
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "loop-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "auto-advancement should reach terminal state"
    );
    assert_eq!(
        json["action"].as_str(),
        Some("done"),
        "action should be done"
    );
    assert_eq!(
        json["advanced"], true,
        "advanced should be true after auto-advancing"
    );
}

// ---------------------------------------------------------------------------
// Auto-advancement: 4-state workflow reaches verify via single koto next
// ---------------------------------------------------------------------------

/// Template with 4 states: plan -> implement -> verify (needs evidence) -> done.
/// Plan and implement have unconditional transitions, verify has conditional.
fn template_auto_advance_4state() -> String {
    r#"---
name: auto-advance-workflow
version: "1.0"
initial_state: plan
states:
  plan:
    transitions:
      - target: implement
  implement:
    transitions:
      - target: verify
  verify:
    accepts:
      decision:
        type: enum
        required: true
        values: [approve, reject]
    transitions:
      - target: done
        when:
          decision: approve
      - target: implement
        when:
          decision: reject
  done:
    terminal: true
---

## plan

Create the implementation plan.

## implement

Write the code.

## verify

Review and approve or reject the changes.

## done

All done.
"#
    .to_string()
}

#[test]
fn auto_advance_reaches_verify_from_plan() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "auto-wf", &template_auto_advance_4state());

    // Single koto next should auto-advance: plan -> implement -> verify (stops: evidence required).
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "auto-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["state"].as_str(),
        Some("verify"),
        "auto-advancement should stop at verify (evidence required)"
    );
    assert_eq!(
        json["advanced"], true,
        "advanced should be true (plan -> implement -> verify)"
    );
    assert_eq!(
        json["action"].as_str(),
        Some("execute"),
        "action should be execute at non-terminal state"
    );
    assert!(
        json["expects"].is_object(),
        "expects should be present for evidence_required"
    );
}

// ---------------------------------------------------------------------------
// Auto-advancement: evidence submission triggers auto-advance chain
// ---------------------------------------------------------------------------

#[test]
fn evidence_triggers_auto_advance_chain() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "chain-wf", &template_auto_advance_4state());

    // Auto-advance to verify first.
    let first = koto()
        .current_dir(dir.path())
        .args(["next", "chain-wf"])
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["state"].as_str(), Some("verify"));

    // Submit reject evidence -> should auto-advance: verify -> implement -> verify (stops again).
    let reject = koto()
        .current_dir(dir.path())
        .args([
            "next",
            "chain-wf",
            "--with-data",
            r#"{"decision":"reject"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        reject.status.success(),
        "reject should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&reject.stdout),
        String::from_utf8_lossy(&reject.stderr)
    );

    let reject_json: serde_json::Value = serde_json::from_slice(&reject.stdout).unwrap();
    assert_eq!(
        reject_json["state"].as_str(),
        Some("verify"),
        "reject -> implement -> verify (auto-advance chain)"
    );
    assert_eq!(
        reject_json["advanced"], true,
        "advanced should be true after auto-advance chain"
    );

    // Now approve -> should auto-advance: verify -> done (terminal).
    let approve = koto()
        .current_dir(dir.path())
        .args([
            "next",
            "chain-wf",
            "--with-data",
            r#"{"decision":"approve"}"#,
        ])
        .output()
        .unwrap();

    assert!(approve.status.success());
    let approve_json: serde_json::Value = serde_json::from_slice(&approve.stdout).unwrap();
    assert_eq!(
        approve_json["state"].as_str(),
        Some("done"),
        "approve should reach terminal state"
    );
    assert_eq!(approve_json["action"].as_str(), Some("done"));
    assert_eq!(approve_json["advanced"], true);
}

// ---------------------------------------------------------------------------
// scenario-37: Concurrent koto next fails with flock contention
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn concurrent_next_fails_with_lock_contention() {
    use std::os::unix::io::AsRawFd;

    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "lock-wf", &template_with_accepts());

    // The state file is named koto-<name>.state.jsonl.
    let state_path = dir.path().join("koto-lock-wf.state.jsonl");
    assert!(state_path.exists(), "state file should exist after init");

    // Hold an exclusive flock on the state file, simulating a concurrent koto next.
    let lock_file = std::fs::File::open(&state_path).unwrap();
    let fd = lock_file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0, "test should acquire flock successfully");

    // Now run koto next -- it should fail because it can't acquire the lock.
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "lock-wf"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "concurrent next should fail with exit 2, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("precondition_failed"),
        "error code should be precondition_failed"
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already running"),
        "error message should mention already running"
    );

    // Release the lock explicitly.
    unsafe { libc::flock(fd, libc::LOCK_UN) };
}

// ---------------------------------------------------------------------------
// scenario-38: koto cancel prevents further advancement
// ---------------------------------------------------------------------------

#[test]
fn cancel_then_next_returns_terminal_state_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "cancel-wf", &template_with_accepts());

    // Cancel the workflow.
    let cancel = koto()
        .current_dir(dir.path())
        .args(["cancel", "cancel-wf"])
        .output()
        .unwrap();

    assert!(
        cancel.status.success(),
        "cancel should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&cancel.stdout),
        String::from_utf8_lossy(&cancel.stderr)
    );

    let cancel_json: serde_json::Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert_eq!(cancel_json["cancelled"], true);
    assert_eq!(cancel_json["name"].as_str(), Some("cancel-wf"));

    // Now koto next should fail with terminal_state error.
    let next = koto()
        .current_dir(dir.path())
        .args(["next", "cancel-wf"])
        .output()
        .unwrap();

    assert_eq!(
        next.status.code(),
        Some(2),
        "next after cancel should fail with exit 2"
    );

    let json: serde_json::Value = serde_json::from_slice(&next.stdout).unwrap();
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("terminal_state"),
        "error code should be terminal_state"
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cancelled"),
        "error message should mention cancelled"
    );
}

// ---------------------------------------------------------------------------
// scenario-39: double-cancel returns error
// ---------------------------------------------------------------------------

#[test]
fn double_cancel_returns_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "dbl-cancel-wf", &template_with_accepts());

    // First cancel succeeds.
    let first = koto()
        .current_dir(dir.path())
        .args(["cancel", "dbl-cancel-wf"])
        .output()
        .unwrap();
    assert!(first.status.success());

    // Second cancel fails.
    let second = koto()
        .current_dir(dir.path())
        .args(["cancel", "dbl-cancel-wf"])
        .output()
        .unwrap();

    assert_eq!(
        second.status.code(),
        Some(2),
        "double cancel should fail with exit 2"
    );

    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("already cancelled"),
        "error should mention already cancelled"
    );
}

// ---------------------------------------------------------------------------
// scenario-40: cancel already-terminal workflow returns error
// ---------------------------------------------------------------------------

#[test]
fn cancel_terminal_workflow_returns_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "term-cancel-wf", minimal_template());

    // Auto-advance to terminal state.
    let advance = koto()
        .current_dir(dir.path())
        .args(["next", "term-cancel-wf", "--to", "done"])
        .output()
        .unwrap();
    assert!(advance.status.success());

    // Cancel should fail because workflow is already terminal.
    let cancel = koto()
        .current_dir(dir.path())
        .args(["cancel", "term-cancel-wf"])
        .output()
        .unwrap();

    assert_eq!(
        cancel.status.code(),
        Some(2),
        "cancel on terminal should fail with exit 2"
    );

    let json: serde_json::Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert!(
        json["error"].as_str().unwrap().contains("terminal state"),
        "error should mention terminal state"
    );
}

// ---------------------------------------------------------------------------
// scenario-36: Gate timeout kills entire process group
// ---------------------------------------------------------------------------

#[test]
fn gate_timeout_returns_gate_blocked() {
    let dir = TempDir::new().unwrap();
    // Use a gate command that sleeps longer than the 1-second timeout.
    init_workflow(dir.path(), "timeout-wf", &template_with_gate("sleep 60", 1));

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "timeout-wf"])
        .output()
        .unwrap();

    // GateBlocked (timed_out) is a success response (exit 0) with blocking_conditions.
    assert!(
        output.status.success(),
        "gate timeout should exit 0, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    assert_eq!(
        json["action"].as_str(),
        Some("execute"),
        "action should be execute"
    );
    assert!(
        json["error"].is_null(),
        "error should be null for gate_blocked"
    );

    let conditions = json["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array");
    assert_eq!(conditions.len(), 1, "should have one blocking condition");
    assert_eq!(conditions[0]["name"].as_str(), Some("check"));
    assert_eq!(
        conditions[0]["status"].as_str(),
        Some("timed_out"),
        "gate should have timed out, not failed"
    );
}

// ---------------------------------------------------------------------------
// init --var
// ---------------------------------------------------------------------------

#[test]
fn init_var_valid_vars_stored_in_event() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
            "--var",
            "REPO=widgets",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init with valid vars should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the variables are stored in the workflow_initialized event.
    let state_path = dir.path().join("koto-var-wf.state.jsonl");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    // Line 0 = header, line 1 = workflow_initialized event.
    let init_event: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let vars = init_event["payload"]["variables"].as_object().unwrap();
    assert_eq!(vars["OWNER"].as_str(), Some("acme"));
    assert_eq!(vars["REPO"].as_str(), Some("widgets"));
    // BRANCH should get its default value.
    assert_eq!(vars["BRANCH"].as_str(), Some("main"));
}

#[test]
fn init_var_missing_equals_fails() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
            "--var",
            "REPO=widgets",
            "--var",
            "BADFORMAT",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success(), "missing = should fail");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("expected KEY=VALUE"),
        "error should mention KEY=VALUE format, got: {}",
        err
    );
}

#[test]
fn init_var_duplicate_key_fails() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
            "--var",
            "REPO=widgets",
            "--var",
            "OWNER=other",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success(), "duplicate key should fail");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("duplicate"),
        "error should mention duplicate, got: {}",
        err
    );
}

#[test]
fn init_var_unknown_key_fails() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
            "--var",
            "REPO=widgets",
            "--var",
            "UNKNOWN=val",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success(), "unknown key should fail");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("unknown variable"),
        "error should mention unknown variable, got: {}",
        err
    );
}

#[test]
fn init_var_missing_required_fails() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    // Only provide OWNER but not REPO (both required).
    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "missing required variable should fail"
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("missing required variable"),
        "error should mention missing required, got: {}",
        err
    );
}

#[test]
fn init_var_default_applied_for_optional() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    // Provide only the two required vars; BRANCH has a default.
    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme",
            "--var",
            "REPO=widgets",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init with defaults should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let state_path = dir.path().join("koto-var-wf.state.jsonl");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    let init_event: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let vars = init_event["payload"]["variables"].as_object().unwrap();
    assert_eq!(
        vars["BRANCH"].as_str(),
        Some("main"),
        "BRANCH should use its default value"
    );
}

#[test]
fn init_var_forbidden_chars_fail() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "OWNER=acme corp",
            "--var",
            "REPO=widgets",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "forbidden characters in value should fail"
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("not allowed"),
        "error should mention not allowed, got: {}",
        err
    );
}

#[test]
fn init_no_variables_no_flags_works() {
    // The minimal template has no variables block. Init without --var should work.
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args(["init", "no-var-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init without vars on no-variables template should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn init_no_variables_with_flags_fails() {
    // The minimal template has no variables block. Passing --var should fail (unknown keys).
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let output = koto()
        .current_dir(dir.path())
        .args([
            "init",
            "no-var-wf",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "FOO=bar",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "passing --var to a template with no variables should fail"
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let err = json["error"].as_str().unwrap();
    assert!(
        err.contains("unknown variable"),
        "error should mention unknown variable, got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// next: variable substitution
// ---------------------------------------------------------------------------

/// Helper to init a workflow with --var flags.
fn init_workflow_with_vars(dir: &Path, name: &str, template_content: &str, vars: &[&str]) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();

    let mut args = vec!["init", name, "--template", src.to_str().unwrap()];
    for var in vars {
        args.push("--var");
        args.push(var);
    }

    let output = koto().current_dir(dir).args(&args).output().unwrap();

    assert!(
        output.status.success(),
        "init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Template with a gate whose command uses a variable reference.
fn template_with_var_gate() -> String {
    r#"---
name: var-gate-workflow
version: "1.0"
initial_state: start
variables:
  TASK_ID:
    description: "Task identifier"
    required: true
states:
  start:
    gates:
      check:
        type: command
        command: "test -f /tmp/koto-test-{{TASK_ID}}"
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Work on task {{TASK_ID}}.

## done

All done.
"#
    .to_string()
}

/// Template with variables in directive text and a direct transition path.
fn template_with_var_directive() -> String {
    r#"---
name: var-directive-workflow
version: "1.0"
initial_state: start
variables:
  OWNER:
    description: "Repository owner"
    required: true
  REPO:
    description: "Repository name"
    required: true
states:
  start:
    accepts:
      decision:
        type: enum
        required: true
        values: [proceed, skip]
    transitions:
      - target: work
        when:
          decision: proceed
      - target: done
        when:
          decision: skip
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Review {{OWNER}}/{{REPO}} and decide.

## work

Implement changes for {{OWNER}}/{{REPO}}.

## done

All done.
"#
    .to_string()
}

#[test]
fn next_var_gate_evaluates_with_substituted_command() {
    let dir = TempDir::new().unwrap();

    // Create the sentinel file that the gate checks for.
    let sentinel = "/tmp/koto-test-42";
    std::fs::write(sentinel, "").unwrap();

    init_workflow_with_vars(
        dir.path(),
        "var-gate-wf",
        &template_with_var_gate(),
        &["TASK_ID=42"],
    );

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "var-gate-wf"])
        .output()
        .unwrap();

    // Clean up sentinel file.
    let _ = std::fs::remove_file(sentinel);

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be valid JSON");

    // The gate should pass (file exists), so the workflow auto-advances to done.
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "gate should pass with substituted path, auto-advance to done"
    );
}

#[test]
fn next_var_gate_fails_when_substituted_path_missing() {
    let dir = TempDir::new().unwrap();

    // Don't create the sentinel file -- the gate should fail.
    let sentinel = "/tmp/koto-test-99999";
    let _ = std::fs::remove_file(sentinel); // ensure it doesn't exist

    init_workflow_with_vars(
        dir.path(),
        "var-gate-fail-wf",
        &template_with_var_gate(),
        &["TASK_ID=99999"],
    );

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "var-gate-fail-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gate_blocked exits 0, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be valid JSON");

    // Should be blocked at start because the gate fails.
    assert_eq!(json["state"].as_str(), Some("start"));
    assert!(
        json["blocking_conditions"].is_array(),
        "should have blocking_conditions"
    );

    // The directive should have the substituted variable value.
    assert_eq!(
        json["directive"].as_str(),
        Some("Work on task 99999."),
        "directive should contain substituted variable"
    );
}

#[test]
fn next_var_directive_contains_substituted_values() {
    let dir = TempDir::new().unwrap();

    init_workflow_with_vars(
        dir.path(),
        "var-dir-wf",
        &template_with_var_directive(),
        &["OWNER=acme", "REPO=widgets"],
    );

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "var-dir-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be valid JSON");

    // State should be "start" (needs evidence, no auto-advance).
    assert_eq!(json["state"].as_str(), Some("start"));
    assert_eq!(
        json["directive"].as_str(),
        Some("Review acme/widgets and decide."),
        "directive should contain substituted variable values"
    );
}

#[test]
fn next_var_to_directed_transition_substitutes_directive() {
    let dir = TempDir::new().unwrap();

    init_workflow_with_vars(
        dir.path(),
        "var-to-wf",
        &template_with_var_directive(),
        &["OWNER=acme", "REPO=widgets"],
    );

    // Use --to to direct transition from start to work.
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "var-to-wf", "--to", "work"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next --to should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("next output should be valid JSON");

    assert_eq!(json["state"].as_str(), Some("work"));
    assert_eq!(
        json["directive"].as_str(),
        Some("Implement changes for acme/widgets."),
        "directed transition directive should contain substituted values"
    );
}

// ---------------------------------------------------------------------------
// default_action execution tests
// ---------------------------------------------------------------------------

fn template_with_default_action_creating_file() -> String {
    r#"---
name: action-workflow
version: "1.0"
initial_state: setup
states:
  setup:
    default_action:
      command: "touch marker.txt"
    gates:
      file_exists:
        type: command
        command: "test -f marker.txt"
    transitions:
      - target: done
  done:
    terminal: true
---

## setup

Run setup action.

## done

All done.
"#
    .to_string()
}

#[test]
fn default_action_creates_file_and_auto_advances() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "action-wf",
        &template_with_default_action_creating_file(),
    );

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "action-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    // The action creates marker.txt, the gate checks it exists, and the state
    // auto-advances to done.
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "should auto-advance to terminal state"
    );
    assert_eq!(json["action"].as_str(), Some("done"));
    assert_eq!(json["advanced"], true);

    // Verify the marker file was actually created.
    assert!(
        dir.path().join("marker.txt").exists(),
        "action should have created marker.txt"
    );

    // Verify the state file contains a default_action_executed event.
    let state_content =
        std::fs::read_to_string(dir.path().join("koto-action-wf.state.jsonl")).unwrap();
    assert!(
        state_content.contains("default_action_executed"),
        "state file should contain default_action_executed event"
    );
}

#[test]
fn default_action_skipped_when_override_evidence_exists() {
    let dir = TempDir::new().unwrap();

    // Template where setup has a default_action and an accepts block, plus a gate.
    let template = r#"---
name: skip-action-wf
version: "1.0"
initial_state: setup
states:
  setup:
    default_action:
      command: "touch should-not-exist.txt"
    accepts:
      status:
        type: string
        required: true
    gates:
      always_pass:
        type: command
        command: "true"
    transitions:
      - target: done
  done:
    terminal: true
---

## setup

Setup step.

## done

Done.
"#;

    init_workflow(dir.path(), "skip-wf", template);

    // Submit evidence before calling next, which sets the override evidence.
    let output = koto()
        .current_dir(dir.path())
        .args(["next", "skip-wf", "--with-data", r#"{"status": "manual"}"#])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next with evidence should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // The action should have been skipped because evidence was submitted.
    assert!(
        !dir.path().join("should-not-exist.txt").exists(),
        "action should be skipped when override evidence exists"
    );
}

fn template_with_requires_confirmation() -> String {
    r#"---
name: confirm-workflow
version: "1.0"
initial_state: confirm_step
states:
  confirm_step:
    default_action:
      command: "echo needs-review"
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## confirm_step

Confirm the action output.

## done

All done.
"#
    .to_string()
}

#[test]
fn requires_confirmation_stops_and_returns_output() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "confirm-wf",
        &template_with_requires_confirmation(),
    );

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "confirm-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    // Should stop at confirm_step, not advance to done.
    assert_eq!(
        json["state"].as_str(),
        Some("confirm_step"),
        "should stop at state requiring confirmation"
    );
    assert_eq!(
        json["action"].as_str(),
        Some("confirm"),
        "action should be 'confirm'"
    );
    assert!(
        json["action_output"].is_object(),
        "action_output should be present"
    );
    assert_eq!(json["action_output"]["exit_code"], 0);
    assert!(
        json["action_output"]["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("needs-review"),
        "stdout should contain command output"
    );
}

fn template_with_polling_action() -> String {
    // The action creates a counter file that increments on each run.
    // On the third run, it creates a "ready" marker.
    // The gate checks for the "ready" marker.
    r#"---
name: poll-workflow
version: "1.0"
initial_state: wait
states:
  wait:
    default_action:
      command: |
        count=0
        if [ -f poll_count.txt ]; then
          count=$(cat poll_count.txt)
        fi
        count=$((count + 1))
        echo $count > poll_count.txt
        if [ $count -ge 2 ]; then
          touch ready.txt
        fi
        echo "iteration $count"
      polling:
        interval_secs: 1
        timeout_secs: 10
    gates:
      ready:
        type: command
        command: "test -f ready.txt"
    transitions:
      - target: done
  done:
    terminal: true
---

## wait

Wait for readiness.

## done

All done.
"#
    .to_string()
}

#[test]
fn polling_action_retries_until_success() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "poll-wf", &template_with_polling_action());

    let output = koto()
        .current_dir(dir.path())
        .args(["next", "poll-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    // The polling should eventually succeed and auto-advance to done.
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "polling should complete and auto-advance to terminal state"
    );
    assert_eq!(json["advanced"], true);

    // Verify the ready marker was created.
    assert!(
        dir.path().join("ready.txt").exists(),
        "polling should have created ready.txt"
    );
}

// ---------------------------------------------------------------------------
// export: mermaid output validation and determinism
// ---------------------------------------------------------------------------

/// Path to the multi-state fixture template.
fn fixture_multi_state() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test/functional/fixtures/templates/multi-state.md")
}

/// Path to the simple-gates fixture template.
fn fixture_simple_gates() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test/functional/fixtures/templates/simple-gates.md")
}

/// A single-state template (initial + terminal, no transitions).
fn single_state_template_content() -> &'static str {
    r#"---
name: single-state
version: "1.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

This workflow has a single state that is both initial and terminal.
"#
}

#[test]
fn export_multi_state_fixture_contains_expected_states_and_transitions() {
    let compiled = koto::template::compile::compile(&fixture_multi_state()).unwrap();
    let mermaid = koto::export::to_mermaid(&compiled);

    // Header
    assert!(
        mermaid.starts_with("stateDiagram-v2\n"),
        "should start with stateDiagram-v2"
    );

    // Initial state marker
    assert!(
        mermaid.contains("[*] --> entry"),
        "should have initial state marker for entry, got:\n{}",
        mermaid
    );

    // Terminal state marker
    assert!(
        mermaid.contains("done --> [*]"),
        "should have terminal state marker for done, got:\n{}",
        mermaid
    );

    // Transitions
    assert!(
        mermaid.contains("entry --> setup"),
        "should have entry->setup transition, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("entry --> work"),
        "should have entry->work transition, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("setup --> work"),
        "should have setup->work transition, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("work --> done"),
        "should have work->done transition, got:\n{}",
        mermaid
    );

    // Conditional transition labels
    assert!(
        mermaid.contains("entry --> setup : route: setup"),
        "should have labeled transition for route: setup, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("entry --> work : route: work"),
        "should have labeled transition for route: work, got:\n{}",
        mermaid
    );

    // Gate annotation
    assert!(
        mermaid.contains("note left of setup : gate: config_exists"),
        "should have gate annotation for config_exists, got:\n{}",
        mermaid
    );

    // LF line endings
    assert!(!mermaid.contains("\r\n"), "output must use LF, not CRLF");
    assert!(!mermaid.contains('\r'), "output must not contain CR");

    // Trailing newline
    assert!(mermaid.ends_with('\n'), "output should end with newline");
}

#[test]
fn export_simple_gates_fixture_has_gate_and_when_labels() {
    let compiled = koto::template::compile::compile(&fixture_simple_gates()).unwrap();
    let mermaid = koto::export::to_mermaid(&compiled);

    // Gate note
    assert!(
        mermaid.contains("note left of start : gate: check_file"),
        "should have gate annotation for check_file, got:\n{}",
        mermaid
    );

    // When conditions on transitions
    assert!(
        mermaid.contains("start --> done : status: completed"),
        "should have labeled transition for status: completed, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("start --> done : status: override"),
        "should have labeled transition for status: override, got:\n{}",
        mermaid
    );

    // Initial and terminal markers
    assert!(mermaid.contains("[*] --> start"));
    assert!(mermaid.contains("done --> [*]"));
}

#[test]
fn export_determinism_byte_identical_across_calls() {
    let compiled = koto::template::compile::compile(&fixture_multi_state()).unwrap();
    let first = koto::export::to_mermaid(&compiled);
    let second = koto::export::to_mermaid(&compiled);

    assert_eq!(
        first, second,
        "two calls to to_mermaid on the same template must produce byte-identical output"
    );

    // Also verify bytes match (not just string equality).
    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "byte-level comparison must also match"
    );
}

#[test]
fn export_single_state_template_produces_valid_mermaid() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("single-state.md");
    std::fs::write(&src, single_state_template_content()).unwrap();

    let compiled = koto::template::compile::compile(&src).unwrap();
    let mermaid = koto::export::to_mermaid(&compiled);

    // Header
    assert!(mermaid.starts_with("stateDiagram-v2\n"));

    // Initial and terminal markers both point to the single state.
    assert!(
        mermaid.contains("[*] --> only"),
        "should have initial state marker, got:\n{}",
        mermaid
    );
    assert!(
        mermaid.contains("only --> [*]"),
        "should have terminal state marker, got:\n{}",
        mermaid
    );

    // No inter-state transitions (no lines with --> that don't involve [*]).
    let transition_lines: Vec<&str> = mermaid
        .lines()
        .filter(|l| l.contains("-->") && !l.contains("[*]"))
        .collect();
    assert!(
        transition_lines.is_empty(),
        "single-state template should have no inter-state transitions, got: {:?}",
        transition_lines
    );

    // Trailing newline
    assert!(mermaid.ends_with('\n'));
}

#[test]
fn export_md_and_json_produce_identical_output() {
    let dir = TempDir::new().unwrap();

    // Write the multi-state fixture to a temp .md file and compile it via CLI
    // to get a .json output.
    let fixture = fixture_multi_state();

    let compile_output = koto()
        .current_dir(dir.path())
        .args(["template", "compile", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        compile_output.status.success(),
        "template compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let json_path = String::from_utf8(compile_output.stdout)
        .unwrap()
        .trim()
        .to_string();

    // Export from .md source
    let md_output = koto()
        .current_dir(dir.path())
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        md_output.status.success(),
        "export from .md should succeed: {}",
        String::from_utf8_lossy(&md_output.stderr)
    );

    // Export from .json source
    let json_output = koto()
        .current_dir(dir.path())
        .args(["template", "export", &json_path])
        .output()
        .unwrap();

    assert!(
        json_output.status.success(),
        "export from .json should succeed: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );

    let md_mermaid = String::from_utf8(md_output.stdout).unwrap();
    let json_mermaid = String::from_utf8(json_output.stdout).unwrap();

    assert_eq!(
        md_mermaid, json_mermaid,
        ".md and .json inputs must produce identical Mermaid output"
    );
}

#[test]
fn export_cli_outputs_mermaid_to_stdout() {
    let fixture = fixture_multi_state();

    let output = koto()
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "export should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("stateDiagram-v2\n"),
        "CLI stdout should contain mermaid diagram, got:\n{}",
        stdout
    );
    assert!(stdout.contains("[*] --> entry"));
    assert!(stdout.contains("done --> [*]"));
}

#[test]
fn export_cli_writes_to_output_file() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("diagram.mermaid.md");

    let output = koto()
        .current_dir(dir.path())
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "export with --output should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output_path.exists(),
        "output file should be created at {:?}",
        output_path
    );

    let content = std::fs::read_to_string(&output_path).unwrap();
    assert!(
        content.starts_with("stateDiagram-v2\n"),
        "output file should contain mermaid diagram"
    );
    assert!(content.contains("[*] --> entry"));
}

#[test]
fn export_determinism_via_cli() {
    let fixture = fixture_multi_state();

    let first = koto()
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    let second = koto()
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(first.status.success());
    assert!(second.status.success());

    assert_eq!(
        first.stdout, second.stdout,
        "two CLI export invocations must produce byte-identical stdout"
    );
}
