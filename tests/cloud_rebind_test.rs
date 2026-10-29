//! `koto session rebind` on the cloud backend, end to end against the
//! in-process S3-compatible endpoint (`tests/support/fake_s3.rs`).
//!
//! koto#310: rebind appended its event through the backend, which pushed
//! the state file, then rewrote the header on the local file only. The next
//! command pulled the remote copy back over the local one, so the header
//! named the old anchor again and `koto next` from the new directory refused
//! with `execution_anchor_mismatch` while rebind had reported success.
//!
//! The bug showed when rebind ran from the directory `koto next` will run
//! from; that is how this test runs it. The session's copy stays under the
//! prefix it was created under whichever directory either runs in.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
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

    // The session's own copy, under the prefix it was created under, names
    // the new anchor; nothing lands under the moved directory's prefix.
    let key = format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&old));
    assert_eq!(
        remote_header(&s3, &key)["execution_dir"],
        serde_json::json!(moved.to_str().unwrap()),
        "the remote header must name the new anchor"
    );
    let moved_prefix = format!("{}/", prefix_of(&moved));
    assert!(
        s3.keys().iter().all(|k| !k.starts_with(&moved_prefix)),
        "{:?}",
        s3.keys()
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

/// Drop `origin.store.prefix` from the header of `name`'s local state file
/// and the bucket's copy at `key`, as a koto from before the session
/// recorded its prefix left it: the session's home then follows its
/// execution anchor.
fn drop_recorded_prefix(host: &Host, s3: &FakeS3, name: &str, key: &str) {
    let path = host.state_path(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    let store = header["origin"]["store"].as_object_mut().unwrap();
    assert!(
        store.remove("prefix").is_some(),
        "a new session records its prefix"
    );
    let text = format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest);
    std::fs::write(&path, &text).unwrap();
    s3.put(key, text.as_bytes());
}

/// `koto context add <name> <key> --from-file` as `host` from `dir`.
fn add_in(host: &Host, dir: &Path, name: &str, key: &str, content: &str) -> Run {
    let file = dir.join("value.txt");
    std::fs::write(&file, content).unwrap();
    koto_in(
        host,
        dir,
        &[
            "context",
            "add",
            name,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ],
    )
}

/// The `context_added` events naming `key` with writer `sync`, from the
/// state log and the host-local read spool.
fn sync_writes(host: &Host, name: &str, key: &str) -> Vec<String> {
    let spool = std::fs::read_to_string(host.session_dir(name).join("context-reads.jsonl"))
        .unwrap_or_default();
    host.state_lines(name)
        .into_iter()
        .chain(spool.lines().map(str::to_string))
        .filter(|line| {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                return false;
            };
            event["type"] == "context_added"
                && event["payload"]["key"] == key
                && event["payload"]["writer"] == "sync"
        })
        .collect()
}

/// A cloud host on `s3` laid out under `root`, and a sibling checkout
/// directory carrying the same project config.
fn host_and_sibling(s3: &FakeS3, root: &Path) -> (Host, std::path::PathBuf) {
    let host = Host::new(
        root,
        "host",
        &Cloud {
            endpoint: s3.endpoint(),
            bucket: s3.bucket().to_string(),
            region: "us-east-1".to_string(),
            path_style: true,
            credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
        },
    );
    let sibling = root.join("host").join("moved");
    std::fs::create_dir_all(sibling.join(".koto")).unwrap();
    std::fs::copy(
        host.ws.join(".koto").join("config.toml"),
        sibling.join(".koto").join("config.toml"),
    )
    .unwrap();
    let sibling = std::fs::canonicalize(&sibling).unwrap();
    (host, sibling)
}

