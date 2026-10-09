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

use fake_s3::FakeS3;
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
/// existed, and no retry sleep. Before, status made two: the GET that pulls
/// the state file, and the listing of this workspace's sessions it scans
/// for superseded branches. The check lists for the marker, which answers
/// 200 whether or not it is there, where a GET of a missing marker would be
/// a 404 that rust-s3 retries after a second.
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
    assert_eq!(
        seen,
        vec![
            // The marker check: the one added request.
            format!("LIST {}migrated.json", session),
            // What status made before the check existed: the state pull ...
            format!("GET {}koto-{}.state.jsonl", session, name),
            // ... and the session listing behind `superseded_branches`.
            format!("LIST {}/", a.prefix()),
        ],
        "koto status made {:?}",
        seen
    );
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

/// Each refusal the slice ships, and that none of them leaves a trace:
/// nothing written under the source's prefix, nothing created locally.
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

    // No such session under A's prefix.
    let run = b.koto(&["session", "import", "nope", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_source_not_found", 2);
    assert!(msg.contains("nope"), "{msg}");

    // A relative --from can't name another host's workspace.
    let run = b.koto(&["session", "import", "wf", "--from", "ws"]);
    refused(&run, "import_source_not_found", 2);

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

    // A cached file that doesn't hash to the session's hash is refused too.
    let cached = b.cache.join("koto").join(format!("{}.json", hash));
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::write(&cached, b"{}").unwrap();
    let run = b.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    refused(&run, "import_template_unavailable", 2);
    std::fs::remove_file(&cached).unwrap();
    compile(&b);

    // The name is taken locally ...
    start_session(&b, "wf");
    let before = std::fs::read(b.state_path("wf")).unwrap();
    let run = b.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    refused(&run, "import_name_taken", 2);
    assert_eq!(std::fs::read(b.state_path("wf")).unwrap(), before);

    // ... or under this workspace's prefix only.
    compile(&c);
    s3.put(&format!("{}/wf/koto-wf.state.jsonl", c.prefix()), b"{}\n");
    let run = c.koto(&["session", "import", "wf", "--from", &a.ws_str()]);
    refused(&run, "import_name_taken", 2);
    assert!(!c.session_dir("wf").exists());
    s3.remove(&format!("{}/wf/koto-wf.state.jsonl", c.prefix()));

    // Nothing so far wrote under A's prefix.
    assert_eq!(writes_under(&s3, &a.prefix()), vec![]);

    // A child session isn't imported on its own.
    let run = a.koto(&["session", "start", "wf.child", "--parent", "wf"]);
    assert!(run.ok(), "session start: {}", run.describe());
    s3.clear_requests();
    let run = c.koto(&["session", "import", "wf.child", "--from", &a.ws_str()]);
    let msg = refused(&run, "import_source_is_child", 2);
    assert!(msg.contains("wf.child"), "{msg}");
    assert!(!c.session_dir("wf.child").exists());
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
    assert!(!d.session_dir("wf").exists());
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
    /// `key`, with nothing left behind: no local session in B and no
    /// object under B's prefix.
    fn assert_refused_naming(&self, key: &str) {
        let run = self
            .b
            .koto(&["session", "import", "wf", "--from", &self.a.ws_str()]);
        let msg = refused(&run, "import_source_unreadable", 1);
        assert!(msg.contains(key), "the refusal doesn't name {key}: {msg}");
        assert!(
            !self.b.session_dir("wf").exists(),
            "a local session was left in B"
        );
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
        .env_remove("CLAUDE_CODE_SESSION_ID")
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

/// Lines of a host's run journal, as JSON.
fn run_journal(host: &Host) -> Vec<serde_json::Value> {
    std::fs::read_to_string(host.home.join(".koto").join("_run_journal.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// An import journals one `session_started` for a new run on the importing
/// host: its new id as both session and run id, the source's id as
/// `imported_from`, the importing command's driver, and the source's
/// template identity. None of the source's records are copied.
#[test]
fn an_import_journals_a_new_run_on_the_importing_host() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = cloud(&s3);
    let a = Host::new(root.path(), "a", &cloud);
    let b = Host::new(root.path(), "b", &cloud);
    let c = Host::new(root.path(), "c", &cloud);
    let name = "journaled";
    let mut carrier = Carrier::new(&a, &b, &c, name);
    carrier.init_a();
    let source_header = a.header(name);
    let source_id = source_header["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        run_journal(&a).len(),
        2,
        "A's own session_started and state"
    );

    carrier.compile_on("import", &b);
    let output = b
        .cmd()
        .env("CLAUDE_CODE_SESSION_ID", "driver-b")
        .args(["session", "import", name, "--from", &a.ws_str()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "import: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let header = b.header(name);
    let new_id = header["session_id"].as_str().unwrap();
    assert_ne!(new_id, source_id);
    assert!(header.get("root_session_id").is_none());
    assert!(header.get("parent_session_id").is_none());

    let journal = run_journal(&b);
    assert_eq!(journal.len(), 1, "only the import's record: {journal:?}");
    let started = &journal[0];
    assert_eq!(started["kind"], "session_started");
    assert_eq!(started["session"], name);
    assert_eq!(started["koto.session.id"], new_id);
    assert_eq!(started["koto.run.id"], new_id);
    assert_eq!(started["imported_from"], source_id.as_str());
    assert_eq!(started["koto.driver.session.id"], "driver-b");
    assert_eq!(
        started["koto.template.name"],
        source_header["template_name"]
    );
    assert_eq!(
        started["koto.template.hash"],
        source_header["template_hash"]
    );
    // The template source directory is under this test's temporary root,
    // so the importing host classifies the session as a fixture.
    assert_eq!(started["koto.fixture"], true);
    assert!(started.get("koto.parent.session.id").is_none());
}
