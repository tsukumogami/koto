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
        assert!(
            run.code == Some(3) && run.stdout.contains(key),
            "add: {}",
            run.describe()
        );
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

    /// `koto session origin` reads link bodies without the push `context
    /// get` makes: a dispatched-by link the home's manifest lacks stays out
    /// of the bucket after the verb read it.
    #[test]
    fn session_origin_never_pushes_a_link_the_home_lacks() {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let (host, _) = host_and_sibling(&s3, root.path());
        let ws = host.ws.clone();
        let template = root.path().join("marker.md");
        std::fs::write(&template, MARKER_TEMPLATE).unwrap();
        let name = "wf";
        let run = koto_in(
            &host,
            &ws,
            &["init", name, "--template", template.to_str().unwrap()],
        );
        assert!(run.ok(), "init: {}", run.describe());
        let run = koto_in(
            &host,
            &ws,
            &["session", "link", name, "--dispatched-by", "zeta"],
        );
        assert!(run.ok(), "link zeta: {}", run.describe());

        // Two links, so the verb reads their bodies to choose; the second's
        // push fails, so only this host holds it.
        let ctx = format!("{}/{name}/ctx/", prefix_of(&ws));
        let key = "koto/links/dispatched-by/alpha";
        s3.fail("PUT", &format!("{ctx}{key}"), 500);
        let _ = koto_in(
            &host,
            &ws,
            &["session", "link", name, "--dispatched-by", "alpha"],
        );
        s3.clear_faults();
        assert!(
            bucket_manifest(&s3, &ctx)["keys"][key].is_null(),
            "the failed push must leave the link out of the home's manifest"
        );
        assert!(host.manifest(name)["keys"][key].is_object());

        let run = koto_in(&host, &ws, &["session", "origin", name]);
        assert!(run.ok(), "session origin: {}", run.describe());
        let got: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
        assert_eq!(got["origin"]["state"], "dispatched", "{}", run.describe());
        assert_eq!(got["origin"]["dispatcher_kind"], "workflow");
        assert_eq!(
            s3.object(&format!("{ctx}{key}")),
            None,
            "{}",
            run.describe()
        );
        assert!(
            bucket_manifest(&s3, &ctx)["keys"][key].is_null(),
            "the verb must not push the link: {}",
            run.describe()
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
        assert!(
            run.code == Some(3) && run.stdout.contains(key),
            "add local: {}",
            run.describe()
        );
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

/// `koto session rebind --whole` on the cloud backend: the move commits the
/// bucket's copy first, with a conditional PUT, then this host's, then each
/// part (held-by links, publish location, command environment), and uploads
/// `version.json` one version up under this machine. A move another host
/// pushed last refuses `rebind_cross_host`. A push, a resolve or a flush
/// from a process that read the session before a move refuses
/// `session_rebound` instead of replacing the moved copy.
///
/// The backend is configured through `KOTO_SESSION_*` variables, so every
/// directory a test runs koto from reaches the same bucket without a
/// project config of its own, and each command starts from an empty
/// environment: no `KOTO_ANCHOR`, `KOTO_DISPATCHER_ANCHOR` or other koto
/// variable of the shell running the suite reaches it.
mod whole {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::{json, Value};

    const NAME: &str = "wf";
    const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
    const GO: &str = r#"{"choice":"go"}"#;
    const OK: &str = r#"{"verdict":"ok"}"#;
    const FAULT_ENV: &str = "KOTO_TEST_REBIND_FAULT";
    const LOCATION: &str = "workflows/publish-location";
    const OTHER_MACHINE: &str = "another-machine";

    /// The points at which `KOTO_TEST_REBIND_FAULT` stops a move on the
    /// cloud backend, in the order the move reaches them.
    const FAULT_POINTS: &[&str] = &[
        "after-remote-commit",
        "after-commit",
        "after-held-by-event",
        "after-held-by",
        "after-publish-event",
        "after-publish",
        "after-environment-event",
        "after-environment",
    ];

    /// Two states that take evidence, so a tick with `GO` stops short of
    /// the terminal state.
    const TEMPLATE: &str = r#"---
name: plain
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: more
        when:
          choice: go
  more:
    accepts:
      verdict:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          verdict: ok
  done:
    terminal: true
---

## start

Decide.

## more

Decide again.

## done

Done.
"#;

    /// A first state whose default action writes context key `one.md`
    /// inside the tick, so the write waits in the tick's batch for the
    /// flush on the way out.
    const FLUSH_TEMPLATE: &str = r#"---
name: flush
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
        set -e
        printf one | koto context add wf one.md
    transitions:
      - target: wait
  wait:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          choice: go
  done:
    terminal: true
---

## work

Work.

## wait

Wait.

## done

Done.
"#;

    fn s(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    fn mk(p: PathBuf) -> PathBuf {
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(p).unwrap()
    }

    fn koto_dir() -> PathBuf {
        crate::koto_command::koto_bin()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// One host on the shared bucket: its own `HOME` (so its own session
    /// store and machine id) and template cache, under a scratch root.
    struct Machine {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        home: PathBuf,
        cache: PathBuf,
        endpoint: String,
        template: PathBuf,
    }

    impl Machine {
        fn new(s3: &FakeS3) -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = mk(tmp.path().to_path_buf());
            let home = mk(root.join("home"));
            let cache = mk(root.join("cache"));
            let template = root.join("wf.md");
            std::fs::write(&template, TEMPLATE).unwrap();
            Machine {
                _tmp: tmp,
                root,
                home,
                cache,
                endpoint: s3.endpoint(),
                template,
            }
        }

        /// A directory under the root, created.
        fn dir(&self, rel: &str) -> PathBuf {
            mk(self.root.join(rel))
        }

        /// koto on the cloud backend from `dir`, with nothing inherited and
        /// `envs` set on top.
        fn cmd(&self, dir: &Path, envs: &[(&str, &str)], args: &[&str]) -> std::process::Command {
            let mut cmd = crate::koto_command::koto_std();
            cmd.env_clear()
                .current_dir(dir)
                .env("PATH", format!("{}:{SYSTEM_PATH}", koto_dir().display()))
                .env("HOME", &self.home)
                .env("XDG_CACHE_HOME", &self.cache)
                .env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
                .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
                .args(args);
            for (name, value) in envs {
                cmd.env(name, value);
            }
            cmd
        }

        fn run(&self, dir: &Path, envs: &[(&str, &str)], args: &[&str]) -> Run {
            let output = self.cmd(dir, envs, args).output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
                let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
                serde_json::from_str(last).unwrap_or(Value::Null)
            });
            Run {
                code: output.status.code(),
                stdout,
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                json,
            }
        }

        fn ok(&self, dir: &Path, envs: &[(&str, &str)], args: &[&str], what: &str) -> Run {
            let run = self.run(dir, envs, args);
            assert!(run.ok(), "{what}: {}", run.describe());
            run
        }

        fn init(&self, dir: &Path, envs: &[(&str, &str)], name: &str, extra: &[&str]) {
            let mut args = vec!["init", name, "--template", s(&self.template)];
            args.extend_from_slice(extra);
            self.ok(dir, envs, &args, &format!("init {name}"));
        }

        /// `koto context add <name> <key>` from `dir`, the content read from
        /// a file under the root.
        fn add(&self, dir: &Path, name: &str, key: &str, content: &str) -> Run {
            let file = self.root.join("value.txt");
            std::fs::write(&file, content).unwrap();
            self.run(
                dir,
                &[],
                &["context", "add", name, key, "--from-file", s(&file)],
            )
        }

        /// Open the anchor for harness session `harness` and return its
        /// name.
        fn open_anchor(&self, dir: &Path, harness: &str) -> String {
            let run = self.ok(
                dir,
                &[],
                &[
                    "session",
                    "anchor",
                    "open",
                    "--harness-session",
                    harness,
                    "--source",
                    "startup",
                ],
                "anchor open",
            );
            run.stdout
                .lines()
                .find_map(|l| l.strip_prefix("anchor="))
                .expect("anchor open printed no anchor line")
                .to_string()
        }

        /// The value of context key `key` in `name`, read from `dir`, or
        /// `None` when the session has no such key.
        fn key(&self, dir: &Path, name: &str, key: &str) -> Option<String> {
            let run = self.run(dir, &[], &["context", "get", name, key]);
            run.ok().then(|| run.stdout.trim().to_string())
        }

        fn session_dir(&self, name: &str) -> PathBuf {
            self.home.join(".koto").join("sessions").join(name)
        }

        fn state_path(&self, name: &str) -> PathBuf {
            self.session_dir(name)
                .join(format!("koto-{name}.state.jsonl"))
        }

        fn local_text(&self, name: &str) -> String {
            std::fs::read_to_string(self.state_path(name)).unwrap_or_default()
        }

        fn header(&self, name: &str) -> Value {
            header_of(&self.local_text(name))
        }

        /// The names of the held-by links in this host's copy of `name`.
        fn held_by(&self, name: &str) -> Vec<String> {
            let dir = self
                .session_dir(name)
                .join("ctx")
                .join("koto")
                .join("links")
                .join("held-by");
            let mut out: Vec<String> = std::fs::read_dir(dir)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.ends_with(".lock"))
                        .collect()
                })
                .unwrap_or_default();
            out.sort();
            out
        }

        /// The machine id this host's copy of `name` records in its
        /// `version.json`.
        fn machine_id(&self, name: &str) -> String {
            let bytes = std::fs::read(self.session_dir(name).join("version.json")).unwrap();
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["machine_id"].as_str().unwrap().to_string()
        }
    }

    /// The bucket key of `rest` under session `name` at `prefix`.
    fn session_key(prefix: &str, name: &str, rest: &str) -> String {
        format!("{prefix}/{name}/{rest}")
    }

    fn state_key(prefix: &str, name: &str) -> String {
        session_key(prefix, name, &format!("koto-{name}.state.jsonl"))
    }

    fn remote_text(s3: &FakeS3, key: &str) -> String {
        let bytes = s3
            .object(key)
            .unwrap_or_else(|| panic!("no remote object at {key}; keys: {:?}", s3.keys()));
        String::from_utf8(bytes).unwrap()
    }

    fn header_of(text: &str) -> Value {
        text.lines()
            .next()
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or(Value::Null)
    }

    fn events_of(text: &str) -> Vec<Value> {
        text.lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("a state log line isn't JSON"))
            .collect()
    }

    /// How many events of type `ty` the log `text` holds, counting only a
    /// whole move's `execution_anchor_rebound`.
    fn count(text: &str, ty: &str) -> usize {
        events_of(text)
            .iter()
            .filter(|e| e["type"] == ty)
            .filter(|e| ty != "execution_anchor_rebound" || e["payload"]["whole"] == true)
            .count()
    }

    /// Every object under `prefix`, by key.
    fn snapshot(s3: &FakeS3, prefix: &str) -> BTreeMap<String, Vec<u8>> {
        s3.keys_under(prefix)
            .into_iter()
            .filter_map(|k| s3.object(&k).map(|b| (k, b)))
            .collect()
    }

    fn remote_json(s3: &FakeS3, key: &str) -> Option<Value> {
        s3.object(key)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    /// `text` with its last event cloned one seq on and changed by `edit`,
    /// as a writer elsewhere would have appended it. The lines already
    /// there are kept byte for byte.
    fn appended(text: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let mut event: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert!(event.get("seq").is_some(), "the log holds no event: {text}");
        event["seq"] = json!(event["seq"].as_u64().unwrap() + 1);
        edit(&mut event);
        lines.push(serde_json::to_string(&event).unwrap());
        lines.join("\n") + "\n"
    }

    /// The state log `text` as a move from `from` to `to` left it, on
    /// another process: the header names `to` at the next anchor epoch,
    /// with `from` retired and, for a whole move, `whole_epoch` set; with
    /// `with_event`, the move's `execution_anchor_rebound` follows the
    /// events already there, which are kept byte for byte.
    fn moved_copy(text: &str, from: &Path, to: &Path, whole: bool, with_event: bool) -> String {
        let mut lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let mut header: Value = serde_json::from_str(&lines[0]).unwrap();
        header["execution_dir"] = json!(s(to));
        let origin = header["origin"]
            .as_object_mut()
            .expect("the header records no origin");
        let epoch = origin
            .get("anchor_epoch")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        origin.insert("anchor".into(), json!(s(to)));
        origin.insert("anchor_epoch".into(), json!(epoch));
        if whole {
            origin.insert("whole_epoch".into(), json!(epoch));
        }
        let mut retired: Vec<Value> = origin
            .get("retired_anchors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !retired.contains(&json!(s(from))) {
            retired.push(json!(s(from)));
        }
        retired.retain(|r| r != &json!(s(to)));
        origin.insert("retired_anchors".into(), Value::Array(retired));
        lines[0] = serde_json::to_string(&header).unwrap();
        let text = lines.join("\n") + "\n";
        if !with_event {
            return text;
        }
        appended(&text, |event| {
            event["type"] = json!("execution_anchor_rebound");
            event["payload"] = if whole {
                json!({"from": s(from), "to": s(to), "whole": true, "epoch": epoch})
            } else {
                json!({"from": s(from), "to": s(to)})
            };
        })
    }

    /// Assert that `run` is a refusal of `koto session rebind` carrying
    /// `code` in the error envelope and exiting `exit`.
    fn assert_refused(run: &Run, exit: i32, code: &str) {
        assert_eq!(run.code, Some(exit), "{}", run.describe());
        assert_eq!(run.json["error"]["code"], code, "{}", run.describe());
        assert!(
            run.json["error"]["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty()),
            "the refusal has no message: {}",
            run.describe()
        );
        assert_eq!(run.json["command"], "session rebind", "{}", run.describe());
    }

    /// Assert that `run` failed and names `session_rebound`.
    fn assert_rebound(run: &Run, what: &str) {
        assert!(!run.ok(), "{what} wasn't refused: {}", run.describe());
        assert!(
            format!("{}{}", run.stdout, run.stderr).contains("session_rebound"),
            "{what} wasn't refused as session_rebound: {}",
            run.describe()
        );
    }

    /// Session [`NAME`] started in `old` under the anchor `old_anchor`,
    /// ticked once and given a key there, publishing to `wf_old`; and the
    /// anchor its new holder runs under in `new`.
    struct Moving {
        s3: FakeS3,
        m: Machine,
        old: PathBuf,
        new: PathBuf,
        prefix: String,
        old_anchor: String,
        new_anchor: String,
        wf_old: PathBuf,
        wf_new: PathBuf,
        xdg_new: PathBuf,
    }

    impl Moving {
        fn new() -> Self {
            let s3 = FakeS3::start(BUCKET);
            let m = Machine::new(&s3);
            let old = m.dir("old");
            let new = m.dir("new");
            let wf_old = m.dir("wf-old");
            let wf_new = m.dir("wf-new");
            let xdg_new = m.dir("xdg-new");
            let old_anchor = m.open_anchor(&old, "old");
            let new_anchor = m.open_anchor(&old, "new");
            let env = [
                ("KOTO_ANCHOR", old_anchor.as_str()),
                ("KOTO_WORKFLOWS_DIR", s(&wf_old)),
            ];
            m.init(&old, &env, NAME, &[]);
            m.ok(
                &old,
                &env,
                &["next", NAME, "--with-data", GO],
                "setup: next",
            );
            let run = m.add(&old, NAME, "notes.md", "notes");
            assert!(run.ok(), "setup: context add: {}", run.describe());
            let prefix = prefix_of(&old);
            let mv = Moving {
                s3,
                m,
                old,
                new,
                prefix,
                old_anchor,
                new_anchor,
                wf_old,
                wf_new,
                xdg_new,
            };
            assert!(
                mv.s3.object(&mv.state_key()).is_some(),
                "setup: no bucket copy at {}; keys: {:?}",
                mv.state_key(),
                mv.s3.keys()
            );
            assert!(
                mv.s3.object(&mv.key("version.json")).is_some(),
                "setup: no version.json in the bucket; keys: {:?}",
                mv.s3.keys()
            );
            assert_eq!(
                mv.m.held_by(NAME),
                vec![mv.old_anchor.clone()],
                "setup: held-by links"
            );
            assert_eq!(
                mv.m.key(&mv.old, NAME, LOCATION).as_deref(),
                Some(s(&mv.wf_old)),
                "setup: publish location"
            );
            mv
        }

        fn key(&self, rest: &str) -> String {
            session_key(&self.prefix, NAME, rest)
        }

        fn state_key(&self) -> String {
            state_key(&self.prefix, NAME)
        }

        fn remote(&self) -> String {
            remote_text(&self.s3, &self.state_key())
        }

        fn version(&self) -> Value {
            remote_json(&self.s3, &self.key("version.json")).expect("no version.json")
        }

        /// The move, run by the new holder from `dir` with `envs` on top
        /// and `extra` after `--whole`.
        fn mover(&self, dir: &Path, envs: &[(&str, &str)], extra: &[&str]) -> Run {
            let mut args = vec!["session", "rebind", NAME, "--whole"];
            args.extend_from_slice(extra);
            let mut env = vec![
                ("KOTO_ANCHOR", self.new_anchor.as_str()),
                ("XDG_CONFIG_HOME", s(&self.xdg_new)),
                ("KOTO_WORKFLOWS_DIR", s(&self.wf_new)),
            ];
            env.extend_from_slice(envs);
            self.m.run(dir, &env, &args)
        }

        /// Assert that every part of the move happened, in the bucket and
        /// on this host, each logged once.
        fn assert_moved(&self, what: &str) {
            let remote = self.remote();
            let local = self.m.local_text(NAME);
            for (side, h) in [("bucket", header_of(&remote)), ("local", header_of(&local))] {
                assert_eq!(h["execution_dir"], s(&self.new), "{what}: {side}: {h}");
                assert_eq!(h["origin"]["anchor"], s(&self.new), "{what}: {side}: {h}");
                assert_eq!(h["origin"]["anchor_epoch"], 1, "{what}: {side}: {h}");
                assert_eq!(h["origin"]["whole_epoch"], 1, "{what}: {side}: {h}");
                assert!(
                    h["origin"]["retired_anchors"]
                        .as_array()
                        .is_some_and(|r| r.contains(&json!(s(&self.old)))),
                    "{what}: {side}: the old anchor isn't retired: {h}"
                );
                assert_eq!(
                    h["command_environment"]["xdg_config_home"],
                    s(&self.xdg_new),
                    "{what}: {side}: the environment wasn't re-recorded: {h}"
                );
            }
            for ty in [
                "execution_anchor_rebound",
                "held_by_rebound",
                "publish_location_dropped",
                "environment_rerecorded",
            ] {
                assert_eq!(count(&remote, ty), 1, "{what}: {ty} events in the bucket");
            }
            assert_eq!(
                self.m.held_by(NAME),
                vec![self.new_anchor.clone()],
                "{what}: held-by links"
            );
            let manifest =
                remote_json(&self.s3, &self.key("ctx/manifest.json")).unwrap_or(Value::Null);
            let link = |anchor: &str| format!("koto/links/held-by/{anchor}");
            assert!(
                manifest["keys"].get(link(&self.new_anchor)).is_some(),
                "{what}: the bucket's manifest lacks the new link: {manifest}"
            );
            assert!(
                manifest["keys"].get(link(&self.old_anchor)).is_none(),
                "{what}: the bucket's manifest keeps the old link: {manifest}"
            );
            assert_ne!(
                self.m.key(&self.new, NAME, LOCATION).as_deref(),
                Some(s(&self.wf_old)),
                "{what}: the old publish location is still stored"
            );
        }
    }

    #[test]
    fn a_cloud_whole_rebind_commits_remote_first_and_moves_every_part() {
        let mv = Moving::new();
        let before = mv.version()["version"].as_u64().unwrap();

        // Stopped right after its commit point, the bucket's copy is moved
        // and this host's isn't yet.
        let run = mv.mover(&mv.new, &[(FAULT_ENV, "after-remote-commit")], &[]);
        assert!(!run.ok(), "the stopped move exited 0: {}", run.describe());
        let remote = header_of(&mv.remote());
        assert_eq!(remote["execution_dir"], s(&mv.new), "{remote}");
        assert_eq!(remote["origin"]["whole_epoch"], 1, "{remote}");
        let local = mv.m.header(NAME);
        assert_eq!(local["execution_dir"], s(&mv.old), "{local}");
        assert!(local["origin"].get("whole_epoch").is_none(), "{local}");

        let run = mv.mover(&mv.new, &[], &[]);
        assert!(run.ok(), "rebind --whole: {}", run.describe());
        assert_eq!(run.json["whole"], true, "{}", run.describe());
        mv.assert_moved("after the move");

        let version = mv.version();
        assert!(
            version["version"].as_u64().unwrap() > before,
            "the move didn't bump version.json past {before}: {version}"
        );
        assert_eq!(
            version["machine_id"],
            mv.m.machine_id(NAME).as_str(),
            "{version}"
        );

        let env = [
            ("KOTO_ANCHOR", mv.new_anchor.as_str()),
            ("KOTO_WORKFLOWS_DIR", s(&mv.wf_new)),
        ];
        mv.m.ok(
            &mv.new,
            &env,
            &["next", NAME, "--with-data", OK, "--no-cleanup"],
            "next after the move",
        );
    }

    #[test]
    fn a_cloud_whole_rebind_that_loses_the_conditional_put_writes_nothing_and_a_rerun_finishes() {
        let mv = Moving::new();
        let key = mv.state_key();
        // Another writer's event lands after the move pulled and before its
        // commit: the move's cross-host read of version.json sits between
        // the two.
        let racer = appended(&mv.remote(), |event| {
            event["payload"]["key"] = json!("theirs.md");
        });
        mv.s3
            .put_after("GET", &mv.key("version.json"), &key, racer.as_bytes());

        let run = mv.mover(&mv.new, &[], &[]);

        assert!(!run.ok(), "the move won over the racer: {}", run.describe());
        assert_eq!(mv.remote(), racer, "the move replaced the racer's copy");
        let local = mv.m.local_text(NAME);
        let h = header_of(&local);
        assert_eq!(h["execution_dir"], s(&mv.old), "{h}");
        assert!(h["origin"].get("whole_epoch").is_none(), "{h}");
        assert_eq!(count(&local, "execution_anchor_rebound"), 0);
        assert_eq!(count(&local, "held_by_rebound"), 0);
        assert_eq!(mv.m.held_by(NAME), vec![mv.old_anchor.clone()]);

        let run = mv.mover(&mv.new, &[], &[]);
        assert!(run.ok(), "the rerun: {}", run.describe());
        mv.assert_moved("after the rerun");
        assert!(
            events_of(&mv.remote())
                .iter()
                .any(|e| e["type"] == "context_added" && e["payload"]["key"] == "theirs.md"),
            "the rerun dropped the racer's event"
        );
    }

    #[test]
    fn a_cloud_whole_rebind_stopped_after_any_write_finishes_when_run_again() {
        for point in FAULT_POINTS {
            let mv = Moving::new();
            let run = mv.mover(&mv.new, &[(FAULT_ENV, *point)], &[]);
            assert!(
                !run.ok(),
                "{point}: the stopped move exited 0: {}",
                run.describe()
            );
            let run = mv.mover(&mv.new, &[], &[]);
            assert!(run.ok(), "{point}: run again: {}", run.describe());
            mv.assert_moved(point);
        }
    }

    #[test]
    fn a_cloud_whole_rebind_run_from_the_old_directory_finishes_its_later_parts() {
        // The mover reads the session at the old anchor, then retires it:
        // its own later parts aren't fenced off by the move it made.
        let mv = Moving::new();

        let run = mv.mover(&mv.old, &[], &["--to", s(&mv.new)]);

        assert!(run.ok(), "rebind --whole --to: {}", run.describe());
        mv.assert_moved("after the move");
        let before = snapshot(&mv.s3, &format!("{}/{NAME}/", mv.prefix));
        let run = mv.m.add(&mv.old, NAME, "late.md", "late");
        assert_rebound(&run, "a later write from the old directory");
        assert_eq!(
            snapshot(&mv.s3, &format!("{}/{NAME}/", mv.prefix)),
            before,
            "the refused write reached the bucket"
        );
    }

    #[test]
    fn a_second_process_in_the_old_directory_is_refused_while_the_cloud_mover_finishes_from_there()
    {
        // The mover runs from the directory it retires; another process
        // there is refused while the move's later parts are still to run.
        let mv = Moving::new();
        let hold = mv.m.root.join("hold");
        let envs = [
            ("KOTO_ANCHOR", mv.new_anchor.as_str()),
            ("XDG_CONFIG_HOME", s(&mv.xdg_new)),
            ("KOTO_WORKFLOWS_DIR", s(&mv.wf_new)),
            ("KOTO_TEST_REBIND_HOLD", s(&hold)),
            ("KOTO_TEST_REBIND_HOLD_AT", "after-held-by"),
        ];
        let args = ["session", "rebind", NAME, "--whole", "--to", s(&mv.new)];
        let mut mover =
            mv.m.cmd(&mv.old, &envs, &args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
        let started = std::time::Instant::now();
        let bound = std::time::Duration::from_secs(60);
        while !hold.exists() {
            if mover.try_wait().unwrap().is_some() {
                let out = mover.wait_with_output().unwrap();
                panic!(
                    "the move exited before its hold point: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            if started.elapsed() > bound {
                let _ = mover.kill();
                panic!("the move didn't reach its hold point within {bound:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(header_of(&mv.remote())["origin"]["whole_epoch"], 1);
        assert_eq!(count(&mv.remote(), "environment_rerecorded"), 0);

        let before = snapshot(&mv.s3, &format!("{}/{NAME}/", mv.prefix));
        let run = mv.m.add(&mv.old, NAME, "late.md", "late");
        assert_rebound(&run, "a write from the old directory during the move");
        assert_eq!(
            snapshot(&mv.s3, &format!("{}/{NAME}/", mv.prefix)),
            before,
            "the refused write reached the bucket"
        );

        std::fs::remove_file(&hold).unwrap();
        while mover.try_wait().unwrap().is_none() {
            if started.elapsed() > bound {
                let _ = mover.kill();
                panic!("the move didn't finish within {bound:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let out = mover.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "rebind --whole --to: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        mv.assert_moved("after the move");
        let run = mv.m.add(&mv.old, NAME, "late.md", "late");
        assert_rebound(&run, "a write from the old directory after the move");
    }

    #[test]
    fn a_cloud_whole_rebind_refuses_when_another_machine_pushed_last() {
        let mv = Moving::new();
        let mut version = mv.version();
        version["machine_id"] = json!(OTHER_MACHINE);
        mv.s3.put(
            &mv.key("version.json"),
            &serde_json::to_vec(&version).unwrap(),
        );
        let prefix = format!("{}/{NAME}/", mv.prefix);
        let before = snapshot(&mv.s3, &prefix);
        let local_before = mv.m.local_text(NAME);

        let run = mv.mover(&mv.new, &[], &[]);

        assert_refused(&run, 2, "rebind_cross_host");
        assert!(
            run.json["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("koto session import")),
            "the refusal doesn't name `koto session import`: {}",
            run.describe()
        );
        assert_eq!(snapshot(&mv.s3, &prefix), before, "the bucket changed");
        assert_eq!(mv.m.local_text(NAME), local_before, "the local log changed");
        assert_eq!(mv.m.held_by(NAME), vec![mv.old_anchor.clone()]);
    }

    #[test]
    fn a_push_refused_for_a_higher_anchor_epoch_answers_session_rebound() {
        // A plain rebind elsewhere, as an older build logs it: the header
        // alone moves to the next epoch. The writer's push lands after it.
        let mv = Moving::new();
        let key = mv.state_key();
        let moved = moved_copy(&mv.remote(), &mv.old, &mv.new, false, false);
        mv.s3
            .put_after("PUT", &mv.key("ctx/manifest.json"), &key, moved.as_bytes());

        let run = mv.m.add(&mv.old, NAME, "late.md", "late");

        assert_rebound(&run, "the push over the higher epoch");
        assert_eq!(mv.remote(), moved, "the push replaced the moved copy");
    }

    #[test]
    fn resolve_keep_local_refuses_to_overwrite_a_higher_anchor_epoch() {
        let mv = Moving::new();
        let key = mv.state_key();
        let moved = moved_copy(&mv.remote(), &mv.old, &mv.new, true, true);
        mv.s3.put(&key, moved.as_bytes());

        let run = mv.m.run(
            &mv.old,
            &[],
            &["session", "resolve", NAME, "--keep", "local"],
        );

        assert_rebound(&run, "resolve --keep local");
        assert_eq!(mv.remote(), moved, "the resolve replaced the moved copy");
    }

    #[test]
    fn resolve_children_accept_local_refuses_to_overwrite_a_moved_child() {
        // The parent is where this host left it; its child was moved with
        // `--whole` elsewhere. Accepting this host's copy of the child would
        // undo the move, so the child is reported errored and its bucket
        // copy is left as the move wrote it.
        let mv = Moving::new();
        mv.m.init(&mv.old, &[], "child", &["--parent", NAME]);
        let child_key = state_key(&mv.prefix, "child");
        assert!(
            mv.s3.object(&child_key).is_some(),
            "setup: no bucket copy of the child at {}; keys: {:?}",
            child_key,
            mv.s3.keys()
        );
        let moved = moved_copy(
            &remote_text(&mv.s3, &child_key),
            &mv.old,
            &mv.new,
            true,
            true,
        );
        mv.s3.put(&child_key, moved.as_bytes());

        let run = mv.m.run(
            &mv.old,
            &[],
            &[
                "session",
                "resolve",
                NAME,
                "--keep",
                "local",
                "--children",
                "accept-local",
            ],
        );

        assert_eq!(
            remote_text(&mv.s3, &child_key),
            moved,
            "the resolve replaced the moved child: {}",
            run.describe()
        );
        let rows = run.json["children"].as_array().cloned().unwrap_or_default();
        let child = rows
            .iter()
            .find(|row| row["name"] == "child")
            .unwrap_or_else(|| panic!("no row for the child: {}", run.describe()));
        assert_eq!(child["action"], "errored", "{}", run.describe());
        assert!(
            child["message"]
                .as_str()
                .is_some_and(|m| m.contains("session_rebound")),
            "the child's refusal doesn't name session_rebound: {}",
            run.describe()
        );
    }

    #[test]
    fn a_cloud_whole_rebind_on_a_child_and_with_every_directory_gone() {
        let s3 = FakeS3::start(BUCKET);
        let m = Machine::new(&s3);
        let parent = m.dir("parent");
        let first = m.dir("first");
        let second = m.dir("second");
        let new = m.dir("new");
        m.init(&parent, &[], NAME, &[]);
        // A child adopts its parent's directory unless told otherwise, so it
        // is bound to `first` explicitly: the move must retire both
        // directories the child itself recorded.
        m.init(
            &first,
            &[],
            "child",
            &["--parent", NAME, "--execution-dir", s(&first)],
        );
        let child_key = state_key(&prefix_of(&first), "child");
        let parent_key = state_key(&prefix_of(&parent), NAME);
        m.ok(
            &second,
            &[],
            &["session", "rebind", "child"],
            "setup: plain rebind",
        );
        std::fs::remove_dir_all(&first).unwrap();
        std::fs::remove_dir_all(&second).unwrap();
        let anchor = m.open_anchor(&new, "new");

        let run = m.ok(
            &new,
            &[("KOTO_ANCHOR", anchor.as_str())],
            &["session", "rebind", "child", "--whole"],
            "rebind --whole child",
        );

        assert_eq!(run.json["whole"], true, "{}", run.describe());
        let remote = remote_text(&s3, &child_key);
        for (side, h) in [("bucket", header_of(&remote)), ("local", m.header("child"))] {
            assert_eq!(h["execution_dir"], s(&new), "{side}: {h}");
            assert_eq!(h["origin"]["anchor_epoch"], 2, "{side}: {h}");
            assert_eq!(h["origin"]["whole_epoch"], 2, "{side}: {h}");
            let retired = h["origin"]["retired_anchors"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for gone in [&first, &second] {
                assert!(retired.contains(&json!(s(gone))), "{side}: {h}");
            }
        }
        assert_eq!(count(&remote, "execution_anchor_rebound"), 1);
        let parent_header = header_of(&remote_text(&s3, &parent_key));
        assert!(
            parent_header["origin"].get("whole_epoch").is_none(),
            "the parent moved too: {parent_header}"
        );

        m.ok(
            &new,
            &[("KOTO_ANCHOR", anchor.as_str())],
            &["next", "child", "--with-data", GO],
            "next child after the move",
        );
    }

    #[test]
    fn a_cloud_writer_that_pulled_after_a_whole_rebind_keeps_its_first_read_epoch() {
        // The move goes into a subdirectory of the old anchor, where the
        // writer runs: the path rule admits it there, so only the epoch it
        // read before the move can refuse it.
        let mv = Moving::new();
        let below = mk(mv.old.join("below"));
        let key = mv.state_key();
        let moved = moved_copy(&mv.remote(), &mv.old, &below, true, true);
        mv.s3
            .put_after("PUT", &mv.key("ctx/manifest.json"), &key, moved.as_bytes());

        let run = mv.m.add(&below, NAME, "late.md", "late");

        assert_rebound(&run, "a write that read the session before the move");
        assert_eq!(mv.remote(), moved, "the write replaced the moved copy");
    }

    #[test]
    fn a_stale_push_after_a_whole_rebind_is_refused_session_rebound_not_replaced() {
        // With the move's event, the stale push and the moved copy diverge;
        // without it (an older build's move), the push extends the moved
        // copy's events and only the header tells them apart.
        for with_event in [true, false] {
            let mv = Moving::new();
            let key = mv.state_key();
            let moved = moved_copy(&mv.remote(), &mv.old, &mv.new, true, with_event);
            mv.s3
                .put_after("PUT", &mv.key("ctx/manifest.json"), &key, moved.as_bytes());

            let run = mv.m.add(&mv.old, NAME, "late.md", "late");

            assert_rebound(&run, &format!("the stale push (event: {with_event})"));
            assert_eq!(
                mv.remote(),
                moved,
                "event: {with_event}: the stale push replaced the moved copy"
            );
        }
    }

    #[test]
    fn a_501_bucket_refuses_a_push_over_a_higher_remote_epoch() {
        // The bucket answers a conditional PUT of the state file with 501,
        // as one without conditional writes does, so the push falls back to
        // an unconditional one.
        let mv = Moving::new();
        let key = mv.state_key();
        let moved = moved_copy(&mv.remote(), &mv.old, &mv.new, true, true);
        mv.s3
            .put_after("PUT", &mv.key("ctx/manifest.json"), &key, moved.as_bytes());
        mv.s3.fail_times("PUT", &key, 501, 2);

        let run = mv.m.add(&mv.old, NAME, "late.md", "late");

        assert_rebound(&run, "the unconditional push over the higher epoch");
        assert_eq!(mv.remote(), moved, "the push replaced the moved copy");
    }

    #[test]
    fn a_flush_refused_by_the_moves_version_answers_session_rebound() {
        // A tick from the old anchor writes a key in its batch; a move
        // elsewhere lands while it runs, and its version.json is one up.
        let s3 = FakeS3::start(BUCKET);
        let m = Machine::new(&s3);
        let template = m.root.join("flush.md");
        std::fs::write(&template, FLUSH_TEMPLATE).unwrap();
        let old = m.dir("old");
        let new = m.dir("new");
        m.ok(
            &old,
            &[],
            &["init", NAME, "--template", s(&template)],
            "setup: init",
        );
        let run = m.add(&old, NAME, "base.md", "base");
        assert!(run.ok(), "setup: context add: {}", run.describe());
        let prefix = prefix_of(&old);
        let key = state_key(&prefix, NAME);
        let version_key = session_key(&prefix, NAME, "version.json");
        let mut version = remote_json(&s3, &version_key).expect("setup: no version.json");
        let next = version["version"].as_u64().unwrap() + 1;
        version["version"] = json!(next);
        version["last_sync_base"] = json!(next);
        version["machine_id"] = json!(m.machine_id(NAME));
        let version = serde_json::to_vec(&version).unwrap();
        let moved = moved_copy(&remote_text(&s3, &key), &old, &new, true, true);
        s3.put_after("GET", &version_key, &version_key, &version);
        s3.put_after("GET", &version_key, &key, moved.as_bytes());

        let run = m.run(&old, &[], &["next", NAME]);

        assert!(
            format!("{}{}", run.stdout, run.stderr).contains("session_rebound"),
            "the flush wasn't refused as session_rebound: {}",
            run.describe()
        );
        assert!(
            s3.object(&session_key(&prefix, NAME, "ctx/one.md"))
                .is_none(),
            "the batch's key reached the bucket"
        );
        assert_eq!(
            remote_text(&s3, &key),
            moved,
            "the flush replaced the moved copy"
        );
        assert_eq!(s3.object(&version_key).unwrap(), version);
    }

    /// The points a move of a session from before koto recorded its prefix
    /// can stop at after its commit landed here, in the order the move
    /// reaches them: each one past the commit, then `after-version`, between
    /// the move's push of `version.json` and its carry of the context.
    const CARRY_POINTS: &[&str] = &[
        "after-commit",
        "after-held-by-event",
        "after-held-by",
        "after-publish-event",
        "after-publish",
        "after-environment-event",
        "after-environment",
        "after-version",
    ];

    impl Moving {
        /// [`Moving::new`] with `origin.store.prefix` dropped from the
        /// header of this host's copy and the bucket's, as a koto from
        /// before the session recorded its prefix left it, and a second key
        /// added after that: the session's home then follows its execution
        /// anchor, so the move takes it from the old directory's prefix to
        /// the new one's.
        fn unrecorded() -> Self {
            let mv = Moving::new();
            let path = mv.m.state_path(NAME);
            let text = std::fs::read_to_string(&path).unwrap();
            let (first, rest) = text.split_once('\n').unwrap();
            let mut header: Value = serde_json::from_str(first).unwrap();
            let store = header["origin"]["store"]
                .as_object_mut()
                .expect("setup: the header records no origin.store");
            assert!(
                store.remove("prefix").is_some(),
                "setup: a new session records its prefix: {header}"
            );
            let text = format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest);
            std::fs::write(&path, &text).unwrap();
            mv.s3.put(&mv.state_key(), text.as_bytes());
            let run = mv.m.add(&mv.old, NAME, "carried.md", "carried");
            assert!(
                run.ok(),
                "setup: context add after the prefix was dropped: {}",
                run.describe()
            );
            mv
        }
    }

    /// koto#739: `koto session rebind --whole` read the session's home
    /// before it moved anything and carried the context there from that
    /// home once the move was done. Stopped after its commit landed here and
    /// run again, the move read the home from the header it had already
    /// moved, found it unchanged and carried nothing, so the keys from
    /// before the move stayed under the old prefix and in this host's copy
    /// only. Each point is run on its own session, and every point whose
    /// carry is missing is named.
    #[test]
    fn a_cloud_whole_rebind_stopped_after_its_commit_still_carries_the_context_to_the_new_home() {
        let mut failed: Vec<String> = Vec::new();
        for point in CARRY_POINTS {
            let point = *point;
            let case = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mv = Moving::unrecorded();
                let new_prefix = prefix_of(&mv.new);
                assert_ne!(
                    new_prefix, mv.prefix,
                    "{point}: setup: the two directories share a prefix"
                );

                let stopped = mv.mover(&mv.new, &[(FAULT_ENV, point)], &[]);
                assert!(
                    !stopped.ok(),
                    "{point}: the stopped move exited 0: {}",
                    stopped.describe()
                );
                let run = mv.mover(&mv.new, &[], &[]);
                assert!(run.ok(), "{point}: run again: {}", run.describe());

                let local = mv.m.session_dir(NAME).join("ctx").join("manifest.json");
                let local: Value =
                    serde_json::from_slice(&std::fs::read(&local).unwrap_or_else(|e| {
                        panic!("{point}: no local manifest at {}: {e}", local.display())
                    }))
                    .unwrap();
                let manifest =
                    remote_json(&mv.s3, &session_key(&new_prefix, NAME, "ctx/manifest.json"))
                        .unwrap_or(Value::Null);
                for (key, value) in [("notes.md", "notes"), ("carried.md", "carried")] {
                    let object = session_key(&new_prefix, NAME, &format!("ctx/{key}"));
                    assert_eq!(
                        mv.s3.object(&object).as_deref(),
                        Some(value.as_bytes()),
                        "{point}: context key '{key}' from before the move isn't at the new \
                         home {object}; keys under it: {:?}; its manifest: {manifest}; the \
                         stopped move: {}; run again: {}",
                        mv.s3.keys_under(&format!("{new_prefix}/{NAME}/")),
                        stopped.describe(),
                        run.describe()
                    );
                    assert!(
                        manifest["keys"].get(key).is_some(),
                        "{point}: the new home's manifest doesn't list '{key}': {manifest}"
                    );
                    assert_eq!(
                        manifest["keys"][key]["hash"], local["keys"][key]["hash"],
                        "{point}: the new home's manifest doesn't name this host's copy of \
                         '{key}'"
                    );
                }
            }));
            if case.is_err() {
                failed.push(point.to_string());
            }
        }
        assert!(
            failed.is_empty(),
            "points after which the re-run left the context behind: {failed:?}"
        );
    }

    /// `koto session resolve <parent> --keep local --children accept-local`
    /// over a child moved with `--whole` elsewhere reports the child errored,
    /// as the move refuses this host's copy. The child's row says what to
    /// run instead, `koto session resolve child --keep remote`, and doesn't
    /// read as a stale write of this command's own: the command wrote the
    /// parent, and the child was moved before it ran, not after it read it.
    #[test]
    fn resolve_children_accept_local_names_keep_remote_for_a_moved_child() {
        let mv = Moving::new();
        mv.m.init(&mv.old, &[], "child", &["--parent", NAME]);
        let child_key = state_key(&mv.prefix, "child");
        assert!(
            mv.s3.object(&child_key).is_some(),
            "setup: no bucket copy of the child at {}; keys: {:?}",
            child_key,
            mv.s3.keys()
        );
        let moved = moved_copy(
            &remote_text(&mv.s3, &child_key),
            &mv.old,
            &mv.new,
            true,
            true,
        );
        mv.s3.put(&child_key, moved.as_bytes());

        let run = mv.m.run(
            &mv.old,
            &[],
            &[
                "session",
                "resolve",
                NAME,
                "--keep",
                "local",
                "--children",
                "accept-local",
            ],
        );

        assert_eq!(
            run.code,
            Some(0),
            "an errored child row doesn't fail the resolve: {}",
            run.describe()
        );
        let rows = run.json["children"].as_array().cloned().unwrap_or_default();
        let child = rows
            .iter()
            .find(|row| row["name"] == "child")
            .unwrap_or_else(|| panic!("no row for the child: {}", run.describe()));
        assert_eq!(child["action"], "errored", "{}", run.describe());
        let message = child["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("koto session resolve child --keep remote")
                || message.contains("--keep remote")
                || message.contains("accept-remote"),
            "the child's row doesn't say to keep the bucket's copy (`koto session resolve child \
             --keep remote` or `accept-remote`): {message}"
        );
        assert!(
            !message.contains("nothing was written"),
            "the child's row says nothing was written, though the resolve wrote the parent: \
             {message}"
        );
        assert!(
            !message.contains("after this command read it"),
            "the child's row says the child was rebound after this command read it, though it \
             was moved before the command ran: {message}"
        );
    }
}
