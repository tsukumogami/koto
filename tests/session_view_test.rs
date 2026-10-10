//! The session snapshot layer measured against real stores: a local
//! session with text, long, binary, invalid-name and invalid-hash keys, a
//! cloud session behind the in-process S3 endpoint, and a migrated one.
//!
//! Everything here asserts the layer's contract from DESIGN-session-view:
//! completeness of the key listing, bounded legible previews, the
//! migration answer, strict read-only behavior, and a metadata cost on
//! cloud that does not grow with the key count.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use fake_s3::FakeS3;
use koto::session::cloud::CloudBackend;
use koto::session::context::ContextStore;
use koto::session::local::LocalBackend;
use koto::session::{Backend, SessionBackend};
use koto::session_view::{read_key_preview, snapshot, KeyPreview, Locality, EXCERPT_BOUND};
use s3::creds::Credentials;
use s3::{Bucket, Region};

const BUCKET: &str = "koto-session-view";

fn header_of(workflow: &str) -> koto::engine::types::StateFileHeader {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": workflow,
        "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn initialized() -> koto::engine::types::Event {
    serde_json::from_value(serde_json::json!({
        "seq": 1,
        "timestamp": "2026-01-01T00:00:00Z",
        "type": "workflow_initialized",
        "payload": {"template_path": "", "variables": {}},
    }))
    .unwrap()
}

/// A local session named `wf` holding the fixture keys: a short text, a
/// text longer than the excerpt bound, and a binary blob.
fn local_fixture(base: &std::path::Path) -> Backend {
    let local = LocalBackend::with_base_dir(base.to_path_buf());
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    local.add("wf", "notes.md", b"short notes").unwrap();
    let long = "long line of text\n".repeat(400); // > 4096 bytes
    local
        .add_with_writer("wf", "research/long.md", long.as_bytes(), "agent")
        .unwrap();
    let blob: Vec<u8> = (0u32..4096).map(|i| (i * 31 % 256) as u8).collect();
    assert!(blob.contains(&0));
    local.add("wf", "data/blob.bin", &blob).unwrap();
    Backend::Local(local)
}

fn cloud_backend(s3: &FakeS3, base: &std::path::Path, prefix: &str) -> CloudBackend {
    let region = Region::Custom {
        region: "us-east-1".to_string(),
        endpoint: s3.endpoint(),
    };
    let credentials = Credentials::new(Some("k"), Some("s"), None, None, None).unwrap();
    let bucket = Bucket::new(BUCKET, region, credentials)
        .unwrap()
        .with_path_style();
    CloudBackend::with_parts(
        LocalBackend::with_base_dir(base.to_path_buf()),
        bucket,
        prefix.to_string(),
    )
}

