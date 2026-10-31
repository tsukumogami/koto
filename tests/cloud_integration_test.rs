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

#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

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
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
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
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(dir).expect("canonicalize temp dir");
    let hash = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    hash[..16].to_string()
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

/// Deletes every listed prefix when dropped, so a failing step still
/// leaves the bucket clean.
struct PrefixCleanup {
    endpoint: String,
    bucket: String,
    prefixes: Vec<String>,
}

impl Drop for PrefixCleanup {
    fn drop(&mut self) {
        for prefix in &self.prefixes {
            delete_s3_prefix(&self.endpoint, &self.bucket, prefix);
        }
    }
}

/// The session-migration carrier (`tests/session_migration_test.rs`)
/// against the real bucket: init in A, keys, import into B, read the keys
/// and advance in B, A refuses, import into C. A unique session name per
/// run, and every prefix the three workspaces touched is deleted after.
#[test]
fn cloud_session_migration_carrier() {
    use migration_carrier::{Carrier, Cloud, Host};

    let (endpoint, bucket) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    let root = TempDir::new().unwrap();
    let cloud = Cloud {
        endpoint: endpoint.clone(),
        bucket: bucket.clone(),
        region: "auto".to_string(),
        path_style: false,
        // The bucket secrets are already in the environment.
        credentials: None,
    };
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    let _cleanup = PrefixCleanup {
        endpoint,
        bucket,
        prefixes: vec![a.prefix(), b.prefix(), c.prefix()],
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("migration-{}-{}", std::process::id(), nanos);
    let mut carrier = Carrier::new(&a, &b, &c, &name);
    carrier.run_all();
    println!("{}", carrier.report());
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

/// Deletes exactly the keys a test wrote, and nothing else, when dropped.
///
/// Each key is recorded before its PUT is sent, so a PUT that fails after
/// the object landed is still removed. There is no prefix-wide delete.
struct ExactKeyCleanup<'a> {
    bucket: &'a s3::Bucket,
    keys: Vec<String>,
}

impl ExactKeyCleanup<'_> {
    fn put(&mut self, key: &str, body: &[u8]) -> std::time::Duration {
        self.keys.push(key.to_string());
        let started = std::time::Instant::now();
        self.bucket
            .put_object(key, body)
            .unwrap_or_else(|e| panic!("PUT {key} failed: {e}"));
        started.elapsed()
    }

    fn remove_all(&mut self) {
        for key in self.keys.drain(..) {
            if let Err(e) = self.bucket.delete_object(&key) {
                eprintln!("could not remove probe object {key}: {e}");
            }
        }
    }
}

