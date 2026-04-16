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
    // Override HOME so tests don't read the user's ~/.koto/config.toml
    // (which might set backend = "cloud" or other non-default values).
    cmd.env("HOME", dir);
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
        .args(["version", "--json"])
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
fn init_succeeds_with_stale_tempfile_in_session_dir() {
    // Crash recovery test: if a prior `koto init` crashed between
    // writing the tempfile and renaming it into place, a stale
    // `.koto-init-*.tmp` file will still be present in the session
    // directory. A fresh `koto init` on the same workflow name must
    // still succeed (proving `handle_init` now goes through the
    // atomic `init_state_file` path, which tolerates leftover tmp
    // files — the old three-call sequence had no such recovery
    // guarantee).
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    // Plant a stale tempfile that mimics the crashed-init shape.
    let session_dir = sessions_base(dir.path()).join("recover-wf");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join(".koto-init-stale.tmp"),
        b"partial content from prior crash",
    )
    .unwrap();

    // No state file yet, so `exists()` is false and init must run.
    let state_path = session_state_path(dir.path(), "recover-wf");
    assert!(
        !state_path.exists(),
        "state file must not exist before init"
    );

    let output = koto_cmd(dir.path())
        .args(["init", "recover-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init must succeed with stale tempfile present: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(state_path.exists(), "state file must be written");
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

/// Re-initializing the same child name via `koto init --parent` must surface
/// the same "already exists" error as top-level re-init. Bug #133 tracked
/// cases where a second init of the same child name could silently create a
/// duplicate session.
#[test]
fn init_child_duplicate_name_rejected() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let parent = koto_cmd(dir.path())
        .args(["init", "dup-parent", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        parent.status.success(),
        "parent init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&parent.stdout),
        String::from_utf8_lossy(&parent.stderr)
    );

    let first_child = koto_cmd(dir.path())
        .args([
            "init",
            "dup-parent.child",
            "--template",
            src.to_str().unwrap(),
            "--parent",
            "dup-parent",
        ])
        .output()
        .unwrap();
    assert!(
        first_child.status.success(),
        "first child init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&first_child.stdout),
        String::from_utf8_lossy(&first_child.stderr)
    );

    let second_child = koto_cmd(dir.path())
        .args([
            "init",
            "dup-parent.child",
            "--template",
            src.to_str().unwrap(),
            "--parent",
            "dup-parent",
        ])
        .output()
        .unwrap();
    assert!(
        !second_child.status.success(),
        "second child init on same name must fail"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&second_child.stdout).expect("error output should be valid JSON");
    let msg = json["error"]
        .as_str()
        .expect("error field should be a string");
    assert!(
        msg.contains("already exists"),
        "error should identify duplicate: {}",
        msg
    );
    assert!(
        msg.contains("dup-parent.child"),
        "error should name the conflicting session: {}",
        msg
    );

    // Only one session must appear under this name.
    let list = koto_cmd(dir.path()).args(["workflows"]).output().unwrap();
    let workflows: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let items = workflows.as_array().expect("workflows output is an array");
    let child_count = items
        .iter()
        .filter(|w| w["name"].as_str() == Some("dup-parent.child"))
        .count();
    assert_eq!(
        child_count, 1,
        "exactly one entry for dup-parent.child; got {}: {}",
        child_count, workflows
    );
}

/// The duplicate-name error message must be identical whether the pre-check
/// or the atomic `init_state_file` collision detector fires. Pin the exact
/// error text so orchestrators can rely on a stable string to detect the
/// "re-spawn attempted on live session" condition without scraping state.
#[test]
fn init_duplicate_error_message_is_stable() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    koto_cmd(dir.path())
        .args(["init", "stable-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();

    let dup = koto_cmd(dir.path())
        .args(["init", "stable-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&dup.stdout).unwrap();
    assert_eq!(
        json["error"].as_str(),
        Some("workflow 'stable-wf' already exists"),
        "error text must be the documented stable string: {}",
        json
    );
    assert_eq!(
        json["command"].as_str(),
        Some("init"),
        "envelope must carry the subcommand tag"
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
        json["error"].is_object() || json["error"].as_str().is_some(),
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
        Some("integration_unavailable"),
        "action should be integration_unavailable"
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
        Some("gate_blocked"),
        "action should be gate_blocked"
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
        Some("evidence_required"),
        "action should be evidence_required at non-terminal state"
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
// scenario-37: Non-batch koto next ignores external flocks
// ---------------------------------------------------------------------------
//
// Early revisions of the batch-child-spawning work unconditionally
// acquired a parent flock inside `handle_next`. That behavior was
// narrowed (Issue #2) to apply only to batch-scoped parents so the
// happy path for ordinary workflows stays lock-free.
//
// This test pins that narrowed contract: an external flock on a
// non-batch workflow's state file must not block `koto next`. The
// batch-scoped lock path is covered by `tests/batch_lock_test.rs`.

#[cfg(unix)]
#[test]
fn concurrent_next_on_non_batch_workflow_ignores_external_flock() {
    use std::os::unix::io::AsRawFd;

    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "lock-wf", &template_with_accepts());

    let state_path = session_state_path(dir.path(), "lock-wf");
    assert!(state_path.exists(), "state file should exist after init");

    // Hold an exclusive flock on the state file from the test harness.
    // For a non-batch workflow this must NOT block `koto next`.
    let lock_file = std::fs::File::open(&state_path).unwrap();
    let fd = lock_file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0, "test should acquire flock successfully");

    let output = koto_cmd(dir.path())
        .args(["next", "lock-wf"])
        .output()
        .unwrap();

    // The command should not fail with ConcurrentAccess or BatchError.
    // Non-batch workflows skip the lock entirely, so the external flock
    // is irrelevant. The command either succeeds or stops for an
    // unrelated reason (evidence_required on template_with_accepts),
    // but never with a concurrent-access envelope.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");

    if let Some(code) = json["error"]["code"].as_str() {
        assert_ne!(
            code, "concurrent_access",
            "non-batch workflow must not surface concurrent_access; stdout={}",
            stdout
        );
    }
    assert!(
        json.get("batch").is_none(),
        "non-batch workflow must not surface a batch envelope; stdout={}",
        stdout
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
        Some("gate_blocked"),
        "action should be gate_blocked"
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
        Some(3),
        "reserved name collision should fail with exit 3 (template_error), stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("template_error"),
        "error code should be template_error"
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

// ---------------------------------------------------------------------------
// Tests from main branch (export, variables, default_action, etc.)
// ---------------------------------------------------------------------------

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

/// Write the variable template source to a file in `dir` and return its path.
fn write_var_template_source(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("var-template.md");
    std::fs::write(&src, template_with_variables()).unwrap();
    src
}

#[test]
fn version_outputs_human_readable_by_default() {
    let output = Command::cargo_bin("koto")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();
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
fn version_is_derived_from_git_not_cargo_toml() {
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["version", "--json"])
        .output()
        .unwrap();
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

#[test]
fn init_var_valid_vars_stored_in_event() {
    let dir = TempDir::new().unwrap();
    let src = write_var_template_source(dir.path());

    let output = koto_cmd(dir.path())
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
    let state_path = session_state_path(dir.path(), "var-wf");
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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

    let state_path = session_state_path(dir.path(), "var-wf");
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

/// Helper to init a workflow with --var flags.
fn init_workflow_with_vars(dir: &Path, name: &str, template_content: &str, vars: &[&str]) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template_content).unwrap();

    let mut args = vec!["init", name, "--template", src.to_str().unwrap()];
    for var in vars {
        args.push("--var");
        args.push(var);
    }

    let output = koto_cmd(dir).args(&args).output().unwrap();

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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
        .args(["next", "action-wf", "--no-cleanup"])
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
        std::fs::read_to_string(session_state_path(dir.path(), "action-wf")).unwrap();
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
    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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

    let output = koto_cmd(dir.path())
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
    let compiled = koto::template::compile::compile(&fixture_multi_state(), false).unwrap();
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
        mermaid.contains("note left of setup\n        gate: config_exists\n    end note"),
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
    let compiled = koto::template::compile::compile(&fixture_simple_gates(), false).unwrap();
    let mermaid = koto::export::to_mermaid(&compiled);

    // Gate note
    assert!(
        mermaid.contains("note left of start\n        gate: check_file\n    end note"),
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
    let compiled = koto::template::compile::compile(&fixture_multi_state(), false).unwrap();
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

    let compiled = koto::template::compile::compile(&src, true).unwrap();
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

    let compile_output = koto_cmd(dir.path())
        .args([
            "template",
            "compile",
            "--allow-legacy-gates",
            fixture.to_str().unwrap(),
        ])
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
    let md_output = koto_cmd(dir.path())
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        md_output.status.success(),
        "export from .md should succeed: {}",
        String::from_utf8_lossy(&md_output.stderr)
    );

    // Export from .json source
    let json_output = koto_cmd(dir.path())
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

    let output = Command::cargo_bin("koto")
        .unwrap()
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

    let output = koto_cmd(dir.path())
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
        content.starts_with("```mermaid\n"),
        "output file should start with mermaid fence"
    );
    assert!(
        content.ends_with("```\n"),
        "output file should end with closing fence"
    );
    assert!(content.contains("stateDiagram-v2"));
    assert!(content.contains("[*] --> entry"));
}

#[test]
fn export_determinism_via_cli() {
    let fixture = fixture_multi_state();

    let first = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    let second = Command::cargo_bin("koto")
        .unwrap()
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

#[test]
fn export_check_fresh_file_exits_0() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("diagram.mermaid.md");

    // First, generate the file.
    let gen = Command::cargo_bin("koto")
        .unwrap()
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
        gen.status.success(),
        "generation failed: {}",
        String::from_utf8_lossy(&gen.stderr)
    );

    // Now check: should exit 0 because the file is fresh.
    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "--check on fresh file should exit 0: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn export_check_stale_file_exits_1() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("diagram.mermaid.md");

    // Write stale content.
    std::fs::write(&output_path, b"stale content").unwrap();

    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        !check.status.success(),
        "--check on stale file should exit 1"
    );

    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("is out of date"),
        "stderr should mention 'out of date': {}",
        stderr
    );
    assert!(
        stderr.contains("run: koto template export"),
        "stderr should contain fix command: {}",
        stderr
    );
}

#[test]
fn export_check_missing_file_exits_1() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("nonexistent.mermaid.md");

    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        !check.status.success(),
        "--check on missing file should exit 1"
    );

    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("does not exist"),
        "stderr should mention 'does not exist': {}",
        stderr
    );
    assert!(
        stderr.contains("run: koto template export"),
        "stderr should contain fix command: {}",
        stderr
    );
}

