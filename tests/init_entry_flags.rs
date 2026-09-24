//! Integration tests for `koto init`'s entry flags: `--vars-file`,
//! `--replace-terminal`, `--attach-live` and `--koto-leg <req>:<leg>`.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory. Every refusal asserts that the
//! session's state file and the request log are byte-for-byte unchanged,
//! except where the refusal is meant to be recorded on the leg.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;

// ===== Harness =====

fn koto_cmd(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(cwd);
    cmd.env("HOME", home);
    cmd.env("KOTO_SESSIONS_BASE", home.join("sessions"));
    cmd
}

fn run_in(home: &Path, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = koto_cmd(home, cwd).args(args).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    run_in(dir, dir, args)
}

fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("not JSON: {e}\n{stdout}"))
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(
        code, 0,
        "expected success from {args:?}\n{stdout}\n{stderr}"
    );
    json(&stdout)
}

/// Run and require failure, returning `(exit code, error body)`.
fn run_err_in(home: &Path, cwd: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let (code, stdout, stderr) = run_in(home, cwd, args);
    assert_ne!(
        code, 0,
        "expected failure from {args:?}\n{stdout}\n{stderr}"
    );
    (code, json(&stdout))
}

fn run_err(dir: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    run_err_in(dir, dir, args)
}

const SCOPE_TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
  INTENT_FLAG:
    values: ["continue", "stop"]
    default: "continue"
  MERGE:
    values: ["true", "false"]
    default: "false"
    rebind: true
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
    result:
      outcome: scoped
      topic: "{{TOPIC}}"
---

## work

Scope {{TOPIC}}.

## done

Done.
"#;

fn write_template(dir: &Path, file: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, SCOPE_TEMPLATE).unwrap();
    path
}

/// Write a vars file holding `pairs` as a JSON list and return its path.
fn vars_file(dir: &Path, file: &str, pairs: &[(&str, &str)]) -> PathBuf {
    let list: Vec<[&str; 2]> = pairs.iter().map(|(k, v)| [*k, *v]).collect();
    let path = dir.join(file);
    std::fs::write(&path, serde_json::to_string(&list).unwrap()).unwrap();
    path
}