fn marker(target: &str, workspace: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "target": {
            "session": target,
            "session_id": "fixture-session-id",
            "workspace": workspace,
            "prefix": "0123456789abcdef",
        },
        "machine_id": "another-machine",
        "migrated_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

/// Every byte of the session directory, for the read-only comparison.
fn dir_bytes(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.to_string_lossy().into_owned(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

// -- the listing ------------------------------------------------------------

#[test]
fn snapshot_lists_every_key_with_metadata_and_absent_facts_stay_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = local_fixture(tmp.path());

    let snap = snapshot(&backend, "wf").unwrap();
    assert!(snap.migrated.is_none());
    assert_eq!(snap.unlisted_keys, 0);

    // Lexicographic, complete.
    let names: Vec<&str> = snap.keys.iter().map(|k| k.display.as_str()).collect();
    assert_eq!(names, vec!["data/blob.bin", "notes.md", "research/long.md"]);
    for k in &snap.keys {
        assert!(k.raw.is_some());
        assert!(k.size.unwrap() > 0);
        assert_eq!(k.locality, Locality::Local);
        assert!(!k.stale);
        assert!(k.created_at.is_some(), "manifest created_at should parse");
    }
    let long = snap
        .keys
        .iter()
        .find(|k| k.display.as_str() == "research/long.md")
        .unwrap();
    assert_eq!(long.writer.as_ref().unwrap().as_str(), "agent");

    // Facts the fixture never recorded are absent, not invented.
    assert_eq!(snap.facts.name, "wf");
    assert!(snap.facts.intent.is_none());
    assert!(snap.facts.execution_dir.is_none());
    assert!(snap.facts.store.is_none());
    assert!(snap.facts.directive.is_none());
    assert_eq!(
        snap.facts.created_at.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
}

#[test]
fn a_hostile_manifest_entry_is_listed_but_never_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = local_fixture(tmp.path());

    // Tamper the manifest: a traversal-shaped name and a bogus hash.
    let manifest_path = tmp.path().join("wf/ctx/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["keys"]["../../etc/hosts"] = serde_json::json!({
        "created_at": "not a time",
        "size": 7,
        "hash": "not-hex",
    });
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    let snap = snapshot(&backend, "wf").unwrap();
    let bad = snap
        .keys
        .iter()
        .find(|k| k.display.as_str().contains("etc/hosts"))
        .unwrap();
    assert!(
        bad.raw.is_none(),
        "an invalid name is never joined to a path"
    );
    assert_eq!(bad.size, Some(7));
    assert!(
        bad.created_at.is_none(),
        "an unparseable timestamp is dropped"
    );

    assert!(matches!(
        read_key_preview(&backend, "wf", "../../etc/hosts", EXCERPT_BOUND),
        KeyPreview::InvalidName
    ));
}

// -- previews ---------------------------------------------------------------

#[test]
fn previews_classify_text_binary_and_unreadable_legibly() {
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = local_fixture(tmp.path());

    match read_key_preview(&backend, "wf", "notes.md", EXCERPT_BOUND) {
        KeyPreview::Text { excerpt, truncated } => {
            assert_eq!(excerpt.as_str(), "short notes");
            assert!(!truncated);
        }
        other => panic!("short text should preview as text, got {other:?}"),
    }

    match read_key_preview(&backend, "wf", "research/long.md", EXCERPT_BOUND) {
        KeyPreview::Text { excerpt, truncated } => {
            assert!(truncated, "a value over the bound carries the flag");
            assert!(excerpt.as_str().len() <= EXCERPT_BOUND);
        }
        other => panic!("long text should preview as truncated text, got {other:?}"),
    }

    match read_key_preview(&backend, "wf", "data/blob.bin", EXCERPT_BOUND) {
        KeyPreview::Binary { size, hash } => {
            assert_eq!(size, 4096, "the recorded size, from the manifest");
            assert!(hash.is_some(), "the store's own hash is 64 hex chars");
        }
        other => panic!("a NUL-bearing value is binary, got {other:?}"),
    }

    // An unreadable key reports itself; every other key still renders.
    let blob_path = tmp.path().join("wf/ctx/data/blob.bin");
    let mut perms = std::fs::metadata(&blob_path).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o000);
    std::fs::set_permissions(&blob_path, perms).unwrap();
    match read_key_preview(&backend, "wf", "data/blob.bin", EXCERPT_BOUND) {
        KeyPreview::Unreadable { reason } => assert!(!reason.as_str().is_empty()),
        other => panic!("chmod 000 should read as unreadable, got {other:?}"),
    }
    assert!(matches!(
        read_key_preview(&backend, "wf", "notes.md", EXCERPT_BOUND),
        KeyPreview::Text { .. }
    ));
}

#[test]
fn a_fifo_or_symlink_at_a_key_path_is_rejected_without_hanging() {
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = local_fixture(tmp.path());
    let ctx = tmp.path().join("wf/ctx");

    // A FIFO where content should be: lstat rejects before any open, so
    // this returns instead of blocking on a writerless pipe.
    let fifo = ctx.join("notes.md");
    std::fs::remove_file(&fifo).unwrap();
    let cpath = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        read_key_preview(&backend, "wf", "notes.md", EXCERPT_BOUND),
        KeyPreview::Unreadable { .. }
    ));

    // A symlink leaf pointing outside the store.
    let outside = tmp.path().join("outside.txt");
    std::fs::write(&outside, b"secret").unwrap();
    let link = ctx.join("data/blob.bin");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(matches!(
        read_key_preview(&backend, "wf", "data/blob.bin", EXCERPT_BOUND),
        KeyPreview::Unreadable { .. }
    ));

    // A symlinked intermediate directory escaping ctx/.
    let outside_dir = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("long.md"), b"planted").unwrap();
    let research = ctx.join("research");
    std::fs::remove_dir_all(&research).unwrap();
    std::os::unix::fs::symlink(&outside_dir, &research).unwrap();
    assert!(matches!(
        read_key_preview(&backend, "wf", "research/long.md", EXCERPT_BOUND),
        KeyPreview::Unreadable { .. }
    ));
}

// -- read-only --------------------------------------------------------------

#[test]
fn rendering_writes_nothing_and_appends_no_event() {
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = local_fixture(tmp.path());
    let before = dir_bytes(&tmp.path().join("wf"));
    let (_, events_before) = backend.read_events("wf").unwrap();

    let snap = snapshot(&backend, "wf").unwrap();
    assert_eq!(snap.keys.len(), 3);
    let mut rendered = 0;
    for key in snap.keys.iter().filter_map(|k| k.raw.as_deref()) {
        match read_key_preview(&backend, "wf", key, EXCERPT_BOUND) {
            KeyPreview::Text { excerpt, .. } => {
                assert!(!excerpt.as_str().is_empty());
                rendered += 1;
            }
            KeyPreview::Binary { size, .. } => {
                assert!(size > 0);
                rendered += 1;
            }
            other => panic!("fixture keys should render, got {other:?}"),
        }
    }
    assert_eq!(rendered, 3, "the render produced non-empty output");

    let after = dir_bytes(&tmp.path().join("wf"));
    assert_eq!(before, after, "the session store is byte-identical");
    let (_, events_after) = backend.read_events("wf").unwrap();
    assert_eq!(events_before.len(), events_after.len(), "no event appended");
}