/// koto#520: a session from before koto recorded its prefix takes its home
/// from its anchor, so a rebind moves the home to the new directory's
/// prefix. When that prefix still holds an older copy of a key the session
/// rewrote since, the first read after the rebind pulled that copy over
/// the local one, and the next write put the stale value back as current.
/// The local copy is newer: the read keeps it and the new home gets it.
#[test]
fn a_read_after_rebind_keeps_a_local_key_newer_than_the_new_homes_copy() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let (host, moved) = host_and_sibling(&s3, root.path());
    let old = host.ws.clone();
    let template = root.path().join("marker.md");
    std::fs::write(&template, MARKER_TEMPLATE).unwrap();

    let name = "wf";
    let key = "record/run";
    let run = koto_in(
        &host,
        &old,
        &["init", name, "--template", template.to_str().unwrap()],
    );
    assert!(run.ok(), "init: {}", run.describe());
    let old_state = format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&old));
    drop_recorded_prefix(&host, &s3, name, &old_state);

    // The copy the moved directory's prefix still holds, as a sync from
    // there days earlier left it.
    let run = add_in(&host, &old, name, key, "stale");
    assert!(run.ok(), "add stale: {}", run.describe());
    let old_ctx = format!("{}/{name}/ctx/", prefix_of(&old));
    let moved_ctx = format!("{}/{name}/ctx/", prefix_of(&moved));
    for k in s3.keys_under(&old_ctx) {
        let copy = format!("{moved_ctx}{}", &k[old_ctx.len()..]);
        s3.put(&copy, &s3.object(&k).unwrap());
    }

    // The session rewrites the key; that copy never reaches the moved
    // directory's prefix.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let run = add_in(&host, &old, name, key, "current");
    assert!(run.ok(), "add current: {}", run.describe());

    let run = koto_in(
        &host,
        &moved,
        &["session", "rebind", name, "--to", moved.to_str().unwrap()],
    );
    assert!(run.ok(), "rebind: {}", run.describe());

    let run = koto_in(&host, &moved, &["context", "get", name, key]);
    assert!(run.ok(), "context get: {}", run.describe());
    assert_eq!(
        run.stdout,
        "current",
        "the read after the rebind must return the local value, not an \
         older bucket copy from the new home: {}",
        run.describe()
    );
    assert!(
        sync_writes(&host, name, key).is_empty(),
        "no sync write may replace the newer local key: {:?}",
        sync_writes(&host, name, key)
    );

    // The next write leaves it in place, and the new home holds the
    // current value.
    let run = add_in(&host, &moved, name, "record/meta", "meta");
    assert!(run.ok(), "add meta: {}", run.describe());
    let run = koto_in(&host, &moved, &["context", "get", name, key]);
    assert_eq!(run.stdout, "current", "{}", run.describe());
    assert_eq!(
        s3.object(&format!("{moved_ctx}{key}")).as_deref(),
        Some(&b"current"[..]),
        "the kept key must reach the new home"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&format!("{moved_ctx}manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["keys"][key]["hash"],
        host.manifest(name)["keys"][key]["hash"],
        "the new home's manifest must name the kept copy"
    );
}

/// A key whose bucket copy really is newer than this host's (another host
/// wrote it after this host last synced) is still pulled, and the pull is
/// logged as a `sync` write.
#[test]
fn a_read_still_pulls_a_key_the_bucket_holds_a_newer_copy_of() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let (host, _) = host_and_sibling(&s3, root.path());
    let ws = host.ws.clone();
    let template = root.path().join("marker.md");
    std::fs::write(&template, MARKER_TEMPLATE).unwrap();

    let name = "wf";
    let key = "record/run";
    let run = koto_in(
        &host,
        &ws,
        &["init", name, "--template", template.to_str().unwrap()],
    );
    assert!(run.ok(), "init: {}", run.describe());
    let run = add_in(&host, &ws, name, key, "mine");
    assert!(run.ok(), "add: {}", run.describe());

    // Another host's later write of the key.
    let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
    let theirs = b"theirs";
    s3.put(&format!("{ctx}{key}"), theirs);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&format!("{ctx}manifest.json")).unwrap()).unwrap();
    manifest["keys"][key] = serde_json::json!({
        "created_at": "2999-01-01T00:00:00.000Z",
        "size": theirs.len(),
        "hash": migration_carrier::sha256_hex(theirs),
        "writer": "agent",
    });
    s3.put(
        &format!("{ctx}manifest.json"),
        &serde_json::to_vec(&manifest).unwrap(),
    );

    let run = koto_in(&host, &ws, &["context", "get", name, key]);
    assert!(run.ok(), "context get: {}", run.describe());
    assert_eq!(run.stdout, "theirs", "{}", run.describe());
    assert_eq!(sync_writes(&host, name, key).len(), 1);
}