/// `koto init <name> --template <dir>/scope.md --vars-file <file> <extra...>`.
fn init_args(dir: &Path, name: &str, template: &str, vars: &Path, extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "init".into(),
        name.into(),
        "--template".into(),
        dir.join(template).to_string_lossy().into_owned(),
        "--vars-file".into(),
        vars.to_string_lossy().into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn as_strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// Create `name` from `scope.md` with the given variables.
fn create_session(dir: &Path, name: &str, pairs: &[(&str, &str)]) -> serde_json::Value {
    write_template(dir, "scope.md");
    let vars = vars_file(dir, &format!("{name}-create.json"), pairs);
    run_ok(dir, &as_strs(&init_args(dir, name, "scope.md", &vars, &[])))
}

/// Tick `name` to its terminal, keeping it on disk.
fn finish(dir: &Path, name: &str) {
    let out = run_ok(
        dir,
        &[
            "next",
            name,
            "--with-data",
            r#"{"status":"ok"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(out["action"], "done", "{out}");
}

fn state_path(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions")
        .join(name)
        .join(format!("koto-{name}.state.jsonl"))
}

fn state_bytes(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(state_path(dir, name)).unwrap()
}

fn pointer_path(dir: &Path, name: &str) -> PathBuf {
    dir.join("sessions").join(name).join("request-leg.toml")
}

fn events(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(state_path(dir, name))
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The session's current binding for `var`, folded the way koto does.
fn binding(dir: &Path, name: &str, var: &str) -> String {
    let mut value = String::new();
    for e in events(dir, name) {
        let vars = match e["type"].as_str() {
            Some("workflow_initialized") => &e["payload"]["variables"],
            Some("variables_rebound") => &e["payload"]["variables"],
            _ => continue,
        };
        if let Some(v) = vars.get(var).and_then(|v| v.as_str()) {
            value = v.to_string();
        }
    }
    value
}

fn create_request(dir: &Path, legs: &str) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            &format!(r#"{{"legs":{legs}}}"#),
            "--requested-by",
            "deliver-a",
            "--coordinator-of-record",
            "deliver-a",
        ],
    );
    envelope["request_id"].as_str().unwrap().to_string()
}

/// A request with a `scope` leg naming `scope.md` and `TOPIC: t1`.
fn scope_request(dir: &Path) -> String {
    create_request(
        dir,
        r#"[{"name":"scope","role":"scope","template":"scope.md","inputs":{"TOPIC":"t1"}}]"#,
    )
}

fn log_bytes(dir: &Path, id: &str) -> Vec<u8> {
    std::fs::read(
        dir.join(".koto")
            .join("requests")
            .join(id)
            .join("request.jsonl"),
    )
    .unwrap()
}

fn leg(dir: &Path, id: &str) -> serde_json::Value {
    run_ok(dir, &["request", "get", id])["legs"]["scope"].clone()
}

// ===== --vars-file =====

#[test]
fn vars_file_creates_a_session() {
    let tmp = TempDir::new().unwrap();
    let out = create_session(tmp.path(), "s", &[("TOPIC", "t1"), ("MERGE", "true")]);
    assert_eq!(out["outcome"], "created");
    assert_eq!(out["state"], "work");
    assert_eq!(binding(tmp.path(), "s", "MERGE"), "true");
    assert_eq!(binding(tmp.path(), "s", "INTENT_FLAG"), "continue");
    let header: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(state_path(tmp.path(), "s"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let origin = &header["origin"];
    assert_eq!(origin["store"]["kind"], "local");
    assert_eq!(
        origin["anchor"].as_str().unwrap(),
        std::fs::canonicalize(tmp.path()).unwrap().to_str().unwrap()
    );
    assert_eq!(
        origin["store"]["base"].as_str().unwrap(),
        std::fs::canonicalize(tmp.path().join("sessions"))
            .unwrap()
            .to_str()
            .unwrap()
    );
}

#[test]
fn a_repeated_key_is_refused_as_duplicate_var_with_no_session() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let vars = vars_file(
        tmp.path(),
        "v.json",
        &[
            ("TOPIC", "t1"),
            ("INTENT_FLAG", "stop"),
            ("INTENT_FLAG", "stop"),
        ],
    );
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, &[])),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "duplicate_var", "{err}");
    assert_eq!(err["var"], "INTENT_FLAG");
    assert!(!tmp.path().join("sessions").join("s").exists());
}

#[test]
fn constraint_and_undeclared_variables_are_refused() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let vars = vars_file(
        tmp.path(),
        "v.json",
        &[("TOPIC", "t1"), ("INTENT_FLAG", "maybe")],
    );
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, &[])),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "invalid_var", "{err}");
    assert_eq!(err["var"], "INTENT_FLAG");
    assert_eq!(err["value"], "maybe");
    assert!(err["constraint"].as_str().unwrap().starts_with("values:"));

    let vars = vars_file(tmp.path(), "u.json", &[("TOPIC", "t1"), ("NOPE", "1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, &[])),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "unknown_var", "{err}");
    assert_eq!(err["var"], "NOPE");

    let vars = vars_file(tmp.path(), "c.json", &[("TOPIC", "a;b")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, &[])),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "invalid_var", "{err}");
    assert!(!tmp.path().join("sessions").join("s").exists());
}

#[test]
fn a_bad_vars_file_is_refused_with_a_typed_error() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");

    let not_json = dir.join("bad.json");
    std::fs::write(&not_json, "TOPIC=t1").unwrap();
    let wrong_shape = dir.join("shape.json");
    std::fs::write(&wrong_shape, r#"{"TOPIC":"t1"}"#).unwrap();
    let huge = dir.join("huge.json");
    std::fs::write(&huge, format!(r#"[["TOPIC","{}"]]"#, "a".repeat(70 * 1024))).unwrap();
    let target = vars_file(dir, "real.json", &[("TOPIC", "t1")]);
    let link = dir.join("link.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let directory = dir.join("adir");
    std::fs::create_dir(&directory).unwrap();

    for path in [&not_json, &wrong_shape, &huge, &link, &directory] {
        let (code, err) = run_err(dir, &as_strs(&init_args(dir, "s", "scope.md", path, &[])));
        assert_eq!(code, 2, "{path:?}: {err}");
        assert_eq!(err["code"], "invalid_vars_file", "{path:?}: {err}");
    }
    assert!(!dir.join("sessions").join("s").exists());
}

#[test]
fn vars_file_with_var_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--var", "MERGE=true"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "invalid_usage", "{err}");
}

#[test]
fn a_bad_variable_against_an_existing_session_is_the_variable_error() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    let before = state_bytes(tmp.path(), "s");
    for pairs in [
        vec![("TOPIC", "t1"), ("INTENT_FLAG", "maybe")],
        vec![("TOPIC", "t1"), ("TOPIC", "t1")],
    ] {
        let vars = vars_file(tmp.path(), "v.json", &pairs);
        for flags in [
            &[][..],
            &["--attach-live"][..],
            &["--attach-live", "--replace-terminal"][..],
        ] {
            let (code, err) = run_err(
                tmp.path(),
                &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, flags)),
            );
            assert_eq!(code, 2, "{err}");
            assert!(
                err["code"] == "invalid_var" || err["code"] == "duplicate_var",
                "{err}"
            );
            assert!(!err["error"].as_str().unwrap().contains("already exists"));
        }
    }
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

// ===== --replace-terminal =====

#[test]
fn a_terminal_session_is_replaced_and_its_result_returned() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "old")]);
    finish(tmp.path(), "s");

    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "new")]);
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--replace-terminal"],
        )),
    );
    assert_eq!(out["outcome"], "replaced", "{out}");
    assert_eq!(out["state"], "work");
    assert_eq!(out["replaced_state"], "done");
    assert_eq!(
        out["replaced_result"]["payload"]["outcome"], "scoped",
        "{out}"
    );
    assert_eq!(out["replaced_result"]["payload"]["topic"], "old", "{out}");
    assert_eq!(binding(tmp.path(), "s", "TOPIC"), "new");
}

