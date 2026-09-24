//! The `request-leg` gate, end to end.
//!
//! A coordinating session routes on what a root session attached to a
//! request leg reported. These tests drive the real binary: the coordinator
//! blocks (temporally) while the leg is open, the attached session reaches a
//! terminal that declares a `result:` map, the promotion records which
//! terminal answered, and the coordinator's next tick routes on the payload
//! and copies values out of it through `context_assignments`. The request log
//! is never written by the gate.
//!
//! Every test points `HOME` and `KOTO_SESSIONS_BASE` into its own temporary
//! directory, so the request store lands at `<tmp>/.koto/requests/`.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

fn run(dir: &Path, args: &[&str]) -> (i32, serde_json::Value, String) {
    let output = koto_cmd(dir).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    (output.status.code().unwrap_or(-1), json, stderr)
}

fn run_ok(dir: &Path, args: &[&str]) -> serde_json::Value {
    let (code, json, stderr) = run(dir, args);
    assert_eq!(code, 0, "expected success from {args:?}\n{json}\n{stderr}");
    json
}

fn init(dir: &Path, name: &str, file: &str, body: &str, vars: &[&str]) {
    let path = dir.join(file);
    std::fs::write(&path, body).unwrap();
    let mut args = vec!["init", name, "--template", path.to_str().unwrap()];
    for v in vars {
        args.push("--var");
        args.push(v);
    }
    run_ok(dir, &args);
}

fn create_request(dir: &Path) -> String {
    let envelope = run_ok(
        dir,
        &[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"scope","role":"scope","template":"scope.md","inputs":"brief"}]}"#,
            "--requested-by",
            "deliver",
            "--coordinator-of-record",
            "deliver",
        ],
    );
    envelope["request_id"].as_str().unwrap().to_string()
}

fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(".koto")
        .join("requests")
        .join(id)
        .join("request.jsonl")
}

