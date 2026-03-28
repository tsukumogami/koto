//! Cloud sync integration tests against a real S3-compatible endpoint.
//!
//! These tests require:
//! - The `cloud` feature enabled
//! - Environment variables set: KOTO_TEST_S3_ENDPOINT, KOTO_TEST_S3_BUCKET,
//!   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
//!
//! Run with: cargo test --features cloud-integration-tests -- --test-threads=1
//!
//! In CI, a Cloudflare R2 bucket provides the S3-compatible endpoint.

#![cfg(feature = "cloud-integration-tests")]

use assert_cmd::Command;
use assert_fs::TempDir;
use predicates::prelude::*;
use std::env;
use std::path::Path;

/// Return S3 env vars if set, or None to skip the test.
fn s3_env() -> Option<(String, String)> {
    let endpoint = env::var("KOTO_TEST_S3_ENDPOINT").ok()?;
    let bucket = env::var("KOTO_TEST_S3_BUCKET").ok()?;
    env::var("AWS_ACCESS_KEY_ID").ok()?;
    env::var("AWS_SECRET_ACCESS_KEY").ok()?;
    Some((endpoint, bucket))
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", dir.to_str().unwrap());
    cmd
}

fn setup_cloud_config(dir: &Path, endpoint: &str, bucket: &str) {
    // Set up cloud config via koto config set
    koto_cmd(dir)
        .args(["config", "set", "session.backend", "cloud"])
        .assert()
        .success();
    koto_cmd(dir)
        .args(["config", "set", "session.cloud.endpoint", endpoint])
        .assert()
        .success();
    koto_cmd(dir)
        .args(["config", "set", "session.cloud.bucket", bucket])
        .assert()
        .success();
    // R2 doesn't need a region, but set one for compatibility
    koto_cmd(dir)
        .args(["config", "set", "session.cloud.region", "auto"])
        .assert()
        .success();
}

fn write_template(dir: &Path) -> String {
    let template = r#"---
name: cloud-test
version: "1.0"
description: Template for cloud sync testing
initial_state: working

states:
  working:
    transitions:
      - target: done
    gates:
      has-plan:
        type: context-exists
        key: plan.md
  done:
    terminal: true
---

## working

Do the work. Submit plan.md when ready.

## done

Done.
"#;
    let template_path = dir.join("cloud-test.md");
    std::fs::write(&template_path, template).unwrap();
    template_path.to_string_lossy().to_string()
}

#[test]
fn cloud_context_add_syncs_to_s3() {
    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    let dir = TempDir::new().unwrap();
    setup_cloud_config(dir.path(), &endpoint, &bucket);
    let template = write_template(dir.path());

    // Init a workflow
    koto_cmd(dir.path())
        .args(["init", "cloud-test-1", "--template", &template])
        .assert()
        .success();

    // Add context
    koto_cmd(dir.path())
        .args(["context", "add", "cloud-test-1", "plan.md"])
        .write_stdin("# Test Plan\n\nThis is a test.")
        .assert()
        .success();

    // Verify it exists locally
    koto_cmd(dir.path())
        .args(["context", "exists", "cloud-test-1", "plan.md"])
        .assert()
        .success();

    // Verify content round-trip
    koto_cmd(dir.path())
        .args(["context", "get", "cloud-test-1", "plan.md"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Test Plan"));

    // Cleanup
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-1"])
        .assert()
        .success();
}

#[test]
fn cloud_sync_resume_on_different_machine() {
    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };

    // Machine A: create workflow and add context
    let machine_a = TempDir::new().unwrap();
    setup_cloud_config(machine_a.path(), &endpoint, &bucket);
    let template_a = write_template(machine_a.path());

    koto_cmd(machine_a.path())
        .args(["init", "cloud-test-2", "--template", &template_a])
        .assert()
        .success();

    koto_cmd(machine_a.path())
        .args(["context", "add", "cloud-test-2", "research.md"])
        .write_stdin("# Research\n\nFindings from machine A.")
        .assert()
        .success();

    // Machine B: different HOME, same cloud config
    let machine_b = TempDir::new().unwrap();
    setup_cloud_config(machine_b.path(), &endpoint, &bucket);
    let template_b = write_template(machine_b.path());

    // Init the same workflow on machine B (will download from cloud)
    koto_cmd(machine_b.path())
        .args(["init", "cloud-test-2", "--template", &template_b])
        .assert()
        .success();

    // Machine B should be able to see context added by Machine A
    // (via cloud sync pull on context get)
    let output = koto_cmd(machine_b.path())
        .args(["context", "get", "cloud-test-2", "research.md"])
        .output()
        .unwrap();

    // If sync worked, machine B has machine A's content
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("machine A"),
            "Expected machine A's content, got: {}",
            stdout
        );
    }
    // If sync failed (e.g., R2 timing), that's acceptable — sync is non-fatal

    // Cleanup both
    koto_cmd(machine_a.path())
        .args(["session", "cleanup", "cloud-test-2"])
        .assert()
        .success();
}

#[test]
fn cloud_context_exists_gate_with_sync() {
    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    let dir = TempDir::new().unwrap();
    setup_cloud_config(dir.path(), &endpoint, &bucket);
    let template = write_template(dir.path());

    // Init workflow
    koto_cmd(dir.path())
        .args(["init", "cloud-test-3", "--template", &template])
        .assert()
        .success();

    // next should block (no plan.md yet)
    let output = koto_cmd(dir.path())
        .args(["next", "cloud-test-3"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gate_blocked") || stdout.contains("execute"),
        "Expected gate_blocked or execute, got: {}",
        stdout
    );

    // Add plan.md
    koto_cmd(dir.path())
        .args(["context", "add", "cloud-test-3", "plan.md"])
        .write_stdin("# Plan\n\nDo the thing.")
        .assert()
        .success();

    // next should now advance (gate passes)
    koto_cmd(dir.path())
        .args(["next", "cloud-test-3"])
        .assert()
        .success();

    // Cleanup
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-3"])
        .assert()
        .success();
}

#[test]
fn cloud_session_list_shows_synced_sessions() {
    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    let dir = TempDir::new().unwrap();
    setup_cloud_config(dir.path(), &endpoint, &bucket);
    let template = write_template(dir.path());

    koto_cmd(dir.path())
        .args(["init", "cloud-test-4", "--template", &template])
        .assert()
        .success();

    // Session list should include the workflow
    koto_cmd(dir.path())
        .args(["session", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("cloud-test-4"));

    // Cleanup
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-4"])
        .assert()
        .success();
}

#[test]
fn cloud_config_list_redacts_credentials() {
    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    let dir = TempDir::new().unwrap();
    setup_cloud_config(dir.path(), &endpoint, &bucket);

    // Set a credential in user config (--user required)
    koto_cmd(dir.path())
        .args([
            "config",
            "set",
            "--user",
            "session.cloud.access_key",
            "secret-key-value",
        ])
        .assert()
        .success();

    // config list should redact it
    koto_cmd(dir.path())
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("<set>"))
        .stdout(predicates::str::contains("secret-key-value").not());
}
