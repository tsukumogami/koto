//! Integration tests for the `koto request` noun group (issues 6-9).
//!
//! Unit tests in `src/cli/request.rs` cover the pure pieces — the
//! contract pin, the exit-class mapping, the store-error routing, the
//! fence comparison, and the JSON flag guards. This file drives the
//! real binary against a real workspace, which is the only way to
//! assert the things the acceptance criteria are actually about:
//! process exit statuses, byte-equal stdout across two reads, and that
//! a read writes nothing to disk.
//!
//! Every test runs with `HOME` pointed at its own temporary directory,
//! so the request store lands at `<tmp>/.koto/requests/`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_fs::TempDir;

// ===== Harness =====

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

/// Run the binary and return `(exit code, stdout, stderr)`.
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Run and require success, returning the parsed envelope.
fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(
        code, 0,
        "expected success from {args:?}\n{stdout}\n{stderr}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

/// Run and require failure, returning `(exit code, error code)`.
fn run_err(dir: &Path, args: &[&str]) -> (i32, String) {
    let (code, stdout, stderr) = run(dir, args);
    assert_ne!(code, 0, "expected failure from {args:?}\n{stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}\n{stderr}"));
    let error = json
        .get("error")
        .unwrap_or_else(|| panic!("no error key in {json}"));
    assert!(
        error.is_object(),
        "the structured nested envelope puts an object here, not a string: {json}"
    );
    let error_code = error["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no error.code in {json}"))
        .to_string();
    assert!(
        error["message"].is_string(),
        "every failure carries a message: {json}"
    );
    assert!(
        error["details"].is_array(),
        "every failure carries a details array, empty when not field-specific: {json}"
    );
    (code, error_code)
}

const TWO_LEGS: &str = r#"{"legs":[
    {"name":"reviewer-a","role":"security","template":"review","inputs":{"pr":42}},
    {"name":"reviewer-b","role":"perf","template":"review","inputs":{"pr":42}}
],"inputs":{"pr":42}}"#;

const THREE_LEGS: &str = r#"{"legs":[
    {"name":"leg-a","role":"a","template":"review","inputs":{}},
    {"name":"leg-b","role":"b","template":"review","inputs":{}},
    {"name":"leg-c","role":"c","template":"review","inputs":{}}
],"inputs":{}}"#;

/// Create a request from a `--with-data` payload and return its id.
fn create(dir: &Path, with_data: &str) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            with_data,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    envelope["request_id"].as_str().unwrap().to_string()
}

fn requests_root(dir: &Path) -> PathBuf {
    dir.join(".koto").join("requests")
}

fn log_path(dir: &Path, id: &str) -> PathBuf {
    requests_root(dir).join(id).join("request.jsonl")
}

/// Snapshot every file under `~/.koto` as (path, length, mtime).
///
/// Used to assert a read verb wrote nothing at all — not the request
/// log, not a coordinator cursor, not the terminal index.
fn workspace_snapshot(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    fn walk(path: &Path, out: &mut Vec<(PathBuf, u64, std::time::SystemTime)>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let md = match entry.metadata() {
                Ok(md) => md,
                Err(_) => continue,
            };
            if md.is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push((
                    entry.path(),
                    md.len(),
                    md.modified().unwrap_or(std::time::UNIX_EPOCH),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(&dir.join(".koto"), &mut out);
    out.sort();
    out
}

// ===== Fenceable children =====
//
// `bind` refuses a child whose session it cannot read and a child whose
// header does not satisfy the dispatch-fence predicate, so a leg cannot
// be bound to something that could never be fenced (Issue 10). Every
// bind in this file therefore needs a real child session shaped like a
// dispatched request-store delegate.

const CHILD_TEMPLATE: &str = r#"---
name: child-task
version: "1.0"
initial_state: work
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

Do work.

## done

Done.
"#;

const PARENT_TEMPLATE: &str = r#"---
name: parent-coord
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      result:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## gather

Gather evidence.

## done

Done.
"#;

fn sessions_base(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

fn child_state_path(dir: &Path, child: &str) -> PathBuf {
    sessions_base(dir)
        .join(child)
        .join(format!("koto-{child}.state.jsonl"))
}

/// Initialize `<parent>` once, so children can be created under it.
fn init_parent(dir: &Path, parent: &str) {
    if child_state_path(dir, parent).exists() {
        return;
    }
    let template = dir.join("parent.md");
    if !template.exists() {
        std::fs::write(&template, PARENT_TEMPLATE).unwrap();
    }
    let (code, stdout, stderr) = run(
        dir,
        &["init", parent, "--template", template.to_str().unwrap()],
    );
    assert_eq!(code, 0, "init parent failed\n{stdout}\n{stderr}");
}

/// Create a child session `bind` will accept: a child workflow whose
/// header carries `needs_agent: true` and the requested dispatch epoch.
fn fenceable_child(dir: &Path, parent: &str, child: &str, dispatch_epoch: u32) {
    let template = dir.join("child.md");
    if !template.exists() {
        std::fs::write(&template, CHILD_TEMPLATE).unwrap();
    }
    let (code, stdout, stderr) = run(
        dir,
        &[
            "init",
            child,
            "--template",
            template.to_str().unwrap(),
            "--parent",
            parent,
        ],
    );
    assert_eq!(code, 0, "init child failed\n{stdout}\n{stderr}");
    koto::engine::claim::rewrite_header_atomically(&child_state_path(dir, child), |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("coord-a".into());
        h.dispatch_epoch = dispatch_epoch;
        h
    })
    .unwrap();
}

/// Advance an existing child's dispatch epoch in place, the way a
/// redelegation does: same session id, bumped epoch.
fn redelegate(dir: &Path, child: &str, new_epoch: u32) {
    koto::engine::claim::rewrite_header_atomically(&child_state_path(dir, child), |mut h| {
        h.dispatch_epoch = new_epoch;
        h
    })
    .unwrap();
}

/// The common shape: one parent and one child at the given epoch.
fn bindable(dir: &Path, child: &str, dispatch_epoch: u32) {
    init_parent(dir, "coord-a");
    fenceable_child(dir, "coord-a", child, dispatch_epoch);
}

// ===== Issue 6: create, get, list =====

#[test]
fn create_accepts_the_full_form_and_prints_the_generated_id() {
    let tmp = TempDir::new().unwrap();
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "create",
            "--with-data",
            TWO_LEGS,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-b",
        ],
    );

    let id = envelope["request_id"].as_str().expect("a generated id");
    assert!(id.starts_with("req-"), "got {id}");
    assert!(
        log_path(tmp.path(), id).exists(),
        "the id in the envelope must name a record on disk"
    );
    assert_eq!(envelope["requested_by"], "coord-a");
    assert_eq!(envelope["coordinator_of_record"], "coord-b");
    assert_eq!(envelope["leg_counts"]["total"], 2);
    assert_eq!(envelope["inputs"]["pr"], 42);
}

#[test]
fn create_accepts_the_one_leg_shorthand() {
    let tmp = TempDir::new().unwrap();
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "create",
            "--role",
            "security",
            "--template",
            "review",
            "--inputs",
            r#"{"pr":7}"#,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );

    assert_eq!(envelope["leg_counts"]["total"], 1);
    // The shorthand carries no leg name, so the leg is named after
    // the role rather than after an invented constant.
    let leg = &envelope["legs"]["security"];
    assert_eq!(leg["declaration"]["role"], "security");
    assert_eq!(leg["declaration"]["template"], "review");
    assert_eq!(leg["declaration"]["inputs"]["pr"], 7);
}

#[test]
fn the_two_create_forms_are_mutually_exclusive_and_one_is_required() {
    let tmp = TempDir::new().unwrap();
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "create",
            "--with-data",
            TWO_LEGS,
            "--role",
            "security",
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "invalid_submission");

    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "create",
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "invalid_submission");
}

