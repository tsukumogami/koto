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
    migration_carrier::project_trust::trust(&host.home.join(".koto"), &moved);
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
    migration_carrier::project_trust::trust(&host.home.join(".koto"), &sibling);
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

/// koto#524: a session's context after a rebind, and which of two differing
/// copies of a key a read keeps.
mod context_carry {
    use super::*;

    /// The bucket's manifest under the context prefix `ctx`, or `Null` when
    /// it has none.
    fn bucket_manifest(s3: &FakeS3, ctx: &str) -> serde_json::Value {
        s3.object(&format!("{ctx}manifest.json"))
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
            .unwrap_or(serde_json::Value::Null)
    }

    /// Rewrite the bucket's manifest under `ctx` with `edit`.
    fn edit_bucket_manifest(s3: &FakeS3, ctx: &str, edit: impl FnOnce(&mut serde_json::Value)) {
        let mut manifest = bucket_manifest(s3, ctx);
        edit(&mut manifest);
        s3.put(
            &format!("{ctx}manifest.json"),
            &serde_json::to_vec(&manifest).unwrap(),
        );
    }

    /// Rewrite `host`'s local manifest of `name` with `edit`.
    fn edit_local_manifest(host: &Host, name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = host.session_dir(name).join("ctx").join("manifest.json");
        let mut manifest = host.manifest(name);
        edit(&mut manifest);
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    /// The stderr lines that report a kept copy of `key`.
    fn kept_lines(run: &Run, key: &str) -> Vec<String> {
        run.stderr
            .lines()
            .filter(|l| l.contains("kept this host's copy") && l.contains(&format!("'{key}'")))
            .map(str::to_string)
            .collect()
    }

    /// A session started in `host`'s workspace from `root`'s marker template,
    /// recorded as a koto from before the session named its prefix left it, so
    /// a rebind moves its home.
    fn init_unrecorded(s3: &FakeS3, host: &Host, root: &Path, name: &str) {
        let template = root.join("marker.md");
        std::fs::write(&template, MARKER_TEMPLATE).unwrap();
        let run = koto_in(
            host,
            &host.ws,
            &["init", name, "--template", template.to_str().unwrap()],
        );
        assert!(run.ok(), "init: {}", run.describe());
        let state = format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(&host.ws));
        drop_recorded_prefix(host, s3, name, &state);
    }