#[test]
fn replace_terminal_refuses_a_live_session_and_leaves_it_alone() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    let before = state_bytes(tmp.path(), "s");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--replace-terminal"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "session_live", "{err}");
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

#[test]
fn replace_terminal_with_no_session_creates_one() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--replace-terminal"],
        )),
    );
    assert_eq!(out["outcome"], "created");
}

// ===== --attach-live =====

#[test]
fn a_matching_live_session_is_attached() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1"), ("INTENT_FLAG", "stop")]);
    let vars = vars_file(
        tmp.path(),
        "v.json",
        &[("TOPIC", "t1"), ("INTENT_FLAG", "stop")],
    );
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(out["outcome"], "attached", "{out}");
    assert_eq!(out["state"], "work");
    assert_eq!(out["rebound"], serde_json::json!({}));
    let inits = events(tmp.path(), "s")
        .iter()
        .filter(|e| e["type"] == "workflow_initialized")
        .count();
    assert_eq!(inits, 1, "attaching creates no session");
}

#[test]
fn a_session_from_another_template_file_is_template_mismatch() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "execute.md");
    write_template(tmp.path(), "execute-coordinated.md");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "execute-t1",
            "execute.md",
            &vars,
            &[],
        )),
    );
    let before = state_bytes(tmp.path(), "execute-t1");
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "execute-t1",
            "execute-coordinated.md",
            &vars,
            &["--attach-live", "--replace-terminal"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "template_mismatch", "{err}");
    assert_eq!(err["recorded"], "execute.md");
    assert_eq!(err["requested"], "execute-coordinated.md");
    assert_eq!(state_bytes(tmp.path(), "execute-t1"), before);
}

#[test]
fn a_session_from_another_worktree_is_origin_mismatch() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    let before = state_bytes(tmp.path(), "s");
    let other = tmp.path().join("other-worktree");
    std::fs::create_dir(&other).unwrap();
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err_in(
        tmp.path(),
        &other,
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "origin_mismatch", "{err}");
    assert!(!err["error"].as_str().unwrap().contains("no origin record"));
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