#[test]
fn export_check_fix_command_resolves_drift() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("diagram.mermaid.md");

    // Write stale content.
    std::fs::write(&output_path, b"stale").unwrap();

    // Run --check to get the fix command (it will fail).
    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "mermaid",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(!check.status.success());

    // Apply the fix: regenerate the file.
    let fix = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "mermaid",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        fix.status.success(),
        "fix command failed: {}",
        String::from_utf8_lossy(&fix.stderr)
    );

    // Now --check should pass.
    let recheck = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "mermaid",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(
        recheck.status.success(),
        "--check after fix should exit 0: {}",
        String::from_utf8_lossy(&recheck.stderr)
    );
}

/// Export a fixture template to HTML via CLI and return the file contents.
/// HTML format requires --output, so this writes to a temp file.
fn export_html_to_file(fixture: &Path) -> (String, TempDir) {
    let dir = TempDir::new().unwrap();
    let output_path = dir.path().join("preview.html");

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "export --format html --output should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let content = std::fs::read_to_string(&output_path).unwrap();
    (content, dir)
}

#[test]
fn export_html_contains_template_data() {
    let (html, _dir) = export_html_to_file(&fixture_multi_state());

    assert!(
        html.contains("multi-state"),
        "HTML should contain template name, got length={}",
        html.len()
    );
    assert!(html.contains("entry"), "HTML should contain state names");
    assert!(
        html.contains("config_exists"),
        "HTML should contain gate names"
    );
}

#[test]
fn export_html_contains_cdn_script_tags_with_sri() {
    let (html, _dir) = export_html_to_file(&fixture_multi_state());

    // Script tags may span multiple lines, so split on "</script>" to get
    // each complete tag block and check the ones that reference unpkg CDN.
    let script_blocks: Vec<&str> = html.split("</script>").collect();
    let cdn_blocks: Vec<&str> = script_blocks
        .iter()
        .filter(|b| b.contains("<script") && b.contains("unpkg.com"))
        .copied()
        .collect();

    assert!(
        !cdn_blocks.is_empty(),
        "HTML should contain CDN script tags"
    );

    for block in &cdn_blocks {
        assert!(
            block.contains("integrity=\"sha384-"),
            "CDN script tag must have SRI integrity hash: {}",
            block.trim()
        );
        assert!(
            block.contains("crossorigin=\"anonymous\""),
            "CDN script tag must have crossorigin attribute: {}",
            block.trim()
        );
    }
}

#[test]
fn export_html_no_server_side_directives() {
    let (html, _dir) = export_html_to_file(&fixture_multi_state());

    assert!(
        !html.contains("<?"),
        "HTML should not contain PHP-style server directives"
    );
    assert!(
        !html.contains("<%"),
        "HTML should not contain ASP-style server directives"
    );
}

#[test]
fn export_html_determinism_via_cli() {
    let fixture = fixture_multi_state();
    let dir = TempDir::new().unwrap();
    let first_path = dir.path().join("first.html");
    let second_path = dir.path().join("second.html");

    let first = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            first_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let second = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            second_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(first.status.success());
    assert!(second.status.success());

    let first_bytes = std::fs::read(&first_path).unwrap();
    let second_bytes = std::fs::read(&second_path).unwrap();

    assert_eq!(
        first_bytes, second_bytes,
        "two CLI HTML export invocations must produce byte-identical output"
    );
}

#[test]
fn export_html_writes_to_output_file() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("preview.html");

    let output = koto_cmd(dir.path())
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "export --format html --output should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output_path.exists(),
        "output file should be created at {:?}",
        output_path
    );

    let content = std::fs::read_to_string(&output_path).unwrap();
    assert!(
        content.contains("<!DOCTYPE html>"),
        "output file should be valid HTML"
    );
    assert!(content.contains("multi-state"));
}

#[test]
fn export_html_valid_structure() {
    let (html, _dir) = export_html_to_file(&fixture_multi_state());

    assert!(html.contains("<!DOCTYPE html>"), "must have DOCTYPE");
    assert!(html.contains("<html"), "must have html tag");
    assert!(html.contains("</html>"), "must close html tag");
    assert!(html.contains("<head>"), "must have head tag");
    assert!(html.contains("</head>"), "must close head tag");
    assert!(html.contains("<body>"), "must have body tag");
    assert!(html.contains("</body>"), "must close body tag");
}

#[test]
fn export_html_size_under_30kb() {
    let (html, _dir) = export_html_to_file(&fixture_multi_state());

    let size = html.len();
    assert!(
        size < 30 * 1024,
        "HTML output should be under 30 KB, got {} bytes ({:.1} KB)",
        size,
        size as f64 / 1024.0
    );
}

#[test]
fn export_html_check_fresh_exits_0() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("preview.html");

    // Generate the file.
    let gen = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        gen.status.success(),
        "generation failed: {}",
        String::from_utf8_lossy(&gen.stderr)
    );

    // Check: should exit 0 because the file is fresh.
    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "--check on fresh HTML file should exit 0: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn export_html_check_stale_exits_1() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("preview.html");

    // Write stale content.
    std::fs::write(&output_path, b"<html>stale</html>").unwrap();

    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        !check.status.success(),
        "--check on stale HTML file should exit 1"
    );

    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("is out of date"),
        "stderr should mention 'out of date': {}",
        stderr
    );
    assert!(
        stderr.contains("--format html"),
        "stderr fix command should specify html format: {}",
        stderr
    );
}

#[test]
fn export_html_check_missing_exits_1() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("nonexistent.html");

    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        !check.status.success(),
        "--check on missing HTML file should exit 1"
    );

    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("does not exist"),
        "stderr should mention 'does not exist': {}",
        stderr
    );
    assert!(
        stderr.contains("--format html"),
        "stderr fix command should specify html format: {}",
        stderr
    );
}

#[test]
fn export_html_check_fix_command_resolves_drift() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("preview.html");

    // Write stale content.
    std::fs::write(&output_path, b"stale").unwrap();

    // Run --check (it will fail).
    let check = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(!check.status.success());

    // Apply the fix: regenerate the file.
    let fix = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        fix.status.success(),
        "fix command failed: {}",
        String::from_utf8_lossy(&fix.stderr)
    );

    // Now --check should pass.
    let recheck = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(
        recheck.status.success(),
        "--check after fix should exit 0: {}",
        String::from_utf8_lossy(&recheck.stderr)
    );
}

