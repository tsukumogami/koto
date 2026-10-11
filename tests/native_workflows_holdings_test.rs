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
/// `materialize_after_commit` (src/session/local.rs) -- with every object
/// in the bucket set to fail on GET, PUT and DELETE. The file is written,
/// no key content reaches it, and the fake S3 records zero requests.
///
/// The construction limit, stated plainly: the local pair carries no S3
/// client, so the zero-request assertion holds by construction for this
/// invocation -- it documents the read-path contract rather than
/// discovering it, and it cannot see a rewiring of the production call
/// site itself (that line is `LocalBackend::append_event`'s). What the
/// test does prove on a cloud-backed fixture is the part that matters to
/// a reader of the file: the render handed the production pair writes the
/// correct holdings from the local copy alone, with the bucket actively
/// hostile.
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

/// The whole holdings object is bounded at its caps: 50 longest-legal key
/// names with maximal sizes plus a capped anchor and store serialize to a
/// bounded blob, so the enriched file cannot grow past a glance surface's
/// budget however hostile the log.
#[test]
fn holdings_at_every_cap_stay_bounded() {
    use koto::session::validate::MAX_CONTEXT_KEY_LEN;
    use koto::session_view::{CUT_MARKER, FACT_CAP};
    use koto::workflows_surface::{HoldingKey, Holdings, HOLDINGS_KEY_CAP, HOLDINGS_STORE_CAP};

    // The worst case each cap allows: the longest legal key names (ASCII
    // by grammar), and anchor/store at their character caps plus the cut
    // marker, built from 4-byte characters so the byte count is maximal.
    let wide = '\u{1F600}';
    let holdings = Holdings {
        key_count: usize::MAX,
        total_bytes: u64::MAX,
        keys: (0..HOLDINGS_KEY_CAP)
            .map(|i| {
                let prefix = format!("{i:03}");
                HoldingKey {
                    key: format!("{prefix}{}", "k".repeat(MAX_CONTEXT_KEY_LEN - prefix.len())),
                    size: u64::MAX,
                }
            })
            .collect(),
        keys_truncated: true,
        anchor: Some(format!("{}{CUT_MARKER}", wide.to_string().repeat(FACT_CAP))),
        store: Some(format!(
            "{}{CUT_MARKER}",
            wide.to_string().repeat(HOLDINGS_STORE_CAP)
        )),
    };
    let bytes = serde_json::to_vec(&holdings).unwrap();
    assert!(
        bytes.len() < 24 * 1024,
        "holdings at every cap must stay under 24 KiB, got {}",
        bytes.len()
    );
}

/// A malformed line mid-log makes the projection underivable; the render
/// then writes nothing, leaving the last good file intact rather than
/// replacing it with a partial or empty one.
#[test]
fn a_malformed_log_line_leaves_the_last_written_file_intact() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wf_dir = tempfile::TempDir::new().unwrap();
    let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    publish_location(&local, &local, "wf", wf_dir.path().to_str().unwrap()).unwrap();
    local.add("wf", "good.md", b"fine").unwrap();
    local
        .append_event(
            "wf",
            &added_event("good.md", b"fine", "agent"),
            "2026-01-01T00:04:00Z",
        )
        .unwrap();

    let path = wf_dir.path().join(workflow_filename("holdings-uuid"));
    let healthy = std::fs::read(&path).expect("file written while the log was healthy");

    // Control: while the log is healthy, a further event DOES rewrite the
    // file, so the equality asserted below is the render declining to write,
    // not a render that would never have changed anything.
    local.add("wf", "more.md", b"more").unwrap();
    local
        .append_event(
            "wf",
            &added_event("more.md", b"more", "agent"),
            "2026-01-01T00:04:30Z",
        )
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_ne!(healthy, before, "a healthy event rewrites the file");

    // Corrupt the log the way a torn write would, then render again.
    let state_path = tmp.path().join("wf").join("koto-wf.state.jsonl");
    let mut log = std::fs::read_to_string(&state_path).unwrap();
    log.push_str("{this is not json\n");
    std::fs::write(&state_path, log).unwrap();

    materialize_after_commit(&local, &local, "wf");

    let after = std::fs::read(&path).unwrap();
    assert_eq!(
        before, after,
        "an underivable projection must not touch the published file"
    );
}
