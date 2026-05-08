use assert_cmd::Command;
use assert_fs::TempDir;
use koto::engine::persistence::append_event;
use koto::engine::types::EventPayload;
use std::path::Path;

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd.env("HOME", dir);
    // Redirect the template cache so koto init writes inside the temp dir,
    // preventing cache leaks to XDG_CACHE_HOME on developer machines or CI.
    cmd.env("XDG_CACHE_HOME", dir.join("cache"));
    cmd
}

fn write_template(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(format!("{}.md", name));
    std::fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_string()
}

/// Template that auto-advances to terminal "done" state.
fn terminal_template() -> &'static str {
    r#"---
name: terminal-workflow
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

Do the work.

## done

Finished.
"#
}

/// Template that keeps the session in "gather" after koto init + one koto next.
///
/// No auto-transition fires without evidence (`result: ok`), so the session
/// stays in "gather" with status_bucket "running". Tests depend on this invariant.
fn running_template() -> &'static str {
    r#"---
name: running-workflow
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      result:
        type: string
        required: true
    transitions:
      - target: done
        when:
          result: ok
  done:
    terminal: true
---

## gather

Gather information.

## done

Finished.
"#
}

#[test]
fn dashboard_help_exits_zero() {
    let dir = TempDir::new().unwrap();
    koto_cmd(dir.path())
        .args(["dashboard", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("dashboard"));
}

#[test]
fn dashboard_once_empty_dir_exits_zero_with_no_output() {
    let dir = TempDir::new().unwrap();
    koto_cmd(dir.path())
        .args(["dashboard", "--once"])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn dashboard_once_produces_tab_separated_output_with_running_and_terminal() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();

    // Create terminal session: init only, then append the final transition via
    // the typed API. koto next deletes the session directory when it becomes
    // terminal, so we cannot use it to drive the session to "done".
    let term_src = write_template(dir.path(), "term-tmpl", terminal_template());
    koto_cmd(dir.path())
        .args(["init", "term-wf", "--template", &term_src])
        .assert()
        .success();

    let state_path = dir
        .path()
        .join("sessions")
        .join("term-wf")
        .join("koto-term-wf.state.jsonl");
    append_event(
        &state_path,
        &EventPayload::Transitioned {
            from: Some("start".to_string()),
            to: "done".to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
        },
        "2026-01-01T00:00:02Z",
    )
    .unwrap();

    // Create running session: stays in "gather" (conditional transition requires evidence).
    let run_src = write_template(dir.path(), "run-tmpl", running_template());
    koto_cmd(dir.path())
        .args(["init", "run-wf", "--template", &run_src])
        .assert()
        .success();
    koto_cmd(dir.path())
        .args(["next", "run-wf"])
        .assert()
        .success();

    // Run --once and capture output.
    let output = koto_cmd(dir.path())
        .args(["dashboard", "--once"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "dashboard --once should exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.trim_end_matches('\n').lines().collect();

    assert_eq!(lines.len(), 2, "expected 2 session lines, got: {:?}", lines);

    // Every line must have exactly 4 tab-separated fields.
    for line in &lines {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            4,
            "line '{}' should have 4 tab-separated fields",
            line
        );
        let valid_buckets = ["running", "done", "failed", "blocked", "unknown"];
        assert!(
            valid_buckets.contains(&fields[3]),
            "status_bucket '{}' is not a valid value",
            fields[3]
        );
    }

    // Verify running session.
    let run_line = lines
        .iter()
        .find(|l| l.starts_with("run-wf\t"))
        .expect("run-wf should appear in output");
    let run_fields: Vec<&str> = run_line.split('\t').collect();
    assert_eq!(
        run_fields[1], "gather",
        "running session should be in 'gather' state"
    );
    assert_eq!(
        run_fields[3], "running",
        "running session should have bucket 'running'"
    );

    // Verify terminal session.
    let term_line = lines
        .iter()
        .find(|l| l.starts_with("term-wf\t"))
        .expect("term-wf should appear in output");
    let term_fields: Vec<&str> = term_line.split('\t').collect();
    assert_eq!(
        term_fields[1], "done",
        "terminal session should be in 'done' state"
    );
    assert_eq!(
        term_fields[3], "done",
        "terminal session should have bucket 'done'"
    );
}
