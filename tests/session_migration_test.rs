//! `koto session import` measured end to end: a session moves between
//! simulated hosts through an in-process S3-compatible endpoint.
//!
//! Each host is its own `HOME`, `XDG_CACHE_HOME` and workspace directory
//! (see `tests/support/migration_carrier.rs`); the endpoint is
//! `tests/support/fake_s3.rs`, which records every request, so the tests
//! can say exactly what the import wrote under which prefix. Nothing here
//! needs a secret or the network, so it runs with the rest of the suite.
//! The same carrier steps run against a real bucket in
//! `tests/cloud_integration_test.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use ::s3::creds::Credentials;
use ::s3::{Bucket, Region};
use fake_s3::FakeS3;
use koto::session::cloud::CloudBackend;
use koto::session::context::ContextStore;
use koto::session::local::LocalBackend;
use koto::session::{SessionBackend, SessionMigrated};
use migration_carrier::{ensure, sha256_hex, Carrier, Cloud, Host, Run};

const BUCKET: &str = "koto-migration";

fn cloud(s3: &FakeS3) -> Cloud {
    Cloud {
        endpoint: s3.endpoint(),
        bucket: s3.bucket().to_string(),
        region: "us-east-1".to_string(),
        path_style: true,
        credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
    }
}

/// Requests that wrote under `prefix`: every PUT and DELETE there, as
/// (method, key).
fn writes_under(s3: &FakeS3, prefix: &str) -> Vec<(String, String)> {
    s3.requests()
        .into_iter()
        .filter(|r| (r.method == "PUT" || r.method == "DELETE") && r.key.starts_with(prefix))
        .map(|r| (r.method, r.key))
        .collect()
}

fn session_prefix(host: &Host, name: &str) -> String {
    format!("{}/{}/", host.prefix(), name)
}

/// A recorded request as `METHOD key`, or `LIST prefix` for a listing.
fn describe(r: &fake_s3::Request) -> String {
    match &r.list_prefix {
        Some(prefix) => format!("LIST {}", prefix),
        None => format!("{} {}", r.method, r.key),
    }
}

/// The marker check costs `koto status` on a session that was never
/// migrated exactly one request beyond what it made before the check
/// existed, and no retry sleep. The baseline, `before_the_check`, is the
/// list of requests `koto status` made before the marker check existed
/// (measured with the check switched off): the GET that pulls the state
/// file, and the listing of this workspace's sessions it scans for
/// superseded branches. The test asserts the exact list: the marker
/// listing, then the baseline, nothing else. The check lists for the
/// marker, which answers 200 whether or not it is there, where a GET of a
/// missing marker would be a 404 that rust-s3 retries after a second.
#[test]
fn the_marker_check_adds_one_request_and_no_retry_to_status() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let a = Host::new(root.path(), "a", &cloud(&s3));
    let name = "plain";
    let template = a.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = a.koto(&["init", name, "--template", template.to_str().unwrap()]);
    assert!(run.ok(), "init: {}", run.describe());

    s3.clear_requests();
    let run = a.koto(&["status", name]);
    assert!(run.ok(), "status: {}", run.describe());

    let session = session_prefix(&a, name);
    let requests = s3.requests();
    let seen: Vec<String> = requests.iter().map(describe).collect();
    let before_the_check = vec![
        // The state pull ...
        format!("GET {}koto-{}.state.jsonl", session, name),
        // ... and the session listing behind `superseded_branches`.
        format!("LIST {}/", a.prefix()),
    ];
    // The marker check: the one added request.
    let mut expected = vec![format!("LIST {}migrated.json", session)];
    expected.extend(before_the_check);
    assert_eq!(seen, expected, "koto status made {:?}", seen);
    assert_eq!(seen.len(), 3, "exactly one request over the baseline's two");
    let check = requests[1].at.duration_since(requests[0].at);
    assert!(
        check < std::time::Duration::from_millis(500),
        "the marker check took {:?}; a retried 404 sleeps a full second",
        check
    );
}

/// The whole carrier, with the endpoint's record checked around the import.
#[test]
fn carrier_moves_a_session_from_a_to_b_to_c() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    let name = "carrier";
    let mut carrier = Carrier::new(&a, &b, &c, name);

    carrier.init_a();
    carrier.keys_a();

    let a_session = session_prefix(&a, name);
    let b_session = session_prefix(&b, name);
    let a_objects_before: Vec<(String, Vec<u8>)> = s3
        .keys_under(&a_session)
        .into_iter()
        .map(|k| {
            let v = s3.object(&k).unwrap();
            (k, v)
        })
        .collect();

    s3.clear_requests();
    carrier.import_b();
    let step = "import-b";
    let import_requests: Vec<String> = s3.requests().iter().map(describe).collect();
    println!(
        "import-b made {} requests:\n  {}",
        import_requests.len(),
        import_requests.join("\n  ")
    );

    // The import wrote exactly one object under A's prefix, the marker,
    // and left every object A had there byte for byte.
    let marker_key = format!("{}migrated.json", a_session);
    ensure(
        step,
        writes_under(&s3, &a.prefix()) == vec![("PUT".to_string(), marker_key.clone())],
        format!(
            "writes under A's prefix were {:?}",
            writes_under(&s3, &a.prefix())
        ),
    );
    for (key, bytes) in &a_objects_before {
        ensure(
            step,
            s3.object(key).as_ref() == Some(bytes),
            format!("{} changed under A's prefix", key),
        );
    }

    // Every object of the new session is under B's prefix ...
    let mut expected = vec![
        format!("{}koto-{}.state.jsonl", b_session, name),
        format!("{}template.json", b_session),
        format!("{}ctx/manifest.json", b_session),
        format!("{}version.json", b_session),
    ];
    for key in carrier.key_hashes.keys() {
        expected.push(format!("{}ctx/{}", b_session, key));
    }
    expected.sort();
    ensure(
        step,
        s3.keys_under(&b_session) == expected,
        format!(
            "B's prefix holds {:?}, expected {:?}",
            s3.keys_under(&b_session),
            expected
        ),
    );
    ensure(
        step,
        s3.object(&format!("{}koto-{}.state.jsonl", b_session, name))
            == std::fs::read(b.state_path(name)).ok(),
        "the pushed state file isn't B's local one",
    );
    ensure(
        step,
        s3.object(&format!("{}template.json", b_session))
            .map(|t| sha256_hex(&t))
            == Some(carrier.template_hash()),
        "the pushed template.json isn't the session's template",
    );

    // ... and every one of them was pushed before the marker.
    let requests = s3.requests();
    let marker_at = requests
        .iter()
        .position(|r| r.method == "PUT" && r.key == marker_key)
        .unwrap();
    for key in &expected {
        let pushed_at = requests
            .iter()
            .position(|r| r.method == "PUT" && &r.key == key);
        ensure(
            step,
            pushed_at.is_some_and(|i| i < marker_at),
            format!("{} was not pushed before the marker", key),
        );
    }

    // The marker names the target in full.
    let marker: serde_json::Value =
        serde_json::from_slice(&s3.object(&marker_key).unwrap()).unwrap();
    let b_header = b.header(name);
    ensure(
        step,
        marker["schema"] == 1
            && marker["target"]["session"] == name
            && marker["target"]["session_id"] == b_header["session_id"]
            && marker["target"]["workspace"] == b.ws_str().as_str()
            && marker["target"]["prefix"] == b.prefix().as_str()
            && marker["machine_id"].as_str().is_some_and(|m| !m.is_empty())
            && marker["migrated_at"]
                .as_str()
                .is_some_and(|t| !t.is_empty()),
        format!("the marker is incomplete: {}", marker),
    );

    carrier.keys_b();
    carrier.advance_b();
    carrier.refuse_a();
    carrier.reimport_c();

    // Moving on from B marked B, not A again: A's prefix gained nothing.
    ensure(
        "reimport-c",
        s3.object(&format!("{}migrated.json", b_session)).is_some(),
        "B's copy carries no marker",
    );
    println!("{}", carrier.report());
}