#[test]
fn the_envelope_carries_the_five_contract_fields() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    let envelope = run_ok(tmp.path(), &["request", "get", &id]);

    assert_eq!(envelope["request_state"], "open");
    assert!(
        envelope.get("close_disposition").is_some(),
        "close_disposition is present and null while open, so a consumer \
         never distinguishes absent from not-yet-closed"
    );
    assert!(envelope["close_disposition"].is_null());
    assert_eq!(envelope["leg_counts"]["total"], 2);
    assert_eq!(envelope["leg_counts"]["open"], 2);
    assert_eq!(envelope["revision"], 1);

    // Two integers, not a "1.0" string: a consumer comparing "1.10"
    // against "1.9" lexicographically would be wrong.
    assert!(envelope["cli_contract"]["major"].is_u64());
    assert!(envelope["cli_contract"]["minor"].is_u64());
    assert_eq!(envelope["cli_contract"]["major"], 1);
    assert_eq!(envelope["cli_contract"]["minor"], 0);
}

#[test]
fn output_is_json_unconditionally_with_no_format_flag() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let (code, stdout, _) = run(tmp.path(), &["request", "get", &id]);
    assert_eq!(code, 0);
    serde_json::from_str::<serde_json::Value>(&stdout).expect("stdout is JSON");

    // No format flag exists to ask for anything else, matching
    // `koto next` and `koto status`.
    let (code, _, _) = run(tmp.path(), &["request", "get", &id, "--format", "json"]);
    assert_eq!(code, 2, "--format must not be a recognized flag");
}

#[test]
fn get_exits_zero_for_open_partially_resolved_and_closed_alike() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // Open.
    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(envelope["request_state"], "open");
    assert_eq!(envelope["leg_counts"]["open"], 2);

    // Partially resolved: still a successful read, not a "not ready"
    // failure. Readiness is `wait`'s job, not `get`'s.
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"looks fine"}"#,
        ],
    );
    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(envelope["request_state"], "open");
    assert_eq!(envelope["leg_counts"]["resolved"], 1);

    // Fully resolved, then closed.
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-b",
            "--with-data",
            r#"{"status":"failure","summary":"regression"}"#,
        ],
    );
    run_ok(tmp.path(), &["request", "get", &id]);
    run_ok(tmp.path(), &["request", "close", &id]);
    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(envelope["request_state"], "closed");
    assert_eq!(envelope["close_disposition"], "all_resolved");
}

#[test]
fn two_gets_on_an_unchanged_request_are_byte_equal() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"halfway"}"#,
        ],
    );

    let (_, first, _) = run(tmp.path(), &["request", "get", &id]);
    let (_, second, _) = run(tmp.path(), &["request", "get", &id]);
    assert_eq!(
        first, second,
        "byte equality is what makes a wrapper's change detection a string compare"
    );
}

#[test]
fn a_request_that_does_not_exist_is_a_caller_error() {
    let tmp = TempDir::new().unwrap();
    let (code, error) = run_err(tmp.path(), &["request", "get", "req-does-not-exist"]);
    assert_eq!(
        code, 2,
        "not-found sits where workflow_not_initialized does"
    );
    assert_eq!(error, "request_not_found");
}

