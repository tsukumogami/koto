//! Issue 185: migration/discovery diagnostics must land on stderr so an
//! automated caller can read a command's result from stdout without
//! filtering. This guards the stream routing against a regression where
//! an `eprintln!` diagnostic is changed to `println!`.

use assert_cmd::Command;
use std::fs;
use std::path::Path;

/// Build a `koto` command rooted at a throwaway HOME so the real local
/// backend (and its old-layout migration) runs against a controlled
/// `~/.koto/sessions/`. `KOTO_SESSIONS_BASE` is deliberately NOT set:
/// that override bypasses the migration path this test exercises.
fn koto_at_home(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env("HOME", home);
    cmd.env_remove("KOTO_SESSIONS_BASE");
    cmd
}

#[test]
fn migration_skipped_notice_goes_to_stderr_not_stdout() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".koto").join("sessions");

    // Old per-repo layout: `sessions/<16-hex>/<session>/`. Placing a
    // session there whose name already exists at the flat level forces
    // the "migration skipped: session already exists" notice.
    let repo_id = "0123456789abcdef";
    let session = "collide-session";
    let old = sessions.join(repo_id).join(session);
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("koto-collide-session.state.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(sessions.join(session)).unwrap();

    let output = koto_at_home(home.path())
        .args(["session", "list"])
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    // The diagnostic must be on stderr, never on stdout.
    assert!(
        stderr.contains("migration skipped"),
        "expected migration notice on stderr, got stderr: {stderr:?}"
    );
    assert!(
        !stdout.contains("migration skipped"),
        "migration notice leaked onto stdout: {stdout:?}"
    );

    // stdout must carry only the command result: a single JSON value.
    let trimmed = stdout.trim();
    serde_json::from_str::<serde_json::Value>(trimmed)
        .unwrap_or_else(|e| panic!("stdout must be a single JSON value, got {stdout:?}: {e}"));
}