/// `session.cloud.path_style` set through `koto config set` makes an IP
/// endpoint work for the ordinary commands.
#[test]
fn path_style_lets_an_ip_endpoint_serve_the_ordinary_commands() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let mut cloud = cloud(&s3);
    cloud.path_style = false;
    let host = Host::new(root.path(), "a", &cloud);
    let name = "plain";

    let run = host.koto(&["config", "set", "session.cloud.path_style", "true"]);
    assert!(run.ok(), "config set: {}", run.describe());
    let run = host.koto(&["config", "get", "session.cloud.path_style"]);
    assert_eq!(run.stdout.trim(), "true", "{}", run.describe());

    let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = host.koto(&["init", name, "--template", template.to_str().unwrap()]);
    assert!(run.ok(), "init: {}", run.describe());
    let prefix = session_prefix(&host, name);
    assert!(
        s3.object(&format!("{}koto-{}.state.jsonl", prefix, name))
            .is_some(),
        "init pushed no state file; bucket holds {:?}",
        s3.keys()
    );

    let src = host.home.join("value.txt");
    std::fs::write(&src, b"hello").unwrap();
    let run = host.koto(&[
        "context",
        "add",
        name,
        "notes.md",
        "--from-file",
        src.to_str().unwrap(),
    ]);
    assert!(run.ok(), "context add: {}", run.describe());
    assert_eq!(
        s3.object(&format!("{}ctx/notes.md", prefix)).as_deref(),
        Some(&b"hello"[..])
    );

    let run = host.koto(&["context", "get", name, "notes.md"]);
    assert!(run.ok(), "context get: {}", run.describe());
    assert_eq!(run.stdout, "hello");

    // A session only the bucket knows shows up in the list, which takes a
    // delimited listing to find.
    s3.put(
        &format!("{}/remote-only/koto-remote-only.state.jsonl", host.prefix()),
        b"{}",
    );
    let run = host.koto(&["session", "list"]);
    assert!(run.ok(), "session list: {}", run.describe());
    let ids: Vec<&str> = run
        .json
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r["id"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(ids, vec![name, "remote-only"], "{}", run.describe());

    let run = host.koto(&["session", "cleanup", name]);
    assert!(run.ok(), "session cleanup: {}", run.describe());
    assert!(
        s3.keys_under(&prefix).is_empty(),
        "cleanup left {:?}",
        s3.keys_under(&prefix)
    );
}

/// Start a session in `host` with the carrier template and one key.
fn start_session(host: &Host, name: &str) {
    let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = host.koto(&["init", name, "--template", template.to_str().unwrap()]);
    assert!(run.ok(), "init {}: {}", name, run.describe());
    let src = host.home.join(format!("{}-notes.md", name));
    std::fs::write(&src, b"notes").unwrap();
    let run = host.koto(&[
        "context",
        "add",
        name,
        "notes.md",
        "--from-file",
        src.to_str().unwrap(),
    ]);
    assert!(run.ok(), "context add: {}", run.describe());
}

fn compile(host: &Host) {
    let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = host.koto(&["template", "compile", template.to_str().unwrap()]);
    assert!(run.ok(), "compile: {}", run.describe());
}