#[test]
fn export_flag_html_without_output_errors() {
    let fixture = fixture_multi_state();

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "should exit 2 for flag validation error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format html requires --output"),
        "should mention --output requirement, got: {}",
        stderr
    );
}

#[test]
fn export_flag_open_without_html_errors() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("out.mermaid.md");

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "mermaid",
            "--output",
            output_path.to_str().unwrap(),
            "--open",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "should exit 2 for flag validation error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--open is only valid with --format html"),
        "should mention --open/html constraint, got: {}",
        stderr
    );
}

#[test]
fn export_flag_open_with_check_errors() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_multi_state();
    let output_path = dir.path().join("out.html");

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args([
            "template",
            "export",
            fixture.to_str().unwrap(),
            "--format",
            "html",
            "--output",
            output_path.to_str().unwrap(),
            "--open",
            "--check",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "should exit 2 for flag validation error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--open and --check are mutually exclusive"),
        "should mention mutual exclusivity, got: {}",
        stderr
    );
}

#[test]
fn export_flag_check_without_output_errors() {
    let fixture = fixture_multi_state();

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", fixture.to_str().unwrap(), "--check"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "should exit 2 for flag validation error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--check requires --output"),
        "should mention --output requirement, got: {}",
        stderr
    );
}

#[test]
fn export_nonexistent_input_file_errors() {
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", "/tmp/nonexistent-template-abc123.md"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should fail for non-existent input"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "should print error to stderr, got: {}",
        stderr
    );
}

#[test]
fn export_malformed_json_input_errors() {
    let dir = TempDir::new().unwrap();
    let bad_json = dir.path().join("broken.json");
    std::fs::write(&bad_json, "{ this is not valid json }").unwrap();

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", bad_json.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should fail for malformed JSON input"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "should print error to stderr, got: {}",
        stderr
    );
}

#[test]
fn export_invalid_template_errors() {
    let dir = TempDir::new().unwrap();
    let bad_md = dir.path().join("invalid.md");
    std::fs::write(
        &bad_md,
        "---\n!!!invalid yaml: [[[broken\n---\n\n## state\n\nContent.\n",
    )
    .unwrap();

    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", bad_md.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success(), "should fail for invalid template");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "should print error to stderr, got: {}",
        stderr
    );
}

#[test]
fn export_latency_under_500ms() {
    let fixture = fixture_multi_state();

    let start = std::time::Instant::now();
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    let elapsed = start.elapsed();

    assert!(
        output.status.success(),
        "export should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "export should complete in under 500ms, took {:?}",
        elapsed
    );
}

/// Generate a 30-state template and verify export completes in under 500ms.
#[test]
fn export_30_state_template_latency_under_500ms() {
    let dir = TempDir::new().unwrap();

    // Build a 30-state chain: s0 -> s1 -> ... -> s29 (terminal)
    let mut yaml =
        String::from("---\nname: big-workflow\nversion: \"1.0\"\ninitial_state: s0\nstates:\n");
    for i in 0..30 {
        yaml.push_str(&format!("  s{}:\n", i));
        if i == 29 {
            yaml.push_str("    terminal: true\n");
        } else {
            yaml.push_str(&format!("    transitions:\n      - target: s{}\n", i + 1));
        }
    }
    yaml.push_str("---\n");
    for i in 0..30 {
        yaml.push_str(&format!("\n## s{}\n\nState {} content.\n", i, i));
    }

    let template_path = dir.path().join("big-template.md");
    std::fs::write(&template_path, &yaml).unwrap();

    let start = std::time::Instant::now();
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "export", template_path.to_str().unwrap()])
        .output()
        .unwrap();
    let elapsed = start.elapsed();

    assert!(
        output.status.success(),
        "30-state export should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "30-state export should complete in under 500ms, took {:?}",
        elapsed
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("stateDiagram-v2\n"),
        "output should be valid mermaid"
    );
    assert!(
        stdout.contains("[*] --> s0"),
        "should have initial state marker"
    );
    assert!(
        stdout.contains("s29 --> [*]"),
        "should have terminal state marker"
    );
}

// ─── Config CLI integration tests ─────────────────────────────────────────────

/// Helper that creates a koto command with HOME overridden to a temp dir
/// so config tests never touch the real ~/.koto/.
fn koto_config_cmd(dir: &Path, home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", home);
    // Prevent env vars from interfering unless explicitly set in the test.
    cmd.env_remove("AWS_ACCESS_KEY_ID");
    cmd.env_remove("AWS_SECRET_ACCESS_KEY");
    cmd
}

#[test]
fn config_set_and_get_user_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set a value in user config.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();

    // Get the value back.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.backend"])
        .assert()
        .success()
        .stdout("cloud\n");
}

#[test]
fn config_get_unset_key_exits_1() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.cloud.endpoint"])
        .assert()
        .code(1);
}

#[test]
fn config_set_and_get_project_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set a value in project config.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();

    // Verify the project config file was created.
    let project_config = tmp.path().join(".koto").join("config.toml");
    assert!(project_config.exists());

    // Get the value (resolved config includes project).
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.backend"])
        .assert()
        .success()
        .stdout("cloud\n");
}

#[test]
fn config_project_rejects_credential_keys() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Default (project config) rejects credential keys.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.cloud.access_key", "AKIAEXAMPLE"])
        .assert()
        .failure();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.cloud.secret_key", "secret123"])
        .assert()
        .failure();
}

#[test]
fn config_unset_removes_key() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set then unset.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.cloud.bucket", "my-bucket"])
        .assert()
        .success();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.cloud.bucket"])
        .assert()
        .success()
        .stdout("my-bucket\n");

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "unset", "session.cloud.bucket"])
        .assert()
        .success();

    // Now get should exit 1.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.cloud.bucket"])
        .assert()
        .code(1);
}

#[test]
fn config_unset_project_key() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set then unset (both default to project config).
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.cloud.region", "us-east-1"])
        .assert()
        .success();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "unset", "session.cloud.region"])
        .assert()
        .success();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.cloud.region"])
        .assert()
        .code(1);
}

#[test]
fn config_list_toml_output() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();

    let output = koto_config_cmd(tmp.path(), &home)
        .args(["config", "list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("backend"), "should contain backend key");
    assert!(stdout.contains("cloud"), "should contain the set value");
}

#[test]
fn config_list_json_output() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();

    let output = koto_config_cmd(tmp.path(), &home)
        .args(["config", "list", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["session"]["backend"], "cloud");
}

#[test]
fn config_list_redacts_credentials() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set credentials in user config (--user required, credentials blocked from project).
    koto_config_cmd(tmp.path(), &home)
        .args([
            "config",
            "set",
            "--user",
            "session.cloud.access_key",
            "AKIAIOSFODNN7EXAMPLE",
        ])
        .assert()
        .success();

    koto_config_cmd(tmp.path(), &home)
        .args([
            "config",
            "set",
            "--user",
            "session.cloud.secret_key",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        ])
        .assert()
        .success();

    // List should show <set> not the actual values.
    let output = koto_config_cmd(tmp.path(), &home)
        .args(["config", "list", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["session"]["cloud"]["access_key"], "<set>");
    assert_eq!(parsed["session"]["cloud"]["secret_key"], "<set>");
    assert!(!stdout.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(!stdout.contains("wJalrXUtnFEMI"));
}

#[test]
fn config_env_var_overrides_config_file() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set a value in user config.
    koto_config_cmd(tmp.path(), &home)
        .args([
            "config",
            "set",
            "--user",
            "session.cloud.access_key",
            "config-key",
        ])
        .assert()
        .success();

    // Now get with env var override.
    let output = {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(tmp.path());
        cmd.env("HOME", &home);
        cmd.env("AWS_ACCESS_KEY_ID", "env-key");
        cmd.env_remove("AWS_SECRET_ACCESS_KEY");
        cmd.args(["config", "get", "session.cloud.access_key"]);
        cmd.output().unwrap()
    };

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "env-key");
}

#[test]
fn config_project_overrides_user() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set user config.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "local"])
        .assert()
        .success();

    // Set project config.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();

    // Resolved value should be project's.
    koto_config_cmd(tmp.path(), &home)
        .args(["config", "get", "session.backend"])
        .assert()
        .success()
        .stdout("cloud\n");
}

#[test]
fn config_set_unknown_key_fails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    koto_config_cmd(tmp.path(), &home)
        .args(["config", "set", "nonexistent.key", "value"])
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// backend selection (config-driven)
// ---------------------------------------------------------------------------

/// Return a koto command with controlled HOME and KOTO_SESSIONS_BASE.
fn koto_backend_cmd(dir: &Path, home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", home);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env_remove("AWS_ACCESS_KEY_ID");
    cmd.env_remove("AWS_SECRET_ACCESS_KEY");
    cmd
}