fn ctx_get(dir: &Path, name: &str, key: &str) -> String {
    let output = koto_cmd(dir)
        .args(["context", "get", name, key])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context get {key}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The answering session: one evidence step, then a terminal whose declared
/// result map becomes the leg's payload.
const SCOPE: &str = r#"---
name: scope
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
    result:
      outcome: scoped
      pr: https://example.test/pr/9
---

## work

Scope it.

## done

Done.
"#;

/// The coordinator: the `/deliver` `scope_run` shape.
const DELIVER: &str = r#"---
name: deliver
version: "1.0"
initial_state: scope_run
variables:
  REQ:
    required: true
states:
  scope_run:
    gates:
      scope_leg:
        type: request-leg
        request: "{{REQ}}"
        leg: scope
        expect:
          outcome: [scoped, declined]
        overridable: false
    transitions:
      - target: scoped
        when:
          gates.scope_leg.disposition: resolved
          gates.scope_leg.payload.outcome: scoped
          gates.scope_leg.valid: true
        context_assignments:
          pr: "${gates.scope_leg.payload.pr}"
          from: "${gates.scope_leg.final_state}"
          tmpl: "${gates.scope_leg.template}"
          source: "${gates.scope_leg.source}"
      - target: declined
        when:
          gates.scope_leg.disposition: resolved
          gates.scope_leg.payload.outcome: declined
      - target: abandoned
        when:
          gates.scope_leg.disposition: abandoned
  scoped:
    terminal: true
  declined:
    terminal: true
  abandoned:
    terminal: true
---

## scope_run

Wait for the scope leg.

## scoped

Scoped.

## declined

Declined.

## abandoned

Abandoned.
"#;

#[test]
fn a_coordinator_waits_on_an_open_leg_then_routes_on_the_promoted_payload() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d);
    init(
        d,
        "deliver-1",
        "deliver.md",
        DELIVER,
        &[&format!("REQ={id}")],
    );

    // Open leg: a temporal block the agent cannot override.
    let before = std::fs::read(log_path(d, &id)).unwrap();
    let next = run_ok(d, &["next", "deliver-1", "--no-cleanup"]);
    assert_eq!(next["action"], "gate_blocked", "{next}");
    let conditions = next["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "{next}");
    assert_eq!(conditions[0]["name"], "scope_leg");
    assert_eq!(conditions[0]["type"], "request-leg");
    assert_eq!(conditions[0]["category"], "temporal");
    assert_eq!(conditions[0]["agent_actionable"], false);
    assert_eq!(conditions[0]["output"]["disposition"], "open");
    assert_eq!(conditions[0]["output"]["bound"], false);
    assert_eq!(
        std::fs::read(log_path(d, &id)).unwrap(),
        before,
        "evaluating the gate must not write to the request log"
    );

    // The answering root session attaches, runs, and terminates.
    init(d, "scope-1", "scope.md", SCOPE, &[]);
    run_ok(
        d,
        &["request", "attach", &id, "scope", "--session", "scope-1"],
    );
    let bound = run_ok(d, &["next", "deliver-1", "--no-cleanup"]);
    assert_eq!(
        bound["blocking_conditions"][0]["output"]["bound"], true,
        "{bound}"
    );
    let done = run_ok(d, &["next", "scope-1", "--with-data", r#"{"status":"ok"}"#]);
    assert_eq!(done["state"], "done", "{done}");

    // The promotion names the terminal it came from.
    let got = run_ok(d, &["request", "get", &id]);
    assert_eq!(got["legs"]["scope"]["result_source"], "promoted", "{got}");
    assert_eq!(got["legs"]["scope"]["result_final_state"], "done", "{got}");

    // The coordinator now routes down the scoped arm and copies the payload.
    let routed = run_ok(d, &["next", "deliver-1", "--no-cleanup"]);
    assert_eq!(routed["state"], "scoped", "{routed}");
    assert_eq!(ctx_get(d, "deliver-1", "pr"), "https://example.test/pr/9");
    assert_eq!(ctx_get(d, "deliver-1", "from"), "done");
    assert_eq!(ctx_get(d, "deliver-1", "tmpl"), "scope.md");
    assert_eq!(ctx_get(d, "deliver-1", "source"), "promoted");
}

#[test]
fn an_explicit_result_and_an_abandoned_leg_route_through_the_gate() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();

    // An explicit result carrying `declined` takes the declined arm.
    let id = create_request(d);
    init(
        d,
        "deliver-2",
        "deliver.md",
        DELIVER,
        &[&format!("REQ={id}")],
    );
    run_ok(
        d,
        &[
            "request",
            "resolve",
            &id,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"no","payload":{"outcome":"declined"}}"#,
        ],
    );
    let next = run_ok(d, &["next", "deliver-2", "--no-cleanup"]);
    assert_eq!(next["state"], "declined", "{next}");

    // An abandoned leg passes with disposition abandoned and routes to the
    // arm keyed on it.
    let id = create_request(d);
    init(
        d,
        "deliver-3",
        "deliver.md",
        DELIVER,
        &[&format!("REQ={id}")],
    );
    run_ok(
        d,
        &[
            "request",
            "abandon",
            &id,
            "scope",
            "--rationale",
            "superseded",
        ],
    );
    let next = run_ok(d, &["next", "deliver-3", "--no-cleanup"]);
    assert_eq!(next["state"], "abandoned", "{next}");
}

/// An overridable leg gate, for the override path.
const OVERRIDABLE: &str = r#"---
name: overridable-leg
version: "1.0"
initial_state: wait
variables:
  REQ:
    required: true
states:
  wait:
    gates:
      leg:
        type: request-leg
        request: "{{REQ}}"
        leg: scope
    transitions:
      - target: done
        when:
          gates.leg.disposition: resolved
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

#[test]
fn an_overridable_leg_gate_records_the_built_in_default() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let id = create_request(d);
    init(d, "ov", "ov.md", OVERRIDABLE, &[&format!("REQ={id}")]);

    let blocked = run_ok(d, &["next", "ov", "--no-cleanup"]);
    assert_eq!(blocked["action"], "gate_blocked", "{blocked}");
    assert_eq!(blocked["blocking_conditions"][0]["agent_actionable"], true);
    assert_eq!(blocked["blocking_conditions"][0]["category"], "temporal");

    run_ok(
        d,
        &[
            "overrides",
            "record",
            "ov",
            "--gate",
            "leg",
            "--rationale",
            "checked by hand",
        ],
    );
    let list = run_ok(d, &["overrides", "list", "ov"]);
    let applied = &list["overrides"]["items"][0]["override_applied"];
    assert_eq!(applied["disposition"], "resolved", "{list}");
    assert_eq!(applied["payload"], serde_json::json!({}), "{list}");

    let next = run_ok(d, &["next", "ov", "--no-cleanup"]);
    assert_eq!(next["state"], "done", "{next}");
}