/// `koto session cleanup` on the source keeps its marker, and so does the
/// cleanup a terminal tick runs.
#[test]
fn cleanup_and_a_terminal_tick_leave_the_marker_in_place() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);

    // Cleanup on the source after an import.
    start_session(&a, "moved");
    compile(&b);
    let run = b.koto(&["session", "import", "moved", "--from", &a.ws_str()]);
    assert!(run.ok(), "import: {}", run.describe());
    let moved = session_prefix(&a, "moved");
    let run = a.koto(&["session", "cleanup", "moved"]);
    assert!(run.ok(), "cleanup: {}", run.describe());
    assert_eq!(
        s3.keys_under(&moved),
        vec![format!("{}migrated.json", moved)],
        "cleanup must delete everything but the marker"
    );

    // A terminal tick on a session whose marker check can't get an answer
    // (the listing for the marker fails): the check fails open with a
    // warning, the tick reaches the terminal state, and its cleanup, whose
    // own listing of the session works, deletes the session's objects but
    // not the marker.
    start_session(&a, "ticked");
    let ticked = session_prefix(&a, "ticked");
    let marker = format!("{}migrated.json", ticked);
    s3.put(&marker, br#"{"schema":1}"#);
    s3.fail("LIST", &marker, 500);
    for args in [
        vec!["next", "ticked"],
        vec!["next", "ticked", "--with-data", r#"{"choice":"go"}"#],
        vec!["next", "ticked", "--with-data", r#"{"verdict":"approve"}"#],
    ] {
        let run = a.koto(&args);
        assert!(run.ok(), "{:?}: {}", args, run.describe());
        assert!(
            run.stderr
                .contains("warning: cloud sync: migration check failed"),
            "an unreadable marker must warn: {}",
            run.describe()
        );
    }
    assert_eq!(
        s3.keys_under(&ticked),
        vec![marker.clone()],
        "the terminal tick's cleanup must keep only the marker"
    );
    assert!(!a.session_dir("ticked").exists());
}

/// Staging directories an import left in `host`'s session store.
fn staging_dirs(host: &Host) -> Vec<String> {
    let store = host.home.join(".koto").join("sessions");
    std::fs::read_dir(&store)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".import-"))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert an import into `host` as `target` left nothing behind: no local
/// session directory, no staging directory, no object under the target's
/// prefix.
fn assert_no_trace(s3: &FakeS3, host: &Host, target: &str) {
    assert!(
        !host.session_dir(target).exists(),
        "a local session '{}' was left in {}",
        target,
        host.label
    );
    assert_eq!(
        staging_dirs(host),
        Vec::<String>::new(),
        "a staging directory was left in {}",
        host.label
    );
    assert_eq!(
        s3.keys_under(&session_prefix(host, target)),
        Vec::<String>::new(),
        "objects were left under {}'s prefix for '{}'",
        host.label,
        target
    );
}

/// What `host` holds for session `name`, here and in the bucket, so a
/// refusal over a name another session owns can be shown to change none of
/// it.
type Holdings = (Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn holdings(s3: &FakeS3, host: &Host, name: &str) -> Holdings {
    let local = std::fs::read(host.state_path(name)).ok();
    let remote = s3
        .keys_under(&session_prefix(host, name))
        .into_iter()
        .map(|k| {
            let v = s3.object(&k).unwrap();
            (k, v)
        })
        .collect();
    (local, remote)
}

/// Assert `run` is the import refusal `code` at `exit`, and return its
/// message.
fn refused(run: &Run, code: &str, exit: i32) -> String {
    assert_eq!(run.code, Some(exit), "{}: {}", code, run.describe());
    assert_eq!(run.json["error"]["code"], code, "{}", run.describe());
    run.json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Each caller-actionable refusal, and that none of them leaves a trace:
/// nothing written under the source's prefix, and in the importing
/// workspace no local session, no staging directory and no object under
/// the target's prefix (or, where another session owns the name, nothing of
/// that session's changed).
#[test]
fn import_refusals() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    start_session(&a, "wf");
    s3.clear_requests();

    // The local backend has no remote copy to read.
    let local = Host::new(root.path(), "local", &cloud);
    std::fs::remove_file(local.ws.join(".koto").join("config.toml")).unwrap();
    let run = local.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    refused(&run, "import_requires_cloud", 2);
    assert_no_trace(&s3, &local, "wf");

    // No such session under A's prefix.
    let run = b.koto(&["session", "import", "nope", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_source_not_found", 2);
    assert!(msg.contains("nope"), "{msg}");
    assert_no_trace(&s3, &b, "nope");

    // A relative --from can't name another host's workspace.
    let run = b.koto(&["session", "import", "wf", "--from", "ws"]);
    refused(&run, "import_source_not_found", 2);
    assert_no_trace(&s3, &b, "wf");

    // The template isn't in B's cache: the refusal names the hash and the
    // template's file name.
    let hash = a.header("wf")["template_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let run = b.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_template_unavailable", 2);
    assert!(
        msg.contains(&hash) && msg.contains(migration_carrier::TEMPLATE_FILE),
        "{msg}"
    );
    assert!(msg.contains("koto template compile"), "{msg}");
    assert_no_trace(&s3, &b, "wf");

    // A cached file that doesn't hash to the session's hash is refused too.
    let cached = b.cache.join("koto").join(format!("{}.json", hash));
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::write(&cached, b"{}").unwrap();
    let run = b.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    refused(&run, "import_template_unavailable", 2);
    assert_no_trace(&s3, &b, "wf");
    std::fs::remove_file(&cached).unwrap();
    compile(&b);

    // The name is taken locally, by a session that isn't an import of
    // this source: the refusal points at --as, and B's own session is
    // left exactly as it was.
    start_session(&b, "wf");
    let before = holdings(&s3, &b, "wf");
    let run = b.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_name_taken", 2);
    assert!(msg.contains("--as"), "{msg}");
    assert_eq!(holdings(&s3, &b, "wf"), before);
    assert_eq!(staging_dirs(&b), Vec::<String>::new());

    // ... or under this workspace's prefix only.
    compile(&c);
    s3.put(&format!("{}/wf/koto-wf.state.jsonl", c.prefix()), b"{}\n");
    let run = c.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_name_taken", 2);
    assert!(msg.contains("--as"), "{msg}");
    assert!(!c.session_dir("wf").exists());
    assert_eq!(staging_dirs(&c), Vec::<String>::new());
    assert_eq!(
        s3.keys_under(&session_prefix(&c, "wf")),
        vec![format!("{}/wf/koto-wf.state.jsonl", c.prefix())],
        "only the object that owns the name may be under C's prefix"
    );
    s3.remove(&format!("{}/wf/koto-wf.state.jsonl", c.prefix()));
    assert_no_trace(&s3, &c, "wf");

    // Nothing so far wrote under A's prefix.
    assert_eq!(writes_under(&s3, &a.prefix()), vec![]);

    // A child session isn't imported on its own.
    let run = a.koto(&["session", "start", "wf.child", "--parent", "wf"]);
    assert!(run.ok(), "session start: {}", run.describe());
    s3.clear_requests();
    let run = c.koto(&["session", "import", "wf.child", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_source_is_child", 2);
    assert!(msg.contains("wf.child"), "{msg}");
    assert_no_trace(&s3, &c, "wf.child");
    assert_eq!(writes_under(&s3, &a.prefix()), vec![]);

    // An import that succeeds marks the source; a second one is refused,
    // naming where it went.
    let run = c.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    assert!(run.ok(), "import: {}", run.describe());
    let d = Host::new(root.path(), "d", &cloud);
    compile(&d);
    let run = d.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_source_migrated", 2);
    assert!(msg.contains(&c.ws_str()), "{msg}");
    assert_no_trace(&s3, &d, "wf");
    assert_eq!(
        writes_under(&s3, &a.prefix()),
        vec![(
            "PUT".to_string(),
            format!("{}/wf/migrated.json", a.prefix())
        )],
        "the only write under A's prefix is the one import's marker"
    );
}

/// A source session `wf` in A (holding `notes.md`), B with the template
/// compiled, and A's manifest as JSON for a test to tamper with.
struct TamperedSource {
    s3: FakeS3,
    _root: tempfile::TempDir,
    a: Host,
    b: Host,
    manifest: serde_json::Value,
}

impl TamperedSource {
    fn new() -> Self {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let cloud = cloud(&s3);
        let a = Host::new(root.path(), "a", &cloud);
        let b = Host::new(root.path(), "b", &cloud);
        start_session(&a, "wf");
        compile(&b);
        let manifest = serde_json::from_slice(
            &s3.object(&Self::source_key_of(&a, "ctx/manifest.json"))
                .unwrap(),
        )
        .unwrap();
        TamperedSource {
            s3,
            _root: root,
            a,
            b,
            manifest,
        }
    }

    fn source_key_of(a: &Host, rest: &str) -> String {
        format!("{}{}", session_prefix(a, "wf"), rest)
    }

    /// The bucket key of `rest` under the source session.
    fn source_key(&self, rest: &str) -> String {
        Self::source_key_of(&self.a, rest)
    }

    /// Write the (tampered) manifest back to the source's prefix.
    fn put_manifest(&self) {
        self.s3.put(
            &self.source_key("ctx/manifest.json"),
            &serde_json::to_vec(&self.manifest).unwrap(),
        );
    }

    /// Import `wf` into B and assert it is refused as unreadable, naming
    /// `key`, with nothing left behind: no local session or staging
    /// directory in B and no object under B's prefix.
    fn assert_refused_naming(&self, key: &str) {
        let run = self
            .b
            .koto(&["session", "import", "wf", "--from", &self.a.ws_str()]);
        let msg = refused(&run, "import_source_unreadable", 1);
        assert!(msg.contains(key), "the refusal doesn't name {key}: {msg}");
        assert_no_trace(&self.s3, &self.b, "wf");
        assert_eq!(
            self.s3.keys_under(&format!("{}/", self.b.prefix())),
            Vec::<String>::new(),
            "objects were left under B's prefix"
        );
    }
}

/// A manifest naming a key that isn't a valid context key is refused
/// before anything is fetched or written. The object is placed both under
/// the literal key and where `..` resolves to, so that without the name
/// check the import would succeed and write outside `ctx/`.
#[test]
fn import_refuses_a_manifest_key_that_is_not_a_valid_context_key() {
    let mut src = TamperedSource::new();
    let bytes = b"escaped".to_vec();
    src.manifest["keys"]["../escape"] = serde_json::json!({
        "created_at": "2026-01-01T00:00:00Z",
        "size": bytes.len(),
        "hash": sha256_hex(&bytes),
    });
    src.put_manifest();
    src.s3.put(&src.source_key("ctx/../escape"), &bytes);
    src.s3.put(&src.source_key("escape"), &bytes);

    src.assert_refused_naming("../escape");
}

/// A key whose bytes were changed after its manifest entry was written,
/// keeping the size, fails the SHA-256 check.
#[test]
fn import_refuses_a_key_whose_bytes_do_not_match_the_manifest_hash() {
    let src = TamperedSource::new();
    let original = src.s3.object(&src.source_key("ctx/notes.md")).unwrap();
    let mut altered = original.clone();
    altered[0] ^= 0x20;
    assert_eq!(altered.len(), original.len());
    src.s3.put(&src.source_key("ctx/notes.md"), &altered);

    src.assert_refused_naming("notes.md");
}

/// A key whose size differs from its manifest entry fails the size check.
/// The entry's hash is set to the new bytes' hash, so only the size is
/// wrong.
#[test]
fn import_refuses_a_key_whose_size_does_not_match_the_manifest() {
    let mut src = TamperedSource::new();
    let longer = b"notes, now longer than the manifest says".to_vec();
    src.s3.put(&src.source_key("ctx/notes.md"), &longer);
    src.manifest["keys"]["notes.md"]["hash"] = serde_json::json!(sha256_hex(&longer));
    assert_ne!(
        src.manifest["keys"]["notes.md"]["size"],
        serde_json::json!(longer.len())
    );
    src.put_manifest();

    src.assert_refused_naming("notes.md");
}

/// A manifest entry whose content object is missing is refused. The entry
/// describes empty content, so a missing object read as empty would pass
/// the size and hash checks: only the missing-object check stops it.
#[test]
fn import_refuses_a_manifest_entry_with_no_content_object() {
    let mut src = TamperedSource::new();
    src.manifest["keys"]["ghost.md"] = serde_json::json!({
        "created_at": "2026-01-01T00:00:00Z",
        "size": 0,
        "hash": sha256_hex(b""),
    });
    src.put_manifest();
    assert!(src.s3.object(&src.source_key("ctx/ghost.md")).is_none());

    src.assert_refused_naming("ghost.md");
}

/// The help text states the stopped-source rule.
#[test]
fn import_help_states_the_stopped_source_rule() {
    let output = assert_cmd::Command::cargo_bin("koto")
        .unwrap()
        .args(["session", "import", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for phrase in [
        "Import only a stopped source",
        "no process may still be advancing the session",
        "its last write must have reached the remote",
        "--from <WORKSPACE_PATH>",
    ] {
        assert!(help.contains(phrase), "help lacks {phrase:?}:\n{help}");
    }
}

// ---------------------------------------------------------------------------
// Import hardening: source checks, staging and rollback, the retry branch,
// --as, --trust-template, the marker on every context read, and the
// request, timing and credential measurements.
// ---------------------------------------------------------------------------

/// Import `name` into `host` from `from`, with `extra` arguments.
fn import(host: &Host, name: &str, from: &Host, extra: &[&str]) -> Run {
    let ws = from.ws_str();
    let mut args = vec!["session", "import", name, "--from", ws.as_str()];
    args.extend_from_slice(extra);
    host.koto(&args)
}

/// Every PUT and DELETE the endpoint received, as (method, key).
fn all_writes(s3: &FakeS3) -> Vec<(String, String)> {
    writes_under(s3, "")
}

/// A marker another import would write, naming `target` in `workspace`.
fn other_marker(target: &str, workspace: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "target": {
            "session": target,
            "session_id": "00000000-0000-4000-8000-000000000000",
            "workspace": workspace,
            "prefix": "0123456789abcdef",
        },
        "machine_id": "another-machine",
        "migrated_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

/// Tick the carrier session `name` in `host` from `start` to its terminal
/// state, whose cleanup removes it.
fn tick_to_terminal(host: &Host, name: &str) {
    for args in [
        vec!["next", name],
        vec!["next", name, "--with-data", r#"{"choice":"go"}"#],
        vec!["next", name, "--with-data", r#"{"verdict":"approve"}"#],
    ] {
        let run = host.koto(&args);
        assert!(run.ok(), "{:?}: {}", args, run.describe());
    }
}

/// A state file the import can't carry intact is refused as unreadable,
/// naming what is wrong, and leaves nothing behind.
#[test]
fn import_refuses_a_source_log_it_cannot_carry() {
    let src = TamperedSource::new();
    src.s3.clear_requests();
    let state_key = src.source_key("koto-wf.state.jsonl");
    let original = src.s3.object(&state_key).unwrap();
    let text = String::from_utf8(original.clone()).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let last: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
    let with_header = |h: &serde_json::Value| {
        let mut out = serde_json::to_string(h).unwrap();
        for line in &lines[1..] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.into_bytes()
    };

    let mut v2 = header.clone();
    v2["schema_version"] = serde_json::json!(2);
    let mut bad_hash = header.clone();
    bad_hash["template_hash"] = serde_json::json!("../../../../tmp/payload");
    let mut garbage = original.clone();
    garbage.extend_from_slice(b"{not an event\n");
    let mut skipped = last.clone();
    skipped["seq"] = serde_json::json!(last["seq"].as_u64().unwrap() + 2);
    let mut gap = original.clone();
    gap.extend(serde_json::to_vec(&skipped).unwrap());
    gap.push(b'\n');

    for (bytes, names) in [
        (with_header(&v2), "schema_version 2"),
        (with_header(&bad_hash), "template_hash"),
        (garbage, "doesn't parse"),
        (gap, "expected"),
    ] {
        src.s3.put(&state_key, &bytes);
        src.assert_refused_naming(names);
    }
    // Nothing the refusals did touched the source either.
    assert_eq!(
        writes_under(&src.s3, &src.a.prefix()),
        Vec::<(String, String)>::new()
    );
}

/// A session that never stored a key imports, and its target has an empty
/// manifest, here and in the bucket.
#[test]
fn a_source_with_no_context_keys_imports_with_an_empty_manifest() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let template = a.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = a.koto(&["init", "bare", "--template", template.to_str().unwrap()]);
    assert!(run.ok(), "init: {}", run.describe());
    assert!(
        s3.object(&format!("{}ctx/manifest.json", session_prefix(&a, "bare")))
            .is_none(),
        "the source must have no manifest at all"
    );
    compile(&b);

    let run = import(&b, "bare", &a, &[]);
    assert!(run.ok(), "import: {}", run.describe());
    assert_eq!(run.json["keys"], 0, "{}", run.describe());
    let empty = serde_json::json!({"keys": {}});
    assert_eq!(b.manifest("bare"), empty);
    let pushed = s3
        .object(&format!("{}ctx/manifest.json", session_prefix(&b, "bare")))
        .expect("the empty manifest is pushed");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&pushed).unwrap(),
        empty
    );
    let run = b.koto(&["context", "list", "bare"]);
    assert!(run.ok(), "context list: {}", run.describe());
    assert_eq!(run.json, serde_json::json!([]));
}

/// A PUT that fails part way through the push: the import takes back
/// exactly the objects it had pushed, removes its staging directory, and
/// leaves no local session.
#[test]
fn a_failed_push_takes_back_exactly_what_it_pushed() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);

    // The state file, the template and notes.md go up before the manifest.
    let target = session_prefix(&b, "wf");
    let manifest = format!("{}ctx/manifest.json", target);
    s3.fail("PUT", &manifest, 500);
    s3.clear_requests();
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_push_failed", 1);
    assert!(msg.contains("ctx/manifest.json"), "{msg}");
    assert_no_trace(&s3, &b, "wf");

    let writes = all_writes(&s3);
    let of = |method: &str| -> std::collections::BTreeSet<String> {
        writes
            .iter()
            .filter(|(m, k)| m == method && k != &manifest)
            .map(|(_, k)| k.clone())
            .collect()
    };
    let pushed: std::collections::BTreeSet<String> = [
        format!("{}koto-wf.state.jsonl", target),
        format!("{}template.json", target),
        format!("{}ctx/notes.md", target),
    ]
    .into_iter()
    .collect();
    assert_eq!(of("PUT"), pushed, "writes were {:?}", writes);
    assert_eq!(of("DELETE"), pushed, "writes were {:?}", writes);
    assert!(
        writes.iter().all(|(_, k)| k.starts_with(&target)),
        "a write landed outside the target's prefix: {:?}",
        writes
    );
}

/// A marker PUT that fails leaves a complete target and exits
/// `import_unmarked`; running the same command again writes the marker and
/// nothing else.
#[test]
fn an_unmarked_import_keeps_its_target_and_a_rerun_only_marks() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);

    let marker = format!("{}migrated.json", session_prefix(&a, "wf"));
    s3.fail("PUT", &marker, 500);
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_unmarked", 1);
    assert!(msg.contains("run the same import again"), "{msg}");
    assert!(b.state_path("wf").exists(), "the target must be kept");
    assert_eq!(staging_dirs(&b), Vec::<String>::new());
    let target = holdings(&s3, &b, "wf");
    assert_eq!(
        target.1.len(),
        5,
        "the target in the bucket: {:?}",
        target.1
    );
    assert!(s3.object(&marker).is_none());

    s3.clear_faults();
    s3.clear_requests();
    let store = b.home.join(".koto").join("sessions");
    let entries = |dir: &std::path::Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let store_before = entries(&store);

    // A local manifest that can't be read fails the re-run rather than
    // reporting no keys, and writes nothing.
    let manifest = b.session_dir("wf").join("ctx").join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, b"not json").unwrap();
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_push_failed", 1);
    assert!(msg.contains("manifest"), "{msg}");
    assert_eq!(all_writes(&s3), Vec::<(String, String)>::new());
    std::fs::write(&manifest, &manifest_bytes).unwrap();

    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "re-run: {}", run.describe());
    assert_eq!(run.json["marked"], true);
    assert_eq!(run.json["keys"], 1);
    // It took no template: the target's is the one the first run stored.
    assert_eq!(run.json["template"], "unchanged");
    assert_eq!(
        all_writes(&s3),
        vec![("PUT".to_string(), marker.clone())],
        "the re-run may write only the marker"
    );
    assert_eq!(holdings(&s3, &b, "wf"), target);
    assert_eq!(entries(&store), store_before);

    let written: serde_json::Value = serde_json::from_slice(&s3.object(&marker).unwrap()).unwrap();
    assert_eq!(
        written["target"]["session_id"],
        b.header("wf")["session_id"]
    );
    let run = a.koto(&["status", "wf"]);
    assert_eq!(run.code, Some(2), "A must refuse now: {}", run.describe());
}

/// A crash between the push and the rename leaves the target only in the
/// bucket. Running the same import again builds the local copy, pushes it
/// over the remote one, and marks the source.
#[test]
fn a_target_only_in_the_bucket_is_rebuilt_by_a_rerun() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);

    let marker = format!("{}migrated.json", session_prefix(&a, "wf"));
    s3.fail("PUT", &marker, 500);
    let run = import(&b, "wf", &a, &[]);
    refused(&run, "import_unmarked", 1);
    // What the crash leaves: the whole target in the bucket, nothing here.
    std::fs::remove_dir_all(b.session_dir("wf")).unwrap();
    assert!(!s3.keys_under(&session_prefix(&b, "wf")).is_empty());
    s3.clear_faults();

    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "re-run: {}", run.describe());
    assert_eq!(staging_dirs(&b), Vec::<String>::new());
    let lines = b.state_lines("wf");
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["type"], "session_imported");
    assert_eq!(
        last["payload"]["from_session_id"],
        a.header("wf")["session_id"]
    );
    assert_eq!(
        std::fs::read(b.session_dir("wf").join("ctx").join("notes.md")).unwrap(),
        b"notes"
    );
    assert_eq!(
        s3.object(&format!("{}koto-wf.state.jsonl", session_prefix(&b, "wf"))),
        std::fs::read(b.state_path("wf")).ok(),
        "the bucket must hold the rebuilt copy"
    );
    let written: serde_json::Value = serde_json::from_slice(&s3.object(&marker).unwrap()).unwrap();
    assert_eq!(
        written["target"]["session_id"],
        b.header("wf")["session_id"]
    );
}