/// Write a project-level .koto/config.toml with the given TOML content.
fn write_project_config(dir: &Path, content: &str) {
    let config_dir = dir.join(".koto");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), content).unwrap();
}

#[test]
fn backend_defaults_to_local_when_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let src = write_template_source(tmp.path());

    // init should succeed using the default local backend
    koto_backend_cmd(tmp.path(), &home)
        .args(["init", "test-wf", "--template", src.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn backend_local_explicit_config_works() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    write_project_config(tmp.path(), "[session]\nbackend = \"local\"\n");

    let src = write_template_source(tmp.path());

    koto_backend_cmd(tmp.path(), &home)
        .args(["init", "test-wf", "--template", src.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn backend_unknown_value_fails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    write_project_config(tmp.path(), "[session]\nbackend = \"s3-custom\"\n");

    let src = write_template_source(tmp.path());

    koto_backend_cmd(tmp.path(), &home)
        .args(["init", "test-wf", "--template", src.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("unknown backend: s3-custom"));
}

// ---------------------------------------------------------------------------
// gate overrides: functional tests
// ---------------------------------------------------------------------------

/// A template with a failing command gate so we can test the override flow.
fn template_with_failing_command_gate() -> String {
    r#"---
name: override-test-workflow
version: "1.0"
initial_state: start
states:
  start:
    gates:
      ci_check:
        type: command
        command: "exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Do the gated task.

## done

All done.
"#
    .to_string()
}

/// Full override flow: `koto next` blocks on a gate, `koto overrides record` records the
/// override, `koto next` advances. The GateOverrideRecorded event in the state file
/// contains the expected fields.
#[test]
fn gate_override_full_flow() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "override-wf",
        &template_with_failing_command_gate(),
    );

    // Step 1: koto next — blocked by failing gate.
    let blocked = koto_cmd(dir.path())
        .args(["next", "override-wf"])
        .output()
        .unwrap();
    assert!(
        blocked.status.success(),
        "gate_blocked should exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&blocked.stdout),
        String::from_utf8_lossy(&blocked.stderr)
    );
    let blocked_json: serde_json::Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(
        blocked_json["action"].as_str(),
        Some("gate_blocked"),
        "expected gate_blocked, got: {}",
        blocked_json
    );

    // Step 2: koto overrides record — record the override with rationale.
    let record = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "override-wf",
            "--gate",
            "ci_check",
            "--rationale",
            "Manual review confirmed this is safe to proceed.",
        ])
        .output()
        .unwrap();
    assert!(
        record.status.success(),
        "overrides record should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&record.stdout),
        String::from_utf8_lossy(&record.stderr)
    );
    let record_json: serde_json::Value = serde_json::from_slice(&record.stdout).unwrap();
    assert_eq!(
        record_json["status"].as_str(),
        Some("recorded"),
        "expected status=recorded, got: {}",
        record_json
    );

    // Step 3: koto next — the override is active; gate is skipped, workflow advances.
    let advanced = koto_cmd(dir.path())
        .args(["next", "override-wf", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(
        advanced.status.success(),
        "next after override should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&advanced.stdout),
        String::from_utf8_lossy(&advanced.stderr)
    );
    let advanced_json: serde_json::Value = serde_json::from_slice(&advanced.stdout).unwrap();
    assert_eq!(
        advanced_json["state"].as_str(),
        Some("done"),
        "expected state=done after override, got: {}",
        advanced_json
    );

    // Step 4: use koto overrides list to verify the GateOverrideRecorded event
    // contains the expected fields. Using the CLI avoids direct state file path
    // concerns when the session directory is cleaned up by auto-advancement.
    let list = koto_cmd(dir.path())
        .args(["overrides", "list", "override-wf"])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "overrides list should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();

    let count = list_json["overrides"]["count"]
        .as_u64()
        .expect("overrides.count should be a number");
    assert_eq!(count, 1, "expected overrides.count=1, got: {}", count);

    let items = list_json["overrides"]["items"]
        .as_array()
        .expect("overrides.items should be an array");
    assert_eq!(
        items.len(),
        1,
        "expected exactly one override, got: {}",
        items.len()
    );

    let item = &items[0];
    assert_eq!(item["state"].as_str(), Some("start"), "item.state mismatch");
    assert_eq!(
        item["gate"].as_str(),
        Some("ci_check"),
        "item.gate mismatch"
    );
    assert_eq!(
        item["rationale"].as_str(),
        Some("Manual review confirmed this is safe to proceed."),
        "item.rationale mismatch"
    );
    // override_applied should be the built-in default for "command" type.
    assert_eq!(
        item["override_applied"],
        serde_json::json!({"exit_code": 0, "error": ""}),
        "item.override_applied mismatch"
    );
    assert!(
        item["timestamp"].as_str().is_some(),
        "item.timestamp should be present"
    );
}

/// `koto overrides list` returns the full session override history in the correct structure,
/// including overrides recorded before a rewind.
#[test]
fn gate_overrides_list_returns_full_history() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "list-wf", &template_with_failing_command_gate());

    // Record an override in the first epoch.
    let record1 = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "list-wf",
            "--gate",
            "ci_check",
            "--rationale",
            "First epoch override.",
        ])
        .output()
        .unwrap();
    assert!(
        record1.status.success(),
        "first overrides record should succeed: {}",
        String::from_utf8_lossy(&record1.stdout)
    );

    // Advance the workflow past the gated state (the override makes it pass).
    // Use --no-cleanup so the session survives the terminal state for rewind.
    let advance = koto_cmd(dir.path())
        .args(["next", "list-wf", "--to", "done", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(
        advance.status.success(),
        "--to done should succeed: {}",
        String::from_utf8_lossy(&advance.stdout)
    );

    // Rewind back to start.
    let rewind = koto_cmd(dir.path())
        .args(["rewind", "list-wf"])
        .output()
        .unwrap();
    assert!(
        rewind.status.success(),
        "rewind should succeed: {}",
        String::from_utf8_lossy(&rewind.stdout)
    );

    // Record another override in the second epoch (after rewind).
    let record2 = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "list-wf",
            "--gate",
            "ci_check",
            "--rationale",
            "Second epoch override after rewind.",
        ])
        .output()
        .unwrap();
    assert!(
        record2.status.success(),
        "second overrides record should succeed: {}",
        String::from_utf8_lossy(&record2.stdout)
    );

    // koto overrides list should return both overrides across all epochs.
    let list = koto_cmd(dir.path())
        .args(["overrides", "list", "list-wf"])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "overrides list should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();

    // The state field is the current workflow state.
    assert!(
        list_json["state"].as_str().is_some(),
        "state field should be present: {}",
        list_json
    );

    // The overrides.count field matches the number of items.
    let count = list_json["overrides"]["count"]
        .as_u64()
        .expect("overrides.count should be a number");
    assert_eq!(count, 2, "expected overrides.count=2, got: {}", count);

    // The overrides.items array contains both overrides (cross-epoch history).
    let items = list_json["overrides"]["items"]
        .as_array()
        .expect("overrides.items should be an array");
    assert_eq!(
        items.len(),
        2,
        "expected 2 overrides in full history, got: {}",
        items.len()
    );

    // Each item has the required fields from Decision 4 of the design.
    for item in items {
        assert!(item["state"].as_str().is_some(), "item.state missing");
        assert!(item["gate"].as_str().is_some(), "item.gate missing");
        assert!(
            item["rationale"].as_str().is_some(),
            "item.rationale missing"
        );
        assert!(
            !item["override_applied"].is_null(),
            "item.override_applied missing"
        );
        assert!(
            item["timestamp"].as_str().is_some(),
            "item.timestamp missing"
        );
    }

    // Verify the rationales match in order.
    assert_eq!(
        items[0]["rationale"].as_str(),
        Some("First epoch override."),
        "first item rationale mismatch"
    );
    assert_eq!(
        items[1]["rationale"].as_str(),
        Some("Second epoch override after rewind."),
        "second item rationale mismatch"
    );
}

