//! koto#193: `koto session recover` is the way out of the migration
//! quarantine.
//!
//! The migration moves a session whose name is already taken at the flat
//! level into `sessions/.migration-conflicts/<repo-id>/<name>/`, where it is
//! preserved and unreachable. These cases drive the real binary through the
//! whole arc -- collide, migrate, report, restore -- and check the thing the
//! reporter actually lost: whether `koto session list` can see the session
//! again afterwards.
//!
//! `KOTO_SESSIONS_BASE` is deliberately not set. That override bypasses
//! `migrate_if_needed`, and the migration is what creates the condition.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

/// A `koto` command rooted at a throwaway HOME, so the real local backend
/// and its migration run against a controlled `~/.koto/sessions/`.
fn koto_at_home(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env("HOME", home);
    cmd.env_remove("KOTO_SESSIONS_BASE");
    cmd
}

fn sessions_dir(home: &Path) -> PathBuf {
    home.join(".koto").join("sessions")
}

/// A header line good enough for `list()` to read.
fn header_line(workflow: &str) -> String {
    format!(
        r#"{{"schema_version":1,"workflow":"{workflow}","template_hash":"testhash","created_at":"2026-01-01T00:00:00Z"}}
"#
    )
}

/// Write a session directory with a readable state file at `dir/<name>/`.
fn write_session(dir: &Path, name: &str) {
    let session = dir.join(name);
    fs::create_dir_all(&session).unwrap();
    fs::write(
        session.join(format!("koto-{name}.state.jsonl")),
        header_line(name),
    )
    .unwrap();
}

/// Stage the defect: `name` exists at the flat level and in `repo_id`'s
/// old-layout directory, so the migration can only keep one of them.
fn stage_collision(home: &Path, repo_id: &str, name: &str) {
    let sessions = sessions_dir(home);
    write_session(&sessions, name);
    write_session(&sessions.join(repo_id), name);
}