#[test]
fn a_malformed_identifier_is_a_caller_error_not_a_transient_one() {
    let tmp = TempDir::new().unwrap();
    for bad in ["../etc", "a/b", "REQ-UPPER", ""] {
        let (code, error) = run_err(tmp.path(), &["request", "get", bad]);
        assert_eq!(
            code, 2,
            "'{bad}' must be a caller error; the crate's existing invalid-session \
             error falls through to the transient class and would be wrong here"
        );
        assert_eq!(error, "invalid_identifier", "for '{bad}'");
    }
    assert!(
        !requests_root(tmp.path()).exists(),
        "a rejected identifier must never reach the filesystem"
    );
}

#[test]
fn a_contract_mismatch_is_a_caller_error_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    let before = workspace_snapshot(tmp.path());

    // On a read...
    let (code, error) = run_err(
        tmp.path(),
        &["request", "get", &id, "--cli-contract", "2.0"],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "contract_mismatch");

    // ...and on a write, where "validated before any IO" is what
    // actually matters.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
            "--cli-contract",
            "9.9",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "contract_mismatch");

    assert_eq!(
        before,
        workspace_snapshot(tmp.path()),
        "a contract mismatch must be rejected before any IO"
    );
}

#[test]
fn the_matching_contract_pin_is_accepted_on_every_subcommand() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 0);
    let pin = ["--cli-contract", "1.0"];

    // One invocation per subcommand, in an order that keeps the
    // request legal at each step.
    run_ok(tmp.path(), &[&["request", "get", &id], &pin[..]].concat());
    run_ok(tmp.path(), &[&["request", "list"], &pin[..]].concat());
    run_ok(
        tmp.path(),
        &[
            &["request", "bind", &id, "reviewer-a", "--child", "child-1"],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "progress",
                &id,
                "reviewer-a",
                "--with-data",
                r#"{"note":"one"}"#,
                // Bound legs always carry an epoch now: `bind` reads it
                // off the child's header rather than leaving it unset.
                "--dispatch-epoch",
                "0",
            ],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "wait",
                &id,
                "--resolved-count",
                "0",
                "--timeout-secs",
                "1",
            ],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "resolve",
                &id,
                "reviewer-b",
                "--with-data",
                r#"{"status":"success","summary":"x"}"#,
            ],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "abandon",
                &id,
                "reviewer-a",
                "--rationale",
                "stopped waiting",
                "--dispatch-epoch",
                "0",
            ],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(tmp.path(), &[&["request", "close", &id], &pin[..]].concat());

    // `create` and `abandon-request` need their own request.
    let second = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "abandon-request",
                &second,
                "--rationale",
                "no longer needed",
            ],
            &pin[..],
        ]
        .concat(),
    );
    run_ok(
        tmp.path(),
        &[
            &[
                "request",
                "create",
                "--with-data",
                TWO_LEGS,
                "--requested-by",
                "coord-a",
                "--coordinator-of-record",
                "coord-a",
            ],
            &pin[..],
        ]
        .concat(),
    );
}

#[test]
fn list_filters_by_requester_coordinator_state_and_unresolved_legs() {
    let tmp = TempDir::new().unwrap();
    let mine = create(tmp.path(), TWO_LEGS);
    let theirs = run_ok(
        tmp.path(),
        &[
            "request",
            "create",
            "--with-data",
            TWO_LEGS,
            "--requested-by",
            "coord-z",
            "--coordinator-of-record",
            "coord-z",
        ],
    )["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    let ids = |envelope: &serde_json::Value| -> Vec<String> {
        envelope["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["request_id"].as_str().unwrap().to_string())
            .collect()
    };

    let all = run_ok(tmp.path(), &["request", "list"]);
    assert_eq!(ids(&all).len(), 2);
    assert!(all["cli_contract"]["major"].is_u64());

    let by_requester = run_ok(
        tmp.path(),
        &["request", "list", "--requested-by", "coord-z"],
    );
    assert_eq!(ids(&by_requester), vec![theirs.clone()]);

    let by_coordinator = run_ok(
        tmp.path(),
        &["request", "list", "--coordinator-of-record", "coord-a"],
    );
    assert_eq!(ids(&by_coordinator), vec![mine.clone()]);

    // Close one, then filter by state.
    run_ok(
        tmp.path(),
        &["request", "abandon-request", &mine, "--rationale", "done"],
    );
    let open = run_ok(tmp.path(), &["request", "list", "--state", "open"]);
    assert_eq!(ids(&open), vec![theirs.clone()]);
    let closed = run_ok(tmp.path(), &["request", "list", "--state", "closed"]);
    assert_eq!(ids(&closed), vec![mine]);

    let unresolved = run_ok(tmp.path(), &["request", "list", "--unresolved-legs"]);
    assert_eq!(ids(&unresolved), vec![theirs]);
}

#[test]
fn every_failure_exit_status_is_zero_one_two_or_three() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
        ],
    );

    let failures: Vec<Vec<&str>> = vec![
        vec!["request", "get", "req-nope"],
        vec!["request", "get", "../escape"],
        vec!["request", "get", &id, "--cli-contract", "3.1"],
        vec!["request", "get", &id, "--cli-contract", "nonsense"],
        vec![
            "request",
            "wait",
            &id,
            "--leg",
            "no-such-leg",
            "--timeout-secs",
            "1",
        ],
        vec![
            "request",
            "wait",
            &id,
            "--resolved-count",
            "99",
            "--timeout-secs",
            "1",
        ],
        vec!["request", "wait", &id, "--all-legs", "--timeout-secs", "0"],
        vec![
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"second"}"#,
        ],
        vec![
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            "not json",
        ],
    ];

    for args in failures {
        let (code, error_code) = run_err(tmp.path(), &args);
        assert!(
            (1..=3).contains(&code),
            "{args:?} exited {code}, outside the 0-3 class space"
        );
        assert!(
            ![64, 65, 66, 75].contains(&code),
            "{args:?} exited {code}, colliding with a sysexits value used elsewhere"
        );
        assert!(
            !error_code.is_empty(),
            "{args:?} produced an empty error code"
        );
    }
}