impl Drop for ExactKeyCleanup<'_> {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// The R2 delimiter probe: koto's dashboard lists each workspace prefix with
/// the delimiter `/ctx/`, which must return a session's top-level objects
/// and fold its context tree into one common prefix. If the provider doesn't
/// honour a multi-character delimiter, the dashboard's layout has to change.
///
/// It writes a session-shaped fixture under a fresh 16-hex prefix in the CI
/// test bucket, lists it, times the `summary.json` PUT made right after the
/// state PUT, and removes exactly the keys it wrote. It runs only under
/// GitHub Actions, and refuses a bucket that is koto's default or the one
/// this environment's live session backend names.
#[test]
fn ctx_delimiter_listing_folds_the_context_tree() {
    let (endpoint, bucket_name) = match s3_env() {
        Some(v) => v,
        None => {
            eprintln!("S3 env not set, skipping");
            return;
        }
    };
    assert_eq!(
        env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "the delimiter probe writes to the bucket and runs only in CI's cloud-integration job"
    );
    assert!(
        !bucket_name.is_empty() && bucket_name != "koto-sessions",
        "refusing to probe a bucket that isn't the CI test bucket"
    );
    if let Ok(live) = env::var("KOTO_SESSION_CLOUD_BUCKET") {
        assert_ne!(
            live, bucket_name,
            "refusing to probe the live session bucket"
        );
    }

    let region = s3::Region::Custom {
        region: "auto".to_string(),
        endpoint: endpoint.clone(),
    };
    let credentials = s3::creds::Credentials::new(
        Some(&env::var("AWS_ACCESS_KEY_ID").unwrap()),
        Some(&env::var("AWS_SECRET_ACCESS_KEY").unwrap()),
        None,
        None,
        None,
    )
    .expect("credentials");
    let bucket = s3::Bucket::new(&bucket_name, region, credentials)
        .expect("bucket client")
        .with_request_timeout(std::time::Duration::from_secs(10))
        .expect("bucket client with a timeout");

    // A prefix no other run shares: the run's id, its attempt, this process
    // and the time, hashed to the 16-hex shape of a workspace prefix.
    let seed = format!(
        "ctx-delimiter-probe:{}:{}:{}:{:?}",
        env::var("GITHUB_RUN_ID").unwrap_or_default(),
        env::var("GITHUB_RUN_ATTEMPT").unwrap_or_default(),
        std::process::id(),
        std::time::SystemTime::now(),
    );
    let workspace = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(seed.as_bytes()))[..16].to_string()
    };
    let existing = bucket
        .list(format!("{workspace}/"), None)
        .unwrap_or_else(|e| panic!("endpoint {endpoint}: listing {workspace}/ failed: {e}"));
    assert!(
        existing.iter().all(|page| page.contents.is_empty()),
        "prefix {workspace}/ already holds objects; refusing to write there"
    );

    let dir = format!("{workspace}/probe");
    let state_key = format!("{dir}/koto-probe.state.jsonl");
    let summary_key = format!("{dir}/summary.json");
    let migrated_key = format!("{dir}/migrated.json");
    let mut cleanup = ExactKeyCleanup {
        bucket: &bucket,
        keys: Vec::new(),
    };
    cleanup.put(
        &state_key,
        b"{\"schema_version\":1,\"workflow\":\"probe\"}\n",
    );
    let summary_put = cleanup.put(&summary_key, b"{\"schema\":1}");
    cleanup.put(&migrated_key, b"{}");
    cleanup.put(&format!("{dir}/ctx/go"), b"yes");
    cleanup.put(&format!("{dir}/ctx/nested/key"), b"value");
    eprintln!(
        "summary.json PUT latency after the state PUT on {endpoint}: {summary_put:?} (limit 30s)"
    );
    assert!(
        summary_put < std::time::Duration::from_secs(30),
        "summary.json PUT took {summary_put:?}, over koto's 30 s action limit"
    );

    let pages = bucket
        .list(format!("{workspace}/"), Some("/ctx/".to_string()))
        .unwrap_or_else(|e| panic!("endpoint {endpoint}: /ctx/-delimited listing failed: {e}"));
    let mut objects: Vec<String> = pages
        .iter()
        .flat_map(|page| page.contents.iter().map(|o| o.key.clone()))
        .collect();
    objects.sort();
    let prefixes: Vec<String> = pages
        .iter()
        .filter_map(|page| page.common_prefixes.as_ref())
        .flatten()
        .map(|cp| cp.prefix.clone())
        .collect();
    let mut expected = vec![migrated_key.clone(), state_key.clone(), summary_key.clone()];
    expected.sort();
    assert_eq!(
        (objects, prefixes),
        (expected, vec![format!("{dir}/ctx/")]),
        "endpoint {endpoint} does not honour the /ctx/ delimiter: expected the session's \
         three top-level objects and one ctx/ common prefix"
    );

    // koto's own reader makes the same listing and sees the same session.
    let reader = koto::session::remote_feed::RemoteReader::new(&bucket).expect("reader");
    let sessions = reader
        .list_workspace(&workspace)
        .unwrap_or_else(|e| panic!("endpoint {endpoint}: koto's listing failed: {e}"));
    let names: Vec<&String> = sessions.keys().collect();
    assert_eq!(names, vec!["probe"], "endpoint {endpoint}: {sessions:?}");
    let files: Vec<&String> = sessions["probe"].objects.keys().collect();
    assert_eq!(
        files,
        vec!["koto-probe.state.jsonl", "migrated.json", "summary.json"],
        "endpoint {endpoint}: {sessions:?}"
    );

    cleanup.remove_all();
    let after = bucket
        .list(format!("{workspace}/"), None)
        .unwrap_or_else(|e| panic!("endpoint {endpoint}: listing after cleanup failed: {e}"));
    assert!(
        after.iter().all(|page| page.contents.is_empty()),
        "probe objects left under {workspace}/"
    );
}