/// The marker is read again just before it is written: when another
/// import marked the source meanwhile, this one keeps its target, leaves
/// the other's marker alone, and exits `import_source_migrated` naming it.
#[test]
fn the_marker_is_read_again_just_before_it_is_written() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);

    // The other import's marker lands right after this one's last push.
    let marker = format!("{}migrated.json", session_prefix(&a, "wf"));
    let theirs = other_marker("elsewhere", "/srv/other-workspace");
    s3.put_after(
        "PUT",
        &format!("{}version.json", session_prefix(&b, "wf")),
        &marker,
        &theirs,
    );
    s3.clear_requests();
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_source_migrated", 2);
    for part in ["'elsewhere'", "/srv/other-workspace", b.ws_str().as_str()] {
        assert!(msg.contains(part), "the refusal doesn't name {part}: {msg}");
    }
    assert!(b.state_path("wf").exists(), "this import's target is kept");
    assert!(s3
        .object(&format!("{}koto-wf.state.jsonl", session_prefix(&b, "wf")))
        .is_some());
    assert_eq!(staging_dirs(&b), Vec::<String>::new());
    assert_eq!(s3.object(&marker), Some(theirs));
    assert!(
        !all_writes(&s3).iter().any(|(_, k)| k == &marker),
        "the other import's marker was written over"
    );
}

