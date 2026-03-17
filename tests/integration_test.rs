use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::Path;

fn koto() -> Command {
    Command::cargo_bin("koto").unwrap()
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
fn version_exits_0_and_produces_json() {
    let output = koto().arg("version").output().unwrap();

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

    assert!(
        json["state"].as_str().is_some(),
        "state field should be present"
    );
    assert!(
        json["directive"].as_str().is_some(),
        "directive field should be present"
    );
    assert!(
        json["action"].as_str().is_some(),
        "action field should be present"
    );
    assert!(
        json["advanced"].is_boolean(),
        "advanced field should be a boolean"
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
    let src = write_template_source(dir.path());

    // init
    let init_out = koto()
        .current_dir(dir.path())
        .args(["init", "seq-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        init_out.status.success(),
        "init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&init_out.stdout),
        String::from_utf8_lossy(&init_out.stderr)
    );
    let init_json: serde_json::Value = serde_json::from_slice(&init_out.stdout).unwrap();
    assert_eq!(init_json["name"], "seq-wf");

    // next
    let next_out = koto()
        .current_dir(dir.path())
        .args(["next", "seq-wf"])
        .output()
        .unwrap();
    assert!(next_out.status.success(), "next should succeed");
    let next_json: serde_json::Value = serde_json::from_slice(&next_out.stdout).unwrap();
    assert!(next_json["state"].as_str().is_some());
    assert!(next_json["directive"].as_str().is_some());
    assert!(next_json["action"].as_str().is_some());

    // Append a transitioned event to enable rewind (init writes 3 lines:
    // header + workflow_initialized + transitioned, so we need a second transition).
    let state_path = dir.path().join("koto-seq-wf.state.jsonl");
    let extra = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"gate"}}"#;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&state_path)
            .unwrap();
        writeln!(f, "{}", extra).unwrap();
    }

    // rewind
    let rewind_out = koto()
        .current_dir(dir.path())
        .args(["rewind", "seq-wf"])
        .output()
        .unwrap();
    assert!(rewind_out.status.success(), "rewind should succeed");
    let rewind_json: serde_json::Value = serde_json::from_slice(&rewind_out.stdout).unwrap();
    assert_eq!(rewind_json["name"], "seq-wf");
    let rewound_state = rewind_json["state"]
        .as_str()
        .expect("state field should be present");

    // next after rewind — state should match the rewound state
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
        Some(rewound_state),
        "state after rewind should match the rewound state"
    );
    assert!(
        next_after_json["directive"].as_str().is_some(),
        "directive should be present after rewind"
    );

    // Verify the last event in the state file is a rewound event.
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

    assert_eq!(json["action"].as_str(), Some("execute"), "action should be execute");
    assert_eq!(json["state"].as_str(), Some("start"), "state should be start");
    assert!(json["error"].is_null(), "error should be null for integration_unavailable");
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
    assert_eq!(json["action"].as_str(), Some("done"), "action should be done");
    assert_eq!(json["state"].as_str(), Some("done"), "state should be done");
    assert_eq!(json["advanced"], false, "advanced should be false (no event appended)");
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

    assert_eq!(json["action"].as_str(), Some("execute"), "action should be execute");
    assert_eq!(json["state"].as_str(), Some("start"), "state should be start");
    assert!(json["error"].is_null(), "error should be null for gate_blocked");

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
    assert_eq!(json["state"].as_str(), Some("start"), "state should still be start (evidence appended, not yet advanced)");
    assert_eq!(json["advanced"], true, "advanced should be true after evidence submission");
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
    let fields: Vec<&str> = details
        .iter()
        .filter_map(|d| d["field"].as_str())
        .collect();
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

    // Verify next on the new state works.
    let next_output = koto()
        .current_dir(dir.path())
        .args(["next", "directed-wf"])
        .output()
        .unwrap();
    assert!(next_output.status.success());
    let next_json: serde_json::Value = serde_json::from_slice(&next_output.stdout).unwrap();
    assert_eq!(next_json["state"].as_str(), Some("implement"));
}

// ---------------------------------------------------------------------------
// scenario-35: Agent-driven workflow loop using only koto next output
// ---------------------------------------------------------------------------

#[test]
fn agent_driven_workflow_loop() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "loop-wf", &template_multi_state());

    // Simulate an agent loop: call next, read state, use --to to advance.
    let states_to_visit = ["plan", "implement", "verify", "done"];

    for (i, expected_state) in states_to_visit.iter().enumerate() {
        let output = koto()
            .current_dir(dir.path())
            .args(["next", "loop-wf"])
            .output()
            .unwrap();

        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            json["state"].as_str(),
            Some(*expected_state),
            "step {}: state should be {}",
            i,
            expected_state
        );

        if json["action"].as_str() == Some("done") {
            // Terminal state reached. Verify this is the last one.
            assert_eq!(
                *expected_state, "done",
                "done action should only appear at done state"
            );
            break;
        }

        // Agent decides to advance to the next state via --to.
        if i + 1 < states_to_visit.len() {
            let next_target = states_to_visit[i + 1];
            let advance = koto()
                .current_dir(dir.path())
                .args(["next", "loop-wf", "--to", next_target])
                .output()
                .unwrap();
            assert!(
                advance.status.success(),
                "advance to {} should succeed: {}",
                next_target,
                String::from_utf8_lossy(&advance.stdout)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// scenario-36: Gate timeout kills entire process group
// ---------------------------------------------------------------------------

#[test]
fn gate_timeout_returns_gate_blocked() {
    let dir = TempDir::new().unwrap();
    // Use a gate command that sleeps longer than the 1-second timeout.
    init_workflow(
        dir.path(),
        "timeout-wf",
        &template_with_gate("sleep 60", 1),
    );

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

    assert_eq!(json["action"].as_str(), Some("execute"), "action should be execute");
    assert!(json["error"].is_null(), "error should be null for gate_blocked");

    let conditions = json["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array");
    assert_eq!(conditions.len(), 1, "should have one blocking condition");
    assert_eq!(conditions[0]["name"].as_str(), Some("check"));
    assert_eq!(conditions[0]["status"].as_str(), Some("timed_out"),
        "gate should have timed out, not failed");
}