#[test]
fn a_session_in_another_store_is_origin_mismatch() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    let before = state_bytes(tmp.path(), "s");
    // The same session directory reached through another store base: a
    // symlinked base canonicalizes to the same place, so copy instead.
    let other_base = tmp.path().join("other-sessions");
    std::fs::create_dir_all(other_base.join("s")).unwrap();
    std::fs::copy(
        state_path(tmp.path(), "s"),
        other_base.join("s").join("koto-s.state.jsonl"),
    )
    .unwrap();
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let output = koto_cmd(tmp.path(), tmp.path())
        .env("KOTO_SESSIONS_BASE", &other_base)
        .args(as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let err = json(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(err["code"], "origin_mismatch", "{err}");
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

#[test]
fn a_session_with_no_origin_record_is_refused_and_named() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "old-s", &[("TOPIC", "t1")]);
    // Strip the origin record, as a session from an earlier koto has none.
    let path = state_path(tmp.path(), "old-s");
    let content = std::fs::read_to_string(&path).unwrap();
    let mut lines = content.lines();
    let mut header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    header.as_object_mut().unwrap().remove("origin").unwrap();
    let mut rewritten = serde_json::to_string(&header).unwrap();
    rewritten.push('\n');
    for l in lines {
        rewritten.push_str(l);
        rewritten.push('\n');
    }
    std::fs::write(&path, &rewritten).unwrap();
    let before = state_bytes(tmp.path(), "old-s");

    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "old-s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "origin_mismatch", "{err}");
    let msg = err["error"].as_str().unwrap();
    assert!(msg.contains("no origin record"), "{msg}");
    assert!(msg.contains("koto session cleanup old-s"), "{msg}");
    assert_eq!(state_bytes(tmp.path(), "old-s"), before);
}

#[test]
fn a_differing_fixed_variable_is_var_mismatch() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1"), ("INTENT_FLAG", "stop")]);
    let before = state_bytes(tmp.path(), "s");
    let vars = vars_file(
        tmp.path(),
        "v.json",
        &[("TOPIC", "t1"), ("INTENT_FLAG", "continue")],
    );
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "var_mismatch", "{err}");
    assert_eq!(err["var"], "INTENT_FLAG");
    assert_eq!(err["recorded"], "stop");
    assert_eq!(err["requested"], "continue");
    assert_eq!(state_bytes(tmp.path(), "s"), before);

    // A fixed variable the caller doesn't pass isn't compared.
    let vars = vars_file(tmp.path(), "w.json", &[("TOPIC", "t1")]);
    run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
}

#[test]
fn an_accepted_attach_re_applies_rebind_variables() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1"), ("MERGE", "true")]);

    // MERGE omitted: back to its default, never inherited.
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(out["rebound"]["MERGE"], "false", "{out}");
    assert_eq!(binding(tmp.path(), "s", "MERGE"), "false");

    let vars = vars_file(tmp.path(), "w.json", &[("TOPIC", "t1"), ("MERGE", "true")]);
    run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(binding(tmp.path(), "s", "MERGE"), "true");
    let rebinds = events(tmp.path(), "s")
        .iter()
        .filter(|e| e["type"] == "variables_rebound")
        .count();
    assert_eq!(rebinds, 2);
}

#[test]
fn attach_live_alone_refuses_a_terminal_session() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    finish(tmp.path(), "s");
    let before = state_bytes(tmp.path(), "s");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "s",
            "scope.md",
            &vars,
            &["--attach-live"],
        )),
    );
    assert_eq!(code, 2);
    assert_eq!(err["code"], "session_terminal", "{err}");
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

#[test]
fn both_flags_attach_replace_or_create() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);
    let both = ["--attach-live", "--replace-terminal"];

    let out = run_ok(
        dir,
        &as_strs(&init_args(dir, "s", "scope.md", &vars, &both)),
    );
    assert_eq!(out["outcome"], "created");
    let out = run_ok(
        dir,
        &as_strs(&init_args(dir, "s", "scope.md", &vars, &both)),
    );
    assert_eq!(out["outcome"], "attached");
    finish(dir, "s");
    let out = run_ok(
        dir,
        &as_strs(&init_args(dir, "s", "scope.md", &vars, &both)),
    );
    assert_eq!(out["outcome"], "replaced");
}

