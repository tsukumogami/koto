//! `koto --workspace <id> <verb>` end to end: a session read in place from
//! another workspace's part of the bucket, through an in-process
//! S3-compatible endpoint that records every request.
//!
//! Each host is its own `HOME`, cache and workspace directory
//! (`tests/support/migration_carrier.rs`), and each read runs with a `TMPDIR`
//! of its own, so a test can say the reader left no scratch directory behind.
//! The reads must write nothing: no PUT or DELETE reaches the bucket, the
//! source's objects stay byte for byte, and nothing lands in the reading
//! host's session store.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use migration_carrier::{Cloud, Host, Run};

const BUCKET: &str = "koto-workspace-read";
const NAME: &str = "record";
const NOTES: &[u8] = b"what the run did\n";

fn cloud(s3: &FakeS3) -> Cloud {
    Cloud {
        endpoint: s3.endpoint(),
        bucket: s3.bucket().to_string(),
        region: "us-east-1".to_string(),
        path_style: true,
        credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
    }
}

/// Run koto on `host` with a `TMPDIR` of the run's own, which must be left
/// empty: the reader's scratch directory goes however the command exits.
fn koto_in(host: &Host, args: &[&str]) -> Run {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = host
        .cmd()
        .env("TMPDIR", tmp.path())
        .args(args)
        .output()
        .unwrap();
    let left: Vec<PathBuf> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        left.is_empty(),
        "`koto {}` left {:?} in its TMPDIR",
        args.join(" "),
        left
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null);
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        json,
    }
}

/// `koto --workspace <workspace> <args>` on `host`.
fn read(host: &Host, workspace: &str, args: &[&str]) -> Run {
    let mut argv = vec!["--workspace", workspace];
    argv.extend_from_slice(args);
    koto_in(host, &argv)
}

fn ok(run: &Run, what: &str) {
    assert!(run.ok(), "{}: {}", what, run.describe());
}

/// A read through `--workspace` warns about nothing: a write it tried and
/// the read-only backend refused would show here.
fn quiet(run: &Run, what: &str) {
    assert!(run.stderr.is_empty(), "{} warned: {}", what, run.describe());
}

/// Every object in the bucket, with its bytes.
fn objects(s3: &FakeS3) -> BTreeMap<String, Vec<u8>> {
    s3.keys()
        .into_iter()
        .map(|k| {
            let v = s3.object(&k).unwrap();
            (k, v)
        })
        .collect()
}

/// Every PUT and DELETE the endpoint saw, as `METHOD key`.
fn writes(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "PUT" || r.method == "DELETE")
        .map(|r| format!("{} {}", r.method, r.key))
        .collect()
}

