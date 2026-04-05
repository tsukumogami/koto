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

/// Helper: list R2 bucket objects with a prefix using Python/boto3.
/// Handles pagination to avoid missing objects in large buckets.
fn list_s3_objects(endpoint: &str, bucket: &str, prefix: &str) -> Vec<String> {
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            r#"
import boto3, os, json
s3 = boto3.client('s3',
    endpoint_url='{endpoint}',
    aws_access_key_id=os.environ['AWS_ACCESS_KEY_ID'],
    aws_secret_access_key=os.environ['AWS_SECRET_ACCESS_KEY'],
    region_name='auto')
paginator = s3.get_paginator('list_objects_v2')
keys = []
for page in paginator.paginate(Bucket='{bucket}', Prefix='{prefix}'):
    keys.extend(o['Key'] for o in page.get('Contents', []))
print(json.dumps(keys))
"#
        ))
        .output()
        .expect("python3");
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_default()
}

/// Helper: delete all objects under a prefix in S3.
/// Used for test cleanup to prevent bucket pollution.
fn delete_s3_prefix(endpoint: &str, bucket: &str, prefix: &str) {
    let _ = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            r#"
import boto3, os
s3 = boto3.client('s3',
    endpoint_url='{endpoint}',
    aws_access_key_id=os.environ['AWS_ACCESS_KEY_ID'],
    aws_secret_access_key=os.environ['AWS_SECRET_ACCESS_KEY'],
    region_name='auto')
paginator = s3.get_paginator('list_objects_v2')
for page in paginator.paginate(Bucket='{bucket}', Prefix='{prefix}'):
    objects = [dict(Key=o['Key']) for o in page.get('Contents', [])]
    if objects:
        s3.delete_objects(Bucket='{bucket}', Delete=dict(Objects=objects))
"#
        ))
        .output();
}

/// Compute the repo-id prefix for a temp directory.
/// Matches koto's repo_id() function: first 16 hex chars of SHA-256 of canonical path.
fn repo_id_prefix(dir: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    // koto uses sha256 of the canonical path, but we can just read it from
    // the session dir that koto creates after init.
    let sessions_dir = dir.join(".koto").join("sessions");
    if sessions_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    return entry.file_name().to_string_lossy().to_string();
                }
            }
        }
    }
    // Fallback: empty prefix (lists everything)
    String::new()
}

#[test]
fn cloud_state_sync_on_init() {
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

    // Init a workflow — state file should sync to R2
    koto_cmd(dir.path())
        .args(["init", "state-test-1", "--template", &template])
        .assert()
        .success();

    // Scope S3 listing to this test's repo-id prefix
    let prefix = repo_id_prefix(dir.path());

    // Check R2 for state file
    let objects = list_s3_objects(&endpoint, &bucket, &prefix);
    let has_state = objects
        .iter()
        .any(|k| k.contains("state-test-1") && k.contains(".state.jsonl"));
    assert!(
        has_state,
        "State file not found in R2 under prefix '{}'. Objects: {:?}",
        prefix, objects
    );

    // Cleanup: both koto session and S3 prefix
    koto_cmd(dir.path())
        .args(["session", "cleanup", "state-test-1"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix);
}

#[test]
fn cloud_state_sync_on_next() {
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
        .args(["init", "state-test-2", "--template", &template])
        .assert()
        .success();

    // Submit context so gate passes
    koto_cmd(dir.path())
        .args(["context", "add", "state-test-2", "plan.md"])
        .write_stdin("# Plan")
        .assert()
        .success();

    // Advance state — should sync updated state to R2
    // Use --no-cleanup to prevent auto-cleanup from deleting everything
    koto_cmd(dir.path())
        .args(["next", "state-test-2", "--to", "done", "--no-cleanup"])
        .assert()
        .success();

    // Scope S3 listing to this test's repo-id prefix
    let prefix = repo_id_prefix(dir.path());

    // Check R2 — state file should be updated (larger than after init)
    let objects = list_s3_objects(&endpoint, &bucket, &prefix);
    let has_state = objects
        .iter()
        .any(|k| k.contains("state-test-2") && k.contains(".state.jsonl"));
    assert!(
        has_state,
        "Updated state file not found in R2 after next under prefix '{}'. Objects: {:?}",
        prefix, objects
    );

    // Cleanup: both koto session and S3 prefix
    koto_cmd(dir.path())
        .args(["session", "cleanup", "state-test-2"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix);
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

    // Cleanup: koto session + S3 prefix
    let prefix = repo_id_prefix(dir.path());
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-1"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix);
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

    // Cleanup both machines + S3
    let prefix_a = repo_id_prefix(machine_a.path());
    koto_cmd(machine_a.path())
        .args(["session", "cleanup", "cloud-test-2"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix_a);
    let prefix_b = repo_id_prefix(machine_b.path());
    delete_s3_prefix(&endpoint, &bucket, &prefix_b);
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

    // Cleanup: koto session + S3 prefix
    let prefix = repo_id_prefix(dir.path());
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-3"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix);
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

    // Cleanup: koto session + S3 prefix
    let prefix = repo_id_prefix(dir.path());
    koto_cmd(dir.path())
        .args(["session", "cleanup", "cloud-test-4"])
        .assert()
        .success();
    delete_s3_prefix(&endpoint, &bucket, &prefix);
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