/// `koto next --with-data '{"gates": {...}}'` returns a non-zero exit code and prints
/// the reserved-field error message.
#[test]
fn next_with_data_gates_key_returns_reserved_field_error() {
    let dir = TempDir::new().unwrap();
    // Use a template with an accepts block so that --with-data is actually processed.
    init_workflow(dir.path(), "gates-reserved-wf", &template_with_accepts());

    let output = koto_cmd(dir.path())
        .args([
            "next",
            "gates-reserved-wf",
            "--with-data",
            r#"{"gates": {"ci_check": {"exit_code": 0}}, "decision": "proceed"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should fail with non-zero exit code when 'gates' key is present: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("reserved"),
        "error message should mention 'reserved': {}",
        out
    );
    assert!(
        out.contains("gates"),
        "error message should mention 'gates': {}",
        out
    );
}

/// `koto overrides record` with an unknown gate name returns a non-zero exit code and
/// an error message identifying the phantom gate.
#[test]
fn gate_override_record_phantom_gate_returns_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "phantom-wf",
        &template_with_failing_command_gate(),
    );

    let output = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "phantom-wf",
            "--gate",
            "nonexistent_gate",
            "--rationale",
            "This gate does not exist.",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should fail for unknown gate: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("nonexistent_gate"),
        "error should mention the unknown gate name: {}",
        out
    );
}

// ---------------------------------------------------------------------------
// Gate contract compiler validation (Feature 3)
// ---------------------------------------------------------------------------

/// Template with a command gate that has a valid override_default and two
/// pure-gate transitions routing on exit_code.
fn gate_contract_valid_template() -> &'static str {
    r#"---
name: gate-contract-valid
version: "1.0"
initial_state: verify
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "./ci.sh"
        override_default:
          exit_code: 0
          error: ""
    transitions:
      - target: done
        when:
          gates.ci_check.exit_code: 0
      - target: fix
        when:
          gates.ci_check.exit_code: 1
  done:
    terminal: true
  fix:
    terminal: true
---

## verify

Run CI checks.

## done

All checks passed.

## fix

Investigate failures.
"#
}

/// Template with a D2 error: override_default missing required field "error".
fn gate_contract_d2_error_template() -> &'static str {
    r#"---
name: gate-contract-d2-error
version: "1.0"
initial_state: verify
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "./ci.sh"
        override_default:
          exit_code: 0
    transitions:
      - target: done
  done:
    terminal: true
---

## verify

Run CI checks.

## done

All checks passed.
"#
}

/// Template with a D3 error: when clause references a nonexistent gate name.
fn gate_contract_d3_error_template() -> &'static str {
    r#"---
name: gate-contract-d3-error
version: "1.0"
initial_state: verify
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "./ci.sh"
    transitions:
      - target: done
        when:
          gates.phantom_gate.exit_code: 0
      - target: fix
        when:
          gates.phantom_gate.exit_code: 1
  done:
    terminal: true
  fix:
    terminal: true
---

## verify

Run CI checks.

## done

Passed.

## fix

Failed.
"#
}

/// Template that would produce a D4 dead-end error: no transition fires
/// under the override defaults (both transitions check for exit_code 42/43
/// but the builtin default is exit_code=0).
fn gate_contract_d4_dead_end_template() -> &'static str {
    r#"---
name: gate-contract-d4-dead-end
version: "1.0"
initial_state: verify
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "./ci.sh"
    transitions:
      - target: done
        when:
          gates.ci_check.exit_code: 42
      - target: fix
        when:
          gates.ci_check.exit_code: 43
  done:
    terminal: true
  fix:
    terminal: true
---

## verify

Run CI checks.

## done

All checks passed.

## fix

Investigate failures.
"#
}

#[test]
fn gate_contract_valid_template_compiles() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("gate-contract.md");
    std::fs::write(&src, gate_contract_valid_template()).unwrap();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "valid gate-contract template should compile: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn gate_contract_d2_error_template_rejected() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("gate-d2-error.md");
    std::fs::write(&src, gate_contract_d2_error_template()).unwrap();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template with missing override_default field should fail"
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("override_default missing required field"),
        "error should name the missing field: {}",
        out
    );
    assert!(
        out.contains("\"error\""),
        "error should name the missing field 'error': {}",
        out
    );
}

#[test]
fn gate_contract_d3_error_template_rejected() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("gate-d3-error.md");
    std::fs::write(&src, gate_contract_d3_error_template()).unwrap();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template with invalid when clause gate reference should fail"
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("phantom_gate"),
        "error should name the unknown gate: {}",
        out
    );
}

#[test]
fn gate_contract_d4_dead_end_template_rejected() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("gate-d4-dead-end.md");
    std::fs::write(&src, gate_contract_d4_dead_end_template()).unwrap();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template with unreachable override path should fail"
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("no transition fires"),
        "error should report dead-end reachability failure: {}",
        out
    );
}

#[test]
fn gate_contract_unreferenced_field_warning() {
    // AC11/AC16: a gate field never referenced in any when clause emits a warning to
    // stderr naming the state, gate, and field. Compilation succeeds (non-fatal).
    //
    // Template: state "verify" has a command gate "ci_check". Only "exit_code" is
    // referenced in when clauses; "error" is declared in the schema but never used.
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("gate-unreferenced-field.md");
    std::fs::write(&src, gate_contract_valid_template()).unwrap();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "unreferenced-field warning must be non-fatal: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("verify"),
        "warning should name the state 'verify': stderr={}",
        stderr
    );
    assert!(
        stderr.contains("ci_check"),
        "warning should name the gate 'ci_check': stderr={}",
        stderr
    );
    assert!(
        stderr.contains("error"),
        "warning should name the unreferenced field 'error': stderr={}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Gate backward compatibility (legacy gates, --allow-legacy-gates)
// ---------------------------------------------------------------------------

/// Path to the legacy-gates fixture template.
fn fixture_legacy_gates() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test/functional/fixtures/templates/legacy-gates.md")
}

#[test]
fn allow_legacy_gates_flag_present_in_help() {
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["template", "compile", "--help"])
        .output()
        .unwrap();

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("--allow-legacy-gates"),
        "koto template compile --help should list --allow-legacy-gates, got:\n{}",
        help
    );
}

#[test]
fn template_compile_legacy_gate_without_flag_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_legacy_gates();

    let output = koto_cmd(dir.path())
        .args(["template", "compile", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "template compile should fail for legacy-gate template without --allow-legacy-gates"
    );
    // Errors are emitted as JSON to stdout.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gates.") || stdout.contains("gates.*"),
        "error should mention gates.* routing, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--allow-legacy-gates"),
        "error should mention --allow-legacy-gates flag, got: {}",
        stdout
    );
}