// ===== Issue 7: bind, progress, resolve =====

#[test]
fn rebinding_the_same_child_at_a_new_epoch_refreshes_the_fence() {
    // A redelegation bumps the child's epoch in place on the same
    // session id. If rebinding were a flat no-op, the recorded epoch
    // would go stale and the fence would invert: the freshly-dispatched
    // agent presenting the new epoch is rejected forever while the
    // displaced agent still holding the old one is admitted. There is
    // no recovery path from that, so the rebind has to re-record.
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-a", 0);
    let child = "child-a";

    run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            child,
            "--dispatch-epoch",
            "0",
        ],
    );

    // The original agent, at epoch 0, is inside the fence.
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":"before"}"#,
            "--dispatch-epoch",
            "0",
        ],
    );

    // Redelegation: the child's epoch advances in place.
    redelegate(tmp.path(), child, 1);
    let rebind = run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            child,
            "--dispatch-epoch",
            "1",
        ],
    );
    assert_eq!(
        rebind["written"], true,
        "a rebind at a new epoch must append, not no-op"
    );

    // The live agent is now inside the fence...
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":"after"}"#,
            "--dispatch-epoch",
            "1",
        ],
    );

    // ...and the displaced one is locked out.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":"stale"}"#,
            "--dispatch-epoch",
            "0",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "epoch_fence_violation");
}

#[test]
fn bind_binds_and_is_idempotent_for_the_same_pair() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 3);
    fenceable_child(tmp.path(), "coord-a", "child-2", 3);

    let first = run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            "child-1",
            "--dispatch-epoch",
            "3",
            "--issued-by",
            "coord-a",
        ],
    );
    assert_eq!(first["written"], true);
    assert_eq!(first["legs"]["reviewer-a"]["bound_child"], "child-1");
    // The recorded epoch is deliberately withheld from the envelope:
    // emitting it here would hand back exactly what the leg pointer
    // omits, one command later, and let a displaced agent present the
    // current value. Its presence is proved by the fence accepting it
    // below, not by reading it out.
    assert!(
        first["legs"]["reviewer-a"].get("bound_epoch").is_none(),
        "the envelope must not publish the fenced epoch"
    );
    let revision = first["revision"].as_u64().unwrap();

    let second = run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            "child-1",
            "--dispatch-epoch",
            "3",
        ],
    );
    assert_eq!(
        second["written"], false,
        "the state the caller asked for already held, so nothing is appended"
    );
    assert_eq!(second["revision"], revision, "no revision advance");

    // Rebinding elsewhere is refused: two children answering one leg
    // is the ambiguity the leg exists to prevent.
    let (code, error) = run_err(
        tmp.path(),
        &["request", "bind", &id, "reviewer-a", "--child", "child-2"],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "leg_bound_to_different_child");

    // The pointer is a sidecar in the child's session directory, not a
    // header rewrite: binding against a running delegate must not
    // rewrite the log a running delegate is appending to.
    let pointer = sessions_base(tmp.path())
        .join("child-1")
        .join("request-leg.toml");
    let body = std::fs::read_to_string(&pointer).expect("the bind writes a child-side pointer");
    assert!(body.contains(&id), "the pointer names the request: {body}");
    assert!(body.contains("reviewer-a"), "and the leg: {body}");
    assert!(
        !body.contains("epoch"),
        "a readable epoch would let a displaced agent present the current value \
         and defeat the fence: {body}"
    );
}

#[test]
fn bind_refuses_an_unreadable_child_and_an_unfenceable_one() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // No session at all.
    let (code, error) = run_err(
        tmp.path(),
        &["request", "bind", &id, "reviewer-a", "--child", "nobody"],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "child_not_found");

    // A top-level session: the fence predicate needs a parent workflow
    // and needs_agent, so a leg bound here could never be fenced.
    init_parent(tmp.path(), "coord-a");
    let (code, error) = run_err(
        tmp.path(),
        &["request", "bind", &id, "reviewer-a", "--child", "coord-a"],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "child_not_fenceable");

    // Nothing was appended on either rejection.
    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert!(envelope["legs"]["reviewer-a"]["bound_child"].is_null());
}

#[test]
fn a_child_fulfils_at_most_one_leg() {
    let tmp = TempDir::new().unwrap();
    let first = create(tmp.path(), TWO_LEGS);
    let second = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 0);

    run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &first,
            "reviewer-a",
            "--child",
            "child-1",
        ],
    );

    // A different leg of the same request.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "bind",
            &first,
            "reviewer-b",
            "--child",
            "child-1",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "child_bound_to_different_leg");

    // And a leg of a different request, which the per-request lock
    // cannot serialize against at all — the child side is the only
    // place this can be caught.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "bind",
            &second,
            "reviewer-a",
            "--child",
            "child-1",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "child_bound_to_different_leg");

    // Rebinding the same pair is still the idempotent no-op.
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &first,
            "reviewer-a",
            "--child",
            "child-1",
        ],
    );
    assert_eq!(envelope["written"], false);
}

