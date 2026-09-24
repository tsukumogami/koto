//! Integration tests for `koto request attach`: a root session binding
//! itself to a request leg, the checks that admit it, and the fenced
//! verbs refused on a self-attached leg.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and every refusal asserts
//! that the request log's bytes and the session's leg pointer are
//! unchanged.

#![cfg(unix)]

use std::path::{Path, PathBuf};

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

fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(
        code, 0,
        "expected success from {args:?}\n{stdout}\n{stderr}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

/// Run and require failure, returning `(exit code, error object)`.
fn run_err(dir: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let (code, stdout, stderr) = run(dir, args);
    assert_ne!(code, 0, "expected failure from {args:?}\n{stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}\n{stderr}"));
    let error = json["error"].clone();
    assert!(error.is_object(), "nested error envelope expected: {json}");
    assert!(error["code"].is_string(), "{json}");
    (code, error)
}

const SCOPE_TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
  INTENT_FLAG:
    pattern: ^(continue|stop)?$
  MERGE:
    values: ["true", "false"]
    default: "false"
    rebind: true
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Scope {{TOPIC}}.

## done

Done.
"#;

/// A template for a live session that does not advance on its own tick.
const GATED_TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
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

fn write_template(dir: &Path, file: &str, body: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, body).unwrap();
    path
}

/// `koto init <name> --template <dir>/<file>` with the given vars.
fn init(dir: &Path, name: &str, file: &str, vars: &[&str]) {
    let template = dir.join(file);
    let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
    for v in vars {
        args.push("--var");
        args.push(v);
    }
    let (code, stdout, stderr) = run(dir, &args);
    assert_eq!(code, 0, "init {name} failed\n{stdout}\n{stderr}");
}

fn create(dir: &Path, with_data: &str) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            with_data,
            "--requested-by",
            "deliver-a",
            "--coordinator-of-record",
            "deliver-a",
        ],
    );
    envelope["request_id"].as_str().unwrap().to_string()
}

/// A one-leg request whose `scope` leg names `template` and `inputs`.
fn scope_request(dir: &Path, template: &str, inputs: &str) -> String {
    create(
        dir,
        &format!(
            r#"{{"legs":[{{"name":"scope","role":"scope","template":{template},"inputs":{inputs}}}]}}"#
        ),
    )
}

fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(".koto")
        .join("requests")
        .join(id)
        .join("request.jsonl")
}

fn log_bytes(dir: &Path, id: &str) -> Vec<u8> {
    std::fs::read(log_path(dir, id)).unwrap()
}

fn session_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions").join(name)
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join(format!("koto-{name}.state.jsonl"))
}

fn pointer_path(dir: &Path, name: &str) -> PathBuf {
    session_dir(dir, name).join("request-leg.toml")
}

/// The pointer's bytes, or `None` when the session carries none.
fn pointer_bytes(dir: &Path, name: &str) -> Option<Vec<u8>> {
    std::fs::read(pointer_path(dir, name)).ok()
}

/// Assert an attach is refused with `code`, exit 2, and writes nothing.
fn assert_attach_refused(
    dir: &Path,
    id: &str,
    leg: &str,
    session: &str,
    code: &str,
) -> serde_json::Value {
    let log_before = log_bytes(dir, id);
    let pointer_before = pointer_bytes(dir, session);
    let (exit, error) = run_err(dir, &["request", "attach", id, leg, "--session", session]);
    assert_eq!(exit, 2, "{error}");
    assert_eq!(error["code"], code, "{error}");
    assert_eq!(
        log_bytes(dir, id),
        log_before,
        "a refused attach must leave the request log byte-for-byte unchanged"
    );
    assert_eq!(
        pointer_bytes(dir, session),
        pointer_before,
        "a refused attach must leave the session's pointer unchanged"
    );
    error
}

fn scope_inputs() -> &'static str {
    r#"{"TOPIC":"t1","INTENT_FLAG":"continue","MERGE":"true"}"#
}