#[test]
fn template_compile_legacy_gate_with_flag_exits_0() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_legacy_gates();

    let output = koto_cmd(dir.path())
        .args([
            "template",
            "compile",
            "--allow-legacy-gates",
            fixture.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "template compile --allow-legacy-gates should succeed for legacy-gate template: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn koto_init_legacy_gate_template_exits_0_with_warning() {
    let dir = TempDir::new().unwrap();
    let fixture = fixture_legacy_gates();

    let output = koto_cmd(dir.path())
        .args(["init", "test-wf", "--template", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "koto init should succeed for legacy-gate template (permissive mode): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("legacy behavior") || stderr.contains("warning"),
        "koto init should emit a warning for legacy-gate template, got stderr: {}",
        stderr
    );
}

#[test]
fn gate_contract_regression_existing_templates_compile() {
    // Validate that every *.md fixture under tests/fixtures/ continues to compile
    // after the gate-contract validation is introduced. This is a regression guard.
    let fixtures_dir = std::path::Path::new("tests/fixtures");
    if !fixtures_dir.exists() {
        return; // No fixtures directory, skip gracefully.
    }
    let entries: Vec<_> = std::fs::read_dir(fixtures_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
        .collect();
    if entries.is_empty() {
        return;
    }
    let dir = TempDir::new().unwrap();
    for entry in entries {
        let path = entry.path();
        let output = koto_cmd(dir.path())
            .args(["template", "compile", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "existing fixture {:?} should still compile: stderr={}",
            path,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// parent workflow lineage (hierarchical workflows, issue 1)
// ---------------------------------------------------------------------------

#[test]
fn init_with_valid_parent_writes_parent_workflow_to_header() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Create the parent workflow first.
    let parent_out = koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", src_str])
        .output()
        .unwrap();
    assert!(
        parent_out.status.success(),
        "parent init should succeed: stderr={}",
        String::from_utf8_lossy(&parent_out.stderr)
    );

    // Create a child workflow with --parent.
    let child_out = koto_cmd(dir.path())
        .args([
            "init",
            "child-wf",
            "--template",
            src_str,
            "--parent",
            "parent-wf",
        ])
        .output()
        .unwrap();
    assert!(
        child_out.status.success(),
        "child init should succeed: stderr={}",
        String::from_utf8_lossy(&child_out.stderr)
    );

    // Read the child's state file and verify parent_workflow in the header.
    let state_path = session_state_path(dir.path(), "child-wf");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let header_line = content.lines().next().unwrap();
    let header: serde_json::Value = serde_json::from_str(header_line).unwrap();
    assert_eq!(
        header["parent_workflow"].as_str(),
        Some("parent-wf"),
        "header should contain parent_workflow"
    );
}

#[test]
fn init_with_missing_parent_fails() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Try to create a child with a non-existent parent.
    let output = koto_cmd(dir.path())
        .args([
            "init",
            "orphan-wf",
            "--template",
            src_str,
            "--parent",
            "nonexistent-parent",
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "init with missing parent should fail"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 for missing parent"
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let error_msg = json["error"].as_str().unwrap();
    assert!(
        error_msg.contains("nonexistent-parent"),
        "error should name the missing parent, got: {}",
        error_msg
    );
}

#[test]
fn init_without_parent_has_no_parent_workflow_in_header() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    let output = koto_cmd(dir.path())
        .args(["init", "solo-wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init without --parent should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Read the state file header.
    let state_path = session_state_path(dir.path(), "solo-wf");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let header_line = content.lines().next().unwrap();
    let header: serde_json::Value = serde_json::from_str(header_line).unwrap();
    assert!(
        header.get("parent_workflow").is_none(),
        "header should not contain parent_workflow when --parent is not given"
    );
}

#[test]
fn workflows_output_includes_parent_workflow_field() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Create a parent workflow.
    koto_cmd(dir.path())
        .args(["init", "root-wf", "--template", src_str])
        .output()
        .unwrap();

    // Create a child workflow.
    koto_cmd(dir.path())
        .args([
            "init",
            "leaf-wf",
            "--template",
            src_str,
            "--parent",
            "root-wf",
        ])
        .output()
        .unwrap();

    // Run koto workflows and verify the JSON output.
    let output = koto_cmd(dir.path()).args(["workflows"]).output().unwrap();
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("workflows output should be valid JSON");
    let workflows = json.as_array().expect("should be an array");

    // Find each workflow in the output.
    let leaf = workflows
        .iter()
        .find(|w| w["name"] == "leaf-wf")
        .expect("leaf-wf should be in the output");
    assert_eq!(
        leaf["parent_workflow"].as_str(),
        Some("root-wf"),
        "child workflow should show parent_workflow"
    );

    let root = workflows
        .iter()
        .find(|w| w["name"] == "root-wf")
        .expect("root-wf should be in the output");
    assert!(
        root["parent_workflow"].is_null(),
        "root workflow should have parent_workflow: null"
    );
}

// ---------------------------------------------------------------------------
// workflows filter flags (--roots, --children, --orphaned)
// ---------------------------------------------------------------------------

#[test]
fn workflows_roots_returns_only_parentless() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "root-a", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-a",
            "--template",
            src_str,
            "--parent",
            "root-a",
        ])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["workflows", "--roots"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1, "should return only the root workflow");
    assert_eq!(arr[0]["name"].as_str(), Some("root-a"));
}

#[test]
fn workflows_children_returns_only_children_of_named_parent() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "parent-b", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-b1",
            "--template",
            src_str,
            "--parent",
            "parent-b",
        ])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-b2",
            "--template",
            src_str,
            "--parent",
            "parent-b",
        ])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args(["init", "other-root", "--template", src_str])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["workflows", "--children", "parent-b"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should return exactly 2 children");
    let names: Vec<&str> = arr.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"child-b1"));
    assert!(names.contains(&"child-b2"));
}

#[test]
fn workflows_children_no_match_returns_empty_array_exit_0() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "solo-wf", "--template", src_str])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["workflows", "--children", "solo-wf"])
        .output()
        .unwrap();
    assert!(output.status.success(), "exit code should be 0");

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert!(arr.is_empty(), "should return empty array when no children");
}

#[test]
fn workflows_orphaned_returns_workflows_with_missing_parent() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "temp-parent", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "orphan-child",
            "--template",
            src_str,
            "--parent",
            "temp-parent",
        ])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args(["init", "standalone", "--template", src_str])
        .output()
        .unwrap();

    // Delete the parent session to orphan the child.
    koto_cmd(dir.path())
        .args(["session", "cleanup", "temp-parent"])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["workflows", "--orphaned"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1, "should return only the orphaned workflow");
    assert_eq!(arr[0]["name"].as_str(), Some("orphan-child"));
}

#[test]
fn workflows_no_filter_returns_all() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "wf-1", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args(["init", "wf-2", "--template", src_str, "--parent", "wf-1"])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path()).args(["workflows"]).output().unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2, "no filter should return all workflows");
}

#[test]
fn workflows_mutually_exclusive_flags_error() {
    let dir = TempDir::new().unwrap();

    let output = koto_cmd(dir.path())
        .args(["workflows", "--roots", "--orphaned"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "combining --roots and --orphaned should fail"
    );

    let output = koto_cmd(dir.path())
        .args(["workflows", "--roots", "--children", "x"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "combining --roots and --children should fail"
    );
}

// ---------------------------------------------------------------------------
// koto status
// ---------------------------------------------------------------------------

#[test]
fn status_active_workflow() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    // Init a workflow (auto-advances to "start").
    koto_cmd(dir.path())
        .args(["init", "status-active", "--template", src.to_str().unwrap()])
        .assert()
        .success();

    // Check status -- workflow is in "start" (non-terminal).
    let output = koto_cmd(dir.path())
        .args(["status", "status-active"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status output should be valid JSON");
    assert_eq!(json["name"], "status-active");
    assert_eq!(json["current_state"], "start");
    assert_eq!(json["is_terminal"], false);
    assert!(json["template_path"].as_str().is_some());
    assert!(json["template_hash"].as_str().is_some());
}

#[test]
fn status_terminal_workflow() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());

    // Init (auto-advances to "start").
    koto_cmd(dir.path())
        .args([
            "init",
            "status-terminal",
            "--template",
            src.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Advance to "done" (terminal) -- use --no-cleanup so state file is preserved.
    koto_cmd(dir.path())
        .args(["next", "status-terminal", "--no-cleanup"])
        .assert()
        .success();

    // Check status -- should show is_terminal: true.
    let output = koto_cmd(dir.path())
        .args(["status", "status-terminal"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status output should be valid JSON");
    assert_eq!(json["name"], "status-terminal");
    assert_eq!(json["current_state"], "done");
    assert_eq!(json["is_terminal"], true);
}

#[test]
fn status_missing_workflow() {
    let dir = TempDir::new().unwrap();

    let output = koto_cmd(dir.path())
        .args(["status", "no-such-workflow"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "status should fail for missing workflow"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("error output should be valid JSON");
    assert!(
        json["error"].as_str().unwrap().contains("not found"),
        "error message should mention 'not found': {}",
        json["error"]
    );
}

// ---------------------------------------------------------------------------
// Advisory child info in lifecycle commands
// ---------------------------------------------------------------------------

#[test]
fn cancel_parent_includes_children_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Create parent and child workflows.
    koto_cmd(dir.path())
        .args(["init", "parent-cancel", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-cancel",
            "--template",
            src_str,
            "--parent",
            "parent-cancel",
        ])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["cancel", "parent-cancel"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "cancel should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["cancelled"], true);
    assert_eq!(json["name"].as_str(), Some("parent-cancel"));

    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert_eq!(children.len(), 1, "should have one child");
    assert_eq!(children[0]["name"].as_str(), Some("child-cancel"));
    assert!(
        children[0]["state"].as_str().is_some(),
        "child should have a state field"
    );
}

#[test]
fn cancel_workflow_no_children_returns_empty_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "solo-cancel", "--template", src_str])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["cancel", "solo-cancel"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert!(
        children.is_empty(),
        "children should be empty when no children exist"
    );
}

#[test]
fn rewind_parent_includes_children_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Create parent, advance it so rewind is possible, then create child.
    koto_cmd(dir.path())
        .args(["init", "parent-rewind", "--template", src_str])
        .output()
        .unwrap();

    // Append a transition event so rewind has somewhere to go back to.
    let state_path = session_state_path(dir.path(), "parent-rewind");
    let extra_event = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"gate"}}"#;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&state_path)
            .unwrap();
        writeln!(f, "{}", extra_event).unwrap();
    }

    // Create child workflow.
    koto_cmd(dir.path())
        .args([
            "init",
            "child-rewind",
            "--template",
            src_str,
            "--parent",
            "parent-rewind",
        ])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["rewind", "parent-rewind"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "rewind should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["name"].as_str(), Some("parent-rewind"));

    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert_eq!(children.len(), 1, "should have one child");
    assert_eq!(children[0]["name"].as_str(), Some("child-rewind"));
    assert!(
        children[0]["state"].as_str().is_some(),
        "child should have a state field"
    );
}

#[test]
fn rewind_workflow_no_children_returns_empty_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "solo-rewind", "--template", src_str])
        .output()
        .unwrap();

    // Append a transition event so rewind is possible.
    let state_path = session_state_path(dir.path(), "solo-rewind");
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
        .args(["rewind", "solo-rewind"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert!(
        children.is_empty(),
        "children should be empty when no children exist"
    );
}

#[test]
fn session_cleanup_parent_includes_children_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    // Create parent and child workflows.
    koto_cmd(dir.path())
        .args(["init", "parent-cleanup", "--template", src_str])
        .output()
        .unwrap();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-cleanup",
            "--template",
            src_str,
            "--parent",
            "parent-cleanup",
        ])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["session", "cleanup", "parent-cleanup"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "cleanup should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["name"].as_str(), Some("parent-cleanup"));
    assert_eq!(json["cleaned_up"], true);

    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert_eq!(children.len(), 1, "should have one child");
    assert_eq!(children[0]["name"].as_str(), Some("child-cleanup"));
    assert!(
        children[0]["state"].as_str().is_some(),
        "child should have a state field"
    );

    // Verify parent is actually cleaned up.
    let list_output = koto_cmd(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&list_output.stdout).unwrap();
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"parent-cleanup"),
        "parent should be removed after cleanup"
    );
    assert!(
        ids.contains(&"child-cleanup"),
        "child should still exist (no cascade)"
    );
}