/// `--as` imports under a new name: the header carries it here and in the
/// bucket, the marker names it, and the session runs under it.
#[test]
fn as_imports_under_a_new_name() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    start_session(&b, "wf");
    compile(&b);
    let own = holdings(&s3, &b, "wf");

    let run = import(&b, "wf", &a, &["--as", "moved"]);
    assert!(run.ok(), "import --as: {}", run.describe());
    assert_eq!(run.json["name"], "moved");
    assert_eq!(run.json["from"]["session"], "wf");

    let header = b.header("moved");
    assert_eq!(header["workflow"], "moved");
    let remote = s3
        .object(&format!(
            "{}koto-moved.state.jsonl",
            session_prefix(&b, "moved")
        ))
        .expect("the target is pushed under its new name");
    assert_eq!(
        Some(remote.clone()),
        std::fs::read(b.state_path("moved")).ok()
    );
    let remote_header: serde_json::Value = serde_json::from_str(
        std::str::from_utf8(&remote)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(remote_header["workflow"], "moved");
    let last: serde_json::Value =
        serde_json::from_str(b.state_lines("moved").last().unwrap()).unwrap();
    assert_eq!(last["payload"]["from_session"], "wf");

    let marker: serde_json::Value = serde_json::from_slice(
        &s3.object(&format!("{}migrated.json", session_prefix(&a, "wf")))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(marker["target"]["session"], "moved");
    assert_eq!(marker["target"]["session_id"], header["session_id"]);

    let run = b.koto(&["status", "moved"]);
    assert!(run.ok(), "status moved: {}", run.describe());
    assert_eq!(holdings(&s3, &b, "wf"), own, "B's own 'wf' changed");
    let run = a.koto(&["status", "wf"]);
    assert!(
        run.json["error"]
            .as_str()
            .is_some_and(|m| m.contains("'moved'")),
        "A's refusal must name the new name: {}",
        run.describe()
    );
}

/// `koto init` pushes the compiled template as `template.json`; the import
/// takes it only under `--trust-template`, after checking its hash and
/// parsing it, and never asks for it otherwise.
#[test]
fn trust_template_takes_the_bucket_copy_only_when_asked() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    start_session(&a, "wf");
    let hash = a.header("wf")["template_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let pushed = s3
        .object(&format!("{}template.json", session_prefix(&a, "wf")))
        .expect("init pushes the compiled template");
    assert_eq!(sha256_hex(&pushed), hash);

    // B hasn't compiled the template. Without the flag it refuses, and
    // never asks the bucket for the source's copy.
    s3.clear_requests();
    let run = import(&b, "wf", &a, &[]);
    refused(&run, "import_template_unavailable", 2);
    assert_no_trace(&s3, &b, "wf");
    let asked: Vec<String> = s3
        .requests()
        .iter()
        .map(describe)
        .filter(|r| r.contains("template.json"))
        .collect();
    assert_eq!(asked, Vec::<String>::new());

    // With it, B takes the bucket's copy and says so.
    let run = import(&b, "wf", &a, &["--trust-template"]);
    assert!(run.ok(), "import --trust-template: {}", run.describe());
    assert_eq!(run.json["template"], "bucket");
    let stored = std::fs::read(b.session_dir("wf").join(format!("{}.json", hash))).unwrap();
    assert_eq!(stored, pushed);
    // The session runs on that copy, with A's cache out of reach.
    std::fs::remove_dir_all(a.cache.join("koto")).unwrap();
    let run = b.koto(&["next", "wf"]);
    assert!(run.ok(), "next: {}", run.describe());
    assert_eq!(run.json["state"], "wait", "{}", run.describe());

    // A copy that doesn't hash to the session's hash is refused.
    start_session(&a, "wf2");
    let template_key = format!("{}template.json", session_prefix(&a, "wf2"));
    s3.put(&template_key, b"{}");
    let run = import(&c, "wf2", &a, &["--trust-template"]);
    let msg = refused(&run, "import_template_unavailable", 2);
    assert!(msg.contains(&hash) && msg.contains("hash to"), "{msg}");
    assert_no_trace(&s3, &c, "wf2");

    // So is one that hashes right but isn't a compiled template: the
    // source's header is made to record the junk's hash.
    let junk = b"not a compiled template".to_vec();
    s3.put(&template_key, &junk);
    let state_key = format!("{}koto-wf2.state.jsonl", session_prefix(&a, "wf2"));
    let text = String::from_utf8(s3.object(&state_key).unwrap()).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header["template_hash"] = serde_json::json!(sha256_hex(&junk));
    lines[0] = serde_json::to_string(&header).unwrap();
    s3.put(&state_key, format!("{}\n", lines.join("\n")).as_bytes());
    let run = import(&c, "wf2", &a, &["--trust-template"]);
    let msg = refused(&run, "import_template_unavailable", 2);
    assert!(msg.contains("doesn't parse"), "{msg}");
    assert_no_trace(&s3, &c, "wf2");
}

/// After an import, the source's `koto context` commands and `koto session
/// rebind` refuse with `session_migrated`, and leave its copy as it was.
#[test]
fn a_migrated_source_refuses_context_commands_and_rebind() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);
    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "import: {}", run.describe());

    let ctx = a.session_dir("wf").join("ctx");
    let notes = std::fs::read(ctx.join("notes.md")).unwrap();
    let state = std::fs::read(a.state_path("wf")).unwrap();
    let src = a.home.join("new.md");
    std::fs::write(&src, b"new").unwrap();
    let src = src.to_string_lossy().into_owned();
    s3.clear_requests();

    for (args, command) in [
        (
            vec![
                "context",
                "add",
                "wf",
                "new.md",
                "--from-file",
                src.as_str(),
            ],
            "context add",
        ),
        (vec!["context", "get", "wf", "notes.md"], "context get"),
        (vec!["context", "list", "wf"], "context list"),
        (
            vec!["context", "exists", "wf", "notes.md"],
            "context exists",
        ),
        (
            vec!["context", "remove", "wf", "notes.md"],
            "context remove",
        ),
    ] {
        let run = a.koto(&args);
        assert_eq!(run.code, Some(2), "{command}: {}", run.describe());
        assert_eq!(run.json["command"], command, "{}", run.describe());
        let error = run.json["error"].as_str().unwrap_or_default();
        assert!(
            error.starts_with("session_migrated:") && error.contains(&b.ws_str()),
            "{command}: {}",
            run.describe()
        );
    }
    let run = a.koto(&["session", "rebind", "wf"]);
    assert!(!run.ok(), "rebind: {}", run.describe());
    assert!(
        run.stderr.contains("session_migrated:"),
        "rebind: {}",
        run.describe()
    );

    assert_eq!(std::fs::read(ctx.join("notes.md")).unwrap(), notes);
    assert!(!ctx.join("new.md").exists());
    assert_eq!(std::fs::read(a.state_path("wf")).unwrap(), state);
    assert_eq!(all_writes(&s3), Vec::<(String, String)>::new());
}