#[test]
fn without_entry_flags_an_existing_session_already_exists() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "s", &[("TOPIC", "t1")]);
    let before = state_bytes(tmp.path(), "s");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let (code, err) = run_err(
        tmp.path(),
        &as_strs(&init_args(tmp.path(), "s", "scope.md", &vars, &[])),
    );
    assert_eq!(code, 1);
    assert!(
        err["error"].as_str().unwrap().contains("already exists"),
        "{err}"
    );
    let template = tmp.path().join("scope.md");
    let (code, err) = run_err(
        tmp.path(),
        &[
            "init",
            "s",
            "--template",
            template.to_str().unwrap(),
            "--var",
            "TOPIC=t1",
        ],
    );
    assert_eq!(code, 1);
    assert!(err["error"].as_str().unwrap().contains("already exists"));
    assert_eq!(state_bytes(tmp.path(), "s"), before);
}

// ===== --koto-leg =====

#[test]
fn a_malformed_koto_leg_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    for bad in [
        "no-colon",
        "Req-UPPER:scope",
        "req-1:-leg",
        "req-1:",
        ":scope",
    ] {
        let (code, err) = run_err(
            tmp.path(),
            &as_strs(&init_args(
                tmp.path(),
                "s",
                "scope.md",
                &vars,
                &["--koto-leg", bad],
            )),
        );
        assert_eq!(code, 2, "{bad}: {err}");
        assert_eq!(err["code"], "invalid_usage", "{bad}: {err}");
    }
    assert!(!tmp.path().join("sessions").join("s").exists());
}

#[test]
fn entry_flags_are_rejected_with_from_stdin_and_parent() {
    let tmp = TempDir::new().unwrap();
    create_session(tmp.path(), "parent", &[("TOPIC", "t1")]);
    for flags in [
        &["--attach-live"][..],
        &["--replace-terminal"][..],
        &["--koto-leg", "req-1:scope"][..],
    ] {
        let mut args = vec!["init", "s", "--from-stdin"];
        args.extend_from_slice(flags);
        let (code, err) = run_err(tmp.path(), &args);
        assert_eq!(code, 2, "{err}");
        assert_eq!(err["code"], "invalid_usage");

        let template = tmp.path().join("scope.md");
        let mut args = vec![
            "init",
            "s",
            "--template",
            template.to_str().unwrap(),
            "--parent",
            "parent",
            "--var",
            "TOPIC=t1",
        ];
        args.extend_from_slice(flags);
        let (code, err) = run_err(tmp.path(), &args);
        assert_eq!(code, 2, "{err}");
        assert_eq!(err["code"], "invalid_usage");
    }
}

#[test]
fn a_created_session_ends_bound_to_the_leg() {
    let tmp = TempDir::new().unwrap();
    write_template(tmp.path(), "scope.md");
    let id = scope_request(tmp.path());
    let vars = vars_file(tmp.path(), "v.json", &[("TOPIC", "t1")]);
    let target = format!("{id}:scope");
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--replace-terminal", "--koto-leg", &target],
        )),
    );
    assert_eq!(out["outcome"], "created");
    assert_eq!(out["leg"]["request_id"], id.as_str());
    assert_eq!(out["leg"]["written"], true);
    let l = leg(tmp.path(), &id);
    assert_eq!(l["bound_child"], "scope-t1");
    assert_eq!(l["attach"], "self");
    assert!(pointer_path(tmp.path(), "scope-t1").exists());

    // Again: attached, the leg already bound to it, nothing written.
    let before = log_bytes(tmp.path(), &id);
    let out = run_ok(
        tmp.path(),
        &as_strs(&init_args(
            tmp.path(),
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &target],
        )),
    );
    assert_eq!(out["outcome"], "attached");
    assert_eq!(out["leg"]["written"], false);
    assert_eq!(log_bytes(tmp.path(), &id), before);

    // The session's result reaches the leg by promotion.
    finish(tmp.path(), "scope-t1");
    let l = leg(tmp.path(), &id);
    assert_eq!(l["result_source"], "promoted");
    assert_eq!(l["result"]["payload"]["outcome"], "scoped");
}