/// A request with a `scope` leg naming `scope.md`, and a live
/// `scope-t1` session built from it with matching variables.
fn attachable(dir: &Path) -> String {
    write_template(dir, "scope.md", SCOPE_TEMPLATE);
    init(
        dir,
        "scope-t1",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    scope_request(dir, r#""scope.md""#, scope_inputs())
}

// ===== Admission =====

#[test]
fn a_root_session_attaches_and_the_leg_records_its_identity() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());

    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &id,
            "scope",
            "--session",
            "scope-t1",
            "--issued-by",
            "deliver-a",
        ],
    );
    assert_eq!(envelope["written"], true);
    assert_eq!(envelope["cli_contract"]["minor"], 1);
    let leg = &envelope["legs"]["scope"];
    assert_eq!(leg["bound_child"], "scope-t1");
    assert_eq!(leg["attach"], "self");
    assert_eq!(leg["bound_template"]["source"], "scope.md");
    assert_eq!(leg["bound_template"]["name"], "scope");

    let header: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(state_path(tmp.path(), "scope-t1"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(leg["bound_template"]["hash"], header["template_hash"]);
    assert_eq!(header["template_source_file"], "scope.md");

    // `get` shows the same fields.
    let got = run_ok(tmp.path(), &["request", "get", &id]);
    assert_eq!(got["legs"]["scope"]["attach"], "self");
    assert_eq!(got["legs"]["scope"]["bound_template"]["source"], "scope.md");

    // The bind event carries attach: self and the identity, no epoch.
    let log = std::fs::read_to_string(log_path(tmp.path(), &id)).unwrap();
    let bound: Vec<serde_json::Value> = log
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["type"] == "request.leg_bound")
        .collect();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0]["payload"]["attach"], "self");
    assert_eq!(bound[0]["payload"]["template"]["source"], "scope.md");
    assert!(bound[0]["payload"].get("dispatch_epoch").is_none());

    // The session's pointer names the leg.
    let pointer = std::fs::read_to_string(pointer_path(tmp.path(), "scope-t1")).unwrap();
    assert!(
        pointer.contains(&id) && pointer.contains("scope"),
        "{pointer}"
    );
}

#[test]
fn attaching_the_same_session_again_is_a_no_op() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    let before = log_bytes(tmp.path(), &id);

    let again = run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    assert_eq!(again["written"], false);
    assert_eq!(
        log_bytes(tmp.path(), &id),
        before,
        "no second request.leg_bound event"
    );
}

#[test]
fn a_leg_naming_a_list_of_templates_admits_any_of_them() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "execute-coordinated.md", GATED_TEMPLATE);
    init(
        tmp.path(),
        "execute-t1",
        "execute-coordinated.md",
        &["TOPIC=t1"],
    );
    let id = scope_request(
        tmp.path(),
        r#"["execute.md","execute-coordinated.md"]"#,
        r#"{"TOPIC":"t1"}"#,
    );

    let envelope = run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "execute-t1"],
    );
    assert_eq!(
        envelope["legs"]["scope"]["declaration"]["template"],
        serde_json::json!(["execute.md", "execute-coordinated.md"])
    );
    assert_eq!(
        envelope["legs"]["scope"]["bound_template"]["source"],
        "execute-coordinated.md"
    );
}

#[test]
fn create_rejects_an_empty_or_overlong_template_list() {
    let tmp = TempDir::new().unwrap();
    let too_many: Vec<String> = (0..9).map(|n| format!("t{n}.md")).collect();
    for template in [
        "[]".to_string(),
        serde_json::to_string(&too_many).unwrap(),
        r#"["a.md",""]"#.to_string(),
    ] {
        let payload = format!(
            r#"{{"legs":[{{"name":"scope","role":"scope","template":{template},"inputs":{{}}}}]}}"#
        );
        let (exit, error) = run_err(
            tmp.path(),
            &[
                "request",
                "create",
                "--with-data",
                &payload,
                "--requested-by",
                "deliver-a",
                "--coordinator-of-record",
                "deliver-a",
            ],
        );
        assert_eq!(exit, 2, "{template}: {error}");
        assert_eq!(error["code"], "invalid_submission", "{template}: {error}");
    }
    let requests = tmp.path().join(".koto").join("requests");
    let count = std::fs::read_dir(&requests).map(|d| d.count()).unwrap_or(0);
    assert_eq!(count, 0, "a rejected creation leaves no record");

    // The single-string form is still accepted.
    scope_request(tmp.path(), r#""scope.md""#, "{}");
}

// ===== Template identity =====

#[test]
fn a_session_from_a_template_the_leg_does_not_name_is_refused() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());

    // A same-shaped throwaway: identical content, identical `name:`,
    // different file.
    write_template(tmp.path(), "throwaway.md", SCOPE_TEMPLATE);
    init(
        tmp.path(),
        "scope-evil",
        "throwaway.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    let error = assert_attach_refused(tmp.path(), &id, "scope", "scope-evil", "template_mismatch");
    assert!(
        error["message"].as_str().unwrap().contains("throwaway.md"),
        "{error}"
    );
    assert!(pointer_bytes(tmp.path(), "scope-evil").is_none());
}

