//! Integration tests for Issues 4 and 5 of DESIGN-orphaned-session-detection.md:
//! `koto status`, `koto session list`, and `koto init`'s collision
//! pre-check surfacing a stale `template_source_dir`.
//!
//! Exercises the real `koto` binary end to end (mirrors
//! `tests/batch_rewind_test.rs`'s `run_koto` pattern) against a
//! `LocalBackend` session whose source template directory has been
//! deleted, a `LocalBackend` session whose directory still exists, and a
//! `Backend::Cloud` session configured against an RFC 5737 (TEST-NET-1)
//! non-routable endpoint -- the same technique
//! `tests/batch_session_resolve_test.rs` and `src/session/cloud.rs`'s unit
//! tests use to exercise `CloudBackend` without a live S3 bucket: every S3
//! call fails fast and non-fatally, so `CloudBackend::list()` /
//! `read_header()` fall back to local-only behavior while `Backend::is_cloud()`
//! still reports `true`, which is exactly the discriminant these two CLI
//! surfaces gate wording on.
//!
//! The `koto init` tests at the bottom of this file
//! (`init_collision_*`) are the repro for tsukumogami/koto#189: a
//! same-named `koto init` after the recorded `template_source_dir` was
//! deleted used to produce a generic, undiagnosable "already exists"
//! error. They exercise the pre-check collision path (the one
//! deterministically reachable from a single-process CLI invocation --
//! the `SpawnErrorKind::Collision` handler only fires on a genuine
//! atomic-rename race that this test harness cannot force
//! in-process; it is covered instead by
//! `src/cli/mod.rs`'s `stale_template_source_dir_clause` unit tests,
//! since both collision paths call that exact same function).

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn run_koto(dir: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    // `koto session list` pretty-prints (multi-line) JSON; most other
    // commands print single-line JSON. Try the whole trimmed stdout first
    // (handles both single-line and pretty-printed output), then fall
    // back to just the last non-empty line for commands that emit
    // incidental log lines before their final JSON line.
    let trimmed = stdout.trim();
    let json: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
    });
    (output.status.success(), json, stderr)
}

const SIMPLE_TEMPLATE: &str = r#"---
name: simple-workflow
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

Start.

## done

Done.
"#;

/// Write the template into a dedicated subdirectory of `dir` (distinct
/// from `dir` itself, which also holds sessions storage) so the caller
/// can later delete just the template's source directory.
fn write_template_in_subdir(dir: &Path, subdir: &str) -> PathBuf {
    let src_dir = dir.join(subdir);
    std::fs::create_dir_all(&src_dir).unwrap();
    let src = src_dir.join("simple.md");
    std::fs::write(&src, SIMPLE_TEMPLATE).unwrap();
    src
}

/// Configure the process to use `Backend::Cloud` pointed at a
/// non-routable endpoint (RFC 5737 TEST-NET-1), so every S3 call fails
/// fast and non-fatally -- no live bucket required. Mirrors
/// `test_cloud_backend` in `src/session/cloud.rs` and
/// `tests/batch_session_resolve_test.rs`, but driven entirely through
/// `koto config set` since this test spawns the real binary.
fn setup_unroutable_cloud_config(dir: &Path) {
    for (key, value) in [
        ("session.backend", "cloud"),
        ("session.cloud.endpoint", "http://192.0.2.1:19000"),
        ("session.cloud.bucket", "test-bucket"),
        ("session.cloud.region", "us-east-1"),
    ] {
        let (ok, _, stderr) = run_koto(dir, &["config", "set", key, value]);
        assert!(ok, "config set {key} failed: {stderr}");
    }
    // Credentials are rejected from project config; store them as user
    // config instead (mirrors `cloud_config_list_redacts_credentials` in
    // tests/cloud_integration_test.rs). Dummy values are fine since the
    // endpoint is non-routable and every S3 call fails before auth
    // matters.
    for (key, value) in [
        ("session.cloud.access_key", "test-key"),
        ("session.cloud.secret_key", "test-secret"),
    ] {
        let (ok, _, stderr) = run_koto(dir, &["config", "set", "--user", key, value]);
        assert!(ok, "config set --user {key} failed: {stderr}");
    }
}

const DIRECT_NOTE: &str = "template source directory no longer exists";
const CLOUD_NOTE_FRAGMENT: &str = "synced from another machine";

// -----------------------------------------------------------------------
// koto status
// -----------------------------------------------------------------------

#[test]
fn status_omits_stale_key_when_template_source_dir_still_exists() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    let (ok, json, stderr) = run_koto(tmp.path(), &["status", "sess"]);
    assert!(ok, "status failed: {stderr}");
    assert!(
        json.get("stale_template_source_dir").is_none(),
        "stale_template_source_dir must be absent when the directory still exists, got: {json}"
    );
}

#[test]
fn status_surfaces_stale_key_with_direct_wording_for_local_backend() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    // Tear down the working tree the template was loaded from.
    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(tmp.path(), &["status", "sess"]);
    assert!(ok, "status failed: {stderr}");
    let stale = json
        .get("stale_template_source_dir")
        .unwrap_or_else(|| panic!("stale_template_source_dir must be present, got: {json}"));
    assert_eq!(stale["note"], serde_json::json!(DIRECT_NOTE));
    assert_eq!(
        stale["path"],
        serde_json::json!(src_dir.canonicalize().unwrap_or(src_dir))
    );
}