fn run_json(cmd: &mut Command) -> serde_json::Value {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout must be JSON, got {:?}: {e}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn listed_ids(home: &Path) -> Vec<String> {
    let json = run_json(koto_at_home(home).args(["session", "list"]));
    json.as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn recover_reports_before_it_moves_anything() {
    let home = tempfile::tempdir().unwrap();
    let repo_id = "0123456789abcdef";
    stage_collision(home.path(), repo_id, "deploy");

    let report = run_json(koto_at_home(home.path()).args(["session", "recover"]));

    assert_eq!(report["applied"], false);
    assert_eq!(report["summary"]["total"], 1);
    assert_eq!(report["summary"]["recovered"], 0);
    assert_eq!(report["sessions"][0]["session"], "deploy");
    assert_eq!(report["sessions"][0]["repo_id"], repo_id);
    assert_eq!(report["sessions"][0]["status"], "pending");
    assert_eq!(
        report["sessions"][0]["recovered_as"],
        "r0123456789abcdef-deploy"
    );
    assert!(
        report["hint"].is_string(),
        "a report should say what to run next"
    );

    // Reporting is read-only: still quarantined, still not listed.
    assert!(sessions_dir(home.path())
        .join(".migration-conflicts")
        .join(repo_id)
        .join("deploy")
        .exists());
    assert_eq!(listed_ids(home.path()), vec!["deploy".to_string()]);
}

/// The defect itself. Before recovery existed, this session had no path back
/// into the list on any number of invocations.
#[test]
fn a_stranded_session_is_listable_again_after_apply() {
    let home = tempfile::tempdir().unwrap();
    let repo_id = "0123456789abcdef";
    stage_collision(home.path(), repo_id, "deploy");

    // The migration runs on the first session-touching command and strands it.
    assert_eq!(listed_ids(home.path()), vec!["deploy".to_string()]);

    let report = run_json(koto_at_home(home.path()).args(["session", "recover", "--apply"]));
    assert_eq!(report["applied"], true);
    assert_eq!(report["summary"]["recovered"], 1);
    assert_eq!(report["summary"]["failed"], 0);
    assert_eq!(report["sessions"][0]["status"], "recovered");
    assert_eq!(report["sessions"][0]["header_rewritten"], true);

    assert_eq!(
        listed_ids(home.path()),
        vec!["deploy".to_string(), "r0123456789abcdef-deploy".to_string()],
        "the stranded session must be back in the list, alongside the one that kept the name"
    );
}

/// The reporter had a thousand of these. One command has to clear the lot.
#[test]
fn apply_recovers_every_stranded_session_in_one_run() {
    let home = tempfile::tempdir().unwrap();
    let repo_a = "0123456789abcdef";
    let repo_b = "fedcba9876543210";
    for name in ["alpha", "beta", "gamma"] {
        stage_collision(home.path(), repo_a, name);
        write_session(&sessions_dir(home.path()).join(repo_b), name);
    }

    let report = run_json(koto_at_home(home.path()).args(["session", "recover", "--apply"]));
    assert_eq!(report["summary"]["recovered"], 6);
    assert_eq!(report["summary"]["failed"], 0);

    let mut expected: Vec<String> = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        expected.push(name.to_string());
        expected.push(format!("r{repo_a}-{name}"));
        expected.push(format!("r{repo_b}-{name}"));
    }
    expected.sort();
    assert_eq!(listed_ids(home.path()), expected);
}

#[test]
fn a_second_apply_has_nothing_left_to_do() {
    let home = tempfile::tempdir().unwrap();
    stage_collision(home.path(), "0123456789abcdef", "deploy");

    run_json(koto_at_home(home.path()).args(["session", "recover", "--apply"]));
    let before = listed_ids(home.path());

    let again = run_json(koto_at_home(home.path()).args(["session", "recover", "--apply"]));
    assert_eq!(again["summary"]["total"], 0);
    assert_eq!(again["summary"]["recovered"], 0);
    assert_eq!(listed_ids(home.path()), before);
}

#[test]
fn session_narrows_the_set_and_names_what_it_did_not_find() {
    let home = tempfile::tempdir().unwrap();
    let repo_id = "0123456789abcdef";
    stage_collision(home.path(), repo_id, "wanted");
    stage_collision(home.path(), repo_id, "not-wanted");

    let report = run_json(koto_at_home(home.path()).args([
        "session",
        "recover",
        "--apply",
        "--session",
        "wanted",
        "--session",
        "never-existed",
    ]));

    assert_eq!(report["summary"]["recovered"], 1);
    assert_eq!(report["unmatched"], serde_json::json!(["never-existed"]));

    let ids = listed_ids(home.path());
    assert!(ids.contains(&format!("r{repo_id}-wanted")));
    assert!(
        !ids.contains(&format!("r{repo_id}-not-wanted")),
        "an unselected session must stay where it is: {ids:?}"
    );
    // And it is still recoverable later.
    assert!(sessions_dir(home.path())
        .join(".migration-conflicts")
        .join(repo_id)
        .join("not-wanted")
        .exists());
}

/// Recovery moves data, so a name that is already taken must survive
/// untouched.
#[test]
fn recovery_does_not_write_over_an_existing_session() {
    let home = tempfile::tempdir().unwrap();
    let repo_id = "0123456789abcdef";
    stage_collision(home.path(), repo_id, "deploy");
    // Something is already sitting on the name recovery would choose.
    let occupied = sessions_dir(home.path()).join(format!("r{repo_id}-deploy"));
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("mine.txt"), b"mine").unwrap();

    let report = run_json(koto_at_home(home.path()).args(["session", "recover", "--apply"]));
    assert_eq!(
        report["sessions"][0]["recovered_as"],
        format!("r{repo_id}-deploy-2")
    );
    assert_eq!(fs::read(occupied.join("mine.txt")).unwrap(), b"mine");
}

#[test]
fn recover_on_a_clean_install_reports_an_empty_quarantine() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(sessions_dir(home.path())).unwrap();

    let report = run_json(koto_at_home(home.path()).args(["session", "recover"]));
    assert_eq!(report["summary"]["total"], 0);
    assert_eq!(report["sessions"], serde_json::json!([]));
    assert!(
        report["hint"].is_null(),
        "nothing to hint at when nothing is stranded"
    );
}