#[test]
fn a_from_stdin_session_has_no_template_file_and_is_refused() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());

    let output = koto_cmd(tmp.path())
        .args([
            "init",
            "scope-inline",
            "--from-stdin",
            "--var",
            "TOPIC=t1",
            "--var",
            "INTENT_FLAG=continue",
        ])
        .write_stdin(SCOPE_TEMPLATE)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );

    let error = assert_attach_refused(
        tmp.path(),
        &id,
        "scope",
        "scope-inline",
        "template_mismatch",
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("no template file"),
        "{error}"
    );
}

// ===== Inputs =====

#[test]
fn a_non_rebind_variable_that_disagrees_with_an_input_is_refused() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md", SCOPE_TEMPLATE);
    init(
        tmp.path(),
        "scope-t1",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=stop"],
    );
    let id = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());

    let error = assert_attach_refused(tmp.path(), &id, "scope", "scope-t1", "input_mismatch");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("INTENT_FLAG") && message.contains("stop") && message.contains("continue"),
        "the refusal names the key, the recorded value and the leg's value: {message}"
    );
    assert_eq!(error["details"][0]["field"], "INTENT_FLAG");
}

#[test]
fn an_input_naming_an_undeclared_variable_is_refused() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md", SCOPE_TEMPLATE);
    init(
        tmp.path(),
        "scope-t1",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    let id = scope_request(
        tmp.path(),
        r#""scope.md""#,
        r#"{"TOPIC":"t1","PLAN_SLUG":"x"}"#,
    );

    let error = assert_attach_refused(tmp.path(), &id, "scope", "scope-t1", "input_mismatch");
    assert!(
        error["message"].as_str().unwrap().contains("PLAN_SLUG"),
        "{error}"
    );
}

#[test]
fn a_rebind_variable_named_in_inputs_is_not_compared() {
    let tmp = TempDir::new().unwrap();
    // The session records MERGE=false (its default); the leg says true.
    let id = attachable(tmp.path());
    let envelope = run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    assert_eq!(envelope["written"], true);
}

// ===== Terminal sessions =====

#[test]
fn a_terminal_session_is_refused() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(tmp.path(), &["next", "scope-t1", "--no-cleanup"]);
    assert!(state_path(tmp.path(), "scope-t1").exists());

    let error = assert_attach_refused(tmp.path(), &id, "scope", "scope-t1", "session_terminal");
    assert!(
        error["message"].as_str().unwrap().contains("done"),
        "{error}"
    );
    assert!(pointer_bytes(tmp.path(), "scope-t1").is_none());
}

#[test]
fn a_cancelled_session_is_refused() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    let (code, stdout, stderr) = run(tmp.path(), &["cancel", "scope-t1"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");

    assert_attach_refused(tmp.path(), &id, "scope", "scope-t1", "session_terminal");
}

// ===== Pointers and takeover =====

#[test]
fn a_leg_bound_to_another_session_is_refused() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    init(
        tmp.path(),
        "scope-t1-other",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );

    assert_attach_refused(
        tmp.path(),
        &id,
        "scope",
        "scope-t1-other",
        "leg_bound_to_different_child",
    );

    // Whatever the other session's state: a finished bound session still
    // holds its leg.
    run_ok(tmp.path(), &["next", "scope-t1", "--no-cleanup"]);
    let second = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    // (The first leg is now resolved; take a fresh request to check the
    // bound case against a terminal holder.)
    init(
        tmp.path(),
        "scope-holder",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &second,
            "scope",
            "--session",
            "scope-holder",
        ],
    );
    run(tmp.path(), &["cancel", "scope-holder"]);
    assert_attach_refused(
        tmp.path(),
        &second,
        "scope",
        "scope-t1-other",
        "leg_bound_to_different_child",
    );
}

#[test]
fn a_session_answering_a_live_leg_is_not_re_pointed_until_that_request_closes() {
    let tmp = TempDir::new().unwrap();
    let first = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &first,
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    let second = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());

    assert_attach_refused(
        tmp.path(),
        &second,
        "scope",
        "scope-t1",
        "child_bound_to_different_leg",
    );

    // A newer run supersedes the older one by abandoning its request.
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon-request",
            &first,
            "--rationale",
            "superseded",
        ],
    );
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &second,
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    assert_eq!(envelope["written"], true);
    let pointer = std::fs::read_to_string(pointer_path(tmp.path(), "scope-t1")).unwrap();
    assert!(pointer.contains(&second), "re-pointed: {pointer}");
}

