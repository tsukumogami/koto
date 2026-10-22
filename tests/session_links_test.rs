//! Integration tests for dispatched-by links: the reserved `koto/` area,
//! `koto session link`, `koto init --dispatched-by`, and the links a
//! leg-bound `koto init --koto-leg` and a self-run `koto request attach`
//! record.
//!
//! Every scenario also checks the single-writer rule: each session other
//! than the one the command opened or linked, the dispatcher included, is
//! byte-identical before and after the command.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;

// ===== Harness =====

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_SESSION_BACKEND");
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

fn run_with_stdin(dir: &Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    let output = koto_cmd(dir)
        .args(args)
        .write_stdin(stdin.as_bytes().to_vec())
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    run_with_stdin(dir, args, "")
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(
        code, 0,
        "expected success from {args:?}\n{stdout}\n{stderr}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

/// Run and require exit `code`, returning stdout.
fn run_code(dir: &Path, args: &[&str], code: i32) -> String {
    let (got, stdout, stderr) = run(dir, args);
    assert_eq!(got, code, "exit of {args:?}\n{stdout}\n{stderr}");
    stdout
}

/// A template whose session stays live until evidence arrives.
const TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
variables:
  TOPIC:
    default: "x"
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work on {{TOPIC}}.

## done

Done.
"#;

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("scope.md"), TEMPLATE).unwrap();
    tmp
}

fn template(dir: &Path) -> String {
    dir.join("scope.md").to_str().unwrap().to_string()
}

fn init(dir: &Path, name: &str, extra: &[&str]) -> serde_json::Value {
    let template = template(dir);
    let mut args = vec!["init", name, "--template", template.as_str()];
    args.extend_from_slice(extra);
    run_ok(dir, &args)
}

fn sessions(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

fn header(dir: &Path, name: &str) -> serde_json::Value {
    let path = sessions(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"));
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

/// Every file under `root`, with its bytes, by path relative to `root`.
fn files_under(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(p.strip_prefix(root).unwrap().to_path_buf(), bytes);
            }
        }
    }
    out
}

/// The files of every session except `except`, by session.
fn other_sessions(dir: &Path, except: &str) -> BTreeMap<String, BTreeMap<PathBuf, Vec<u8>>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(sessions(dir)) else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == except || !e.path().is_dir() {
            continue;
        }
        out.insert(name, files_under(&e.path()));
    }
    out
}

fn session_files(dir: &Path, name: &str) -> BTreeMap<PathBuf, Vec<u8>> {
    files_under(&sessions(dir).join(name))
}

/// Run `f`, asserting every session but `target` is byte-identical across it.
fn single_writer<T>(dir: &Path, target: &str, f: impl FnOnce() -> T) -> T {
    let before = other_sessions(dir, target);
    assert!(!before.is_empty(), "the scenario needs another session");
    let out = f();
    assert_eq!(
        other_sessions(dir, target),
        before,
        "a session other than '{target}' changed"
    );
    out
}

/// The context keys `name` holds.
fn keys(dir: &Path, name: &str) -> Vec<String> {
    let v = run_ok(dir, &["context", "list", name]);
    v.as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap().to_string())
        .collect()
}

/// The dispatched-by link body `name` holds for `peer`, if any.
fn link(dir: &Path, name: &str, peer: &str) -> Option<serde_json::Value> {
    let key = format!("koto/links/dispatched-by/{peer}");
    let (code, stdout, _) = run(dir, &["context", "get", name, &key]);
    (code == 0).then(|| serde_json::from_str(&stdout).unwrap())
}

fn link_keys(dir: &Path, name: &str) -> Vec<String> {
    keys(dir, name)
        .into_iter()
        .filter(|k| k.starts_with("koto/"))
        .collect()
}

fn create_request(dir: &Path, requested_by: &str) -> String {
    let v = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"scope","role":"scope","template":"scope.md","inputs":{}}]}"#,
            "--requested-by",
            requested_by,
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    v["request_id"].as_str().unwrap().to_string()
}

// ===== The reserved area =====

#[test]
fn context_add_and_remove_refuse_the_reserved_area() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "s", &[]);
    init(dir, "other", &[]);
    let before = session_files(dir, "s");

    for key in [
        "koto/links/dispatched-by/other",
        "KOTO/links/dispatched-by/other",
        "Koto/anything",
        "koto",
    ] {
        single_writer(dir, "s", || {
            let (code, stdout, _) = run_with_stdin(dir, &["context", "add", "s", key], "{}");
            assert_eq!(code, 2, "add {key}: {stdout}");
            assert!(stdout.contains("koto/"), "{stdout}");
            let (code, stdout, _) = run(dir, &["context", "remove", "s", key]);
            assert_eq!(code, 2, "remove {key}: {stdout}");
            assert!(stdout.contains("koto/"), "{stdout}");
        });
    }
    assert_eq!(session_files(dir, "s"), before);

    // An ordinary key still works, and a key that merely starts with the
    // letters isn't reserved.
    let (code, _, stderr) = run_with_stdin(dir, &["context", "add", "s", "kotox/notes"], "n");
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn context_remove_refuses_a_link_koto_wrote() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "a", &[]);
    init(dir, "b", &["--dispatched-by", "a"]);
    let before = session_files(dir, "b");
    run_code(
        dir,
        &["context", "remove", "b", "koto/links/dispatched-by/a"],
        2,
    );
    assert_eq!(session_files(dir, "b"), before);
    assert!(link(dir, "b", "a").is_some());
}