// -- cloud ------------------------------------------------------------------

/// Add `n` keys to a cloud session and return the request count a full
/// metadata snapshot costs on a fresh backend (fresh caches), asserting
/// no `ctx/` content GET and no write happened on the way.
fn cloud_snapshot_cost(s3: &FakeS3, base: &std::path::Path, prefix: &str, n: usize) -> usize {
    let writer = cloud_backend(s3, base, prefix);
    writer
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    for i in 0..n {
        writer
            .add(
                "wf",
                &format!("k{i:03}.md"),
                format!("value {i}").as_bytes(),
            )
            .unwrap();
    }

    // A fresh backend: no manifest cache, no migration-check cache.
    let reader = cloud_backend(s3, base, prefix);
    s3.clear_requests();
    let snap = snapshot(&Backend::Cloud(reader), "wf").unwrap();
    assert_eq!(snap.keys.len(), n);
    assert!(snap.keys.iter().all(|k| k.locality == Locality::Local));

    let requests = s3.requests();
    for r in &requests {
        assert_ne!(r.method, "PUT", "a snapshot never writes: {}", r.key);
        assert_ne!(r.method, "DELETE", "a snapshot never deletes: {}", r.key);
        if r.method == "GET" && r.list_prefix.is_none() {
            assert!(
                !r.key.contains("/ctx/") || r.key.ends_with("/ctx/manifest.json"),
                "a snapshot never fetches key content: {}",
                r.key
            );
        }
    }
    requests.len()
}

#[test]
fn cloud_metadata_cost_does_not_grow_with_the_key_count() {
    let s3 = FakeS3::start(BUCKET);
    let tmp_a = tempfile::TempDir::new().unwrap();
    let tmp_b = tempfile::TempDir::new().unwrap();
    let small = cloud_snapshot_cost(&s3, tmp_a.path(), "pfx-small", 5);
    let large = cloud_snapshot_cost(&s3, tmp_b.path(), "pfx-large", 50);
    assert_eq!(
        small, large,
        "metadata requests must not grow with the key count"
    );
}

#[test]
fn a_remote_only_key_shows_metadata_and_a_not_local_state() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let writer = cloud_backend(&s3, tmp.path(), "pfx");
    writer
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    writer.add("wf", "local.md", b"here").unwrap();

    // A key only the remote manifest knows: another host added it.
    let remote_manifest = serde_json::json!({
        "keys": {
            "local.md": serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(tmp.path().join("wf/ctx/manifest.json")).unwrap()
            ).unwrap()["keys"]["local.md"],
            "remote-only.md": {
                "created_at": "2026-01-02T00:00:00Z",
                "size": 42,
                "hash": "a".repeat(64),
            },
        }
    });
    s3.put(
        "pfx/wf/ctx/manifest.json",
        &serde_json::to_vec(&remote_manifest).unwrap(),
    );

    let reader = cloud_backend(&s3, tmp.path(), "pfx");
    let backend = Backend::Cloud(reader);
    let snap = snapshot(&backend, "wf").unwrap();
    let remote = snap
        .keys
        .iter()
        .find(|k| k.display.as_str() == "remote-only.md")
        .unwrap();
    assert_eq!(remote.locality, Locality::RemoteOnly);
    assert_eq!(remote.size, Some(42));

    s3.clear_requests();
    match read_key_preview(&backend, "wf", "remote-only.md", EXCERPT_BOUND) {
        KeyPreview::NotLocal { size, hash } => {
            assert_eq!(size, 42);
            assert!(hash.is_some());
        }
        other => panic!("a remote-only key is shown, not fetched: {other:?}"),
    }
    assert!(
        !s3.requests().iter().any(|r| r.method == "GET"
            && r.key.contains("/ctx/")
            && !r.key.ends_with("manifest.json")),
        "the preview never pulled remote content"
    );
}

#[test]
fn a_migrated_session_answers_with_its_successor_and_lists_no_keys() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let writer = cloud_backend(&s3, tmp.path(), "pfx");
    writer
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    writer.add("wf", "notes.md", b"old copy").unwrap();

    s3.put(
        "pfx/wf/migrated.json",
        &marker("wf-elsewhere", "/srv/other-workspace"),
    );

    let reader = cloud_backend(&s3, tmp.path(), "pfx");
    let snap = snapshot(&Backend::Cloud(reader), "wf").unwrap();
    let migrated = snap.migrated.expect("the marker answers the snapshot");
    assert_eq!(migrated.target.as_str(), "wf-elsewhere");
    assert_eq!(migrated.workspace.as_str(), "/srv/other-workspace");
    assert!(snap.keys.is_empty(), "no key list masquerades as content");
    // The local copy's facts still identify the session.
    assert_eq!(snap.facts.name, "wf");
}
