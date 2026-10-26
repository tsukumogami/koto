//! Check telemetry end to end: a session driven through the built `koto`
//! writes `koto.check.reason`, `koto.check.not_ready` and
//! `koto.check.pre_directive` on its `check_evaluated` records,
//! `koto.finding.source` on its `check_finding` records, one
//! `evidence_field` record per enum or boolean field it submits, and an
//! `instructions_delivered` event for every directive `koto next` prints.
//! None of it changes what `koto next` prints or how the session routes.
//!
//! Every test runs koto with `HOME` and `KOTO_SESSIONS_BASE` in its own
//! temporary directory and `CLAUDE_CODE_SESSION_ID` cleared, so the store
//! and the journal (which a redirected store keeps inside its base) are the
//! test's own.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tempfile::TempDir;

/// `check` waits on a command gate whose script exits with the code in
/// `code.txt` and prints `out.txt`; exit codes 3 and 4 mean "not ready".
/// `answer` takes an enum, a boolean and a free-text field.
const TEMPLATE: &str = r#"---
name: check-telemetry
version: "1.0"
initial_state: check
states:
  check:
    gates:
      g:
        type: command
        command: ./g.sh
        not_ready_exit_codes: [3, 4]
    transitions:
      - target: answer
        when:
          gates.g.exit_code: 0
  answer:
    accepts:
      verdict:
        type: enum
        values: ["pass", "fail"]
        required: true
      approved:
        type: boolean
        required: false
      note:
        type: string
        required: false
    transitions:
      - target: done
        when:
          verdict: pass
      - target: check
        when:
          verdict: fail
  done:
    terminal: true
---

## check

Make the check pass.

## answer

Say whether it passed.

## done

Done.
"#;

const SCRIPT: &str =
    "#!/bin/sh\ncat out.txt 2>/dev/null\nexit \"$(cat code.txt 2>/dev/null || echo 0)\"\n";

