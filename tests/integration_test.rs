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
        json["transitions"].is_array(),
        "transitions field should be an array"
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
    assert!(
        json["error"].as_str().is_some(),
        "error field should be present"
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
    assert!(next_json["transitions"].is_array());

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
