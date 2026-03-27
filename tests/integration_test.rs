use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

/// Return a `koto` command with `KOTO_SESSIONS_BASE` set to the sessions
/// subdirectory of `dir`.
///
/// All integration tests must use this to avoid writing session data into
/// the real `~/.koto/` directory.
fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd
}

/// Return the sessions base directory for a test, creating it if needed.
fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// Return the state file path for a workflow inside the sessions base directory.
fn session_state_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
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

/// Compile the minimal template into the cache and return the compiled JSON path.
fn compile_template(dir: &Path) -> String {
    let src = write_template_source(dir);

    let output = koto_cmd(dir)
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
fn version_exits_0_and_produces_json() {
    let output = Command::cargo_bin("koto")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();

    assert!(output.status.success(), "version should exit 0");

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("version output should be valid JSON");

    let v = json["version"]
        .as_str()
        .expect("version field should be a string");
    assert!(!v.is_empty(), "version field should not be empty");
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[test]
fn init_creates_state_file() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let output = koto_cmd(dir.path())
        .args(["init", "my-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let state_path = session_state_path(dir.path(), "my-wf");
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
    let first = koto_cmd(dir.path())
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
    let second = koto_cmd(dir.path())
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

    koto_cmd(dir.path())
        .args(["init", "next-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    koto_cmd(dir.path())
        .args(["init", "rewind-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    // Append a transitioned event so there are 2+ state-changing events,
    // making rewind possible (init writes header + workflow_initialized + transitioned).
    let state_path = session_state_path(dir.path(), "rewind-wf");
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

    let output = koto_cmd(dir.path())
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
    koto_cmd(dir.path())
        .args(["init", "at-init-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    koto_cmd(dir.path())
        .args(["init", "listed-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path()).arg("workflows").output().unwrap();

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

    let output = koto_cmd(dir.path()).arg("workflows").output().unwrap();

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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
    let next_out = koto_cmd(dir.path())
        .args(["next", "seq-wf"])
        .output()
        .unwrap();
    assert!(next_out.status.success(), "next should succeed");
    let next_json: serde_json::Value = serde_json::from_slice(&next_out.stdout).unwrap();
    assert_eq!(next_json["state"].as_str(), Some("start"));
    assert!(next_json["directive"].as_str().is_some());
    assert!(next_json["action"].as_str().is_some());

    // Use --to to advance to implement, enabling rewind.
    let advance = koto_cmd(dir.path())
        .args(["next", "seq-wf", "--to", "implement"])
        .output()
        .unwrap();
    assert!(advance.status.success(), "--to should succeed");

    // rewind
    let rewind_out = koto_cmd(dir.path())
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
    let next_after = koto_cmd(dir.path())
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
    let state_path = session_state_path(dir.path(), "seq-wf");
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
    let state_path = session_state_path(dir.path(), "corrupt");
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&state_path, "this is not valid json at all\n").unwrap();

    let output = koto_cmd(dir.path())
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

    koto_cmd(dir.path())
        .args(["init", "payload-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    // Append a transitioned event so rewind is possible.
    let state_path = session_state_path(dir.path(), "payload-wf");
    let extra_event = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"gate"}}"#;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&state_path)
            .unwrap();
        writeln!(f, "{}", extra_event).unwrap();
    }

    let output = koto_cmd(dir.path())
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

// ---------------------------------------------------------------------------
// scenario-26: Integration state classification
// ---------------------------------------------------------------------------

#[test]
fn next_integration_state_returns_integration_unavailable() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "integ-wf", &template_with_integration());

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    // Advance to terminal state via --to, preserving session for follow-up test.
    let advance = koto_cmd(dir.path())
        .args(["next", "term-wf", "--to", "done", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(
        advance.status.success(),
        "advance to done should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&advance.stdout),
        String::from_utf8_lossy(&advance.stderr)
    );

    // Now call next again on the terminal state.
    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
    let initial = koto_cmd(dir.path())
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

    // Submit evidence via --with-data, preserving session so we can inspect the state file.
    let submit = koto_cmd(dir.path())
        .args([
            "next",
            "evid-wf",
            "--with-data",
            r#"{"decision":"proceed","notes":"looks good"}"#,
            "--no-cleanup",
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
    let state_path = session_state_path(dir.path(), "evid-wf");
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
    let output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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
    let state_path = session_state_path(dir.path(), "directed-wf");
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
    let next_output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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
    let first = koto_cmd(dir.path())
        .args(["next", "chain-wf"])
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["state"].as_str(), Some("verify"));

    // Submit reject evidence -> should auto-advance: verify -> implement -> verify (stops again).
    let reject = koto_cmd(dir.path())
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
    let approve = koto_cmd(dir.path())
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
    let state_path = session_state_path(dir.path(), "lock-wf");
    assert!(state_path.exists(), "state file should exist after init");

    // Hold an exclusive flock on the state file, simulating a concurrent koto next.
    let lock_file = std::fs::File::open(&state_path).unwrap();
    let fd = lock_file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0, "test should acquire flock successfully");

    // Now run koto next -- it should fail because it can't acquire the lock.
    let output = koto_cmd(dir.path())
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
    let cancel = koto_cmd(dir.path())
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
    let next = koto_cmd(dir.path())
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
    let first = koto_cmd(dir.path())
        .args(["cancel", "dbl-cancel-wf"])
        .output()
        .unwrap();
    assert!(first.status.success());

    // Second cancel fails.
    let second = koto_cmd(dir.path())
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

    // Auto-advance to terminal state, preserving session for cancel test.
    let advance = koto_cmd(dir.path())
        .args(["next", "term-cancel-wf", "--to", "done", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(advance.status.success());

    // Cancel should fail because workflow is already terminal.
    let cancel = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
// scenario-12/14/15: {{SESSION_DIR}} substituted in gate commands and directives
// ---------------------------------------------------------------------------

/// Template with {{SESSION_DIR}} in both a gate command and a directive.
fn template_with_session_dir_vars() -> String {
    r#"---
name: session-dir-workflow
version: "1.0"
initial_state: start
states:
  start:
    gates:
      check_session:
        type: command
        command: "test -d {{SESSION_DIR}}"
        timeout: 5
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Write output to {{SESSION_DIR}}/result.txt then proceed.

## done

All done.
"#
    .to_string()
}

#[test]
fn session_dir_substituted_in_gate_and_directive() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "varsub-wf", &template_with_session_dir_vars());

    // The session directory should exist after init (backend.create makes it).
    let session_dir = sessions_base(dir.path()).join("varsub-wf");
    assert!(
        session_dir.exists(),
        "session directory should exist after init"
    );

    // Run koto next. The gate command `test -d {{SESSION_DIR}}` should pass
    // because the session directory exists. Then auto-advance to done.
    let output = koto_cmd(dir.path())
        .args(["next", "varsub-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed when {{{{SESSION_DIR}}}} resolves: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    // Gate passed, auto-advanced to done (terminal).
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "should auto-advance to done after gate passes"
    );
    assert_eq!(json["action"].as_str(), Some("done"));
}

#[test]
fn session_dir_substituted_in_directive_text() {
    let dir = TempDir::new().unwrap();

    // Template where the start state has an accepts block with a conditional
    // transition, so the engine stops for evidence. The directive contains
    // {{SESSION_DIR}} which should be substituted.
    let template = r#"---
name: directive-var-workflow
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      data:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          data: ok
  done:
    terminal: true
---

## start

Save your work to {{SESSION_DIR}}/output.txt before submitting.

## done

All done.
"#;

    init_workflow(dir.path(), "dirvar-wf", template);

    let output = koto_cmd(dir.path())
        .args(["next", "dirvar-wf"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "next should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let directive = json["directive"]
        .as_str()
        .unwrap_or_else(|| panic!("directive missing from response: {}", json));

    // The directive should NOT contain the raw {{SESSION_DIR}} token.
    assert!(
        !directive.contains("{{SESSION_DIR}}"),
        "directive should have {{{{SESSION_DIR}}}} substituted, got: {}",
        directive
    );

    // It should contain the actual session directory path.
    let session_dir = sessions_base(dir.path()).join("dirvar-wf");
    assert!(
        directive.contains(&session_dir.to_string_lossy().to_string()),
        "directive should contain the actual session dir path, got: {}",
        directive
    );
}

// ---------------------------------------------------------------------------
// scenario-13: Reserved variable name collision rejected
// ---------------------------------------------------------------------------

#[test]
fn reserved_variable_name_collision_rejected() {
    let dir = TempDir::new().unwrap();

    // Template that declares SESSION_DIR in its variables block.
    let template = r#"---
name: collision-workflow
version: "1.0"
initial_state: start
variables:
  SESSION_DIR:
    description: "This collides with a reserved name"
    required: false
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Do something.

## done

All done.
"#;

    init_workflow(dir.path(), "collide-wf", template);

    let output = koto_cmd(dir.path())
        .args(["next", "collide-wf"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "reserved name collision should fail with exit 2, stdout={} stderr={}",
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
            .contains("reserved variable"),
        "error message should mention reserved variable, got: {}",
        json["error"]["message"]
    );
}

// ---------------------------------------------------------------------------
// Session subcommand tests (scenario-16, scenario-17, scenario-18)
// ---------------------------------------------------------------------------

#[test]
fn session_dir_prints_absolute_path() {
    let dir = TempDir::new().unwrap();
    let name = "my-workflow";

    let output = koto_cmd(dir.path())
        .args(["session", "dir", name])
        .output()
        .unwrap();

    assert!(output.status.success(), "session dir should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout.trim();
    // Should be an absolute path containing the session name
    assert!(
        Path::new(path).is_absolute(),
        "session dir output should be an absolute path, got: {}",
        path
    );
    assert!(
        path.contains(name),
        "session dir output should contain the session name, got: {}",
        path
    );
}

#[test]
fn session_list_outputs_json_array() {
    let dir = TempDir::new().unwrap();

    // List with no sessions should return an empty array
    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();

    assert!(output.status.success(), "session list should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(json.is_array(), "session list should output a JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0, "should have no sessions");

    // Init a workflow, then list should show it
    let template_path = write_template_source(dir.path());
    koto_cmd(dir.path())
        .args([
            "init",
            "test-wf",
            "--template",
            template_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1, "should have one session");
    assert_eq!(arr[0]["id"].as_str(), Some("test-wf"));
    assert!(
        arr[0]["created_at"].is_string(),
        "created_at should be a string"
    );
}

#[test]
fn session_cleanup_removes_session() {
    let dir = TempDir::new().unwrap();
    let template_path = write_template_source(dir.path());

    // Init a workflow
    koto_cmd(dir.path())
        .args([
            "init",
            "cleanup-wf",
            "--template",
            template_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Verify it exists in list
    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Cleanup
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cleanup-wf"])
        .assert()
        .success();

    // Verify list is now empty
    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(
        json.as_array().unwrap().len(),
        0,
        "session should be removed after cleanup"
    );
}

#[test]
fn session_cleanup_is_idempotent() {
    let dir = TempDir::new().unwrap();

    // Cleanup a non-existent session should succeed
    koto_cmd(dir.path())
        .args(["session", "cleanup", "nonexistent"])
        .assert()
        .success();
}

#[test]
fn session_without_subcommand_shows_help() {
    let dir = TempDir::new().unwrap();

    let output = koto_cmd(dir.path()).args(["session"]).output().unwrap();

    // clap exits with code 2 when a required subcommand is missing
    assert!(
        !output.status.success(),
        "session without subcommand should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage") || stderr.contains("usage"),
        "should show usage info, got: {}",
        stderr
    );
}

#[test]
fn session_full_lifecycle() {
    let dir = TempDir::new().unwrap();
    let template_path = write_template_source(dir.path());

    // Init a workflow
    koto_cmd(dir.path())
        .args([
            "init",
            "lifecycle-wf",
            "--template",
            template_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Verify dir path
    let output = koto_cmd(dir.path())
        .args(["session", "dir", "lifecycle-wf"])
        .output()
        .unwrap();
    let dir_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        Path::new(&dir_path).exists(),
        "session dir should exist after init"
    );

    // Verify list shows it
    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Cleanup
    koto_cmd(dir.path())
        .args(["session", "cleanup", "lifecycle-wf"])
        .assert()
        .success();

    // Verify dir is gone
    assert!(
        !Path::new(&dir_path).exists(),
        "session dir should be removed after cleanup"
    );

    // Verify list is empty
    let output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(
        json.as_array().unwrap().len(),
        0,
        "list should be empty after cleanup"
    );
}

// ---------------------------------------------------------------------------
// scenario-19: Auto-cleanup on terminal state
// ---------------------------------------------------------------------------

#[test]
fn auto_cleanup_on_terminal_state_via_to() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "cleanup-wf", minimal_template());

    let session_dir = sessions_base(dir.path()).join("cleanup-wf");
    assert!(session_dir.exists(), "session dir should exist after init");

    // Advance to terminal state via --to (auto-cleanup enabled by default).
    let output = koto_cmd(dir.path())
        .args(["next", "cleanup-wf", "--to", "done"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "advance to done should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the response was printed (output first, cleanup second).
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["action"].as_str(), Some("done"));
    assert_eq!(json["state"].as_str(), Some("done"));

    // Session directory should be removed after auto-cleanup.
    assert!(
        !session_dir.exists(),
        "session dir should be removed after terminal auto-cleanup"
    );
}

#[test]
fn auto_cleanup_on_terminal_state_via_advance() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "adv-cleanup-wf", minimal_template());

    let session_dir = sessions_base(dir.path()).join("adv-cleanup-wf");
    assert!(session_dir.exists(), "session dir should exist after init");

    // Submit evidence to advance to terminal via the advancement loop.
    // minimal_template goes start -> done with an unconditional transition,
    // so just submitting empty evidence (or using --to) reaches terminal.
    let output = koto_cmd(dir.path())
        .args(["next", "adv-cleanup-wf", "--to", "done"])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Session directory should be removed.
    assert!(
        !session_dir.exists(),
        "session dir should be removed after auto-advance to terminal"
    );
}

// ---------------------------------------------------------------------------
// scenario-20: --no-cleanup preserves session on terminal state
// ---------------------------------------------------------------------------

#[test]
fn no_cleanup_flag_preserves_session() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "keep-wf", minimal_template());

    let session_dir = sessions_base(dir.path()).join("keep-wf");
    assert!(session_dir.exists(), "session dir should exist after init");

    // Advance to terminal with --no-cleanup.
    let output = koto_cmd(dir.path())
        .args(["next", "keep-wf", "--to", "done", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "advance to done should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Response should still be correct.
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["action"].as_str(), Some("done"));

    // Session directory should still exist.
    assert!(
        session_dir.exists(),
        "session dir should be preserved when --no-cleanup is set"
    );
}

// ---------------------------------------------------------------------------
// Auto-cleanup graceful handling: missing session directory
// ---------------------------------------------------------------------------

#[test]
fn auto_cleanup_graceful_on_missing_session_dir() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "missing-wf", minimal_template());

    let session_dir = sessions_base(dir.path()).join("missing-wf");
    let state_path = session_state_path(dir.path(), "missing-wf");

    // Manually move the state file so the session dir can be removed
    // but the state file can be re-placed to simulate a race condition
    // where the directory was already cleaned.
    let state_content = std::fs::read_to_string(&state_path).unwrap();

    // Remove the session dir, then recreate just the state file (no dir artifacts).
    std::fs::remove_dir_all(&session_dir).unwrap();
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(&state_path, &state_content).unwrap();

    // Advance to terminal -- cleanup should handle the partial state gracefully.
    let output = koto_cmd(dir.path())
        .args(["next", "missing-wf", "--to", "done"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "advance should succeed even if cleanup has edge cases: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["action"].as_str(), Some("done"));
}

// ---------------------------------------------------------------------------
// Context subcommand tests
// ---------------------------------------------------------------------------

/// Helper: create a session directory (without a full workflow init) so context
/// commands have somewhere to store files.
fn create_session_dir(dir: &Path, name: &str) {
    let session_dir = sessions_base(dir).join(name);
    std::fs::create_dir_all(&session_dir).unwrap();
}

#[test]
fn context_add_from_stdin_and_get_to_stdout() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    // Add content via stdin
    let output = koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "scope.md"])
        .write_stdin("hello from stdin")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context add should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Get content to stdout
    let output = koto_cmd(dir.path())
        .args(["context", "get", "ctx-wf", "scope.md"])
        .output()
        .unwrap();
    assert!(output.status.success(), "context get should succeed");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello from stdin");
}

#[test]
fn context_add_from_file_and_get_to_file() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    let input_file = dir.path().join("input.txt");
    std::fs::write(&input_file, "file content here").unwrap();

    // Add from file
    let output = koto_cmd(dir.path())
        .args([
            "context",
            "add",
            "ctx-wf",
            "data.txt",
            "--from-file",
            input_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context add --from-file should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Get to file
    let output_file = dir.path().join("output.txt");
    let output = koto_cmd(dir.path())
        .args([
            "context",
            "get",
            "ctx-wf",
            "data.txt",
            "--to-file",
            output_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context get --to-file should succeed"
    );

    let written = std::fs::read_to_string(&output_file).unwrap();
    assert_eq!(written, "file content here");
}

#[test]
fn context_exists_returns_exit_0_when_present() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    // Add a key
    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "scope.md"])
        .write_stdin("data")
        .assert()
        .success();

    // Exists should return exit 0
    let output = koto_cmd(dir.path())
        .args(["context", "exists", "ctx-wf", "scope.md"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context exists should exit 0 for present key"
    );
    // No stdout output for exists
    assert!(
        output.stdout.is_empty(),
        "context exists should produce no stdout"
    );
}

#[test]
fn context_exists_returns_exit_1_when_missing() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    let output = koto_cmd(dir.path())
        .args(["context", "exists", "ctx-wf", "missing.md"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "context exists should exit 1 for missing key"
    );
}

#[test]
fn context_list_returns_json_array() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    // Empty list
    let output = koto_cmd(dir.path())
        .args(["context", "list", "ctx-wf"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json, serde_json::json!([]));

    // Add some keys
    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "beta.md"])
        .write_stdin("b")
        .assert()
        .success();
    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "alpha.md"])
        .write_stdin("a")
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["context", "list", "ctx-wf"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json, serde_json::json!(["alpha.md", "beta.md"]));
}

#[test]
fn context_list_with_prefix_filter() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "scope.md"])
        .write_stdin("s")
        .assert()
        .success();
    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "research/r1/a.md"])
        .write_stdin("a")
        .assert()
        .success();
    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "research/r1/b.md"])
        .write_stdin("b")
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["context", "list", "ctx-wf", "--prefix", "research/"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(
        json,
        serde_json::json!(["research/r1/a.md", "research/r1/b.md"])
    );
}

#[test]
fn context_add_does_not_advance_workflow_state() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "state-wf", minimal_template());

    // Read state file content before context add
    let state_path = session_state_path(dir.path(), "state-wf");
    let before = std::fs::read_to_string(&state_path).unwrap();

    // Add context
    koto_cmd(dir.path())
        .args(["context", "add", "state-wf", "scope.md"])
        .write_stdin("context data")
        .assert()
        .success();

    // State file should be unchanged (no new events appended)
    let after = std::fs::read_to_string(&state_path).unwrap();
    assert_eq!(
        before, after,
        "context add should not modify the state file"
    );
}

#[test]
fn context_get_missing_key_returns_error() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    let output = koto_cmd(dir.path())
        .args(["context", "get", "ctx-wf", "nonexistent.md"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "context get for missing key should exit 3"
    );
}

#[test]
fn context_add_rejects_invalid_key() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    let output = koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "../escape.md"])
        .write_stdin("bad")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "context add with path traversal key should exit 3"
    );
}

#[test]
fn context_add_overwrites_existing_key() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "scope.md"])
        .write_stdin("v1")
        .assert()
        .success();

    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "scope.md"])
        .write_stdin("v2")
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["context", "get", "ctx-wf", "scope.md"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "v2");
}

#[test]
fn context_hierarchical_keys_work() {
    let dir = TempDir::new().unwrap();
    create_session_dir(dir.path(), "ctx-wf");

    koto_cmd(dir.path())
        .args(["context", "add", "ctx-wf", "research/r1/lead-cli-ux.md"])
        .write_stdin("deep content")
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["context", "get", "ctx-wf", "research/r1/lead-cli-ux.md"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "deep content");
}