    /// koto#524: a rebind that moved the home of a session from before koto
    /// recorded its prefix pushed only the state file there. The new home kept
    /// an older copy of one key and lacked another, and each reached it only
    /// when it was next read or written. The rebind brings them all, so
    /// another host reading the session from its new home sees its context.
    #[test]
    fn a_rebind_brings_the_sessions_context_to_the_new_home() {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let (host, moved) = host_and_sibling(&s3, root.path());
        let old = host.ws.clone();
        let name = "wf";
        init_unrecorded(&s3, &host, root.path(), name);

        // The moved directory's prefix holds an older copy of record/run, as a
        // sync from there days earlier left it, and no record/plan.
        let run = add_in(&host, &old, name, "record/run", "stale");
        assert!(run.ok(), "add stale: {}", run.describe());
        let old_ctx = format!("{}/{name}/ctx/", prefix_of(&old));
        let moved_ctx = format!("{}/{name}/ctx/", prefix_of(&moved));
        for k in s3.keys_under(&old_ctx) {
            let copy = format!("{moved_ctx}{}", &k[old_ctx.len()..]);
            s3.put(&copy, &s3.object(&k).unwrap());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        let run = add_in(&host, &old, name, "record/run", "current");
        assert!(run.ok(), "add current: {}", run.describe());
        let run = add_in(&host, &old, name, "record/plan", "plan");
        assert!(run.ok(), "add plan: {}", run.describe());

        let run = koto_in(
            &host,
            &moved,
            &["session", "rebind", name, "--to", moved.to_str().unwrap()],
        );
        assert!(run.ok(), "rebind: {}", run.describe());

        // No read or write after the rebind: the new home already holds both.
        let manifest = bucket_manifest(&s3, &moved_ctx);
        let local = host.manifest(name);
        for (key, value) in [("record/run", "current"), ("record/plan", "plan")] {
            assert_eq!(
                s3.object(&format!("{moved_ctx}{key}")).as_deref(),
                Some(value.as_bytes()),
                "a rebind must leave the new home with the session's current context; \
             '{key}' is missing or stale there: {}",
                run.describe()
            );
            assert_eq!(
                manifest["keys"][key]["hash"], local["keys"][key]["hash"],
                "the new home's manifest must name this host's copy of '{key}'"
            );
        }
    }

    /// A key the rebind couldn't push to the new home is named on stderr; the
    /// rebind itself still succeeds, and the other keys get there.
    #[test]
    fn a_rebind_names_the_context_keys_it_could_not_bring() {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let (host, moved) = host_and_sibling(&s3, root.path());
        let old = host.ws.clone();
        let name = "wf";
        init_unrecorded(&s3, &host, root.path(), name);
        let run = add_in(&host, &old, name, "record/run", "current");
        assert!(run.ok(), "add run: {}", run.describe());
        let run = add_in(&host, &old, name, "record/plan", "plan");
        assert!(run.ok(), "add plan: {}", run.describe());

        let moved_ctx = format!("{}/{name}/ctx/", prefix_of(&moved));
        s3.fail("PUT", &format!("{moved_ctx}record/plan"), 500);
        let run = koto_in(
            &host,
            &moved,
            &["session", "rebind", name, "--to", moved.to_str().unwrap()],
        );
        assert!(run.ok(), "rebind: {}", run.describe());
        assert!(
            run.stderr
                .lines()
                .any(|l| l.contains("'record/plan'") && l.contains("new home")),
            "a rebind must leave the new home with the session's current context, or say on \
         stderr which keys it couldn't bring: {}",
            run.describe()
        );
        assert_eq!(
            s3.object(&format!("{moved_ctx}record/run")).as_deref(),
            Some(&b"current"[..]),
            "the keys that could be pushed must reach the new home: {}",
            run.describe()
        );
    }

    /// koto#524: a read of a key the home's manifest didn't list left the home
    /// without it until the key was next written, so another host reading the
    /// session there didn't see it. Here the key's own push failed when it was
    /// written; the read pushes it, as a read of a kept newer key does.
    #[test]
    fn a_read_pushes_a_key_the_homes_manifest_lacks() {
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
        let run = add_in(&host, &ws, name, "record/other", "other");
        assert!(run.ok(), "add other: {}", run.describe());

        let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
        s3.fail("PUT", &format!("{ctx}{key}"), 500);
        let run = add_in(&host, &ws, name, key, "mine");
        assert!(run.ok(), "add: {}", run.describe());
        assert!(
            bucket_manifest(&s3, &ctx)["keys"][key].is_null(),
            "the failed push must leave the key out of the home's manifest"
        );
        s3.clear_faults();

        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(run.ok(), "context get: {}", run.describe());
        assert_eq!(run.stdout, "mine", "{}", run.describe());
        assert_eq!(
            s3.object(&format!("{ctx}{key}")).as_deref(),
            Some(&b"mine"[..]),
            "a read of a key the home lacks must push it there: {}",
            run.describe()
        );
        assert_eq!(
            bucket_manifest(&s3, &ctx)["keys"][key]["hash"],
            host.manifest(name)["keys"][key]["hash"],
            "the home's manifest must name the pushed copy"
        );
    }

    /// A key removed from the home after this host last synced it isn't pushed
    /// back by a read: the home lacks it because another host removed it.
    #[test]
    fn a_read_leaves_out_a_key_removed_from_the_home_since_this_host_synced_it() {
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
        let run = add_in(&host, &ws, name, "record/other", "other");
        assert!(run.ok(), "add other: {}", run.describe());

        // Another host's removal of the key.
        let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
        s3.remove(&format!("{ctx}{key}"));
        edit_bucket_manifest(&s3, &ctx, |m| {
            m["keys"].as_object_mut().unwrap().remove(key);
        });

        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(run.ok(), "context get: {}", run.describe());
        assert!(
            s3.object(&format!("{ctx}{key}")).is_none(),
            "a read must not put back a key removed from the home: {}",
            run.describe()
        );
        assert!(bucket_manifest(&s3, &ctx)["keys"][key].is_null());
    }

    /// A session whose local copy of `key` was written after its last push
    /// failed: the bucket holds `synced`, this host `local`.
    fn kept_key_session(s3: &FakeS3, host: &Host, root: &Path, name: &str, key: &str) -> String {
        let ws = host.ws.clone();
        let template = root.join("marker.md");
        std::fs::write(&template, MARKER_TEMPLATE).unwrap();
        let run = koto_in(
            host,
            &ws,
            &["init", name, "--template", template.to_str().unwrap()],
        );
        assert!(run.ok(), "init: {}", run.describe());
        let run = add_in(host, &ws, name, key, "synced");
        assert!(run.ok(), "add synced: {}", run.describe());
        let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
        std::thread::sleep(std::time::Duration::from_millis(20));
        s3.fail("PUT", &format!("{ctx}{key}"), 500);
        let run = add_in(host, &ws, name, key, "local");
        assert!(run.ok(), "add local: {}", run.describe());
        s3.clear_faults();
        assert_eq!(
            s3.object(&format!("{ctx}{key}")).as_deref(),
            Some(&b"synced"[..])
        );
        ctx
    }

    /// koto#524: the kept-key warning said this host's copy "is pushed over"
    /// the bucket's and only then pushed it, so a push that failed was still
    /// reported as done. The message now follows the push and says what
    /// happened.
    #[test]
    fn the_kept_key_message_follows_the_push_and_says_whether_it_went_up() {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let (host, _) = host_and_sibling(&s3, root.path());
        let ws = host.ws.clone();
        let name = "wf";
        let key = "record/run";
        let ctx = kept_key_session(&s3, &host, root.path(), name, key);

        // The bucket is unreachable for the push.
        s3.fail("PUT", &format!("{ctx}{key}"), 500);
        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(run.ok(), "context get: {}", run.describe());
        assert_eq!(run.stdout, "local", "{}", run.describe());
        let lines = kept_lines(&run, key);
        assert!(
            lines.len() == 1 && lines[0].contains("failed") && !lines[0].contains("pushed over"),
            "a kept copy whose push failed must be reported as failed, not as pushed: {}",
            run.describe()
        );
        assert_eq!(
            s3.object(&format!("{ctx}{key}")).as_deref(),
            Some(&b"synced"[..])
        );

        // The bucket answers again: the push goes up and the message says so.
        s3.clear_faults();
        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(run.ok(), "context get: {}", run.describe());
        assert_eq!(run.stdout, "local", "{}", run.describe());
        let lines = kept_lines(&run, key);
        assert!(
            lines.len() == 1 && lines[0].contains("pushed") && !lines[0].contains("failed"),
            "a kept copy that went up must be reported as pushed: {}",
            run.describe()
        );
        assert_eq!(
            s3.object(&format!("{ctx}{key}")).as_deref(),
            Some(&b"local"[..])
        );
    }

    /// A kept copy whose push the version check refuses is reported as failed,
    /// and the read fails with the conflict, as it did.
    #[test]
    fn a_kept_key_push_refused_as_a_version_conflict_is_reported_as_failed() {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let (host, _) = host_and_sibling(&s3, root.path());
        let ws = host.ws.clone();
        let name = "wf";
        let key = "record/run";
        kept_key_session(&s3, &host, root.path(), name, key);

        // This host has unpushed writes, and another host pushed past the base
        // both started from.
        let path = host.session_dir(name).join("version.json");
        let mut local: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let base = local["last_sync_base"].as_u64().unwrap();
        local["version"] = serde_json::json!(base + 1);
        std::fs::write(&path, serde_json::to_vec(&local).unwrap()).unwrap();
        s3.put(
            &format!("{}/{name}/version.json", prefix_of(&ws)),
            &serde_json::to_vec(&serde_json::json!({
                "version": base + 5,
                "last_sync_base": base + 5,
                "machine_id": "other-host",
            }))
            .unwrap(),
        );

        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(
            !run.ok(),
            "the conflict must fail the read: {}",
            run.describe()
        );
        let lines = kept_lines(&run, key);
        assert!(
            lines.len() == 1 && lines[0].contains("failed") && !lines[0].contains("pushed over"),
            "a kept copy whose push the version check refused must be reported as failed: {}",
            run.describe()
        );
    }

    /// koto#524: which copy of a key is newer came from the manifests'
    /// `created_at`, stamped by each writer's own clock. A host whose clock ran
    /// ahead kept its own copy over a later write from a host whose clock was
    /// right, and pushed it over that write. This host's clock is set far
    /// ahead in its stamps; the other host's later write still comes down.
    #[test]
    fn a_later_write_from_a_host_whose_clock_is_behind_still_comes_down() {
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

        // This host's clock runs years ahead: its write carries that stamp, here
        // and in the bucket.
        let ahead = "2999-01-01T00:00:00.000Z";
        edit_local_manifest(&host, name, |m| {
            m["keys"][key]["created_at"] = serde_json::json!(ahead);
        });
        let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
        edit_bucket_manifest(&s3, &ctx, |m| {
            m["keys"][key]["created_at"] = serde_json::json!(ahead);
        });

        // Another host, whose clock is right, read that write and wrote the
        // key after it.
        let theirs = b"theirs";
        s3.put(&format!("{ctx}{key}"), theirs);
        edit_bucket_manifest(&s3, &ctx, |m| {
            m["keys"][key] = serde_json::json!({
                "created_at": "2030-01-01T00:00:00.000Z",
                "size": theirs.len(),
                "hash": migration_carrier::sha256_hex(theirs),
                "writer": "agent",
            });
        });

        let run = koto_in(&host, &ws, &["context", "get", name, key]);
        assert!(run.ok(), "context get: {}", run.describe());
        assert_eq!(
            run.stdout,
            "theirs",
            "the later write must win whatever the hosts' clocks say: {}",
            run.describe()
        );
        assert_eq!(
            s3.object(&format!("{ctx}{key}")).as_deref(),
            Some(&theirs[..]),
            "this host's older copy must not be pushed over the later write"
        );
        assert!(kept_lines(&run, key).is_empty(), "{}", run.describe());
    }
}
