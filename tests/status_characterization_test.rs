//! Characterization of `koto status` output ahead of the directive-helper
//! refactor (the status-refactor group of PLAN-session-view).
//!
//! The mid-run shape — the one the refactor touches — is pinned as a
//! full exact JSON compare, with the per-run template path the single
//! normalized value; the tampered-template shape pins the mismatch
//! object and the rendered directive; the terminal, migrated and
//! never-advanced shapes pin their key fields, exit codes and exact
//! error objects. Committed against the unrefactored `handle_status`,
//! these must pass unchanged once it renders its directive through
//! `session_view::render_directive`: a byte that moves on the rendering
//! path fails an exact comparison here.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;

use assert_cmd::Command;
use assert_fs::TempDir;
use fake_s3::FakeS3;
use koto::session::local::LocalBackend;
use koto::session::Backend;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID"); // koto#331
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn parse_last_json(stdout: &[u8]) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(stdout).to_string();
    let trimmed = stdout.trim();
    serde_json::from_str(trimmed).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
    })
}

fn init_workflow(dir: &Path, name: &str, template: &str, vars: &[&str]) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    let mut args = vec!["init", name, "--template", src.to_str().unwrap()];
    for var in vars {
        args.push("--var");
        args.push(var);
    }
    koto_cmd(dir).args(&args).assert().success();
}

fn status_raw(dir: &Path, name: &str) -> (bool, serde_json::Value) {
    let output = koto_cmd(dir).args(["status", name]).output().unwrap();
    (output.status.success(), parse_last_json(&output.stdout))
}

const TEMPLATE: &str = r#"---
name: characterization-fixture
version: "1.0"
initial_state: gather
variables:
  TOPIC:
    description: "Topic under work"
    default: "untitled"
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          route: go
  done:
    terminal: true
---

## gather

Collect inputs for {{TOPIC}} in {{SESSION_DIR}} as {{SESSION_NAME}}.

<!-- details -->

Detailed gather steps for {{TOPIC}}.

## done

Done.
"#;

/// Normalize the one volatile field: the per-run absolute template path.
/// Everything else in a mid-run status is deterministic for this fixture.
fn normalized(mut v: serde_json::Value, dir: &Path) -> serde_json::Value {
    if let Some(obj) = v.as_object_mut() {
        if let Some(tp) = obj.get("template_path").and_then(|t| t.as_str()) {
            assert!(
                tp.starts_with(&*dir.to_string_lossy()),
                "template_path {tp} should sit under the scratch home"
            );
            obj.insert(
                "template_path".to_string(),
                serde_json::json!("<template_path>"),
            );
        }
    }
    v
}

#[test]
fn mid_run_status_json_is_pinned_exactly() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &["TOPIC=session-view"]);

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok);
    let hash = json["template_hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64);
    let sd = sessions_base(tmp.path()).join("wf");

    let expected = serde_json::json!({
        "name": "wf",
        "current_state": "gather",
        "template_path": "<template_path>",
        "template_hash": hash,
        "is_terminal": false,
        "directive": format!(
            "Collect inputs for session-view in {} as wf.",
            sd.display()
        ),
        "details": "Detailed gather steps for session-view.",
        "expects": {
            "event_type": "evidence_submitted",
            "fields": {
                "route": {"type": "enum", "required": true, "values": ["go"]}
            },
            "options": [
                {"target": "done", "when": {"route": "go"}}
            ]
        },
    });
    assert_eq!(normalized(json, tmp.path()), expected);
}

#[test]
fn terminal_status_carries_result_and_no_directive() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &[]);
    koto_cmd(tmp.path())
        .args([
            "next",
            "wf",
            "--no-cleanup",
            "--with-data",
            r#"{"route":"go"}"#,
        ])
        .assert()
        .success();

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok);
    assert_eq!(json["name"], "wf");
    assert_eq!(json["current_state"], "done");
    assert_eq!(json["is_terminal"], true);
    for absent in ["directive", "details", "expects", "template_hash_mismatch"] {
        assert!(
            json.get(absent).is_none(),
            "{absent} must be absent on a terminal status"
        );
    }
    assert_eq!(json["result"]["status"], "success");
}

