//! `koto session rebind` on the cloud backend, end to end against the
//! in-process S3-compatible endpoint (`tests/support/fake_s3.rs`).
//!
//! koto#310: rebind appended its event through the backend, which pushed
//! the state file, then rewrote the header on the local file only. The next
//! command pulled the remote copy back over the local one, so the header
//! named the old anchor again and `koto next` from the new directory refused
//! with `execution_anchor_mismatch` while rebind had reported success.
//!
//! The remote prefix is derived from the directory a command runs in, so the
//! bug shows when rebind runs from the directory `koto next` will run from;
//! that is how this test runs it.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use std::path::Path;

use fake_s3::FakeS3;
use migration_carrier::{prefix_of, Cloud, Host, Run};

const BUCKET: &str = "koto-rebind";

/// Writes `marker.txt` in the directory the tick runs in, so the test can
/// see where the action ran.
const MARKER_TEMPLATE: &str = r#"---
name: marker
version: "1.0"
initial_state: mark
states:
  mark:
    default_action:
      command: "printf ok > marker.txt"
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## mark

Write the marker.

## done

Done.
"#;

/// Run koto as `host` (its `HOME`, cache and credentials) from `dir`.
fn koto_in(host: &Host, dir: &Path, args: &[&str]) -> Run {
    let output = host.cmd().current_dir(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
    });
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        json,
    }
}

/// The header line of a remote state file.
fn remote_header(s3: &FakeS3, key: &str) -> serde_json::Value {
    let bytes = s3
        .object(key)
        .unwrap_or_else(|| panic!("no remote object at {key}; keys: {:?}", s3.keys()));
    let text = String::from_utf8(bytes).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

#[test]
fn a_cloud_rebind_reaches_the_remote_and_next_runs_from_the_new_directory() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let host = Host::new(
        root.path(),
        "host",
        &Cloud {
            endpoint: s3.endpoint(),
            bucket: s3.bucket().to_string(),
            region: "us-east-1".to_string(),
            path_style: true,
            credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
        },
    );
    // The session starts in the host's workspace; the checkout then moves
    // to a sibling directory carrying the same project config.
    let old = host.ws.clone();
    let moved = root.path().join("host").join("moved");
    std::fs::create_dir_all(moved.join(".koto")).unwrap();
    std::fs::copy(
        old.join(".koto").join("config.toml"),
        moved.join(".koto").join("config.toml"),
    )
    .unwrap();
    let moved = std::fs::canonicalize(&moved).unwrap();
    let template = root.path().join("marker.md");
    std::fs::write(&template, MARKER_TEMPLATE).unwrap();

    let name = "wf";
    let run = koto_in(
        &host,
        &old,
        &["init", name, "--template", template.to_str().unwrap()],
    );
    assert!(run.ok(), "init: {}", run.describe());

    let run = koto_in(
        &host,
        &moved,
        &["session", "rebind", name, "--to", moved.to_str().unwrap()],
    );
    assert!(run.ok(), "rebind: {}", run.describe());
    assert_eq!(run.json["rebound"], serde_json::json!(true));

    // The state file under the prefix the moved directory's commands use
    // names the new anchor.
    let key = format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&moved));
    assert_eq!(
        remote_header(&s3, &key)["execution_dir"],
        serde_json::json!(moved.to_str().unwrap()),
        "the remote header must name the new anchor"
    );

    // The tick from the new directory pulls that copy and runs there.
    let run = koto_in(&host, &moved, &["next", name]);
    assert!(
        run.ok(),
        "koto next from the new directory: {}",
        run.describe()
    );
    assert!(
        run.json["error"].is_null(),
        "koto next refused: {}",
        run.describe()
    );
    assert!(
        moved.join("marker.txt").exists(),
        "the action must run at the new anchor"
    );
    assert!(!old.join("marker.txt").exists());
    assert_eq!(
        host.header(name)["execution_dir"],
        serde_json::json!(moved.to_str().unwrap())
    );
}