#[test]
fn an_attached_session_moves_to_a_new_run_and_rebinds() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1"), ("MERGE", "false")]);
    let first = scope_request(dir);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);
    run_ok(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &format!("{first}:scope")],
        )),
    );

    // A newer run abandons the old request and attaches with MERGE=true.
    run_ok(
        dir,
        &[
            "request",
            "abandon-request",
            &first,
            "--rationale",
            "superseded",
        ],
    );
    let second = scope_request(dir);
    let merge = vars_file(dir, "m.json", &[("TOPIC", "t1"), ("MERGE", "true")]);
    let out = run_ok(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &merge,
            &["--attach-live", "--koto-leg", &format!("{second}:scope")],
        )),
    );
    assert_eq!(out["outcome"], "attached");
    assert_eq!(out["rebound"]["MERGE"], "true");
    assert_eq!(binding(dir, "scope-t1", "MERGE"), "true");
    assert_eq!(leg(dir, &second)["bound_child"], "scope-t1");
}

/// A stale invocation (the old run's request, now abandoned, or a leg
/// another session holds) passing `MERGE=true` is refused before any
/// rebind: the session's `MERGE` and its whole log are unchanged.
#[test]
fn a_refused_attach_performs_no_rebind() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1"), ("MERGE", "false")]);
    let merge = vars_file(dir, "m.json", &[("TOPIC", "t1"), ("MERGE", "true")]);

    // An abandoned leg.
    let stale = scope_request(dir);
    run_ok(
        dir,
        &[
            "request",
            "abandon",
            &stale,
            "scope",
            "--rationale",
            "superseded",
        ],
    );
    // A leg bound to another session.
    create_session(dir, "scope-other", &[("TOPIC", "t1")]);
    let taken = scope_request(dir);
    run_ok(
        dir,
        &[
            "request",
            "attach",
            &taken,
            "scope",
            "--session",
            "scope-other",
        ],
    );

    for (id, code) in [
        (&stale, "leg_abandoned"),
        (&taken, "leg_bound_to_different_child"),
    ] {
        let state_before = state_bytes(dir, "scope-t1");
        let log_before = log_bytes(dir, id);
        let (exit, err) = run_err(
            dir,
            &as_strs(&init_args(
                dir,
                "scope-t1",
                "scope.md",
                &merge,
                &["--attach-live", "--koto-leg", &format!("{id}:scope")],
            )),
        );
        assert_eq!(exit, 2, "{err}");
        assert_eq!(err["code"], code, "{err}");
        assert_eq!(state_bytes(dir, "scope-t1"), state_before);
        assert_eq!(log_bytes(dir, id), log_before);
        assert_eq!(binding(dir, "scope-t1", "MERGE"), "false");
        assert!(!pointer_path(dir, "scope-t1").exists());
    }
}

#[test]
fn a_session_answering_a_live_leg_is_not_re_pointed() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1")]);
    let first = scope_request(dir);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);
    run_ok(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &format!("{first}:scope")],
        )),
    );
    let second = scope_request(dir);
    let before = log_bytes(dir, &second);
    let (exit, err) = run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &format!("{second}:scope")],
        )),
    );
    assert_eq!(exit, 2);
    assert_eq!(err["code"], "child_bound_to_different_leg", "{err}");
    // The refusal is recorded on the open, unbound second leg.
    let l = leg(dir, &second);
    assert_eq!(l["result_source"], "refused", "{l}");
    assert_eq!(
        l["result"]["payload"]["reason"],
        "child-bound-to-different-leg"
    );
    assert_ne!(log_bytes(dir, &second), before);
}

#[test]
fn a_refusal_is_recorded_on_an_open_unbound_leg() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");
    let bad = vars_file(dir, "v.json", &[("TOPIC", "t1"), ("INTENT_FLAG", "maybe")]);

    // Without --koto-leg, for the reference output.
    let (plain_code, plain_out, _) =
        run(dir, &as_strs(&init_args(dir, "s", "scope.md", &bad, &[])));

    let id = scope_request(dir);
    let target = format!("{id}:scope");
    let (code, out, _) = run(
        dir,
        &as_strs(&init_args(
            dir,
            "s",
            "scope.md",
            &bad,
            &["--koto-leg", &target],
        )),
    );
    assert_eq!(code, plain_code);
    assert_eq!(code, 2);
    assert_eq!(out, plain_out, "recording must not change the output");
    assert!(!dir.join("sessions").join("s").exists());

    let l = leg(dir, &id);
    assert_eq!(l["disposition"], "resolved", "{l}");
    assert_eq!(l["result_source"], "refused");
    assert_eq!(l["result"]["status"], "failure");
    let payload = &l["result"]["payload"];
    assert_eq!(payload["outcome"], "refused");
    assert_eq!(payload["reason"], "invalid-var:INTENT_FLAG");
    assert_eq!(payload["var"], "INTENT_FLAG");
    assert_eq!(payload["requested"], "maybe");
    assert_eq!(payload["recorded"], "");
    assert!(l["bound_child"].is_null(), "a refusal never binds: {l}");
}