struct Env {
    _tmp: TempDir,
    dir: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("t.md"), TEMPLATE).unwrap();
        let script = dir.join("g.sh");
        std::fs::write(&script, SCRIPT).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        Env { _tmp: tmp, dir }
    }

    fn cmd(&self) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        cmd.env_remove("KOTO_HOME");
        cmd.current_dir(&self.dir);
        cmd.env("KOTO_SESSIONS_BASE", self.dir.join("sessions"));
        cmd.env("HOME", &self.dir);
        cmd
    }

    /// Run koto and return its last stdout line as JSON.
    fn koto(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {}: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap();
        serde_json::from_str(last).unwrap()
    }

    /// The gate's next run exits with `code` and prints `out`.
    fn set_check(&self, code: i32, out: &str) {
        std::fs::write(self.dir.join("code.txt"), code.to_string()).unwrap();
        std::fs::write(self.dir.join("out.txt"), out).unwrap();
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("sessions").join("_run_journal.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn raw_journal(&self) -> String {
        std::fs::read_to_string(self.dir.join("sessions").join("_run_journal.jsonl")).unwrap()
    }

    fn of_kind(&self, kind: &str) -> Vec<Value> {
        self.journal()
            .into_iter()
            .filter(|r| r["kind"] == kind)
            .collect()
    }

    fn events(&self) -> Vec<Value> {
        let path = self
            .dir
            .join("sessions")
            .join("wf")
            .join("koto-wf.state.jsonl");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

#[test]
fn check_records_carry_reason_readiness_directive_timing_and_finding_source() {
    let env = Env::new();
    env.koto(&["init", "wf", "--template", "t.md"]);

    // The first tick after init: no directive shown yet. Exit 2 with the
    // marker at a line's start and no finding of its own.
    env.set_check(2, "::koto-not-ready::\nwaiting on the input\n");
    let first = env.koto(&["next", "wf"]);
    assert_eq!(first["action"], "gate_blocked");

    // The directive was printed by that tick: the next evaluation of the
    // visit isn't pre-directive. Exit 4 is a listed "not ready" code, and
    // an error finding of the check's own makes it `rule_failed`.
    env.set_check(
        4,
        "::koto-finding::{\"rule_id\":\"pr-body/one-separator\",\"level\":\"error\",\"message\":\"planted-message-7a2c\"}\n",
    );
    let second = env.koto(&["next", "wf"]);
    assert_eq!(second["action"], "gate_blocked");

    // Exit 1 with the marker indented: not ready is false.
    env.set_check(1, "  ::koto-not-ready::\n");
    env.koto(&["next", "wf"]);

    let evaluated = env.of_kind("check_evaluated");
    assert_eq!(evaluated.len(), 3, "{evaluated:#?}");
    let col = |key: &str| -> Vec<Value> { evaluated.iter().map(|r| r[key].clone()).collect() };
    assert_eq!(
        col("koto.check.reason"),
        vec!["undecided", "rule_failed", "no_finding"]
    );
    assert_eq!(col("koto.check.not_ready"), vec![true, true, false]);
    assert_eq!(col("koto.check.pre_directive"), vec![true, false, false]);

    let findings = env.of_kind("check_finding");
    let source_of = |rule: &str| -> Vec<Value> {
        findings
            .iter()
            .filter(|r| r["koto.rule.id"] == rule)
            .map(|r| r["koto.finding.source"].clone())
            .collect()
    };
    assert_eq!(source_of("g"), vec!["fallback", "fallback"]);
    assert_eq!(source_of("pr-body/one-separator"), vec!["check"]);
    assert!(!env.raw_journal().contains("planted-message-7a2c"));
}

#[test]
fn the_marker_and_the_listed_codes_change_no_response() {
    // Two sessions, one whose check prints the marker and exits with a
    // listed code, one that does neither: the responses differ in nothing.
    let marked = Env::new();
    let plain = Env::new();
    for env in [&marked, &plain] {
        env.koto(&["init", "wf", "--template", "t.md"]);
    }
    marked.set_check(3, "::koto-not-ready::\nwaiting\n");
    plain.set_check(1, "waiting\n");
    let a = marked.koto(&["next", "wf"]);
    let b = plain.koto(&["next", "wf"]);
    assert_eq!(a["action"], b["action"]);
    assert_eq!(a["state"], b["state"]);
    let routing = |v: &Value| v["blocking_conditions"][0]["status"].clone();
    assert_eq!(routing(&a), routing(&b));
    let fallback =
        |v: &Value| v["blocking_conditions"][0]["failure"]["findings"][0]["rule_id"].clone();
    assert_eq!(fallback(&a), "g");
    assert_eq!(fallback(&a), fallback(&b));
    assert_eq!(
        marked.of_kind("check_evaluated")[0]["koto.check.not_ready"],
        true
    );
    assert_eq!(
        plain.of_kind("check_evaluated")[0]["koto.check.not_ready"],
        false
    );
}

#[test]
fn evidence_relays_enum_and_boolean_fields_and_never_free_text() {
    let env = Env::new();
    env.koto(&["init", "wf", "--template", "t.md"]);
    env.set_check(0, "");
    let at_answer = env.koto(&["next", "wf"]);
    assert_eq!(at_answer["state"], "answer");

    let done = env.koto(&[
        "next",
        "wf",
        "--no-cleanup",
        "--with-data",
        r#"{"verdict":"pass","approved":true,"note":"planted-note-7a2c https://example.invalid/x"}"#,
    ]);
    assert_eq!(done["state"], "done");

    let fields: Vec<(String, String)> = env
        .of_kind("evidence_field")
        .iter()
        .map(|r| {
            assert_eq!(r["koto.state"], "answer");
            (
                r["koto.evidence.field"].as_str().unwrap().to_string(),
                r["koto.evidence.value"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        fields,
        vec![
            ("approved".to_string(), "true".to_string()),
            ("verdict".to_string(), "pass".to_string()),
        ]
    );
    let raw = env.raw_journal();
    assert!(!raw.contains("planted-note-7a2c"), "{raw}");
    assert!(!raw.contains("example.invalid"), "{raw}");

    // The session log keeps the map beside the submitted fields.
    let submitted = env
        .events()
        .into_iter()
        .find(|e| e["type"] == "evidence_submitted")
        .unwrap();
    assert_eq!(
        submitted["payload"]["exported_fields"],
        serde_json::json!({"approved": "true", "verdict": "pass"})
    );
}

#[test]
fn every_printed_directive_records_instructions_delivered_with_its_details_flag() {
    let env = Env::new();
    env.koto(&["init", "wf", "--template", "t.md"]);
    env.set_check(1, "");
    env.koto(&["next", "wf"]);
    env.koto(&["next", "wf"]);
    let delivered: Vec<Value> = env
        .events()
        .into_iter()
        .filter(|e| e["type"] == "instructions_delivered")
        .collect();
    assert_eq!(delivered.len(), 2, "{delivered:#?}");
    for d in &delivered {
        assert_eq!(d["payload"]["state"], "check");
        // The template declares no details, so none was carried.
        assert_eq!(d["payload"]["details_included"], false);
    }
}