// ===== koto session link =====

#[test]
fn session_link_records_one_link_and_rerunning_changes_nothing() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "dispatcher", &[]);
    init(dir, "worker", &[]);

    let out = single_writer(dir, "worker", || {
        run_ok(
            dir,
            &["session", "link", "worker", "--dispatched-by", "dispatcher"],
        )
    });
    assert_eq!(out["name"], "worker");
    assert_eq!(out["kind"], "dispatched-by");
    assert_eq!(out["peer"], "dispatcher");
    assert_eq!(out["written"], true);

    let body = link(dir, "worker", "dispatcher").expect("link written");
    let fields: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(fields.len(), 5, "{body}");
    assert_eq!(body["v"], 1);
    assert_eq!(body["session"], "dispatcher");
    assert_eq!(
        body["session_id"],
        header(dir, "dispatcher")["session_id"],
        "{body}"
    );
    assert!(body["workspace"].is_string(), "{body}");
    assert!(body["at"].is_string(), "{body}");
    assert_eq!(
        link_keys(dir, "worker"),
        ["koto/links/dispatched-by/dispatcher"]
    );

    let before = session_files(dir, "worker");
    let again = single_writer(dir, "worker", || {
        run_ok(
            dir,
            &["session", "link", "worker", "--dispatched-by", "dispatcher"],
        )
    });
    assert_eq!(again["written"], false);
    assert_eq!(session_files(dir, "worker"), before);
}

#[test]
fn session_link_to_an_absent_dispatcher_has_null_hints() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "worker", &[]);
    init(dir, "bystander", &[]);
    single_writer(dir, "worker", || {
        run_ok(
            dir,
            &["session", "link", "worker", "--dispatched-by", "gone"],
        );
    });
    let body = link(dir, "worker", "gone").unwrap();
    assert!(body["session_id"].is_null(), "{body}");
    assert!(body["workspace"].is_null(), "{body}");
}

#[test]
fn session_link_refuses_bad_names_and_writes_nothing() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "worker", &[]);
    init(dir, "dispatcher", &[]);
    let before = files_under(&sessions(dir));

    for bad in ["a~1", "parent~1.task-a", "_x", "a/b", "worker"] {
        let stdout = run_code(
            dir,
            &["session", "link", "worker", "--dispatched-by", bad],
            2,
        );
        let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(v["command"], "session link", "{stdout}");
    }
    run_code(
        dir,
        &[
            "session",
            "link",
            "missing",
            "--dispatched-by",
            "dispatcher",
        ],
        2,
    );
    assert_eq!(files_under(&sessions(dir)), before);
}

// ===== koto init --dispatched-by =====

#[test]
fn init_dispatched_by_records_the_link_in_the_new_session_only() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "dispatcher", &[]);

    single_writer(dir, "worker", || {
        init(dir, "worker", &["--dispatched-by", "dispatcher"]);
    });
    let body = link(dir, "worker", "dispatcher").expect("link");
    assert_eq!(body["session_id"], header(dir, "dispatcher")["session_id"]);

    // The verb finds it already there.
    let before = session_files(dir, "worker");
    let again = run_ok(
        dir,
        &["session", "link", "worker", "--dispatched-by", "dispatcher"],
    );
    assert_eq!(again["written"], false);
    assert_eq!(session_files(dir, "worker"), before);
}

#[test]
fn init_dispatched_by_with_entry_flags_and_from_stdin_records_the_link() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "dispatcher", &[]);

    // The entry-flag path, creating.
    single_writer(dir, "w1", || {
        init(
            dir,
            "w1",
            &["--attach-live", "--dispatched-by", "dispatcher"],
        );
    });
    assert!(link(dir, "w1", "dispatcher").is_some());

    // The entry-flag path, attaching to a live session: the link lands in
    // the attached session.
    init(dir, "w2", &[]);
    single_writer(dir, "w2", || {
        init(
            dir,
            "w2",
            &["--attach-live", "--dispatched-by", "dispatcher"],
        );
    });
    assert!(link(dir, "w2", "dispatcher").is_some());

    // The inline path.
    single_writer(dir, "w3", || {
        let (code, stdout, stderr) = run_with_stdin(
            dir,
            &[
                "init",
                "w3",
                "--from-stdin",
                "--dispatched-by",
                "dispatcher",
            ],
            TEMPLATE,
        );
        assert_eq!(code, 0, "{stdout}\n{stderr}");
    });
    assert!(link(dir, "w3", "dispatcher").is_some());
}