#[test]
fn check_refusals_are_recorded_with_their_reasons() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1"), ("INTENT_FLAG", "stop")]);
    write_template(dir, "other.md");

    type Case<'a> = (Vec<(&'a str, &'a str)>, &'a str, &'a str);
    let cases: Vec<Case> = vec![
        (
            vec![("TOPIC", "t1"), ("INTENT_FLAG", "continue")],
            "scope.md",
            "var-mismatch:INTENT_FLAG",
        ),
        (vec![("TOPIC", "t1")], "other.md", "template-mismatch"),
        (
            vec![("TOPIC", "t1"), ("TOPIC", "t2")],
            "scope.md",
            "duplicate-var:TOPIC",
        ),
        (vec![("NOPE", "1")], "scope.md", "unknown-var:NOPE"),
    ];
    for (pairs, template, reason) in cases {
        let id = scope_request(dir);
        let vars = vars_file(dir, "v.json", &pairs);
        let state_before = state_bytes(dir, "scope-t1");
        let (exit, _) = run_err(
            dir,
            &as_strs(&init_args(
                dir,
                "scope-t1",
                template,
                &vars,
                &["--attach-live", "--koto-leg", &format!("{id}:scope")],
            )),
        );
        assert_eq!(exit, 2);
        assert_eq!(state_bytes(dir, "scope-t1"), state_before);
        let l = leg(dir, &id);
        assert_eq!(l["result_source"], "refused", "{reason}: {l}");
        assert_eq!(l["result"]["payload"]["reason"], reason, "{l}");
    }

    // The var-mismatch record carries both values.
    let id = scope_request(dir);
    let vars = vars_file(
        dir,
        "v.json",
        &[("TOPIC", "t1"), ("INTENT_FLAG", "continue")],
    );
    run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &format!("{id}:scope")],
        )),
    );
    let payload = &leg(dir, &id)["result"]["payload"];
    assert_eq!(payload["recorded"], "stop");
    assert_eq!(payload["requested"], "continue");
}

#[test]
fn a_leg_input_mismatch_is_refused_before_the_session_is_created() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");
    let id = scope_request(dir);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t2")]);
    let (exit, err) = run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t2",
            "scope.md",
            &vars,
            &["--koto-leg", &format!("{id}:scope")],
        )),
    );
    assert_eq!(exit, 2);
    assert_eq!(err["code"], "input_mismatch", "{err}");
    assert!(!dir.join("sessions").join("scope-t2").exists());
    let payload = &leg(dir, &id)["result"]["payload"];
    assert_eq!(payload["reason"], "input-mismatch");
    assert_eq!(payload["var"], "TOPIC");
    assert_eq!(payload["recorded"], "t2");
    assert_eq!(payload["requested"], "t1");
}

#[test]
fn a_refusal_writes_nothing_on_a_leg_that_is_not_open_and_unbound() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");
    create_session(dir, "scope-other", &[("TOPIC", "t1")]);
    let bad = vars_file(dir, "v.json", &[("TOPIC", "t1"), ("INTENT_FLAG", "maybe")]);

    let bound = scope_request(dir);
    run_ok(
        dir,
        &[
            "request",
            "attach",
            &bound,
            "scope",
            "--session",
            "scope-other",
        ],
    );
    let resolved = scope_request(dir);
    run_ok(
        dir,
        &[
            "request",
            "resolve",
            &resolved,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"done"}"#,
        ],
    );
    let abandoned = scope_request(dir);
    run_ok(
        dir,
        &[
            "request",
            "abandon",
            &abandoned,
            "scope",
            "--rationale",
            "x",
        ],
    );
    let closed = scope_request(dir);
    run_ok(dir, &["request", "close", &closed]);

    for id in [&bound, &resolved, &abandoned, &closed] {
        let before = log_bytes(dir, id);
        let (exit, err) = run_err(
            dir,
            &as_strs(&init_args(
                dir,
                "s",
                "scope.md",
                &bad,
                &["--koto-leg", &format!("{id}:scope")],
            )),
        );
        assert_eq!(exit, 2);
        assert_eq!(err["code"], "invalid_var", "the original error: {err}");
        assert_eq!(log_bytes(dir, id), before, "{id}");
    }

    // A request that doesn't exist: still the original error.
    let (exit, err) = run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "s",
            "scope.md",
            &bad,
            &["--koto-leg", "req-does-not-exist:scope"],
        )),
    );
    assert_eq!(exit, 2);
    assert_eq!(err["code"], "invalid_var");
    assert!(!dir.join(".koto/requests/req-does-not-exist").exists());
}