#[test]
fn a_session_whose_old_request_was_closed_is_re_pointed() {
    let tmp = TempDir::new().unwrap();
    // The first request has a second leg, so closing it leaves the
    // scope leg open but the request closed.
    write_template(tmp.path(), "scope.md", SCOPE_TEMPLATE);
    init(
        tmp.path(),
        "scope-t1",
        "scope.md",
        &["TOPIC=t1", "INTENT_FLAG=continue"],
    );
    let first = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &first,
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    run_ok(tmp.path(), &["request", "close", &first]);

    let second = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &second,
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    assert_eq!(envelope["written"], true);
}

#[test]
fn attach_on_a_closed_request_or_a_finished_leg_uses_the_existing_codes() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());

    // Resolved leg.
    run_ok(
        tmp.path(),
        &[
            "request",
            "resolve",
            &id,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"done elsewhere"}"#,
        ],
    );
    assert_attach_refused(tmp.path(), &id, "scope", "scope-t1", "leg_already_resolved");

    // Abandoned leg.
    let abandoned = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon",
            &abandoned,
            "scope",
            "--rationale",
            "not needed",
        ],
    );
    assert_attach_refused(tmp.path(), &abandoned, "scope", "scope-t1", "leg_abandoned");

    // Closed request.
    let closed = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    run_ok(tmp.path(), &["request", "close", &closed]);
    assert_attach_refused(tmp.path(), &closed, "scope", "scope-t1", "request_closed");
}

#[test]
fn identifiers_are_validated_like_bind() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    let (exit, error) = run_err(
        tmp.path(),
        &[
            "request",
            "attach",
            "Req-UPPER",
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    assert_eq!(exit, 2);
    assert_eq!(error["code"], "invalid_identifier");
    let (exit, error) = run_err(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "../escape"],
    );
    assert_eq!(exit, 2);
    assert_eq!(error["code"], "invalid_identifier");
    let (exit, error) = run_err(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "nobody"],
    );
    assert_eq!(exit, 2);
    assert_eq!(error["code"], "child_not_found");
}

// ===== Dispatched and non-dispatched children =====

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

#[test]
fn a_non_dispatched_child_is_refused_and_told_about_the_root_carve_out() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "parent.md", GATED_TEMPLATE);
    init(tmp.path(), "coord-a", "parent.md", &["TOPIC=t"]);
    write_template(tmp.path(), "child.md", CHILD_TEMPLATE);
    let child_template = tmp.path().join("child.md");
    let (code, stdout, stderr) = run(
        tmp.path(),
        &[
            "init",
            "child-1",
            "--template",
            child_template.to_str().unwrap(),
            "--parent",
            "coord-a",
        ],
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let id = scope_request(tmp.path(), r#""child.md""#, "{}");

    let error = assert_attach_refused(tmp.path(), &id, "scope", "child-1", "child_not_fenceable");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("koto request attach"),
        "the message names the root carve-out: {error}"
    );
}

#[test]
fn a_dispatched_child_attaches_exactly_as_bind_does() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "parent.md", GATED_TEMPLATE);
    init(tmp.path(), "coord-a", "parent.md", &["TOPIC=t"]);
    write_template(tmp.path(), "child.md", CHILD_TEMPLATE);
    let child_template = tmp.path().join("child.md");
    let (code, stdout, stderr) = run(
        tmp.path(),
        &[
            "init",
            "child-1",
            "--template",
            child_template.to_str().unwrap(),
            "--parent",
            "coord-a",
        ],
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    koto::engine::claim::rewrite_header_atomically(&state_path(tmp.path(), "child-1"), |mut h| {
        h.needs_agent = Some(true);
        h.role = Some("scrutineer".into());
        h.coordinator_of_record = Some("coord-a".into());
        h.dispatch_epoch = 4;
        h
    })
    .unwrap();
    // The leg names a template the child was not built from: a dispatched
    // child is admitted by its epoch, not its template, exactly as bind.
    let id = scope_request(tmp.path(), r#""review""#, r#"{"brief":"x"}"#);

    let envelope = run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "child-1"],
    );
    let leg = &envelope["legs"]["scope"];
    assert_eq!(leg["bound_child"], "child-1");
    assert!(leg.get("attach").is_none(), "{leg}");

    // Fenced at the captured epoch.
    let (exit, error) = run_err(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "scope",
            "--with-data",
            r#"{"note":"x"}"#,
            "--dispatch-epoch",
            "3",
        ],
    );
    assert_eq!(exit, 2);
    assert_eq!(error["code"], "epoch_fence_violation");
    run_ok(
        tmp.path(),
        &[
            "request",
            "progress",
            &id,
            "scope",
            "--with-data",
            r#"{"note":"x"}"#,
            "--dispatch-epoch",
            "4",
        ],
    );
}

