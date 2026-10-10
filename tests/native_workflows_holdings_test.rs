//! The holdings summary measured against real stores: the written
//! `/workflows` file reflects the event log (not the manifest), and the
//! render path makes no remote request on a cloud-backed session.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use fake_s3::FakeS3;
use koto::session::cloud::CloudBackend;
use koto::session::context::ContextStore;
use koto::session::context_log::added_event;
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use koto::workflows_surface::{materialize_after_commit, publish_location, workflow_filename};
use s3::creds::Credentials;
use s3::{Bucket, Region};

const BUCKET: &str = "koto-native-workflows";
const CONTENT_MARKER: &str = "CONTENT-MARKER-NEVER-RENDERED";

fn header_of(workflow: &str) -> koto::engine::types::StateFileHeader {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": workflow,
        "session_id": "holdings-uuid",
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

/// The written file reflects the log, and the render costs no remote call.
///
/// The fixture is a cloud-backed session whose manifest deliberately
/// disagrees with its log: `stale.md` was added through the store (so the
/// manifest lists it) and then removed by a bare `context_removed` event
/// that left the manifest untouched. The holdings fold follows the log, so
/// the written file drops the key while the manifest still carries it.
///
/// The render is then run exactly as production runs it -- the local
/// backend/store pair `LocalBackend::append_event` hands to
/// `materialize_after_commit` -- with every object in the bucket set to
/// fail on GET, PUT and DELETE. The file is written, no key content
/// reaches it, and the fake S3 records zero requests.
#[test]
fn holdings_reflect_the_log_and_the_render_makes_no_remote_call() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();

    let writer = cloud_backend(&s3, tmp.path(), "pfx");
    writer
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    publish_location(&writer, &writer, "wf", wf_dir.path().to_str().unwrap()).unwrap();

    // Store write + event append, the two halves `koto context add` makes.
    let add = |key: &str, content: &[u8]| {
        writer.add("wf", key, content).unwrap();
        writer
            .append_event(
                "wf",
                &added_event(key, content, "agent"),
                "2026-01-01T00:04:00Z",
            )
            .unwrap();
    };
    add("notes.md", b"short notes");
    add("stale.md", CONTENT_MARKER.as_bytes());

    // The disagreement: remove `stale.md` in the log only. The manifest
    // (and the stored content) keep it.
    writer
        .append_event(
            "wf",
            &koto::engine::types::EventPayload::ContextRemoved {
                key: "stale.md".to_string(),
                writer: Some("agent".to_string()),
            },
            "2026-01-01T00:05:00Z",
        )
        .unwrap();
    let manifest = std::fs::read_to_string(tmp.path().join("wf/ctx/manifest.json")).unwrap();
    assert!(
        manifest.contains("stale.md"),
        "fixture precondition: the manifest still lists the removed key"
    );

    // Production pair: `LocalBackend::append_event` materializes with its
    // own local backend and store. Fail every bucket object and count.
    let local_view = LocalBackend::with_base_dir(tmp.path().to_path_buf());
    for key in s3.keys() {
        for method in ["GET", "PUT", "DELETE"] {
            s3.fail(method, &key, 500);
        }
    }
    s3.clear_requests();

    materialize_after_commit(&local_view, &local_view, "wf");

    assert!(
        s3.requests().is_empty(),
        "the render path made a remote request: {:?}",
        s3.requests()
    );

    let path = wf_dir.path().join(workflow_filename("holdings-uuid"));
    let bytes = std::fs::read(&path).expect("file written");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let holdings = &v["koto"]["holdings"];
    let names: Vec<&str> = holdings["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["key"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"notes.md"), "live key listed: {names:?}");
    assert!(
        !names.contains(&"stale.md"),
        "the log wins over the manifest: {names:?}"
    );

    // No content byte reaches the file, even for a key the store holds.
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains(CONTENT_MARKER), "no key content in the file");
    assert!(!text.contains("short notes"), "no key content in the file");
}

/// Re-adding and removing keys across the log yields last-writer-wins
/// results in the written file (the end-to-end pin of the fold's
/// unit-tested behavior; the no-keys case is pinned at the fold, since a
/// published session always holds the publish-location key).
#[test]
fn holdings_follow_the_last_writer_across_the_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    publish_location(&local, &local, "wf", wf_dir.path().to_str().unwrap()).unwrap();

    let add = |key: &str, content: &[u8]| {
        local.add("wf", key, content).unwrap();
        local
            .append_event(
                "wf",
                &added_event(key, content, "agent"),
                "2026-01-01T00:04:00Z",
            )
            .unwrap();
    };
    add("a.md", b"one");
    add("a.md", b"one, rewritten longer");
    add("b.md", b"two");
    local.remove("wf", "b.md").unwrap();
    local
        .append_event(
            "wf",
            &koto::engine::types::EventPayload::ContextRemoved {
                key: "b.md".to_string(),
                writer: Some("agent".to_string()),
            },
            "2026-01-01T00:05:00Z",
        )
        .unwrap();

    let path = wf_dir.path().join(workflow_filename("holdings-uuid"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let holdings = &v["koto"]["holdings"];
    let keys = holdings["keys"].as_array().unwrap();
    let a = keys
        .iter()
        .find(|k| k["key"] == "a.md")
        .expect("re-added key listed once");
    assert_eq!(
        a["size"].as_u64().unwrap(),
        b"one, rewritten longer".len() as u64,
        "the last write's size wins"
    );
    assert!(!keys.iter().any(|k| k["key"] == "b.md"), "removed key gone");
}