#[test]
fn a_terminal_or_live_refusal_is_recorded_too() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1")]);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);

    let id = scope_request(dir);
    run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--replace-terminal", "--koto-leg", &format!("{id}:scope")],
        )),
    );
    assert_eq!(leg(dir, &id)["result"]["payload"]["reason"], "session-live");

    finish(dir, "scope-t1");
    let id = scope_request(dir);
    run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--attach-live", "--koto-leg", &format!("{id}:scope")],
        )),
    );
    assert_eq!(
        leg(dir, &id)["result"]["payload"]["reason"],
        "session-terminal"
    );
    assert!(!pointer_path(dir, "scope-t1").exists());
}

/// Hold the request's write lock from the test, so a bind that passed its
/// unlocked checks can't take the lock: the same position as losing a race
/// to a concurrent writer.
fn hold_request_lock(dir: &Path, id: &str) -> std::fs::File {
    use std::os::unix::io::AsRawFd;
    let path = dir
        .join(".koto")
        .join("requests")
        .join(id)
        .join("request.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap();
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0, "the test should hold the request lock");
    file
}

#[test]
fn a_bind_that_loses_the_race_removes_the_session_it_created() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_template(dir, "scope.md");
    let id = scope_request(dir);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);
    let before = log_bytes(dir, &id);
    let lock = hold_request_lock(dir, &id);
    let (exit, err) = run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &["--koto-leg", &format!("{id}:scope")],
        )),
    );
    drop(lock);
    assert_eq!(exit, 1, "{err}");
    assert_eq!(err["code"], "lock_contention", "{err}");
    assert!(
        !dir.join("sessions").join("scope-t1").exists(),
        "the session made for the leg is removed"
    );
    assert_eq!(log_bytes(dir, &id), before);
}

#[test]
fn a_bind_that_loses_the_race_leaves_an_attached_session_unchanged() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1"), ("MERGE", "false")]);
    let id = scope_request(dir);
    let merge = vars_file(dir, "m.json", &[("TOPIC", "t1"), ("MERGE", "true")]);
    let before = state_bytes(dir, "scope-t1");
    let lock = hold_request_lock(dir, &id);
    let (exit, err) = run_err(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &merge,
            &["--attach-live", "--koto-leg", &format!("{id}:scope")],
        )),
    );
    drop(lock);
    assert_eq!(exit, 1, "{err}");
    assert_eq!(err["code"], "lock_contention", "{err}");
    assert_eq!(state_bytes(dir, "scope-t1"), before);
    assert_eq!(binding(dir, "scope-t1", "MERGE"), "false");
}

#[test]
fn a_replaced_session_is_bound_to_the_leg() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    create_session(dir, "scope-t1", &[("TOPIC", "t1")]);
    finish(dir, "scope-t1");
    let id = scope_request(dir);
    let vars = vars_file(dir, "v.json", &[("TOPIC", "t1")]);
    let out = run_ok(
        dir,
        &as_strs(&init_args(
            dir,
            "scope-t1",
            "scope.md",
            &vars,
            &[
                "--attach-live",
                "--replace-terminal",
                "--koto-leg",
                &format!("{id}:scope"),
            ],
        )),
    );
    assert_eq!(out["outcome"], "replaced");
    assert_eq!(leg(dir, &id)["bound_child"], "scope-t1");
    assert!(pointer_path(dir, "scope-t1").exists());
}
