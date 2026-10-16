//! A passing check's warnings on the `koto next` response, and disputes that
//! move nothing.
//!
//! Every case runs the built `koto` against a template whose gates are
//! scripts written into the test's temporary directory. Each `koto` runs with
//! a cleared environment, so nothing from the developer's shell reaches a
//! script, and with its sessions (and so its run journal) under that
//! directory.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        Env { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn koto(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_koto"));
        cmd.env_clear()
            .current_dir(self.path())
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path())
            .env("KOTO_SESSIONS_BASE", self.path().join("sessions"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.koto().args(args).output().unwrap()
    }

    /// A script printing `findings` finding lines and exiting `exit`.
    fn script(&self, name: &str, findings: &[(&str, &str)], exit: i32) -> String {
        let mut body = String::new();
        for (rule, level) in findings {
            body.push_str(&format!(
                "echo '::koto-finding::{{\"rule_id\":\"{rule}\",\"level\":\"{level}\",\"message\":\"{rule} says so\"}}'\n"
            ));
        }
        body.push_str(&format!("exit {exit}\n"));
        let path: PathBuf = self.path().join(name);
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
    }

    fn init(&self, template: &str) {
        let tpl = self.path().join("template.md");
        std::fs::write(&tpl, template).unwrap();
        let out = self.run(&["init", "wf", "--template", tpl.to_str().unwrap()]);
        assert!(out.status.success(), "init: {}", describe(&out));
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("koto {args:?} should print JSON: {}", describe(&out)))
    }

    fn next(&self) -> Value {
        self.json(&["next", "wf"])
    }

    fn log_path(&self) -> PathBuf {
        self.path()
            .join("sessions")
            .join("wf")
            .join("koto-wf.state.jsonl")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap()
    }

    fn events(&self, event_type: &str) -> Vec<Value> {
        self.log()
            .lines()
            .skip(1)
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["type"] == event_type)
            .collect()
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path().join("sessions").join("_run_journal.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// `koto disputes record wf` with `args`.
    fn dispute(&self, args: &[&str]) -> Output {
        self.koto()
            .args(["disputes", "record", "wf"])
            .args(args)
            .output()
            .unwrap()
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `check` holds the gates in `gates` (name, command) and moves on to
/// `work`, which holds one more gate, `fmt`, and waits for a verdict.
fn template(gates: &[(&str, String)], fmt: &str) -> String {
    let mut check_gates = String::new();
    for (name, command) in gates {
        check_gates.push_str(&format!(
            "      {name}:\n        type: command\n        command: '{command}'\n"
        ));
    }
    format!(
        r#"---
name: lifecycle
version: "1.0"
initial_state: check
states:
  check:
    gates:
{check_gates}    transitions:
      - target: work
  work:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    gates:
      fmt:
        type: command
        command: '{fmt}'
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## check

Check.

## work

Work.

## done

Done.
"#
    )
}

#[test]
fn a_passing_gates_warning_reaches_the_response_as_an_advisory() {
    let env = Env::new();
    let lint = env.script("lint.sh", &[("W291", "warning"), ("I1", "info")], 0);
    let fmt = env.script("fmt.sh", &[], 0);
    env.init(&template(&[("lint", lint)], &fmt));
    let resp = env.next();
    assert_eq!(resp["state"], "work", "{resp}");
    let advisories = resp["advisories"]
        .as_array()
        .unwrap_or_else(|| panic!("expected advisories: {resp}"));
    assert_eq!(advisories.len(), 1, "{resp}");
    let a = &advisories[0];
    assert_eq!(a["state"], "check");
    assert_eq!(a["gate"], "lint");
    let findings = a["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "warnings only: {a}");
    assert_eq!(findings[0]["rule_id"], "W291");
    assert_eq!(findings[0]["level"], "warning");
    assert_eq!(findings[0]["message"], "W291 says so");
    assert_eq!(findings[0]["message_source"], "check");
    assert!(resp.get("advisories_truncated").is_none(), "{resp}");
    // Nothing blocks, and the warning isn't a blocking condition.
    assert!(resp
        .get("blocking_conditions")
        .and_then(Value::as_array)
        .is_none_or(|b| b.is_empty()));

    // The journal holds each finding, the info one included.
    let journal = env.journal();
    let found: Vec<(&str, &str)> = journal
        .iter()
        .filter(|r| r["kind"] == "check_finding" && r["koto.check"] == "lint")
        .map(|r| {
            (
                r["koto.rule.id"].as_str().unwrap(),
                r["koto.finding.level"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(found, vec![("W291", "warning"), ("I1", "info")]);
}

#[test]
fn a_response_with_no_passing_warnings_carries_no_advisories_key() {
    let env = Env::new();
    let lint = env.script("lint.sh", &[("I1", "info")], 0);
    let fmt = env.script("fmt.sh", &[], 0);
    env.init(&template(&[("lint", lint)], &fmt));
    let resp = env.next();
    assert_eq!(resp["state"], "work", "{resp}");
    assert!(resp.get("advisories").is_none(), "{resp}");
}

#[test]
fn a_failing_gates_warnings_stay_on_its_failure_not_in_advisories() {
    let env = Env::new();
    let lint = env.script("lint.sh", &[("E1", "error"), ("W1", "warning")], 1);
    let fmt = env.script("fmt.sh", &[], 0);
    env.init(&template(&[("lint", lint)], &fmt));
    let resp = env.next();
    assert_eq!(resp["state"], "check", "{resp}");
    assert!(resp.get("advisories").is_none(), "{resp}");
}

#[test]
fn exactly_one_hundred_warnings_are_not_marked_truncated() {
    let env = Env::new();
    let fifty: Vec<(String, &str)> = (0..50).map(|i| (format!("W{i:02}"), "warning")).collect();
    let fifty: Vec<(&str, &str)> = fifty.iter().map(|(r, l)| (r.as_str(), *l)).collect();
    let gates: Vec<(&str, String)> = ["a", "b"]
        .iter()
        .map(|g| (*g, env.script(&format!("{g}.sh"), &fifty, 0)))
        .collect();
    let fmt = env.script("fmt.sh", &[], 0);
    env.init(&template(&gates, &fmt));
    let resp = env.next();
    let total: usize = resp["advisories"]
        .as_array()
        .unwrap_or_else(|| panic!("expected advisories: {resp}"))
        .iter()
        .map(|a| a["findings"].as_array().unwrap().len())
        .sum();
    assert_eq!(total, 100, "{resp}");
    assert!(resp.get("advisories_truncated").is_none(), "{resp}");
}

#[test]
fn advisories_carry_at_most_one_hundred_findings_and_never_ride_to_or_status() {
    let env = Env::new();
    let forty: Vec<(String, &str)> = (0..40).map(|i| (format!("W{i:02}"), "warning")).collect();
    let forty: Vec<(&str, &str)> = forty.iter().map(|(r, l)| (r.as_str(), *l)).collect();
    let gates: Vec<(&str, String)> = ["a", "b", "c"]
        .iter()
        .map(|g| (*g, env.script(&format!("{g}.sh"), &forty, 0)))
        .collect();
    let fmt = env.script("fmt.sh", &[], 0);
    let tpl = template(&gates, &fmt);

    // `--to` evaluates the gates it guards on, and still carries none.
    env.init(&tpl);
    let to = env.json(&["next", "wf", "--to", "work"]);
    assert!(to.get("advisories").is_none(), "--to: {to}");

    let env = Env::new();
    let gates: Vec<(&str, String)> = ["a", "b", "c"]
        .iter()
        .map(|g| (*g, env.script(&format!("{g}.sh"), &forty, 0)))
        .collect();
    let fmt = env.script("fmt.sh", &[], 0);
    env.init(&template(&gates, &fmt));
    let resp = env.next();
    assert_eq!(resp["state"], "work", "{resp}");
    let advisories = resp["advisories"].as_array().unwrap();
    let total: usize = advisories
        .iter()
        .map(|a| a["findings"].as_array().unwrap().len())
        .sum();
    assert_eq!(total, 100, "{resp}");
    let gates: Vec<&str> = advisories
        .iter()
        .map(|a| a["gate"].as_str().unwrap())
        .collect();
    assert_eq!(gates, vec!["a", "b", "c"]);
    assert_eq!(advisories[2]["findings"].as_array().unwrap().len(), 20);
    assert_eq!(resp["advisories_truncated"], true, "{resp}");

    let status = env.json(&["status", "wf"]);
    assert!(status.get("advisories").is_none(), "status: {status}");
}

#[test]
fn a_passing_default_actions_warnings_are_an_advisory_under_action() {
    let env = Env::new();
    let act = env.script("act.sh", &[("W9", "warning")], 0);
    let tpl = format!(
        r#"---
name: acting
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: '{act}'
    transitions:
      - target: work
  work:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## run

Run it.

## work

Work.

## done

Done.
"#
    );
    env.init(&tpl);
    let resp = env.next();
    assert_eq!(resp["state"], "work", "{resp}");
    let a = &resp["advisories"][0];
    assert_eq!(a["state"], "run", "{resp}");
    assert_eq!(a["gate"], "__action__");
    assert_eq!(a["findings"][0]["rule_id"], "W9");
}

// ----- Disputes -----

/// A session in `work`, having evaluated `lint` in `check` and `fmt` in
/// `work`, both passing with a warning.
fn disputed_session() -> Env {
    let env = Env::new();
    let lint = env.script("lint.sh", &[("W291", "warning")], 0);
    let fmt = env.script("fmt.sh", &[("F1", "warning")], 0);
    env.init(&template(&[("lint", lint)], &fmt));
    let resp = env.next();
    assert_eq!(resp["state"], "work", "{resp}");
    env
}

#[test]
fn a_dispute_in_the_current_state_and_one_in_an_earlier_state_each_append_one_event() {
    let env = disputed_session();
    let out = env.dispute(&[
        "--gate",
        "fmt",
        "--rule",
        "F1",
        "--reason",
        "the formatter disagrees with itself",
    ]);
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed, serde_json::json!({"status": "recorded"}));

    let out = env.dispute(&[
        "--gate",
        "lint",
        "--rule",
        "W291",
        "--reason",
        "generated file",
        "--state",
        "check",
        "--by",
        "person",
    ]);
    assert!(out.status.success(), "{}", describe(&out));

    let disputes = env.events("finding_disputed");
    assert_eq!(disputes.len(), 2);
    assert_eq!(
        disputes[0]["payload"],
        serde_json::json!({
            "state": "work",
            "gate": "fmt",
            "rule_id": "F1",
            "reason": "the formatter disagrees with itself",
            "source": "agent",
        })
    );
    assert_eq!(
        disputes[1]["payload"],
        serde_json::json!({
            "state": "check",
            "gate": "lint",
            "rule_id": "W291",
            "reason": "generated file",
            "source": "person",
        })
    );

    let journal = env.journal();
    let records: Vec<&Value> = journal
        .iter()
        .filter(|r| r["kind"] == "check_disputed")
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["koto.state"], "work");
    assert_eq!(records[0]["koto.check"], "fmt");
    assert_eq!(records[0]["koto.rule.id"], "F1");
    assert_eq!(records[0]["koto.dispute.source"], "agent");
    assert_eq!(records[1]["koto.state"], "check");
    assert_eq!(records[1]["koto.dispute.source"], "person");
    let raw =
        std::fs::read_to_string(env.path().join("sessions").join("_run_journal.jsonl")).unwrap();
    assert!(!raw.contains("generated file"), "{raw}");
    assert!(!raw.contains("disagrees"), "{raw}");
}

#[test]
fn a_dispute_changes_nothing_the_next_response_shows() {
    let env = disputed_session();
    // A second call settles what a repeated call prints (details are
    // delivered once).
    let before = env.next();
    let out = env.dispute(&["--gate", "fmt", "--rule", "F1", "--reason", "wrong"]);
    assert!(out.status.success(), "{}", describe(&out));
    let after = env.next();
    assert_eq!(after, before);
    assert_eq!(after["state"], "work");
}

#[test]
fn a_dispute_reason_is_redacted_before_it_is_stored() {
    let env = disputed_session();
    let token = "ghp_0123456789abcdefSECRET";
    let out = env
        .koto()
        .env("GH_TOKEN", token)
        .args([
            "disputes",
            "record",
            "wf",
            "--gate",
            "fmt",
            "--rule",
            "F1",
            "--reason",
            &format!("the check printed {token} and called it a finding"),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let reason = env.events("finding_disputed")[0]["payload"]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        reason,
        "the check printed [REDACTED:GH_TOKEN] and called it a finding"
    );
    assert!(!env.log().contains(token));
}

#[test]
fn refused_disputes_exit_two_with_a_code_and_append_nothing() {
    let env = disputed_session();
    let long_rule = "r".repeat(129);
    let long_reason = "x".repeat(1001);
    let cases: Vec<(Vec<&str>, Option<&str>)> = vec![
        (
            vec!["--gate", "nosuch", "--rule", "R", "--reason", "x"],
            Some("gate_not_evaluated"),
        ),
        (
            vec!["--gate", "lint", "--rule", "R", "--reason", "x"],
            Some("gate_not_evaluated"),
        ),
        (
            vec![
                "--gate", "fmt", "--rule", "R", "--reason", "x", "--state", "done",
            ],
            Some("gate_not_evaluated"),
        ),
        (
            vec!["--gate", "fmt", "--rule", "", "--reason", "x"],
            Some("invalid_rule"),
        ),
        (
            vec!["--gate", "fmt", "--rule", &long_rule, "--reason", "x"],
            Some("invalid_rule"),
        ),
        (
            vec!["--gate", "fmt", "--rule", "R", "--reason", ""],
            Some("invalid_reason"),
        ),
        (
            vec!["--gate", "fmt", "--rule", "R", "--reason", &long_reason],
            Some("invalid_reason"),
        ),
        (
            vec![
                "--gate", "fmt", "--rule", "R", "--reason", "x", "--by", "robot",
            ],
            None,
        ),
    ];
    let log = env.log();
    for (args, code) in cases {
        let out = env.dispute(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", describe(&out));
        if let Some(code) = code {
            let body: Value = serde_json::from_slice(&out.stdout)
                .unwrap_or_else(|_| panic!("{args:?}: {}", describe(&out)));
            assert_eq!(body["error"]["code"], code, "{args:?}: {body}");
            assert_eq!(body["command"], "disputes record");
        }
        assert_eq!(env.log(), log, "{args:?} appended to the log");
    }
    // A rule id and reason exactly at their bounds are accepted.
    let rule = "r".repeat(128);
    let reason = "x".repeat(1000);
    let out = env.dispute(&["--gate", "fmt", "--rule", &rule, "--reason", &reason]);
    assert!(out.status.success(), "{}", describe(&out));
}

/// A polling gate that has only answered pending judged nothing, so a
/// dispute of it is refused; once it settles (here, failing), the same
/// dispute is recorded.
#[test]
fn a_dispute_of_a_gate_only_ever_pending_is_refused() {
    let env = Env::new();
    let ci = env.script("ci.sh", &[("W1", "warning")], 75);
    let tpl = format!(
        r#"---
name: polling
version: "1.0"
initial_state: ci
states:
  ci:
    accepts:
      again:
        type: enum
        values: ["yes"]
    gates:
      ci:
        type: command
        command: '{ci}'
        poll:
          interval_secs: 1
          timeout_secs: 600
    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
      - target: ci
        when:
          gates.ci.exit_code: 75
          again: "yes"
  done:
    terminal: true
---

## ci

Wait for CI.

## done

Done.
"#
    );
    env.init(&tpl);
    env.next();
    let evals = env.events("gate_evaluated");
    assert!(!evals.is_empty());
    assert!(
        evals.iter().all(|e| e["payload"]["outcome"] == "pending"),
        "{evals:?}"
    );

    let log = env.log();
    let args = ["--gate", "ci", "--rule", "W1", "--reason", "flaky runner"];
    let out = env.dispute(&args);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "gate_not_evaluated", "{body}");
    assert_eq!(env.log(), log, "the refused dispute appended nothing");

    // The gate now fails: a settled evaluation, which can be disputed.
    env.script("ci.sh", &[("W1", "warning")], 1);
    env.next();
    let out = env.dispute(&args);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(env.events("finding_disputed").len(), 1);
}

#[test]
fn a_dispute_of_a_missing_session_exits_one() {
    let env = Env::new();
    let out = env.dispute(&["--gate", "lint", "--rule", "R", "--reason", "x"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!env.path().join("sessions").join("wf").exists());
}
