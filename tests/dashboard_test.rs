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

/// Create a session directory whose state file has an unparseable header.
/// The dashboard read path must skip it silently (counting it as unreadable)
/// rather than emitting a per-session warning.
fn write_corrupt_session(sessions_dir: &Path, id: &str) {
    let session_dir = sessions_dir.join(id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join(format!("koto-{}.state.jsonl", id)),
        "not a valid header line\n",
    )
    .unwrap();
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

/// Regression guard: a sessions base containing corrupt sessions must NOT
/// produce per-session `warning: skipping session ...` lines on stderr. The
/// dashboard read path (`list()`) is invoked on every TUI refresh tick, so a
/// per-session eprintln there floods the alternate screen. Unreadable sessions
/// are surfaced exactly once via the `note: N unreadable session(s) skipped`
/// line computed from the tally. This test exercises the shared `--once` read
/// path (same `list()` call the TUI refresh uses) at the binary's stderr
/// boundary.
#[test]
fn dashboard_once_corrupt_sessions_emit_count_note_not_per_session_warnings() {
    let dir = TempDir::new().unwrap();
    let sessions_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    // Two sessions whose headers do not parse.
    write_corrupt_session(&sessions_dir, "corrupt-alpha");
    write_corrupt_session(&sessions_dir, "corrupt-beta");

    let output = koto_cmd(dir.path())
        .args(["dashboard", "--once"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "dashboard --once should exit 0 even with corrupt sessions; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8(output.stderr).unwrap();

    // The flood: zero per-session warnings, regardless of corrupt count.
    assert_eq!(
        stderr.matches("warning: skipping session").count(),
        0,
        "per-session corrupt warnings must not be printed on the read path; \
         stderr was:\n{stderr}"
    );

    // The correct surfacing: exactly one tally note covering both sessions.
    let note_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("unreadable session(s) skipped"))
        .collect();
    assert_eq!(
        note_lines.len(),
        1,
        "exactly one tally note expected; stderr was:\n{stderr}"
    );
    assert_eq!(
        note_lines[0].trim(),
        "note: 2 unreadable session(s) skipped",
        "tally must report both corrupt sessions"
    );

    // The corrupt sessions are excluded from the tab-separated stdout contract.
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("corrupt-alpha") && !stdout.contains("corrupt-beta"),
        "corrupt sessions must not appear in stdout; stdout was:\n{stdout}"
    );
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

    // Run --once (default view) and capture output. The default view excludes
    // the receded set, so the terminal (done) session must NOT appear; only the
    // live running session is shown.
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

    assert_eq!(
        lines.len(),
        1,
        "default view excludes the receded (done) session; got: {:?}",
        lines
    );

    // Each line has exactly eight tab-separated fields: the original six plus
    // the appended idle (7) and liveness (8) columns.
    for line in &lines {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            8,
            "line '{}' should have 8 tab-separated fields",
            line
        );
        let valid_buckets = ["running", "done", "failed", "blocked", "unknown"];
        assert!(
            valid_buckets.contains(&fields[3]),
            "status_bucket '{}' is not a valid value",
            fields[3]
        );
        let valid_liveness = [
            "needs-you-blocked",
            "needs-you-failed",
            "needs-you-stalled",
            "active",
            "idle",
            "pending",
            "done",
        ];
        assert!(
            valid_liveness.contains(&fields[7]),
            "liveness token '{}' is not a valid value",
            fields[7]
        );
    }

    // Verify the running session: first six columns unchanged, appended idle +
    // liveness present.
    let run_line = lines
        .iter()
        .find(|l| l.starts_with("run-wf\t"))
        .expect("run-wf should appear in the default view");
    let run_fields: Vec<&str> = run_line.split('\t').collect();
    assert_eq!(
        run_fields[1], "gather",
        "running session should be in 'gather' state"
    );
    assert_eq!(
        run_fields[3], "running",
        "running session should have bucket 'running'"
    );
    assert_eq!(
        run_fields[7], "active",
        "freshly-advanced running session should be 'active'"
    );

    // The terminal session is receded and excluded by default.
    assert!(
        !lines.iter().any(|l| l.starts_with("term-wf\t")),
        "terminal session must be excluded from the default view"
    );

    // --all includes the receded set: now both sessions appear, with the
    // terminal one carrying liveness 'done'.
    let all_output = koto_cmd(dir.path())
        .args(["dashboard", "--once", "--all"])
        .output()
        .unwrap();
    assert!(all_output.status.success());
    let all_stdout = String::from_utf8(all_output.stdout).unwrap();
    let all_lines: Vec<&str> = all_stdout.trim_end_matches('\n').lines().collect();
    assert_eq!(
        all_lines.len(),
        2,
        "--all should include both sessions; got: {:?}",
        all_lines
    );
    let term_line = all_lines
        .iter()
        .find(|l| l.starts_with("term-wf\t"))
        .expect("term-wf should appear under --all");
    let term_fields: Vec<&str> = term_line.split('\t').collect();
    assert_eq!(
        term_fields[1], "done",
        "terminal session should be in 'done' state"
    );
    assert_eq!(
        term_fields[3], "done",
        "terminal session should have bucket 'done'"
    );
    assert_eq!(
        term_fields[7], "done",
        "terminal session should have liveness 'done'"
    );
}