/// `koto next` on a session that was never migrated makes exactly one
/// request more than it did before the marker check existed, the listing
/// for the marker; the local backend makes none at all.
#[test]
fn the_marker_check_adds_one_request_to_next_and_none_on_the_local_backend() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    start_session(&a, "plain");

    s3.clear_requests();
    let run = a.koto(&["next", "plain"]);
    assert!(run.ok(), "next: {}", run.describe());
    let session = session_prefix(&a, "plain");
    let seen: Vec<String> = s3.requests().iter().map(describe).collect();
    let state = format!("{}koto-plain.state.jsonl", session);
    let get = format!("GET {}", state);
    let put = format!("PUT {}", state);
    // The requests `koto next` made before the marker check existed
    // (measured with the check switched off): the pulls of the state file
    // and the pushes of the events the tick appends.
    let before_the_check = vec![get.clone(), get.clone(), put.clone(), put.clone(), put, get];
    // The marker check: the one added request, answered once for the
    // whole tick though the gate reads a context key.
    let mut expected = vec![format!("LIST {}migrated.json", session)];
    expected.extend(before_the_check);
    assert_eq!(seen, expected, "koto next made {:?}", seen);

    // The local backend, with the same cloud settings still in the config,
    // never reaches the endpoint.
    let local = Host::new(root.path(), "local", &cloud);
    let config = local.ws.join(".koto").join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("backend = \"cloud\"", "backend = \"local\""),
    )
    .unwrap();
    s3.clear_requests();
    start_session(&local, "plain");
    for args in [
        vec!["status", "plain"],
        vec!["next", "plain"],
        vec!["context", "get", "plain", "notes.md"],
        vec!["context", "exists", "plain", "notes.md"],
        vec!["context", "list", "plain"],
    ] {
        let run = local.koto(&args);
        assert!(run.ok(), "{:?}: {}", args, run.describe());
    }
    assert_eq!(s3.requests(), Vec::new(), "the local backend made requests");
}