#[test]
fn bind_records_the_childs_own_epoch_and_refuses_a_disagreeing_one() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 7);

    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            "child-1",
            "--dispatch-epoch",
            "6",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "epoch_fence_violation");

    // Omitting the flag is fine: the recorded epoch is read off the
    // child's header rather than taken from the caller, so the value the
    // fence later compares against cannot be a caller's guess.
    let envelope = run_ok(
        tmp.path(),
        &["request", "bind", &id, "reviewer-a", "--child", "child-1"],
    );
    assert!(
        envelope["legs"]["reviewer-a"].get("bound_epoch").is_none(),
        "the envelope must not publish the fenced epoch"
    );

    // The header's epoch (7) is what got recorded — proved by the fence,
    // which is the only thing that reads it.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":"one"}"#,
            "--dispatch-epoch",
            "0",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "epoch_fence_violation");
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":"one"}"#,
            "--dispatch-epoch",
            "7",
        ],
    );
}

#[test]
fn progress_appends_and_is_rejected_once_the_leg_is_terminal() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"halfway"}"#,
            "--issued-by",
            "child-1",
        ],
    );
    assert_eq!(envelope["written"], true);
    let entry = &envelope["legs"]["reviewer-a"]["progress"][0];
    assert_eq!(entry["content"]["note"], "halfway");
    assert_eq!(
        entry["issued_by"], "child-1",
        "--issued-by is recorded as audit attribution"
    );

    // Resolved: further progress is refused.
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"done"}"#,
        ],
    );
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"too late"}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "leg_already_resolved");

    // Abandoned: refused with a distinct code, so the caller can tell
    // "someone beat you to it" from "nobody is waiting any more".
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-b",
            "--rationale",
            "stopped waiting",
        ],
    );
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-b",
            "--with-data",
            r#"{"note":"too late"}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "leg_abandoned");
}

#[test]
fn ten_appends_read_back_in_the_order_they_were_made() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    for n in 0..10 {
        run_ok(
            tmp.path(),
            &[
                "request",
                "progress",
                &id,
                "reviewer-a",
                "--with-data",
                &format!(r#"{{"step":{n}}}"#),
            ],
        );
    }

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    let progress = envelope["legs"]["reviewer-a"]["progress"]
        .as_array()
        .expect("progress entries");
    assert_eq!(progress.len(), 10);
    for (n, entry) in progress.iter().enumerate() {
        assert_eq!(entry["content"]["step"], n, "entry {n} is out of order");
    }
    // The sequence number, not the timestamp, is the ordering key:
    // ten appends inside one millisecond still order correctly.
    let seqs: Vec<u64> = progress
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(seqs, sorted);
}

#[test]
fn resolve_records_a_result_on_an_unbound_leg_and_is_refused_on_a_bound_one() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"skipped","summary":"nothing to review","payload":{"files":0}}"#,
            "--issued-by",
            "coord-a",
        ],
    );
    let leg = &envelope["legs"]["reviewer-a"];
    assert_eq!(leg["disposition"], "resolved");
    assert_eq!(leg["result"]["status"], "skipped");
    assert_eq!(leg["result"]["payload"]["files"], 0);
    assert_eq!(
        leg["result_source"], "explicit",
        "an explicitly recorded result is distinguishable from a promoted one"
    );

    // A bound leg's result is promoted from its child's terminal tick,
    // so an explicit resolve there is refused with its own code.
    bindable(tmp.path(), "child-1", 0);
    run_ok(
        tmp.path(),
        &["request", "bind", &id, "reviewer-b", "--child", "child-1"],
    );
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-b",
            "--with-data",
            r#"{"status":"success","summary":"pre-empted"}"#,
            // The fence runs before the bound-leg rejection, so a
            // caller that skips it learns about the fence first.
            "--dispatch-epoch",
            "0",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "explicit_resolve_on_bound_leg");

    // A second result on the already-resolved leg gets a different
    // code again.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"second"}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "leg_already_resolved");
}

#[test]
fn issued_by_is_accepted_on_every_mutating_verb_and_recorded_on_the_log() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 0);

    run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            "child-1",
            "--issued-by",
            "binder",
        ],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"x"}"#,
            "--dispatch-epoch",
            "0",
            "--issued-by",
            "progresser",
        ],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-b",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
            "--issued-by",
            "resolver",
        ],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-a",
            "--rationale",
            "gave up",
            "--dispatch-epoch",
            "0",
            "--issued-by",
            "abandoner",
        ],
    );
    run_ok(
        tmp.path(),
        &["request", "close", &id, "--issued-by", "closer"],
    );

    let log = std::fs::read_to_string(log_path(tmp.path(), &id)).unwrap();
    for principal in ["binder", "progresser", "resolver", "abandoner", "closer"] {
        assert!(
            log.contains(&format!("\"issued_by\":\"{principal}\"")),
            "'{principal}' is missing from the log:\n{log}"
        );
    }
}

#[test]
fn issued_by_is_on_none_of_the_read_verbs() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    for args in [
        vec!["request", "get", &id, "--issued-by", "someone"],
        vec!["request", "list", "--issued-by", "someone"],
        vec![
            "request",
            "wait",
            &id,
            "--closed",
            "--timeout-secs",
            "1",
            "--issued-by",
            "someone",
        ],
    ] {
        let (code, _, _) = run(tmp.path(), &args);
        assert_eq!(
            code, 2,
            "{args:?}: a read has no issuing principal to record"
        );
    }
}