#[test]
fn init_with_an_invalid_dispatcher_exits_two_and_creates_nothing() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "dispatcher", &[]);
    let before = files_under(&sessions(dir));
    let template = template(dir);
    for bad in ["a~1", "_x", "worker"] {
        let (code, stdout, _) = run(
            dir,
            &[
                "init",
                "worker",
                "--template",
                &template,
                "--dispatched-by",
                bad,
            ],
        );
        assert_eq!(code, 2, "{bad}: {stdout}");
        assert!(stdout.contains("--dispatched-by"), "{stdout}");
    }
    assert_eq!(files_under(&sessions(dir)), before);
    assert!(!sessions(dir).join("worker").exists());
}

// ===== Leg-bound links =====

#[test]
fn init_koto_leg_links_the_requester() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    let id = create_request(dir, "requester");

    let target = format!("{id}:scope");
    single_writer(dir, "worker", || {
        init(dir, "worker", &["--koto-leg", &target]);
    });
    let body = link(dir, "worker", "requester").expect("leg-bound link");
    assert_eq!(body["session_id"], header(dir, "requester")["session_id"]);
    assert_eq!(
        link_keys(dir, "worker"),
        ["koto/links/dispatched-by/requester"]
    );
}

#[test]
fn init_koto_leg_on_an_attached_session_links_the_requester() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    init(dir, "worker", &[]);
    let id = create_request(dir, "requester");
    let target = format!("{id}:scope");
    single_writer(dir, "worker", || {
        init(dir, "worker", &["--attach-live", "--koto-leg", &target]);
    });
    assert!(link(dir, "worker", "requester").is_some());
}

#[test]
fn a_requester_a_link_cannot_name_leaves_no_link() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "bystander", &[]);
    // `koto request create` admits the same characters the session-name
    // rule does, so the one requester it accepts that a link can't carry is
    // a name too long to fit the link's key.
    let long = "r".repeat(240);
    let id = create_request(dir, &long);
    let target = format!("{id}:scope");
    single_writer(dir, "worker", || {
        init(dir, "worker", &["--koto-leg", &target]);
    });
    assert!(link_keys(dir, "worker").is_empty());
}

#[test]
fn a_self_run_request_attach_links_the_requester() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    init(dir, "worker", &[]);
    let id = create_request(dir, "requester");

    single_writer(dir, "worker", || {
        run_ok(
            dir,
            &["request", "attach", &id, "scope", "--session", "worker"],
        );
    });
    let body = link(dir, "worker", "requester").expect("self-attach link");
    assert_eq!(body["session_id"], header(dir, "requester")["session_id"]);
}

#[test]
fn a_self_run_attach_naming_its_own_session_as_issuer_links_the_requester() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    init(dir, "worker", &[]);
    let id = create_request(dir, "requester");
    single_writer(dir, "worker", || {
        run_ok(
            dir,
            &[
                "request",
                "attach",
                &id,
                "scope",
                "--session",
                "worker",
                "--issued-by",
                "worker",
            ],
        );
    });
    assert!(link(dir, "worker", "requester").is_some());
}

#[test]
fn an_attach_issued_for_another_session_writes_nothing_into_it() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    init(dir, "worker", &[]);
    let id = create_request(dir, "requester");
    let before = session_files(dir, "worker");
    let ctx_before: BTreeMap<_, _> = before
        .iter()
        .filter(|(p, _)| p.starts_with("ctx"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();

    single_writer(dir, "worker", || {
        run_ok(
            dir,
            &[
                "request",
                "attach",
                &id,
                "scope",
                "--session",
                "worker",
                "--issued-by",
                "someone-else",
            ],
        );
    });
    assert!(link_keys(dir, "worker").is_empty());
    // The attach writes its leg pointer; the context is untouched.
    let ctx_after: BTreeMap<_, _> = session_files(dir, "worker")
        .into_iter()
        .filter(|(p, _)| p.starts_with("ctx"))
        .collect();
    assert_eq!(ctx_after, ctx_before);
}

#[test]
fn a_refused_attach_writes_no_link() {
    let tmp = setup();
    let dir = tmp.path();
    init(dir, "requester", &[]);
    init(dir, "worker", &[]);
    let id = create_request(dir, "requester");
    let before = files_under(&sessions(dir));
    // No such leg.
    let (code, _, _) = run(
        dir,
        &["request", "attach", &id, "nope", "--session", "worker"],
    );
    assert_ne!(code, 0);
    assert_eq!(files_under(&sessions(dir)), before);
}