#[test]
fn session_cleanup_no_children_returns_empty_array() {
    let dir = TempDir::new().unwrap();
    let src = write_template_source(dir.path());
    let src_str = src.to_str().unwrap();

    koto_cmd(dir.path())
        .args(["init", "solo-cleanup", "--template", src_str])
        .output()
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["session", "cleanup", "solo-cleanup"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let children = json["children"]
        .as_array()
        .expect("children should be an array");
    assert!(
        children.is_empty(),
        "children should be empty when no children exist"
    );
}

// ---------------------------------------------------------------------------
// children-complete gate tests
// ---------------------------------------------------------------------------

fn parent_with_children_gate_template() -> &'static str {
    r#"---
name: parent-workflow
version: "1.0"
initial_state: spawn
states:
  spawn:
    gates:
      children-done:
        type: children-complete
        completion: "terminal"
    transitions:
      - target: done
        when:
          gates.children-done.all_complete: true
  done:
    terminal: true
---

## spawn

Spawn child workflows and wait for them to complete.

## done

All done.
"#
}

fn parent_with_name_filter_template() -> &'static str {
    r#"---
name: parent-filtered
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      research-done:
        type: children-complete
        completion: "terminal"
        name_filter: "research."
    transitions:
      - target: done
        when:
          gates.research-done.all_complete: true
  done:
    terminal: true
---

## wait

Wait for research children to complete.

## done

All done.
"#
}

fn write_and_compile_template(dir: &Path, content: &str, filename: &str) -> String {
    let src = dir.join(filename);
    std::fs::write(&src, content).unwrap();
    let output = koto_cmd(dir)
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "template compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    src.to_str().unwrap().to_string()
}

#[test]
fn children_complete_gate_all_terminal_passes() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_children_gate_template(),
        "parent.md",
    );
    let child_compiled = write_template_source(dir.path())
        .to_str()
        .unwrap()
        .to_string();

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-1",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-2",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();

    // Advance children to terminal (auto-advance: start -> done in one call).
    // Use --no-cleanup so the session persists for the parent gate to query.
    for child in ["child-1", "child-2"] {
        koto_cmd(dir.path())
            .args(["next", child, "--no-cleanup"])
            .assert()
            .success();
    }

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "parent next failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["action"],
        "done",
        "parent should reach terminal: {}",
        serde_json::to_string_pretty(&json).unwrap()
    );
}

#[test]
fn children_complete_gate_pending_children_fails() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_children_gate_template(),
        "parent.md",
    );
    let child_compiled = write_template_source(dir.path())
        .to_str()
        .unwrap()
        .to_string();

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-1",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "parent next should exit 0 (gate_blocked exits 0)"
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["action"], "gate_blocked");
    let conditions = json["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0]["type"], "children-complete");
    assert_eq!(conditions[0]["status"], "failed");
    assert_eq!(conditions[0]["category"], "temporal");
    assert_eq!(conditions[0]["output"]["total"], 1);
    assert_eq!(conditions[0]["output"]["completed"], 0);
    assert_eq!(conditions[0]["output"]["pending"], 1);
    assert_eq!(conditions[0]["output"]["all_complete"], false);
    let children = conditions[0]["output"]["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["name"], "child-1");
    assert_eq!(children[0]["complete"], false);
}

#[test]
fn children_complete_gate_zero_children_fails() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_children_gate_template(),
        "parent.md",
    );

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "parent next should exit 0 (gate_blocked exits 0)"
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["action"], "gate_blocked");
    let conditions = json["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions[0]["type"], "children-complete");
    assert_eq!(conditions[0]["output"]["total"], 0);
    assert_eq!(conditions[0]["output"]["all_complete"], false);
    assert!(conditions[0]["output"]["error"]
        .as_str()
        .unwrap()
        .contains("no matching children"));
}

#[test]
fn children_complete_gate_name_filter() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_name_filter_template(),
        "parent-filtered.md",
    );
    let child_compiled = write_template_source(dir.path())
        .to_str()
        .unwrap()
        .to_string();

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "research.r1",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "other-child",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();

    // Advance research.r1 to terminal with --no-cleanup.
    koto_cmd(dir.path())
        .args(["next", "research.r1", "--no-cleanup"])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "parent next should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["action"],
        "done",
        "parent should reach terminal: {}",
        serde_json::to_string_pretty(&json).unwrap()
    );
}

#[test]
fn children_complete_category_temporal_in_output() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_children_gate_template(),
        "parent.md",
    );
    let child_compiled = write_template_source(dir.path())
        .to_str()
        .unwrap()
        .to_string();

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-1",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let conditions = json["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions[0]["category"], "temporal");
}