/// The terminal shape, pinned as the whole object rather than a subset:
/// exactly these six members and nothing else, so a field that starts
/// leaking onto (or off) the terminal answer fails an exact comparison.
#[test]
fn terminal_status_json_is_pinned_exactly() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &[]);
    koto_cmd(tmp.path())
        .args([
            "next",
            "wf",
            "--no-cleanup",
            "--with-data",
            r#"{"route":"go"}"#,
        ])
        .assert()
        .success();

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok);
    let hash = json["template_hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64);
    let expected = serde_json::json!({
        "name": "wf",
        "current_state": "done",
        "template_path": "<template_path>",
        "template_hash": hash,
        "is_terminal": true,
        "result": {
            "status": "success",
            "summary": "completed at done",
        },
    });
    assert_eq!(normalized(json, tmp.path()), expected);
}

/// A state that declares no `<!-- details -->` block answers with the
/// directive and `expects` but no `details` member -- the details-absent
/// half of the optional-fields contract, which the standard fixture
/// (whose state always carries details) never exercises.
#[test]
fn a_state_without_details_omits_the_details_member() {
    const NO_DETAILS_TEMPLATE: &str = r#"---
name: characterization-fixture
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          route: go
  done:
    terminal: true
---

## gather

Collect inputs.

## done

Done.
"#;
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", NO_DETAILS_TEMPLATE, &[]);

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok);
    assert_eq!(json["directive"], "Collect inputs.");
    assert!(
        json.get("details").is_none(),
        "a phase that declares no details carries no details member"
    );
    assert!(json.get("expects").is_some());
}

/// A current state the compiled template does not name: the session's log
/// says `ghost`, the template has no such state. `koto status` answers
/// rather than failing -- not terminal, no directive block, no result --
/// and the whole object is pinned so the shape cannot drift silently.
#[test]
fn a_current_state_missing_from_the_template_answers_minimally() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &[]);

    let state_path = sessions_base(tmp.path())
        .join("wf")
        .join("koto-wf.state.jsonl");
    koto::engine::persistence::append_event(
        &state_path,
        &koto::engine::types::EventPayload::Transitioned {
            from: Some("gather".to_string()),
            to: "ghost".to_string(),
            condition_type: "evidence".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        },
        "2026-01-01T00:00:02Z",
    )
    .unwrap();

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok, "an unknown current state is an answer, not a failure");
    let hash = json["template_hash"].as_str().unwrap().to_string();
    let expected = serde_json::json!({
        "name": "wf",
        "current_state": "ghost",
        "template_path": "<template_path>",
        "template_hash": hash,
        "is_terminal": false,
    });
    assert_eq!(normalized(json, tmp.path()), expected);
}

/// A recorded variable value that fails re-validation: `koto status`
/// exits 3 with the exact error object, the defense-in-depth path in the
/// directive block (`Variables::from_events`) that no fixture reached
/// before.
#[test]
fn a_variable_failing_revalidation_exits_three() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &["TOPIC=clean"]);

    // Corrupt the recorded value in place: the CLI refused `;` at init,
    // so rewrite the workflow_initialized line as a hostile writer would.
    let state_path = sessions_base(tmp.path())
        .join("wf")
        .join("koto-wf.state.jsonl");
    let rewritten: Vec<String> = std::fs::read_to_string(&state_path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v["type"] == "workflow_initialized" {
                v["payload"]["variables"]["TOPIC"] = serde_json::json!("bad;value");
            }
            serde_json::to_string(&v).unwrap()
        })
        .collect();
    std::fs::write(&state_path, rewritten.join("\n") + "\n").unwrap();

    let output = koto_cmd(tmp.path())
        .args(["status", "wf"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let json = parse_last_json(&output.stdout);
    assert_eq!(json["command"], "status");
    let err = json["error"].as_str().unwrap();
    assert!(
        err.starts_with("variable re-validation failed:"),
        "exact failure wording, got: {err}"
    );
}

#[test]
fn a_tampered_template_reports_the_mismatch_without_failing() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &[]);

    // Tamper the compiled template the recorded path points at, keeping
    // it parseable: the status must report, not fail.
    let (_, before) = status_raw(tmp.path(), "wf");
    let tp = before["template_path"].as_str().unwrap().to_string();
    let recorded = before["template_hash"].as_str().unwrap().to_string();
    let tampered = std::fs::read_to_string(&tp)
        .unwrap()
        .replace("Collect inputs", "Tampered inputs");
    std::fs::write(&tp, tampered).unwrap();

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok, "a mismatch is reported, never a failure");
    let mismatch = &json["template_hash_mismatch"];
    assert_eq!(mismatch["recorded"], serde_json::json!(recorded));
    let actual = mismatch["actual"].as_str().unwrap();
    assert_eq!(actual.len(), 64);
    assert_ne!(actual, recorded);
    assert!(
        json["directive"]
            .as_str()
            .unwrap()
            .contains("Tampered inputs"),
        "the directive renders from the bytes on disk, reported as mismatched"
    );
}