#[test]
fn exceeding_the_append_bound_is_the_documented_rejection() {
    let tmp = TempDir::new().unwrap();
    // The append and leg bounds are operator-tunable under the
    // existing `[request_store]` table; lowering them keeps the test
    // fast and exercises the config wiring at the same time.
    std::fs::create_dir_all(tmp.path().join(".koto")).unwrap();
    std::fs::write(
        tmp.path().join(".koto").join("config.toml"),
        "[request_store]\nrequest_leg_append_cap = 2\nrequest_leg_cap = 2\n",
    )
    .unwrap();

    let id = create(tmp.path(), TWO_LEGS);
    for n in 0..2 {
        run_ok(
            tmp.path(),
            &[
                "request",
                "progress",
                &id,
                "reviewer-a",
                "--with-data",
                &format!(r#"{{"step":{n}}}"#),
            ],
        );
    }
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"step":2}"#,
        ],
    );
    assert_eq!(
        code, 2,
        "a bound rejection is a caller error, not transient"
    );
    assert_eq!(error, "bound_exceeded");

    // The leg cap rejects at create, before any directory is made.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "create",
            "--with-data",
            THREE_LEGS,
            "--requested-by",
            "coord-a",
            "--coordinator-of-record",
            "coord-a",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "bound_exceeded");
}

#[test]
fn a_bound_leg_is_fenced_against_a_stale_dispatch_epoch() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    bindable(tmp.path(), "child-1", 4);
    run_ok(
        tmp.path(),
        &[
            "request",
            "bind",
            &id,
            "reviewer-a",
            "--child",
            "child-1",
            "--dispatch-epoch",
            "4",
        ],
    );

    // The fence compares against the epoch recorded on the bind event,
    // not against the child's header. Delete the child's session
    // outright — the header it would otherwise read is gone, and the
    // fence still holds, which is the whole point of recording the
    // epoch on the event.
    std::fs::remove_dir_all(sessions_base(tmp.path()).join("child-1")).unwrap();
    let before = workspace_snapshot(tmp.path());
    for stale in ["3", "5"] {
        let (code, error) = run_err(
            tmp.path(),
            &[
                "request",
                "progress",
                &id,
                "reviewer-a",
                "--with-data",
                r#"{"note":"stale"}"#,
                "--dispatch-epoch",
                stale,
            ],
        );
        assert_eq!(code, 2);
        assert_eq!(error, "epoch_fence_violation", "for epoch {stale}");
    }
    // Omitting the flag on a bound leg is an implicit mismatch.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"unfenced"}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "epoch_fence_violation");
    assert_eq!(
        before,
        workspace_snapshot(tmp.path()),
        "the fence must reject before any write"
    );

    // Leg-scoped abandon is fenced alongside progress, because
    // abandonment is what reaches a delegate's directive.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-a",
            "--rationale",
            "displaced agent",
            "--dispatch-epoch",
            "3",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "epoch_fence_violation");

    // The matching epoch passes.
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"fresh"}"#,
            "--dispatch-epoch",
            "4",
        ],
    );

    // An unbound leg has no epoch and is unfenceable by construction.
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-b",
            "--with-data",
            r#"{"note":"no fence here"}"#,
        ],
    );
}

// ===== Issue 8: abandon, abandon-request, close =====

#[test]
fn abandon_stops_one_leg_and_leaves_the_others_open() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-a",
            "--rationale",
            "the reviewer went dark",
            "--issued-by",
            "coord-a",
        ],
    );
    assert_eq!(envelope["request_state"], "open");
    assert_eq!(envelope["leg_counts"]["abandoned"], 1);
    assert_eq!(envelope["leg_counts"]["open"], 1);
    assert_eq!(envelope["legs"]["reviewer-a"]["disposition"], "abandoned");
    assert_eq!(
        envelope["legs"]["reviewer-a"]["abandoned_rationale"], "the reviewer went dark",
        "the rationale is readable from the record, verbatim"
    );
    assert_eq!(envelope["legs"]["reviewer-b"]["disposition"], "open");

    let log = std::fs::read_to_string(log_path(tmp.path(), &id)).unwrap();
    assert!(log.contains("request.leg_abandoned"));
    assert!(log.contains("the reviewer went dark"));
    assert!(log.contains("\"issued_by\":\"coord-a\""));
}

#[test]
fn abandon_request_abandons_every_open_leg_and_closes() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), THREE_LEGS);

    // One leg already answered; it must keep its result rather than
    // being overwritten by the sweep.
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "leg-a",
            "--with-data",
            r#"{"status":"success","summary":"answered"}"#,
        ],
    );

    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "abandon-request",
            &id,
            "--rationale",
            "the coordinator moved on",
            "--issued-by",
            "coord-a",
        ],
    );
    assert_eq!(envelope["request_state"], "closed");
    assert_eq!(envelope["close_disposition"], "request_abandoned");
    assert_eq!(envelope["leg_counts"]["open"], 0);
    assert_eq!(envelope["leg_counts"]["abandoned"], 2);
    assert_eq!(envelope["leg_counts"]["resolved"], 1);
    assert_eq!(envelope["legs"]["leg-a"]["disposition"], "resolved");
    assert_eq!(
        envelope["legs"]["leg-b"]["abandoned_rationale"],
        "the coordinator moved on"
    );
}

#[test]
fn abandon_and_abandon_request_are_separate_subcommands() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // The escalation this separation exists to prevent: an unset
    // shell variable in the leg position must fail argument parsing,
    // not abandon the whole request.
    let (code, _, _) = run(
        tmp.path(),
        &["request", "abandon", &id, "", "--rationale", "oops"],
    );
    assert_eq!(code, 2, "an empty leg name must be refused");

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(
        envelope["request_state"], "open",
        "the request must be untouched by the failed leg abandonment"
    );
    assert_eq!(envelope["leg_counts"]["abandoned"], 0);
}