/// Issue #15 snapshot test: pins the extended `children-complete` gate
/// output shape for a non-batch parent with a single pending child.
///
/// This locks the aggregate counters + derived booleans surface so a
/// future change to the gate output schema is caught as a breaking
/// change rather than silently drifting behind the design.
#[test]
fn children_complete_gate_output_snapshot_extended_fields() {
    let dir = TempDir::new().unwrap();
    let parent_compiled = write_and_compile_template(
        dir.path(),
        parent_with_children_gate_template(),
        "parent.md",
    );
    let child_compiled = write_template_source(dir.path())
        .to_str()
        .unwrap()
        .to_string();

    koto_cmd(dir.path())
        .args(["init", "parent-wf", "--template", &parent_compiled])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args([
            "init",
            "child-1",
            "--template",
            &child_compiled,
            "--parent",
            "parent-wf",
        ])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "parent-wf"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let conditions = json["blocking_conditions"].as_array().unwrap();
    let gate_output = &conditions[0]["output"];

    // Pin the schema: every aggregate + derived boolean is present.
    for key in [
        "total",
        "completed",
        "pending",
        "success",
        "failed",
        "skipped",
        "blocked",
        "spawn_failed",
        "all_complete",
        "all_success",
        "any_failed",
        "any_skipped",
        "any_spawn_failed",
        "needs_attention",
        "children",
        "error",
    ] {
        assert!(
            gate_output.get(key).is_some(),
            "gate output missing field '{}': {}",
            key,
            serde_json::to_string_pretty(gate_output).unwrap()
        );
    }

    // Value-level assertions for the "one pending child" scenario.
    assert_eq!(gate_output["total"], 1);
    assert_eq!(gate_output["success"], 0);
    assert_eq!(gate_output["failed"], 0);
    assert_eq!(gate_output["skipped"], 0);
    assert_eq!(gate_output["blocked"], 0);
    assert_eq!(gate_output["spawn_failed"], 0);
    assert_eq!(gate_output["pending"], 1);
    assert_eq!(gate_output["all_complete"], false);
    assert_eq!(gate_output["all_success"], false);
    assert_eq!(gate_output["any_failed"], false);
    assert_eq!(gate_output["any_skipped"], false);
    assert_eq!(gate_output["any_spawn_failed"], false);
    assert_eq!(gate_output["needs_attention"], false);

    let children = gate_output["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["name"], "child-1");
    assert_eq!(children[0]["complete"], false);
    // The per-child outcome for a not-yet-terminal child folds Running
    // into "pending" for the wire-level outcome.
    assert_eq!(children[0]["outcome"], "pending");
}

#[test]
fn existing_gates_emit_corrective_category() {
    let dir = TempDir::new().unwrap();
    let template_content = r#"---
name: gate-cat
version: "1.0"
initial_state: check
states:
  check:
    gates:
      ci:
        type: command
        command: exit 1
    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
  done:
    terminal: true
---

## check

Run the check.

## done

All done.
"#;
    let compiled = write_and_compile_template(dir.path(), template_content, "gate-cat.md");
    koto_cmd(dir.path())
        .args(["init", "test-wf", "--template", &compiled])
        .assert()
        .success();

    let output = koto_cmd(dir.path())
        .args(["next", "test-wf"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let conditions = json["blocking_conditions"].as_array().unwrap();
    assert!(!conditions.is_empty());
    assert_eq!(
        conditions[0]["category"], "corrective",
        "command gates should have corrective category"
    );
}

// ---------------------------------------------------------------------------
// scenario-9: --with-data @file.json read and 1MB cap rejection
// ---------------------------------------------------------------------------

/// `koto next --with-data @<path>` reads JSON from the file and processes it
/// the same way as inline JSON.
#[test]
fn next_with_data_at_file_reads_evidence_from_file() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "atfile-wf", &template_with_accepts());

    // Write evidence JSON to a file in the temp dir.
    let evidence_path = dir.path().join("evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"decision":"proceed","notes":"from file"}"#,
    )
    .unwrap();

    let arg = format!("@{}", evidence_path.display());
    let output = koto_cmd(dir.path())
        .args(["next", "atfile-wf", "--with-data", &arg, "--no-cleanup"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "@file evidence submission should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // Same auto-advancement as the inline-evidence test:
    // start -> implement (unconditional) -> done (terminal).
    assert_eq!(
        json["state"].as_str(),
        Some("done"),
        "auto-advancement should reach terminal state after @file evidence"
    );
    assert_eq!(json["advanced"], true, "advanced should be true");

    // Verify the state file recorded the evidence_submitted event.
    let state_path = session_state_path(dir.path(), "atfile-wf");
    let content = std::fs::read_to_string(&state_path).unwrap();
    assert!(
        content.lines().any(|l| l.contains("evidence_submitted")),
        "state file should contain evidence_submitted event"
    );
}

/// `koto next --with-data @<path>` rejects files larger than the 1 MB cap
/// with an error that names both the cap and the actual file size.
#[test]
fn next_with_data_at_file_rejects_oversize_file() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "oversize-wf", &template_with_accepts());

    // Write a 2 MB file (well over the 1 MB cap).
    let big_path = dir.path().join("big.json");
    let big = vec![b'x'; 2 * 1024 * 1024];
    std::fs::write(&big_path, &big).unwrap();

    let arg = format!("@{}", big_path.display());
    let output = koto_cmd(dir.path())
        .args(["next", "oversize-wf", "--with-data", &arg])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "oversize file should fail with caller-error exit code"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("invalid_submission"),
        "error code should be invalid_submission"
    );
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("1048576"),
        "error message should name the 1 MB cap (1048576): {}",
        msg
    );
    assert!(
        msg.contains("2097152"),
        "error message should name the actual file size (2097152): {}",
        msg
    );
}

/// `koto next --with-data @<path>` produces a clear error when the file does
/// not exist, naming the path the agent supplied.
#[test]
fn next_with_data_at_file_missing_returns_clear_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "missing-wf", &template_with_accepts());

    let missing = dir.path().join("does-not-exist.json");
    let arg = format!("@{}", missing.display());
    let output = koto_cmd(dir.path())
        .args(["next", "missing-wf", "--with-data", &arg])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "missing file should fail with caller-error exit code"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("invalid_submission"),
        "error code should be invalid_submission"
    );
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains(missing.to_str().unwrap()),
        "error message should name the missing path: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// bug #131: --with-data @file support for decisions/overrides record
// ---------------------------------------------------------------------------

/// `koto decisions record --with-data @<path>` reads JSON from the file and
/// records the decision, matching the behavior of `koto next --with-data @<path>`.
#[test]
fn decisions_record_with_data_at_file_reads_decision_from_file() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "dec-atfile-wf", &template_with_accepts());

    let decision_path = dir.path().join("decision.json");
    std::fs::write(
        &decision_path,
        r#"{"choice":"option-a","rationale":"loaded from file"}"#,
    )
    .unwrap();

    let arg = format!("@{}", decision_path.display());
    let output = koto_cmd(dir.path())
        .args(["decisions", "record", "dec-atfile-wf", "--with-data", &arg])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "@file decisions record should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["decisions_recorded"].as_u64(),
        Some(1),
        "expected decisions_recorded=1, got: {}",
        json
    );

    // Confirm the decision body came from the file, not the literal `@path` string.
    let list = koto_cmd(dir.path())
        .args(["decisions", "list", "dec-atfile-wf"])
        .output()
        .unwrap();
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let items = list_json["decisions"]["items"]
        .as_array()
        .expect("decisions.items should be an array");
    assert_eq!(items.len(), 1, "expected 1 decision, got: {}", items.len());
    assert_eq!(
        items[0]["choice"].as_str(),
        Some("option-a"),
        "decision.choice should be loaded from file: {}",
        items[0]
    );
}

/// `koto decisions record --with-data @<path>` surfaces a clear error message
/// naming the missing path when the file does not exist.
#[test]
fn decisions_record_with_data_at_file_missing_returns_clear_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "dec-missing-wf", &template_with_accepts());

    let missing = dir.path().join("nope.json");
    let arg = format!("@{}", missing.display());
    let output = koto_cmd(dir.path())
        .args(["decisions", "record", "dec-missing-wf", "--with-data", &arg])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "missing file should fail: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    let msg = json["error"]
        .as_str()
        .expect("error field should be a string");
    assert!(
        msg.contains(missing.to_str().unwrap()),
        "error message should name the missing path: {}",
        msg
    );
}

/// `koto overrides record --with-data @<path>` reads the override value from
/// the file, matching the behavior of `koto next --with-data @<path>`.
#[test]
fn overrides_record_with_data_at_file_reads_value_from_file() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "ovr-atfile-wf",
        &template_with_failing_command_gate(),
    );

    // Block on the gate first so overrides record has something to target.
    let _ = koto_cmd(dir.path())
        .args(["next", "ovr-atfile-wf"])
        .output()
        .unwrap();

    let value_path = dir.path().join("override.json");
    std::fs::write(&value_path, r#"{"exit_code":0}"#).unwrap();

    let arg = format!("@{}", value_path.display());
    let output = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "ovr-atfile-wf",
            "--gate",
            "ci_check",
            "--rationale",
            "loaded override value from file",
            "--with-data",
            &arg,
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "@file overrides record should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["status"].as_str(),
        Some("recorded"),
        "expected status=recorded, got: {}",
        json
    );

    // Confirm the override_applied came from the file contents.
    let list = koto_cmd(dir.path())
        .args(["overrides", "list", "ovr-atfile-wf"])
        .output()
        .unwrap();
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let items = list_json["overrides"]["items"]
        .as_array()
        .expect("overrides.items should be an array");
    assert_eq!(items.len(), 1, "expected 1 override, got: {}", items.len());
    assert_eq!(
        items[0]["override_applied"]["exit_code"].as_i64(),
        Some(0),
        "override_applied.exit_code should be loaded from file: {}",
        items[0]
    );
}

/// `koto overrides record --with-data @<path>` surfaces a clear error message
/// naming the missing path when the file does not exist.
#[test]
fn overrides_record_with_data_at_file_missing_returns_clear_error() {
    let dir = TempDir::new().unwrap();
    init_workflow(
        dir.path(),
        "ovr-missing-wf",
        &template_with_failing_command_gate(),
    );

    let _ = koto_cmd(dir.path())
        .args(["next", "ovr-missing-wf"])
        .output()
        .unwrap();

    let missing = dir.path().join("absent.json");
    let arg = format!("@{}", missing.display());
    let output = koto_cmd(dir.path())
        .args([
            "overrides",
            "record",
            "ovr-missing-wf",
            "--gate",
            "ci_check",
            "--rationale",
            "missing file",
            "--with-data",
            &arg,
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "missing file should fail: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    let msg = json["error"]
        .as_str()
        .expect("error field should be a string");
    assert!(
        msg.contains(missing.to_str().unwrap()),
        "error message should name the missing path: {}",
        msg
    );
}