#[test]
fn a_migrated_session_exits_two_with_the_exact_error_object() {
    let s3 = FakeS3::start("koto-status-characterization");
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("ws")).unwrap();
    let config = format!(
        "[session]\nbackend = \"cloud\"\n[session.cloud]\nendpoint = \"{}\"\nbucket = \"koto-status-characterization\"\nregion = \"us-east-1\"\npath_style = true\n",
        s3.endpoint()
    );
    let koto_dir = tmp.path().join("ws/.koto");
    std::fs::create_dir_all(&koto_dir).unwrap();
    std::fs::write(koto_dir.join("config.toml"), config).unwrap();

    let ws = tmp.path().join("ws");
    let src = ws.join("t.md");
    std::fs::write(&src, TEMPLATE).unwrap();
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(&ws);
    cmd.env("HOME", tmp.path());
    cmd.env("AWS_ACCESS_KEY_ID", "k");
    cmd.env("AWS_SECRET_ACCESS_KEY", "s");
    cmd.args(["init", "wf", "--template", src.to_str().unwrap()])
        .assert()
        .success();

    // The marker another workspace's import would have written.
    let prefix = s3
        .keys()
        .iter()
        .find_map(|k| k.split('/').next().map(str::to_string))
        .expect("the init pushed under a prefix");
    s3.put(
        &format!("{prefix}/wf/migrated.json"),
        &serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "target": {
                "session": "wf-elsewhere",
                "session_id": "x",
                "workspace": "/srv/other",
                "prefix": "0123456789abcdef",
            },
            "machine_id": "m",
            "migrated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
    );

    let mut status = Command::cargo_bin("koto").unwrap();
    status.env_remove("CLAUDE_CODE_SESSION_ID");
    // The marker landed a moment after the init's check, which a read
    // inside the check window would reuse; this asks the bucket itself.
    status.env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0");
    status.current_dir(&ws);
    status.env("HOME", tmp.path());
    status.env("AWS_ACCESS_KEY_ID", "k");
    status.env("AWS_SECRET_ACCESS_KEY", "s");
    let output = status.args(["status", "wf"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "caller-actionable exit");
    let json = parse_last_json(&output.stdout);
    assert_eq!(
        json,
        serde_json::json!({
            "error": "session_migrated: session 'wf' was migrated to 'wf-elsewhere' in /srv/other; continue it there",
            "command": "status",
        })
    );
}

#[test]
fn a_never_advanced_session_exits_three_as_corrupt() {
    // The exit the snapshot layer deliberately diverges from: a session
    // whose log holds only workflow_initialized derives no state, and
    // `koto status` answers with its infrastructure exit. Pinned here so
    // the divergence the view documents cannot drift silently.
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &[]);
    let state_path = sessions_base(tmp.path())
        .join("wf")
        .join("koto-wf.state.jsonl");
    let text = std::fs::read_to_string(&state_path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    // Keep the header and the workflow_initialized event only.
    lines.truncate(2);
    std::fs::write(&state_path, format!("{}\n", lines.join("\n"))).unwrap();

    let output = koto_cmd(tmp.path())
        .args(["status", "wf"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "infrastructure exit");
    let json = parse_last_json(&output.stdout);
    assert_eq!(
        json,
        serde_json::json!({
            "error": "corrupt state file: cannot derive current state",
            "command": "status",
        })
    );
}

#[test]
fn status_and_snapshot_render_the_same_directive() {
    let tmp = TempDir::new().unwrap();
    init_workflow(tmp.path(), "wf", TEMPLATE, &["TOPIC=parity"]);

    let (ok, json) = status_raw(tmp.path(), "wf");
    assert!(ok);
    let status_directive = json["directive"].as_str().unwrap();

    let backend = Backend::Local(LocalBackend::with_base_dir(sessions_base(tmp.path())));
    let snap = koto::session_view::snapshot(&backend, "wf").unwrap();
    assert_eq!(
        snap.facts.directive.as_ref().map(|d| d.as_str()),
        Some(status_directive),
        "one directive rendering, two readers"
    );
}