/// Create `NAME` on `host`, store notes, and run it to its terminal state,
/// keeping it (`--no-cleanup`) as a finished run's record.
fn finished_session(host: &Host) {
    let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
    ok(
        &host.koto(&["init", NAME, "--template", template.to_str().unwrap()]),
        "init",
    );
    let notes = host.home.join("notes.md");
    std::fs::write(&notes, NOTES).unwrap();
    ok(
        &host.koto(&[
            "context",
            "add",
            NAME,
            "notes.md",
            "--from-file",
            notes.to_str().unwrap(),
        ]),
        "context add",
    );
    ok(&host.koto(&["next", NAME]), "next");
    ok(
        &host.koto(&["next", NAME, "--with-data", r#"{"choice":"go"}"#]),
        "next go",
    );
    ok(
        &host.koto(&[
            "next",
            NAME,
            "--with-data",
            r#"{"verdict":"approve"}"#,
            "--no-cleanup",
        ]),
        "next approve",
    );
    let status = host.koto(&["status", NAME]);
    ok(&status, "status");
    assert_eq!(status.json["is_terminal"], true, "{}", status.describe());
}

/// Two hosts on one bucket: `a` holds a finished session, `b` is a fresh
/// checkout elsewhere that has never seen it.
struct Fixture {
    s3: FakeS3,
    _root: tempfile::TempDir,
    a: Host,
    b: Host,
}

fn fixture() -> Fixture {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let a = Host::new(root.path(), "a", &cloud(&s3));
    let b = Host::new(root.path(), "b", &cloud(&s3));
    finished_session(&a);
    Fixture {
        s3,
        _root: root,
        a,
        b,
    }
}

/// Every file under `dir`, for a before-and-after comparison.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// The reads the option exists for, from a host that never had the
/// session: `status`, `context get`, `exists` and `list` answer from A's
/// copy, by A's id and by A's path, and nothing is written anywhere.
#[test]
fn reads_a_finished_session_from_another_host_in_place() {
    let f = fixture();
    let prefix = f.a.prefix();
    let before = objects(&f.s3);
    f.s3.clear_requests();

    for workspace in [prefix.clone(), f.a.ws_str()] {
        let status = read(&f.b, &workspace, &["status", NAME]);
        ok(&status, "status");
        quiet(&status, "status");
        assert_eq!(status.json["name"], NAME);
        assert_eq!(status.json["current_state"], "done");
        assert_eq!(status.json["is_terminal"], true);
        assert!(
            status.json.get("template_hash_mismatch").is_none(),
            "{}",
            status.describe()
        );

        let get = read(&f.b, &workspace, &["context", "get", NAME, "notes.md"]);
        ok(&get, "context get");
        quiet(&get, "context get");
        assert_eq!(get.stdout.as_bytes(), NOTES);

        let list = read(&f.b, &workspace, &["context", "list", NAME]);
        ok(&list, "context list");
        quiet(&list, "context list");
        assert_eq!(list.json, serde_json::json!(["notes.md"]));

        let exists = read(&f.b, &workspace, &["context", "exists", NAME, "notes.md"]);
        assert_eq!(exists.code, Some(0), "{}", exists.describe());
        quiet(&exists, "context exists");
        let absent = read(&f.b, &workspace, &["context", "exists", NAME, "absent.md"]);
        assert_eq!(absent.code, Some(1), "{}", absent.describe());
    }

    assert_eq!(writes(&f.s3), Vec::<String>::new(), "the reads wrote");
    assert_eq!(objects(&f.s3), before, "the bucket changed");
    // A listing is recorded with its prefix, any other request with its key.
    let elsewhere: Vec<String> =
        f.s3.requests()
            .into_iter()
            .map(|r| (r.method, r.list_prefix.unwrap_or(r.key)))
            .filter(|(_, target)| !target.starts_with(&prefix))
            .map(|(method, target)| format!("{} {}", method, target))
            .collect();
    assert!(
        elsewhere.is_empty(),
        "requests outside A's prefix: {:?}",
        elsewhere
    );
    assert!(
        !f.b.session_dir(NAME).exists(),
        "the read left a session in B's store"
    );
}

/// A reader whose own workspace holds a session of the same name reads the
/// other workspace's, and its own session, local copy and prefix are left
/// as they were.
#[test]
fn the_named_workspace_wins_over_a_session_of_the_same_name_here() {
    let f = fixture();
    let template = f.b.ws.join(migration_carrier::TEMPLATE_FILE);
    ok(
        &f.b.koto(&["init", NAME, "--template", template.to_str().unwrap()]),
        "init on B",
    );
    let b_local = tree(&f.b.session_dir(NAME));
    let before = objects(&f.s3);
    f.s3.clear_requests();

    let status = read(&f.b, &f.a.prefix(), &["status", NAME]);
    ok(&status, "status");
    assert_eq!(status.json["current_state"], "done");
    let get = read(&f.b, &f.a.prefix(), &["context", "get", NAME, "notes.md"]);
    ok(&get, "context get");
    assert_eq!(get.stdout.as_bytes(), NOTES);

    let own = f.b.koto(&["status", NAME]);
    ok(&own, "B's own status");
    assert_eq!(own.json["current_state"], "start");
    assert_eq!(writes(&f.s3), Vec::<String>::new());
    assert_eq!(objects(&f.s3), before);
    assert_eq!(tree(&f.b.session_dir(NAME)), b_local, "B's copy changed");
}

/// On the host that created the session, a read from another workspace
/// leaves the shared store's copy of it exactly as it was.
#[test]
fn a_same_host_read_leaves_the_shared_store_alone() {
    let f = fixture();
    let mut c = Host::new(f._root.path(), "c", &cloud(&f.s3));
    c.home = f.a.home.clone();
    c.cache = f.a.cache.clone();
    let a_local = tree(&f.a.session_dir(NAME));
    let before = objects(&f.s3);
    f.s3.clear_requests();

    for args in [
        vec!["status", NAME],
        vec!["context", "get", NAME, "notes.md"],
        vec!["context", "list", NAME],
    ] {
        ok(&read(&c, &f.a.prefix(), &args), &args.join(" "));
    }
    assert_eq!(writes(&f.s3), Vec::<String>::new());
    assert_eq!(objects(&f.s3), before);
    assert_eq!(tree(&f.a.session_dir(NAME)), a_local, "A's copy changed");
}

/// A command a `koto next` tick runs carries the tick's marker. Naming a
/// session of the same name, it would defer a write into the tick's batch
/// or log a read as the session's own; under `--workspace` it does neither.
#[test]
fn a_tick_marker_for_the_same_name_changes_nothing() {
    let f = fixture();
    let before = objects(&f.s3);
    f.s3.clear_requests();
    let tmp = tempfile::TempDir::new().unwrap();
    for args in read_verbs(NAME) {
        let output =
            f.b.cmd()
                .env("TMPDIR", tmp.path())
                .env("KOTO_TICK_SESSION", NAME)
                .args(["--workspace", &f.a.prefix()])
                .args(&args)
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(writes(&f.s3), Vec::<String>::new());
    assert_eq!(objects(&f.s3), before);
}

/// The four read verbs, on session `name`.
fn read_verbs(name: &str) -> Vec<Vec<&str>> {
    vec![
        vec!["status", name],
        vec!["context", "get", name, "notes.md"],
        vec!["context", "exists", name, "notes.md"],
        vec!["context", "list", name],
    ]
}

/// A session the workspace doesn't hold, and a workspace with no session
/// at all, are each reported as not found, exit 2, by every read verb.
#[test]
fn a_missing_session_or_workspace_is_not_found() {
    let f = fixture();
    f.s3.clear_requests();
    let nowhere = "ffffffffffffffff";
    for args in read_verbs("missing") {
        let run = read(&f.b, &f.a.prefix(), &args);
        assert_eq!(run.code, Some(2), "{}", run.describe());
        assert_eq!(run.json["code"], "session_not_found", "{}", run.describe());
        assert!(run.json["error"].as_str().unwrap().contains("missing"));
    }
    for args in read_verbs(NAME) {
        let run = read(&f.b, nowhere, &args);
        assert_eq!(run.code, Some(2), "{}", run.describe());
        assert_eq!(
            run.json["code"],
            "workspace_not_found",
            "{}",
            run.describe()
        );
        assert!(run.json["error"].as_str().unwrap().contains(nowhere));
    }
    assert_eq!(writes(&f.s3), Vec::<String>::new());
}

/// A bucket that can't be listed is unreadable, exit 3, never an empty
/// workspace or a missing session.
#[test]
fn an_unreadable_bucket_is_reported_as_unreadable() {
    let f = fixture();
    f.s3.fail("LIST", &format!("{}/{}/", f.a.prefix(), NAME), 500);
    for args in read_verbs(NAME) {
        let run = read(&f.b, &f.a.prefix(), &args);
        assert_eq!(run.code, Some(3), "{}", run.describe());
        assert_eq!(
            run.json["code"],
            "workspace_unreadable",
            "{}",
            run.describe()
        );
    }

    // The session listing that tells a missing workspace from a missing
    // session fails the same way.
    f.s3.clear_faults();
    f.s3.fail("LIST", &format!("{}/", f.a.prefix()), 500);
    let run = read(&f.b, &f.a.prefix(), &["status", "missing"]);
    assert_eq!(run.code, Some(3), "{}", run.describe());
    assert_eq!(
        run.json["code"],
        "workspace_unreadable",
        "{}",
        run.describe()
    );
}

/// A session an older koto created records no prefix in its origin, and
/// one started with `--execution-dir` or rebound has an anchor whose prefix
/// isn't the one it lives under. A command from another directory refuses
/// such a session as bound elsewhere, judging by its anchor; the reader
/// must not, since it reads the session under the prefix it names, or its
/// context reads would answer from an empty scratch copy.
#[test]
fn a_session_with_no_recorded_prefix_and_an_anchor_elsewhere_reads_in_full() {
    let f = fixture();
    let key = format!("{}/{}/koto-{}.state.jsonl", f.a.prefix(), NAME, NAME);
    let log = String::from_utf8(f.s3.object(&key).unwrap()).unwrap();
    let (header, events) = log.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(header).unwrap();
    let elsewhere = f._root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere = std::fs::canonicalize(elsewhere).unwrap();
    header["origin"]["store"]
        .as_object_mut()
        .expect("a cloud session records its store")
        .remove("prefix");
    header["origin"]["anchor"] = serde_json::json!(elsewhere);
    header["execution_dir"] = serde_json::json!(elsewhere);
    f.s3.put(&key, format!("{}\n{}", header, events).as_bytes());
    let before = objects(&f.s3);
    f.s3.clear_requests();

    let status = read(&f.b, &f.a.prefix(), &["status", NAME]);
    ok(&status, "status");
    assert_eq!(status.json["current_state"], "done");
    let get = read(&f.b, &f.a.prefix(), &["context", "get", NAME, "notes.md"]);
    ok(&get, "context get");
    assert_eq!(get.stdout.as_bytes(), NOTES);
    let list = read(&f.b, &f.a.prefix(), &["context", "list", NAME]);
    ok(&list, "context list");
    assert_eq!(list.json, serde_json::json!(["notes.md"]));
    let exists = read(
        &f.b,
        &f.a.prefix(),
        &["context", "exists", NAME, "notes.md"],
    );
    assert_eq!(exists.code, Some(0), "{}", exists.describe());
    assert_eq!(writes(&f.s3), Vec::<String>::new());
    assert_eq!(objects(&f.s3), before);
}

/// The template path a session's log records comes from the bucket. One
/// that climbs out with `..` is refused when `status` puts the session's
/// compiled template in place, so nothing is written outside the reader's
/// scratch directory, whose parent here is the run's own `TMPDIR`.
#[test]
fn a_template_path_that_climbs_out_writes_nothing_outside_scratch() {
    let f = fixture();
    let key = format!("{}/{}/koto-{}.state.jsonl", f.a.prefix(), NAME, NAME);
    let log = String::from_utf8(f.s3.object(&key).unwrap()).unwrap();
    let mut planted = false;
    let lines: Vec<String> = log
        .lines()
        .map(|line| {
            let mut event: serde_json::Value = serde_json::from_str(line).unwrap();
            if event["type"] == "workflow_initialized" {
                event["payload"]["template_path"] = serde_json::json!("../../escaped.json");
                planted = true;
            }
            event.to_string()
        })
        .collect();
    assert!(planted, "no workflow_initialized event in {}", log);
    f.s3.put(&key, format!("{}\n", lines.join("\n")).as_bytes());

    let tmp = tempfile::TempDir::new().unwrap();
    let output =
        f.b.cmd()
            .env("TMPDIR", tmp.path())
            .args(["--workspace", &f.a.prefix(), "status", NAME])
            .output()
            .unwrap();
    let left: Vec<PathBuf> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        left.is_empty(),
        "status wrote {:?} outside its scratch directory (exit {:?}, stdout {})",
        left,
        output.status.code(),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Credentials in the endpoint's URL never reach what a read prints when
/// the bucket can't be read.
#[test]
fn an_unreadable_bucket_prints_no_endpoint_credentials() {
    let root = tempfile::TempDir::new().unwrap();
    let reader = Host::new(
        root.path(),
        "reader",
        &Cloud {
            endpoint: "http://reader:hunter2-secret@127.0.0.1:1".to_string(),
            bucket: BUCKET.to_string(),
            region: "us-east-1".to_string(),
            path_style: true,
            credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
        },
    );
    for args in read_verbs(NAME) {
        let run = read(&reader, "0123456789abcdef", &args);
        assert_eq!(run.code, Some(3), "{}", run.describe());
        assert_eq!(
            run.json["code"],
            "workspace_unreadable",
            "{}",
            run.describe()
        );
        assert!(
            !run.stdout.contains("hunter2-secret") && !run.stderr.contains("hunter2-secret"),
            "the endpoint's credentials were printed: {}",
            run.describe()
        );
    }
}

/// A session imported into another workspace refuses the read as it
/// refuses any other, naming where it went.
#[test]
fn a_migrated_session_refuses_and_names_the_new_one() {
    let f = fixture();
    let marker = serde_json::json!({
        "schema": 1,
        "target": {
            "session": "moved",
            "session_id": "00000000-0000-0000-0000-000000000000",
            "workspace": f.b.ws_str(),
            "prefix": f.b.prefix(),
        },
        "machine_id": "test",
        "migrated_at": "2026-01-01T00:00:00Z",
    });
    f.s3.put(
        &format!("{}/{}/migrated.json", f.a.prefix(), NAME),
        marker.to_string().as_bytes(),
    );
    for args in [
        vec!["status", NAME],
        vec!["context", "get", NAME, "notes.md"],
        vec!["context", "list", NAME],
    ] {
        let run = read(&f.b, &f.a.prefix(), &args);
        assert_eq!(run.code, Some(2), "{}", run.describe());
        let error = run.json["error"].as_str().unwrap_or_default();
        assert!(error.contains("session_migrated"), "{}", run.describe());
        assert!(error.contains("moved"), "{}", run.describe());
    }
}

/// Every verb that writes or removes refuses `--workspace` with
/// `workspace_read_only`, exit 2, before it makes a single request.
/// `src/cli/workspace_read.rs` walks every verb of the CLI; this runs the
/// ones a reader is most likely to reach for, end to end.
#[test]
fn write_and_removal_verbs_refuse_workspace() {
    let f = fixture();
    let notes = f.b.home.join("notes.md");
    std::fs::write(&notes, NOTES).unwrap();
    let template = f.b.ws.join(migration_carrier::TEMPLATE_FILE);
    let before = objects(&f.s3);
    f.s3.clear_requests();
    let a_ws = f.a.ws_str();
    let refused: Vec<Vec<&str>> = vec![
        vec!["init", NAME, "--template", template.to_str().unwrap()],
        vec!["next", NAME],
        vec!["next", NAME, "--with-data", r#"{"choice":"go"}"#],
        vec!["rewind", NAME],
        vec!["cancel", NAME],
        vec![
            "context",
            "add",
            NAME,
            "k.md",
            "--from-file",
            notes.to_str().unwrap(),
        ],
        vec!["context", "remove", NAME, "notes.md"],
        vec!["session", "cleanup", NAME],
        vec!["session", "prune", "--all", "--apply"],
        vec!["session", "prune", "--migrated-from", &a_ws],
        vec!["workspace", "prune", "--root", NAME, "--yes"],
        vec!["session", "import", NAME, "--from", &a_ws],
        vec!["session", "rebind", NAME],
        vec!["session", "resolve", NAME, "--keep", "local"],
        vec!["session", "update", NAME, "--intent", "x"],
        vec!["decisions", "record", NAME, "--with-data", "{}"],
        vec!["session", "list"],
        vec!["workflows"],
    ];
    for args in refused {
        let run = read(&f.b, &f.a.prefix(), &args);
        assert_eq!(
            run.code,
            Some(2),
            "`{}`: {}",
            args.join(" "),
            run.describe()
        );
        assert_eq!(
            run.json["code"],
            "workspace_read_only",
            "`{}`: {}",
            args.join(" "),
            run.describe()
        );
        assert!(run.json["error"].as_str().unwrap().contains("--workspace"));
    }
    assert!(f.s3.requests().is_empty(), "a refused verb made requests");
    assert_eq!(objects(&f.s3), before);
    let status = f.a.koto(&["status", NAME]);
    assert_eq!(status.json["current_state"], "done");
}

/// `--workspace` names a workspace by its id or its absolute path, and
/// reads the cloud bucket: anything else is refused before any request.
#[test]
fn a_workspace_that_names_nothing_or_a_local_backend_is_refused() {
    let f = fixture();
    f.s3.clear_requests();
    for given in ["relative/dir", "0123456789ABCDEF", "name:shared"] {
        let run = read(&f.b, given, &["status", NAME]);
        assert_eq!(run.code, Some(2), "{given}: {}", run.describe());
        assert_eq!(run.json["code"], "invalid_workspace", "{}", run.describe());
    }
    let run = read(&f.b, &f.a.prefix(), &["status", "../escape"]);
    assert_eq!(run.code, Some(2), "{}", run.describe());
    assert_eq!(run.json["code"], "invalid_session", "{}", run.describe());
    assert!(f.s3.requests().is_empty());

    // A workspace whose session backend is local has no bucket to read.
    let home = tempfile::TempDir::new().unwrap();
    let ws = tempfile::TempDir::new().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let output = koto_command::koto()
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("KOTO_SESSIONS_BASE")
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env_remove("XDG_CONFIG_HOME")
        .env("TMPDIR", tmp.path())
        .current_dir(ws.path())
        .args(["--workspace", &f.a.prefix(), "status", NAME])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["code"], "workspace_needs_cloud", "{json}");
}