#[test]
fn status_surfaces_stale_key_with_softened_wording_for_cloud_backend() {
    let tmp = TempDir::new().unwrap();
    setup_unroutable_cloud_config(tmp.path());
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(tmp.path(), &["status", "sess"]);
    assert!(ok, "status failed: {stderr}");
    let stale = json
        .get("stale_template_source_dir")
        .unwrap_or_else(|| panic!("stale_template_source_dir must be present, got: {json}"));
    let note = stale["note"].as_str().unwrap();
    assert!(
        note.contains(CLOUD_NOTE_FRAGMENT),
        "cloud wording must be softened, got: {note}"
    );
    assert_ne!(note, DIRECT_NOTE);
}

// -----------------------------------------------------------------------
// koto session list
// -----------------------------------------------------------------------

#[test]
fn list_omits_note_when_template_source_dir_still_exists() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    let (ok, json, stderr) = run_koto(tmp.path(), &["session", "list"]);
    assert!(ok, "session list failed: {stderr}");
    let rows = json.as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["id"] == "sess")
        .expect("session 'sess' must be present");
    assert_eq!(row["template_source_status"]["exists"], true);
    assert!(
        row["template_source_status"].get("note").is_none(),
        "note must be absent when the directory exists, got: {row}"
    );
}

#[test]
fn list_surfaces_direct_wording_for_local_backend() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(tmp.path(), &["session", "list"]);
    assert!(ok, "session list failed: {stderr}");
    let rows = json.as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["id"] == "sess")
        .expect("session 'sess' must be present");
    assert_eq!(row["template_source_status"]["exists"], false);
    assert_eq!(
        row["template_source_status"]["note"],
        serde_json::json!(DIRECT_NOTE)
    );
}

#[test]
fn list_surfaces_softened_wording_for_cloud_backend() {
    let tmp = TempDir::new().unwrap();
    setup_unroutable_cloud_config(tmp.path());
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(tmp.path(), &["session", "list"]);
    assert!(ok, "session list failed: {stderr}");
    let rows = json.as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["id"] == "sess")
        .expect("session 'sess' must be present");
    assert_eq!(row["template_source_status"]["exists"], false);
    let note = row["template_source_status"]["note"].as_str().unwrap();
    assert!(
        note.contains(CLOUD_NOTE_FRAGMENT),
        "cloud wording must be softened, got: {note}"
    );
    assert_ne!(note, DIRECT_NOTE);
}

// -----------------------------------------------------------------------
// koto init collision (Issue 5 / tsukumogami/koto#189 repro)
// -----------------------------------------------------------------------

#[test]
fn init_collision_omits_clause_when_template_source_dir_still_exists() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    // Re-init the same name while the template source directory still
    // exists: the base "already exists" message must be unchanged, with
    // no staleness clause appended.
    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(!ok, "second init on the same name must fail");
    let msg = json["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error field should be a string, got: {json} ({stderr})"));
    assert!(
        msg.starts_with("workflow 'sess' already exists; run `koto session cleanup sess`"),
        "base message must be unchanged: {msg}"
    );
    assert!(
        !msg.contains(DIRECT_NOTE),
        "no staleness clause should be present while the directory exists: {msg}"
    );
}

/// Repro for tsukumogami/koto#189: a session whose `template_source_dir`
/// working tree was torn down (a reaped ephemeral sandbox, a removed git
/// worktree, a container teardown) used to be indistinguishable, from
/// `koto init`'s perspective, from a real concurrent session collision --
/// the error was a generic "already exists" with no way to tell the two
/// apart. This test confirms the error now identifies the recorded,
/// missing `template_source_dir` explicitly.
#[test]
fn init_collision_diagnoses_stale_template_source_dir() {
    let tmp = TempDir::new().unwrap();
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    // Tear down the working tree the template was loaded from -- the
    // exact condition tsukumogami/koto#189 reports.
    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(!ok, "second init on the same name must fail");
    let msg = json["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error field should be a string, got: {json} ({stderr})"));

    // Base message (cleanup guidance) is untouched.
    assert!(
        msg.starts_with("workflow 'sess' already exists; run `koto session cleanup sess`"),
        "base message must be unchanged: {msg}"
    );
    // The bug's fix: the error now names the staleness condition and the
    // recorded (now-missing) path, rather than leaving it undiagnosable.
    assert!(
        msg.contains(DIRECT_NOTE),
        "error must diagnose the stale template_source_dir, got: {msg}"
    );
    let expected_path = src_dir.canonicalize().unwrap_or(src_dir);
    assert!(
        msg.contains(&expected_path.display().to_string()),
        "error must identify the missing recorded path {expected_path:?}, got: {msg}"
    );
}

#[test]
fn init_collision_softened_wording_for_cloud_backend() {
    let tmp = TempDir::new().unwrap();
    setup_unroutable_cloud_config(tmp.path());
    let src = write_template_in_subdir(tmp.path(), "srctpl");
    let src_dir = src.parent().unwrap().to_path_buf();

    let (ok, _, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(ok, "init failed: {stderr}");

    std::fs::remove_dir_all(&src_dir).unwrap();

    let (ok, json, stderr) = run_koto(
        tmp.path(),
        &["init", "sess", "--template", src.to_str().unwrap()],
    );
    assert!(!ok, "second init on the same name must fail");
    let msg = json["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error field should be a string, got: {json} ({stderr})"));
    assert!(
        msg.contains(CLOUD_NOTE_FRAGMENT),
        "cloud wording must be softened, got: {msg}"
    );
    assert!(
        !msg.contains(DIRECT_NOTE),
        "direct (non-cloud) wording must not appear, got: {msg}"
    );
}