// ===== Fenced verbs on a self-attached leg =====

#[test]
fn the_fenced_verbs_are_refused_on_a_self_attached_leg() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    let before = log_bytes(tmp.path(), &id);

    let verbs: Vec<Vec<&str>> = vec![
        vec![
            "request",
            "progress",
            &id,
            "scope",
            "--with-data",
            r#"{"note":"x"}"#,
        ],
        vec![
            "request",
            "resolve",
            &id,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"forged"}"#,
        ],
        vec!["request", "abandon", &id, "scope", "--rationale", "stop"],
    ];
    for verb in &verbs {
        for epoch in [None, Some("0"), Some("1")] {
            let mut args = verb.clone();
            if let Some(epoch) = epoch {
                args.push("--dispatch-epoch");
                args.push(epoch);
            }
            let (exit, error) = run_err(tmp.path(), &args);
            assert_eq!(exit, 2, "{args:?}: {error}");
            assert_eq!(error["code"], "self_attached_leg", "{args:?}: {error}");
        }
    }
    assert_eq!(
        log_bytes(tmp.path(), &id),
        before,
        "no fenced verb may write to a self-attached leg"
    );
}

#[test]
fn request_scoped_abandon_and_close_stay_available() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    let envelope = run_ok(
        tmp.path(),
        &[
            "request",
            "abandon-request",
            &id,
            "--rationale",
            "superseded",
        ],
    );
    assert_eq!(envelope["request_state"], "closed");
    assert_eq!(envelope["legs"]["scope"]["disposition"], "abandoned");

    let second = scope_request(tmp.path(), r#""scope.md""#, scope_inputs());
    run_ok(
        tmp.path(),
        &[
            "request",
            "attach",
            &second,
            "scope",
            "--session",
            "scope-t1",
        ],
    );
    let closed = run_ok(tmp.path(), &["request", "close", &second]);
    assert_eq!(closed["request_state"], "closed");
}

// ===== Promotion and stale runs =====

#[test]
fn a_self_attached_root_promotes_its_result_and_keeps_its_session() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );

    let terminal = run_ok(tmp.path(), &["next", "scope-t1", "--no-cleanup"]);
    assert_eq!(terminal["action"], "done", "{terminal}");
    assert!(
        state_path(tmp.path(), "scope-t1").exists(),
        "--no-cleanup keeps the session on disk"
    );

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    let leg = &envelope["legs"]["scope"];
    assert_eq!(leg["disposition"], "resolved");
    assert_eq!(leg["result_source"], "promoted");
    assert!(leg["result"]["status"].is_string(), "{leg}");

    // A second terminal tick writes nothing more.
    let before = log_bytes(tmp.path(), &id);
    run_ok(tmp.path(), &["next", "scope-t1", "--no-cleanup"]);
    assert_eq!(log_bytes(tmp.path(), &id), before);
}

#[test]
fn a_root_whose_request_was_abandoned_does_not_resolve_its_leg() {
    let tmp = TempDir::new().unwrap();
    let id = attachable(tmp.path());
    run_ok(
        tmp.path(),
        &["request", "attach", &id, "scope", "--session", "scope-t1"],
    );
    run_ok(
        tmp.path(),
        &[
            "request",
            "abandon-request",
            &id,
            "--rationale",
            "superseded",
        ],
    );
    let before = log_bytes(tmp.path(), &id);

    let (code, stdout, stderr) = run(tmp.path(), &["next", "scope-t1", "--no-cleanup"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");

    let envelope = run_ok(tmp.path(), &["request", "get", &id]);
    let leg = &envelope["legs"]["scope"];
    assert_eq!(leg["disposition"], "abandoned");
    assert!(leg["result"].is_null(), "{leg}");
    assert_eq!(log_bytes(tmp.path(), &id), before);

    // The session keeps its own record of where it ended.
    let log = std::fs::read_to_string(state_path(tmp.path(), "scope-t1")).unwrap();
    assert!(log.contains(r#""to":"done""#), "{log}");
}