/// A source whose terminal tick removed it is not found; an imported
/// session's own terminal tick removes only its own prefix.
#[test]
fn terminal_ticks_remove_a_source_and_only_their_own_target() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    compile(&b);

    start_session(&a, "finished");
    tick_to_terminal(&a, "finished");
    assert_eq!(
        s3.keys_under(&session_prefix(&a, "finished")),
        Vec::<String>::new()
    );
    let run = import(&b, "finished", &a, &[]);
    refused(&run, "import_source_not_found", 2);
    assert_no_trace(&s3, &b, "finished");

    start_session(&a, "wf");
    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "import: {}", run.describe());
    let a_before: Vec<(String, Option<Vec<u8>>)> = s3
        .keys_under(&format!("{}/", a.prefix()))
        .into_iter()
        .map(|k| {
            let v = s3.object(&k);
            (k, v)
        })
        .collect();
    assert!(a_before
        .iter()
        .any(|(k, _)| k.ends_with("/wf/migrated.json")));

    s3.clear_requests();
    tick_to_terminal(&b, "wf");
    let target = session_prefix(&b, "wf");
    assert_eq!(s3.keys_under(&target), Vec::<String>::new());
    assert!(!b.session_dir("wf").exists());
    let a_after: Vec<(String, Option<Vec<u8>>)> = s3
        .keys_under(&format!("{}/", a.prefix()))
        .into_iter()
        .map(|k| {
            let v = s3.object(&k);
            (k, v)
        })
        .collect();
    assert_eq!(a_after, a_before, "B's terminal tick touched A's prefix");
    let deletes: Vec<String> = all_writes(&s3)
        .into_iter()
        .filter(|(m, _)| m == "DELETE")
        .map(|(_, k)| k)
        .collect();
    assert!(!deletes.is_empty());
    assert!(
        deletes.iter().all(|k| k.starts_with(&target)),
        "a delete outside B's prefix: {:?}",
        deletes
    );
}

/// With the endpoint stopped, `koto status` on a cloud session answers
/// from its local copy and warns.
#[test]
fn with_the_endpoint_stopped_status_works_on_the_local_copy() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let a = Host::new(root.path(), "a", &cloud(&s3));
    let template = a.ws.join(migration_carrier::TEMPLATE_FILE);
    let run = a.koto(&["init", "plain", "--template", template.to_str().unwrap()]);
    assert!(run.ok(), "init: {}", run.describe());
    drop(s3);

    let run = a.koto(&["status", "plain"]);
    assert!(run.ok(), "status: {}", run.describe());
    assert_eq!(run.json["current_state"], "start", "{}", run.describe());
    assert!(
        run.stderr.contains("warning: cloud sync"),
        "no warning: {}",
        run.describe()
    );
}

/// Seed `n` context keys for session `name` in `host` straight into the
/// bucket, with a manifest naming them.
fn seed_keys(s3: &FakeS3, host: &Host, name: &str, n: usize) {
    let prefix = session_prefix(host, name);
    let mut keys = serde_json::Map::new();
    for i in 0..n {
        let bytes = format!("key {i} of {name}").into_bytes();
        s3.put(&format!("{}ctx/k{}.txt", prefix, i), &bytes);
        keys.insert(
            format!("k{}.txt", i),
            serde_json::json!({
                "created_at": "2026-01-01T00:00:00Z",
                "size": bytes.len(),
                "hash": sha256_hex(&bytes),
            }),
        );
    }
    s3.put(
        &format!("{}ctx/manifest.json", prefix),
        &serde_json::to_vec(&serde_json::json!({ "keys": keys })).unwrap(),
    );
}

/// An import's requests grow by one GET and one PUT per context key and by
/// nothing else: importing 10 keys costs exactly 10 requests more than
/// importing 5.
#[test]
fn import_requests_grow_by_one_get_and_one_put_per_key() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let template = a.ws.join(migration_carrier::TEMPLATE_FILE);
    compile(&b);

    let mut totals = Vec::new();
    for (name, n) in [("five", 5), ("ten", 10)] {
        let run = a.koto(&["init", name, "--template", template.to_str().unwrap()]);
        assert!(run.ok(), "init: {}", run.describe());
        seed_keys(&s3, &a, name, n);

        s3.clear_requests();
        let run = import(&b, name, &a, &[]);
        assert!(run.ok(), "import {name}: {}", run.describe());
        assert_eq!(run.json["keys"], n);
        let requests = s3.requests();
        let per_key = |method: &str, host: &Host| {
            let ctx = format!("{}ctx/", session_prefix(host, name));
            requests
                .iter()
                .filter(|r| {
                    r.method == method
                        && r.list_prefix.is_none()
                        && r.key.starts_with(&ctx)
                        && !r.key.ends_with("manifest.json")
                })
                .count()
        };
        assert_eq!(per_key("GET", &a), n, "GETs of {name}'s keys");
        assert_eq!(per_key("PUT", &b), n, "PUTs of {name}'s keys");
        totals.push(requests.len());
    }
    assert_eq!(
        totals[1] - totals[0],
        5 * 2,
        "5 more keys cost {} more requests, not 10 (totals {:?})",
        totals[1] as i64 - totals[0] as i64,
        totals
    );
}

/// With sentinel credentials, nothing the import prints, on success or on
/// any failure, nor the marker it writes, holds either value; and an
/// endpoint configured with credentials in its URL never prints them.
#[test]
fn credentials_never_reach_import_output_errors_or_the_marker() {
    const ACCESS: &str = "AKIASENTINELACCESS042";
    const SECRET: &str = "sentinel+Secret/Value042";
    const URL_USER: &str = "sentinel-url-user";
    const URL_PASSWORD: &str = "sentinel-url-password";

    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let mut cloud = cloud(&s3);
    cloud.credentials = Some((ACCESS.to_string(), SECRET.to_string()));
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    start_session(&a, "wf");
    compile(&b);
    let mut runs = Vec::new();

    // A push that fails.
    let manifest = format!("{}ctx/manifest.json", session_prefix(&b, "wf"));
    s3.fail("PUT", &manifest, 500);
    let run = import(&b, "wf", &a, &[]);
    refused(&run, "import_push_failed", 1);
    runs.push(run);
    s3.clear_faults();

    // A marker that can't be written, then the re-run that writes it.
    let marker = format!("{}migrated.json", session_prefix(&a, "wf"));
    s3.fail("PUT", &marker, 500);
    let run = import(&b, "wf", &a, &[]);
    refused(&run, "import_unmarked", 1);
    runs.push(run);
    s3.clear_faults();
    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "re-run: {}", run.describe());
    runs.push(run);

    // Running it once more completes without writing; importing it again
    // under another name is refused. Then a source listing that fails.
    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "third run: {}", run.describe());
    runs.push(run);
    let run = import(&b, "wf", &a, &["--as", "again"]);
    refused(&run, "import_source_migrated", 2);
    runs.push(run);
    start_session(&a, "other");
    s3.fail(
        "LIST",
        &format!("{}migrated.json", session_prefix(&a, "other")),
        500,
    );
    let run = import(&b, "other", &a, &[]);
    refused(&run, "import_source_unreadable", 1);
    runs.push(run);

    // An endpoint that can't be reached, with credentials in its URL.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let mut unreachable = cloud.clone();
    unreachable.endpoint = format!("http://{URL_USER}:{URL_PASSWORD}@127.0.0.1:{port}");
    let e = Host::new(root.path(), "e", &unreachable);
    let run = import(&e, "other", &a, &[]);
    assert!(!run.ok(), "{}", run.describe());
    assert!(
        run.json["error"]["code"].is_string(),
        "no error object: {}",
        run.describe()
    );
    println!("unreachable endpoint: {}", run.describe());
    runs.push(run);

    for run in &runs {
        for secret in [ACCESS, SECRET, URL_USER, URL_PASSWORD] {
            assert!(
                !run.stdout.contains(secret) && !run.stderr.contains(secret),
                "{secret} printed: {}",
                run.describe()
            );
        }
    }
    let written = String::from_utf8(s3.object(&marker).unwrap()).unwrap();
    for secret in [ACCESS, SECRET] {
        assert!(!written.contains(secret), "{secret} in the marker");
    }
}

// -- the cloud backend's own surface, driven directly ----------------------

