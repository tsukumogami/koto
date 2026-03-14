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
    transitions: [done]
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

    assert!(
        dir.path().join("koto-my-wf.state.jsonl").exists(),
        "state file should be created"
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

    // Append a second event directly so the state file has 2+ events,
    // making rewind possible (init-only state has exactly 1 event).
    let state_path = dir.path().join("koto-rewind-wf.state.jsonl");
    let extra_event = r#"{"type":"transition","state":"done","timestamp":"2026-01-01T00:00:00Z"}"#;
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

    // The last line must be a rewind event.
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
        Some("rewind"),
        "last event should be a rewind event"
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
        arr.iter().any(|v| v.as_str() == Some("listed-wf")),
        "array should contain the initialized workflow name, got: {:?}",
        arr
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
fn template_validate_fails_for_invalid_json() {
    let dir = TempDir::new().unwrap();
    // Write a JSON file that lacks required fields.
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

    // Append a second event to enable rewind.
    let state_path = dir.path().join("koto-seq-wf.state.jsonl");
    let extra = r#"{"type":"transition","state":"done","timestamp":"2026-01-01T00:00:00Z"}"#;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&state_path)
        .unwrap();
    writeln!(f, "{}", extra).unwrap();
    drop(f);

    // rewind
    let rewind_out = koto()
        .current_dir(dir.path())
        .args(["rewind", "seq-wf"])
        .output()
        .unwrap();
    assert!(rewind_out.status.success(), "rewind should succeed");
    let rewind_json: serde_json::Value = serde_json::from_slice(&rewind_out.stdout).unwrap();
    assert_eq!(rewind_json["name"], "seq-wf");
    assert!(rewind_json["state"].as_str().is_some());
}