#[test]
fn rationale_is_required_on_both_abandon_forms() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let (code, _, _) = run(tmp.path(), &["request", "abandon", &id, "reviewer-a"]);
    assert_eq!(code, 2, "leg abandonment must state why");

    let (code, _, _) = run(tmp.path(), &["request", "abandon-request", &id]);
    assert_eq!(code, 2, "request abandonment must state why");

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(envelope["leg_counts"]["abandoned"], 0);
}

#[test]
fn close_records_each_of_the_three_dispositions() {
    let tmp = TempDir::new().unwrap();

    // All resolved.
    let id = create(tmp.path(), TWO_LEGS);
    for leg in ["reviewer-a", "reviewer-b"] {
        run_ok(
            tmp.path(),
            &[
                "request",
                "resolve",
                &id,
                leg,
                "--with-data",
                r#"{"status":"success","summary":"x"}"#,
            ],
        );
    }
    let envelope = run_ok(tmp.path(), &["request", "close", &id]);
    assert_eq!(envelope["close_disposition"], "all_resolved");

    // Closed short-handed, with a leg abandoned.
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
        ],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-b",
            "--rationale",
            "proceeding short-handed",
        ],
    );
    let envelope = run_ok(tmp.path(), &["request", "close", &id]);
    assert_eq!(envelope["close_disposition"], "closed_with_abandoned_legs");

    // The whole request given up on — only the caller can assert this
    // one, which is why it is never derived.
    let id = create(tmp.path(), TWO_LEGS);
    let envelope = run_ok(
        tmp.path(),
        &["request", "abandon-request", &id, "--rationale", "dropped"],
    );
    assert_eq!(envelope["close_disposition"], "request_abandoned");
}

#[test]
fn closing_an_already_closed_request_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(tmp.path(), &["request", "close", &id]);

    let (code, error) = run_err(tmp.path(), &["request", "close", &id]);
    assert_eq!(code, 2);
    assert_eq!(
        error, "request_closed",
        "the second caller believes it is recording a disposition; \
         silently keeping the first would tell it something false"
    );

    let (code, error) = run_err(
        tmp.path(),
        &["request", "abandon-request", &id, "--rationale", "again"],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "request_closed");

    // Leg mutations on a closed request are refused too.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"note":"x"}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "request_closed");
}

#[test]
fn koto_cancel_is_unchanged_and_reaches_no_request_operation() {
    let tmp = TempDir::new().unwrap();
    let template = tmp.path().join("workflow.md");
    std::fs::write(
        &template,
        r#"---
name: canceller
version: "1.0"
initial_state: work
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

Do work.

## done

Done.
"#,
    )
    .unwrap();

    let id = create(tmp.path(), TWO_LEGS);
    let before = workspace_snapshot(tmp.path());

    let (code, _, stderr) = run(
        tmp.path(),
        &[
            "init",
            "cancel-me",
            "--template",
            template.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, _, stderr) = run(tmp.path(), &["cancel", "cancel-me"]);
    assert_eq!(code, 0, "cancel still works unchanged: {stderr}");

    // The request record is byte-for-byte what it was: no request
    // operation is reachable from `cancel`.
    let request_files: Vec<_> = before
        .iter()
        .filter(|(p, _, _)| p.starts_with(requests_root(tmp.path())))
        .cloned()
        .collect();
    let after: Vec<_> = workspace_snapshot(tmp.path())
        .into_iter()
        .filter(|(p, _, _)| p.starts_with(requests_root(tmp.path())))
        .collect();
    assert_eq!(request_files, after);

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(envelope["request_state"], "open");
    assert_eq!(envelope["revision"], 1);
}

// ===== Issue 9: wait =====

#[test]
fn wait_requires_exactly_one_predicate_and_a_timeout() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // None.
    let (code, _, _) = run(tmp.path(), &["request", "wait", &id, "--timeout-secs", "1"]);
    assert_eq!(code, 2);

    // Two.
    let (code, _, _) = run(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--all-legs",
            "--closed",
            "--timeout-secs",
            "1",
        ],
    );
    assert_eq!(code, 2);

    // No timeout: a wait with no deadline is a hang.
    let (code, _, _) = run(tmp.path(), &["request", "wait", &id, "--all-legs"]);
    assert_eq!(code, 2);
}

#[test]
fn a_satisfied_predicate_exits_zero_and_an_unsatisfied_one_exits_transient() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
        ],
    );

    // Satisfied immediately: the leg predicate on the resolved leg.
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--leg",
            "reviewer-a",
            "--timeout-secs",
            "2",
        ],
    );
    assert_eq!(envelope["legs"]["reviewer-a"]["disposition"], "resolved");
    assert!(
        envelope.get("written").is_none(),
        "a wait is a read; it must not claim to have written"
    );

    // Unsatisfied at the deadline: transient, so a shell loop is
    // told it may retry.
    let (code, error) = run_err(
        tmp.path(),
        &["request", "wait", &id, "--all-legs", "--timeout-secs", "1"],
    );
    assert_eq!(code, 1);
    assert_eq!(error, "wait_timeout");
}