/// A cloud backend storing sessions under `base`, against `s3`, with
/// remote prefix `prefix`.
fn backend_on(s3: &FakeS3, base: &std::path::Path, prefix: &str) -> CloudBackend {
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

fn header_of(workflow: &str) -> koto::engine::types::StateFileHeader {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": workflow,
        "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn initialized(template_path: &str) -> koto::engine::types::Event {
    serde_json::from_value(serde_json::json!({
        "seq": 1,
        "timestamp": "2026-01-01T00:00:00Z",
        "type": "workflow_initialized",
        "payload": {"template_path": template_path, "variables": {}},
    }))
    .unwrap()
}

/// Every `ContextStore` method refuses a migrated session: those that can
/// return an error return `SessionMigrated`, `ctx_exists` answers false and
/// `meta` none, though the local store holds the key. Nothing is written
/// here or remotely, and one marker check answers all of them.
#[test]
fn every_context_store_method_refuses_a_migrated_session() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
    local
        .init_state_file("wf", header_of("wf"), Vec::new())
        .unwrap();
    local.add("wf", "notes.md", b"notes").unwrap();
    s3.put("pfx/wf/migrated.json", &other_marker("wf", "/srv/b"));
    let backend = backend_on(&s3, tmp.path(), "pfx");
    s3.clear_requests();

    let migrated = |what: &str, result: anyhow::Result<()>| {
        let err = result.expect_err(what);
        assert!(
            err.downcast_ref::<SessionMigrated>().is_some(),
            "{what}: {err:#}"
        );
    };
    migrated("add", backend.add("wf", "k", b"v"));
    migrated(
        "add_with_writer",
        backend.add_with_writer("wf", "k", b"v", "agent"),
    );
    migrated("get", backend.get("wf", "notes.md").map(drop));
    migrated("remove", backend.remove("wf", "notes.md"));
    migrated("list_keys", backend.list_keys("wf", None).map(drop));
    assert!(!backend.ctx_exists("wf", "notes.md"), "ctx_exists");
    assert!(backend.meta("wf", "notes.md").is_none(), "meta");

    assert_eq!(local.get("wf", "notes.md").unwrap(), b"notes");
    assert!(!local.ctx_exists("wf", "k"));
    assert_eq!(all_writes(&s3), Vec::<(String, String)>::new());
    let seen: Vec<String> = s3.requests().iter().map(describe).collect();
    assert_eq!(
        seen,
        vec![
            "LIST pfx/wf/migrated.json".to_string(),
            "GET pfx/wf/migrated.json".to_string(),
        ]
    );
}

/// `relocate` moves a session's objects to the new name but leaves its
/// marker where it is.
#[test]
fn relocate_leaves_the_marker_where_it_is() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let backend = backend_on(&s3, tmp.path(), "pfx");
    LocalBackend::with_base_dir(tmp.path().to_path_buf())
        .init_state_file("old", header_of("old"), Vec::new())
        .unwrap();
    s3.put("pfx/old/koto-old.state.jsonl", b"{}\n");
    s3.put("pfx/old/ctx/notes.md", b"notes");
    s3.put("pfx/old/migrated.json", &other_marker("old", "/srv/b"));

    backend.relocate("old", "new").unwrap();
    assert_eq!(
        s3.keys_under("pfx/old/"),
        vec!["pfx/old/migrated.json".to_string()]
    );
    assert!(s3.object("pfx/new/migrated.json").is_none());
    assert_eq!(
        s3.object("pfx/new/ctx/notes.md").as_deref(),
        Some(&b"notes"[..])
    );
}

/// `init_state_file` pushes the compiled template its
/// `workflow_initialized` event records, absolute or relative to the
/// session directory, and pushes none when it records none.
#[test]
fn init_pushes_the_compiled_template_it_records() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    let backend = backend_on(&s3, &base, "pfx");

    let compiled = tmp.path().join("compiled.json");
    std::fs::write(&compiled, b"top template").unwrap();
    backend
        .init_state_file(
            "top",
            header_of("top"),
            vec![initialized(compiled.to_str().unwrap())],
        )
        .unwrap();
    assert_eq!(
        s3.object("pfx/top/template.json").as_deref(),
        Some(&b"top template"[..])
    );

    std::fs::create_dir_all(base.join("kid")).unwrap();
    std::fs::write(base.join("kid").join("own.json"), b"kid template").unwrap();
    backend
        .init_state_file("kid", header_of("kid"), vec![initialized("own.json")])
        .unwrap();
    assert_eq!(
        s3.object("pfx/kid/template.json").as_deref(),
        Some(&b"kid template"[..])
    );

    backend
        .init_state_file("bare", header_of("bare"), Vec::new())
        .unwrap();
    assert!(s3.object("pfx/bare/template.json").is_none());
}

/// A marker PUT that landed though the endpoint reported a failure: the
/// re-run finds the source marked for its own target, finishes, and writes
/// nothing. A marker naming another target still refuses.
#[test]
fn a_marker_that_landed_despite_an_error_completes_the_rerun() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    start_session(&a, "wf");
    compile(&b);
    compile(&c);

    let marker = format!("{}migrated.json", session_prefix(&a, "wf"));
    s3.fail_after_storing(&marker, 500);
    let run = import(&b, "wf", &a, &[]);
    refused(&run, "import_unmarked", 1);
    let landed = s3.object(&marker).expect("the marker landed");
    s3.clear_faults();

    s3.clear_requests();
    let run = import(&b, "wf", &a, &[]);
    assert!(run.ok(), "re-run: {}", run.describe());
    assert_eq!(run.json["name"], "wf");
    assert_eq!(run.json["marked"], true);
    assert_eq!(run.json["keys"], 1);
    assert_eq!(run.json["template"], "unchanged");
    assert_eq!(all_writes(&s3), Vec::<(String, String)>::new());
    assert_eq!(s3.object(&marker), Some(landed.clone()));

    // The same marker is someone else's to C, and to B under another name.
    let run = import(&c, "wf", &a, &[]);
    let msg = refused(&run, "import_source_migrated", 2);
    assert!(msg.contains(&b.ws_str()), "{msg}");
    assert_no_trace(&s3, &c, "wf");
    let run = import(&b, "wf", &a, &["--as", "again"]);
    refused(&run, "import_source_migrated", 2);
    assert_no_trace(&s3, &b, "again");

    // And a target of that name here that isn't the marked session (its
    // session id differs) doesn't count either.
    let theirs = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "target": {
            "session": "wf",
            "session_id": "00000000-0000-4000-8000-000000000000",
            "workspace": b.ws_str(),
            "prefix": b.prefix(),
        },
        "machine_id": "m",
        "migrated_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    s3.put(&marker, &theirs);
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_source_migrated", 2);
    assert!(
        msg.contains("own marker")
            && msg.contains("is a different one")
            && msg.contains(&b.session_dir("wf").to_string_lossy().into_owned())
            && msg.contains("--as")
            && !msg.contains("bucket"),
        "{msg}"
    );
    assert_eq!(s3.object(&marker), Some(theirs));

    // With the local copy gone, the refusal says it is missing; nothing is
    // rebuilt over a marked source.
    std::fs::remove_dir_all(b.session_dir("wf")).unwrap();
    s3.put(&marker, &landed);
    s3.clear_requests();
    let run = import(&b, "wf", &a, &[]);
    let msg = refused(&run, "import_source_migrated", 2);
    assert!(
        msg.contains("own marker") && msg.contains("missing here") && msg.contains("bucket"),
        "{msg}"
    );
    assert!(!b.session_dir("wf").exists());
    assert_eq!(all_writes(&s3), Vec::<(String, String)>::new());
}