#[test]
fn the_closed_predicate_is_satisfied_by_a_close() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    let (code, error) = run_err(
        tmp.path(),
        &["request", "wait", &id, "--closed", "--timeout-secs", "1"],
    );
    assert_eq!(code, 1);
    assert_eq!(error, "wait_timeout");

    run_ok(
        tmp.path(),
        &["request", "abandon-request", &id, "--rationale", "dropped"],
    );
    let envelope = run_ok(
        tmp.path(),
        &["request", "wait", &id, "--closed", "--timeout-secs", "2"],
    );
    assert_eq!(envelope["request_state"], "closed");
}

#[test]
fn a_count_predicate_is_satisfied_by_a_partial_fan_out() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), THREE_LEGS);

    for leg in ["leg-a", "leg-b"] {
        run_ok(
            tmp.path(),
            &[
                "request",
                "resolve",
                &id,
                leg,
                "--with-data",
                r#"{"status":"success","summary":"x"}"#,
            ],
        );
    }

    // Two of three: satisfied without waiting for the third.
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--resolved-count",
            "2",
            "--timeout-secs",
            "2",
        ],
    );
    assert_eq!(envelope["leg_counts"]["resolved"], 2);
    assert_eq!(envelope["leg_counts"]["open"], 1);

    // Three of three is not yet reached, and --all-legs agrees.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--resolved-count",
            "3",
            "--timeout-secs",
            "1",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(error, "wait_timeout");
}

#[test]
fn a_structurally_impossible_predicate_is_rejected_before_polling() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // Five resolved legs on a two-leg request can never hold. The
    // rejection must be immediate and in the caller-error class, not
    // a transient timeout a shell loop would retry forever.
    let started = Instant::now();
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--resolved-count",
            "5",
            "--timeout-secs",
            "60",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "predicate_impossible");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the rejection must come before polling, not at the deadline"
    );

    // A leg the request does not have is caught the same way.
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--leg",
            "no-such-leg",
            "--timeout-secs",
            "60",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(error, "leg_not_found");
}

#[test]
fn a_predicate_that_becomes_impossible_mid_wait_is_distinct_from_a_timeout() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // Start a wait that would otherwise run for a minute, then
    // abandon the leg it is waiting on.
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"))
        .args([
            "request",
            "wait",
            &id,
            "--leg",
            "reviewer-a",
            "--timeout-secs",
            "60",
            "--interval-secs",
            "1",
        ])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    std::thread::sleep(Duration::from_millis(1200));
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &id,
            "reviewer-a",
            "--rationale",
            "the reviewer went dark",
        ],
    );

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(
        output.status.code(),
        Some(2),
        "'never' is a caller error; 'not yet' is the transient class"
    );
    assert_eq!(json["error"]["code"], "predicate_became_impossible");
}

#[test]
fn an_interrupted_wait_exits_transient_with_its_own_code() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // A minute-long deadline and a minute-long interval: only the
    // hundred-millisecond sleep slicing lets the signal be noticed
    // promptly.
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"))
        .args([
            "request",
            "wait",
            &id,
            "--all-legs",
            "--timeout-secs",
            "60",
            "--interval-secs",
            "60",
        ])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    std::thread::sleep(Duration::from_millis(600));
    let started = Instant::now();
    // SAFETY: `child` is alive and owned by this test.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let output = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "the signal must be noticed within a sleep slice, not after the \
         whole poll interval; took {elapsed:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(
        output.status.code(),
        Some(1),
        "an interrupted wait is transient, not a success"
    );
    assert_eq!(json["error"]["code"], "wait_interrupted");
}

#[test]
fn the_deadline_is_absolute_and_the_interval_cannot_spin() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);

    // An interval of zero is clamped to the floor rather than
    // honored; without the clamp this would spin for two seconds.
    let started = Instant::now();
    let (code, error) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--all-legs",
            "--timeout-secs",
            "2",
            "--interval-secs",
            "0",
        ],
    );
    let elapsed = started.elapsed();
    assert_eq!(code, 1);
    assert_eq!(error, "wait_timeout");
    assert!(
        elapsed >= Duration::from_secs(2),
        "the absolute deadline must be honored; returned after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the deadline is computed once, so a slow poll cannot extend it; \
         took {elapsed:?}"
    );

    // An interval longer than the deadline does not extend it either.
    let started = Instant::now();
    let (code, _) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--all-legs",
            "--timeout-secs",
            "1",
            "--interval-secs",
            "30",
        ],
    );
    assert_eq!(code, 1);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the sleep is capped at the deadline, not at the interval"
    );
}

#[test]
fn wait_reads_through_the_same_path_as_get_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let id = create(tmp.path(), TWO_LEGS);
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "reviewer-a",
            "--with-data",
            r#"{"status":"success","summary":"x"}"#,
        ],
    );

    let before = workspace_snapshot(tmp.path());

    // A wait that times out polls several times; a wait that succeeds
    // reads once. Neither may write.
    let (code, _) = run_err(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--all-legs",
            "--timeout-secs",
            "2",
            "--interval-secs",
            "1",
        ],
    );
    assert_eq!(code, 1);
    let waited = run_ok(
        tmp.path(),
        &[
            "request",
            "wait",
            &id,
            "--leg",
            "reviewer-a",
            "--timeout-secs",
            "2",
        ],
    );

    assert_eq!(
        before,
        workspace_snapshot(tmp.path()),
        "wait must advance no cursor and touch no file"
    );

    // Same projection `get` returns, minus the write-only `written`
    // field neither read carries.
    let (_, got, _) = run(tmp.path(), &["request", "get", &id]);
    let got: serde_json::Value = serde_json::from_str(&got).unwrap();
    assert_eq!(waited, got, "wait reads through the same path as get");
}
