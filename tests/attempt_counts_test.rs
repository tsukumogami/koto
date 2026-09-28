//! Attempt stamps, per-check rule counts, findings and captured output on
//! the check events, and the `attempts` object on a blocked `koto next`
//! response (DESIGN-koto-failure-reporting.md, Decision 2; PRD-koto-failure-reporting.md, R17-R20).
//!
//! Every case runs the built `koto` against a template whose checks are
//! scripts written into the test's temporary directory, then reads the
//! session's JSONL log the way an exporter would. Each `koto` runs with a
//! cleared environment, so nothing from the developer's shell reaches a
//! script.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const E501: &str =
    r#"echo '::koto-finding::{"rule_id":"E501","level":"error","message":"line too long"}'"#;
const F401: &str =
    r#"echo '::koto-finding::{"rule_id":"F401","level":"error","message":"unused import"}'"#;
const W291: &str = r#"echo '::koto-finding::{"rule_id":"W291","level":"warning","message":"trailing whitespace"}'"#;

struct Env {
    dir: tempfile::TempDir,
    vars: Vec<(String, String)>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        Env {
            dir,
            vars: Vec::new(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn set(&mut self, name: &str, value: &str) {
        self.vars.push((name.to_string(), value.to_string()));
    }

    fn koto(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_koto"));
        cmd.env_clear()
            .current_dir(self.path())
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path())
            .env("KOTO_SESSIONS_BASE", self.path().join("sessions"));
        for (n, v) in &self.vars {
            cmd.env(n, v);
        }
        cmd
    }

    /// Write `body` as a script and return the command that runs it.
    fn script(&self, name: &str, body: &str) -> String {
        let path: PathBuf = self.path().join(name);
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
    }

    fn touch(&self, name: &str) {
        std::fs::write(self.path().join(name), "").unwrap();
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.path().join(name)).unwrap();
    }

    fn init(&self, template: &str) {
        let tpl = self.path().join("template.md");
        std::fs::write(&tpl, template).unwrap();
        let out = self
            .koto()
            .args(["init", "wf", "--template", tpl.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "init: {}", describe(&out));
    }

    fn run(&self, args: &[&str]) -> Output {
        self.koto().args(args).output().unwrap()
    }

    fn next_with(&self, extra: &[&str]) -> Value {
        let out = self.run(&[&["next", "wf", "--no-cleanup"], extra].concat());
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("next should print JSON: {}", describe(&out)))
    }

    fn next(&self) -> Value {
        self.next_with(&[])
    }

    fn submit(&self, verdict: &str) -> Value {
        self.next_with(&["--with-data", &json!({ "verdict": verdict }).to_string()])
    }

    fn override_gate(&self, gate: &str) {
        let out = self.run(&[
            "overrides",
            "record",
            "wf",
            "--gate",
            gate,
            "--rationale",
            "checked by hand",
        ]);
        assert!(out.status.success(), "override: {}", describe(&out));
    }

    fn log_path(&self) -> PathBuf {
        self.path()
            .join("sessions")
            .join("wf")
            .join("koto-wf.state.jsonl")
    }

    /// Every event in the log, header skipped.
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log_path())
            .unwrap()
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    /// The payloads of `gate_evaluated` events for `gate` on `state`.
    fn gate_events(&self, state: &str, gate: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| {
                e["type"] == "gate_evaluated"
                    && e["payload"]["state"] == state
                    && e["payload"]["gate"] == gate
            })
            .map(|e| e["payload"].clone())
            .collect()
    }

    /// The payloads of `default_action_executed` events on `state`.
    fn action_events(&self, state: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == "default_action_executed" && e["payload"]["state"] == state)
            .map(|e| e["payload"].clone())
            .collect()
    }

    /// Every check event, of either kind.
    fn check_events(&self) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == "gate_evaluated" || e["type"] == "default_action_executed")
            .map(|e| e["payload"].clone())
            .collect()
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

/// `(attempt, visit_attempt)` of each event.
fn stamps(events: &[Value]) -> Vec<(u64, u64)> {
    events
        .iter()
        .map(|e| {
            (
                e["attempt"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("no attempt: {e}")),
                e["visit_attempt"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("no visit_attempt: {e}")),
            )
        })
        .collect()
}

fn count(visit: u64, session: u64) -> Value {
    json!({ "visit": visit, "session": session })
}

/// One state, `check`, gated by command gates `(name, command)`, moving on
/// to a terminal state when they pass.
fn gates_template(gates: &[(&str, &str)]) -> String {
    let gates: String = gates
        .iter()
        .map(|(name, cmd)| {
            format!("      {name}:\n        type: command\n        command: '{cmd}'\n")
        })
        .collect();
    format!(
        r#"---
name: gated
version: "1.0"
initial_state: check
states:
  check:
    gates:
{gates}    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check.

## done

Done.
"#
    )
}

/// `check`, gated by `lint`, which the agent can loop on (`again`), leave
/// for `other` (`leave`) or finish (`finish`); `other` returns (`back`).
fn loop_template(cmd: &str) -> String {
    format!(
        r#"---
name: looping
version: "1.0"
initial_state: check
states:
  check:
    accepts:
      verdict:
        type: enum
        required: true
        values: [again, leave, finish]
    gates:
      lint:
        type: command
        command: '{cmd}'
    transitions:
      - target: check
        when:
          verdict: again
      - target: other
        when:
          verdict: leave
      - target: done
        when:
          verdict: finish
  other:
    accepts:
      verdict:
        type: enum
        required: true
        values: [back]
    transitions:
      - target: check
        when:
          verdict: back
  done:
    terminal: true
---

## check

Check.

## other

Elsewhere.

## done

Done.
"#
    )
}

/// One state running `command` as its `default_action`, with `extra` lines
/// under the action.
fn action_template(command: &str, extra: &str) -> String {
    format!(
        r#"---
name: acting
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: '{command}'
{extra}    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run it.

## done

Done.
"#
    )
}

// ---------------------------------------------------------------------------
// the attempt stamp
// ---------------------------------------------------------------------------

#[test]
fn three_failures_then_a_pass_number_the_attempts_1_to_4() {
    let env = Env::new();
    let cmd = env.script("lint.sh", "test -f ok\n");
    env.init(&gates_template(&[("lint", &cmd)]));
    for _ in 0..3 {
        assert_eq!(env.next()["action"], "gate_blocked");
    }
    env.touch("ok");
    let resp = env.next();
    assert_eq!(resp["state"], "done", "{resp}");
    assert!(resp.get("attempts").is_none(), "a passing response: {resp}");
    assert_eq!(
        stamps(&env.gate_events("check", "lint")),
        vec![(1, 1), (2, 2), (3, 3), (4, 4)]
    );
}

#[test]
fn the_visit_count_runs_through_evidence_and_self_transitions_and_restarts_on_arrival() {
    let env = Env::new();
    let cmd = env.script("lint.sh", &format!("{E501}\nexit 1\n"));
    env.init(&loop_template(&cmd));

    env.next(); // attempt 1
                // Evidence is recorded, the gate re-checked (2), and the self-transition
                // re-enters the state in the same invocation (3).
    let resp = env.submit("again");
    assert_eq!(resp["state"], "check", "{resp}");
    env.submit("leave"); // attempt 4, then off to `other`
    let out = env.run(&["rewind", "wf"]);
    assert!(out.status.success(), "rewind: {}", describe(&out));
    env.next(); // attempt 5, first of the visit the rewind opened
    env.submit("leave"); // 6
    env.submit("back"); // 7, first after arriving from `other`

    let events = env.gate_events("check", "lint");
    assert_eq!(
        stamps(&events),
        vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 1), (6, 2), (7, 1)]
    );
    // Three laps around the self-transition count three in the visit and
    // the session.
    assert_eq!(events[2]["rule_counts"]["E501"], count(3, 3));
    assert_eq!(events[6]["rule_counts"]["E501"], count(1, 7));
}

#[test]
fn an_override_keeps_the_visit_running_and_overridden_gates_record_nothing() {
    let env = Env::new();
    let lint = env.script("lint.sh", "exit 1\n");
    let style = env.script("style.sh", "exit 1\n");
    env.init(&gates_template(&[("lint", &lint), ("style", &style)]));

    env.next();
    // Two failing gates on one entry share one stamp.
    assert_eq!(stamps(&env.gate_events("check", "lint")), vec![(1, 1)]);
    assert_eq!(stamps(&env.gate_events("check", "style")), vec![(1, 1)]);

    env.override_gate("lint");
    let resp = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(stamps(&env.gate_events("check", "lint")), vec![(1, 1)]);
    assert_eq!(
        stamps(&env.gate_events("check", "style")),
        vec![(1, 1), (2, 2)]
    );
    assert_eq!(resp["attempts"]["visit"], 2);
    assert_eq!(resp["attempts"]["session"], 2);

    // With every gate overridden nothing is evaluated, so nothing records.
    env.override_gate("style");
    let resp = env.next();
    assert_eq!(resp["state"], "done", "{resp}");
    assert_eq!(env.check_events().len(), 3);
}

#[test]
fn a_state_left_and_re_entered_in_one_invocation_gets_two_attempts() {
    let env = Env::new();
    let cmd = env.script("ok.sh", "exit 0\n");
    env.init(&format!(
        r#"---
name: cycling
version: "1.0"
initial_state: a
states:
  a:
    gates:
      g:
        type: command
        command: '{cmd}'
    transitions:
      - target: b
  b:
    transitions:
      - target: a
  done:
    terminal: true
---

## a

A.

## b

B.

## done

Done.
"#
    ));
    env.next();
    // Arriving from `b` opens a new visit; the session count keeps rising.
    assert_eq!(stamps(&env.gate_events("a", "g")), vec![(1, 1), (2, 1)]);
}

#[test]
fn a_polling_loop_that_re_evaluates_five_times_is_one_attempt() {
    let env = Env::new();
    let bump = env.script(
        "bump.sh",
        "n=$(cat count 2>/dev/null || echo 0)\necho $((n + 1)) > count\n",
    );
    let ready = env.script("ready.sh", "test \"$(cat count)\" -ge 5\n");
    let template = format!(
        r#"---
name: polling
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: '{bump}'
      polling:
        interval_secs: 1
        timeout_secs: 60
    gates:
      ready:
        type: command
        command: '{ready}'
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Poll.

## done

Done.
"#
    );
    env.init(&template);
    let resp = env.next();
    assert_eq!(resp["state"], "done", "{resp}");
    assert_eq!(
        std::fs::read_to_string(env.path().join("count"))
            .unwrap()
            .trim(),
        "5"
    );
    let events = env.check_events();
    assert_eq!(
        events.len(),
        2,
        "one action event and one gate event: {events:?}"
    );
    assert_eq!(stamps(&events), vec![(1, 1), (1, 1)]);
}

#[test]
fn a_state_without_checks_and_koto_next_to_record_no_attempt() {
    let env = Env::new();
    let cmd = env.script("lint.sh", "exit 1\n");
    env.init(&loop_template(&cmd));
    // `--to` evaluates nothing and records no check event.
    let resp = env.next_with(&["--to", "other"]);
    assert_eq!(resp["state"], "other", "{resp}");
    // `other` has no checks: the tick asks for evidence and records none.
    let resp = env.next();
    assert_eq!(resp["action"], "evidence_required", "{resp}");
    assert!(resp.get("attempts").is_none(), "{resp}");
    assert!(env.check_events().is_empty(), "{:?}", env.check_events());
}

// ---------------------------------------------------------------------------
// rule counts
// ---------------------------------------------------------------------------

#[test]
fn rule_counts_are_kept_per_check_and_count_a_rule_once_per_attempt() {
    let env = Env::new();
    let body = format!("{E501}\n{E501}\n{W291}\nexit 1\n");
    let a = env.script("a.sh", &body);
    let b = env.script("b.sh", &body);
    env.init(&gates_template(&[("a", &a), ("b", &b)]));
    env.next();
    let resp = env.next();
    for gate in ["a", "b"] {
        let events = env.gate_events("check", gate);
        assert_eq!(events[0]["rule_counts"], json!({ "E501": count(1, 1) }));
        assert_eq!(events[1]["rule_counts"], json!({ "E501": count(2, 2) }));
        assert_eq!(
            resp["attempts"]["rules"][gate],
            json!({ "E501": count(2, 2) })
        );
    }
}

#[test]
fn the_same_rule_in_two_states_counts_separately() {
    let env = Env::new();
    let cmd = env.script("lint.sh", &format!("{E501}\nexit 1\n"));
    env.init(&format!(
        r#"---
name: two
version: "1.0"
initial_state: first
states:
  first:
    accepts:
      verdict:
        type: enum
        required: true
        values: [leave]
    gates:
      lint:
        type: command
        command: '{cmd}'
    transitions:
      - target: second
        when:
          verdict: leave
  second:
    gates:
      lint:
        type: command
        command: '{cmd}'
    transitions:
      - target: done
  done:
    terminal: true
---

## first

First.

## second

Second.

## done

Done.
"#
    ));
    env.next();
    let resp = env.next_with(&["--with-data", r#"{"verdict":"leave"}"#]);
    assert_eq!(resp["state"], "second", "{resp}");
    assert_eq!(
        env.gate_events("first", "lint")[1]["rule_counts"]["E501"],
        count(2, 2)
    );
    assert_eq!(
        env.gate_events("second", "lint")[0]["rule_counts"]["E501"],
        count(1, 1)
    );
    assert_eq!(
        resp["attempts"]["rules"],
        json!({ "lint": { "E501": count(1, 1) } })
    );
}

#[test]
fn leaving_and_returning_twice_counts_one_in_the_visit_and_three_in_the_session() {
    let env = Env::new();
    let cmd = env.script("lint.sh", &format!("{E501}\nexit 1\n"));
    env.init(&loop_template(&cmd));
    env.next();
    for _ in 0..2 {
        env.next_with(&["--to", "other"]);
        env.submit("back");
    }
    let events = env.gate_events("check", "lint");
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["rule_counts"]["E501"], count(1, 3));
    let resp = env.next_with(&["--to", "other"]);
    assert!(resp.get("attempts").is_none(), "{resp}");
}

#[test]
fn rule_counts_come_from_every_finding_and_hold_at_most_50_keys() {
    // Sixty distinct rules at `error`: 50 keys, flagged.
    let env = Env::new();
    let many: String = (0..60)
        .map(|i| {
            format!(
                "echo '::koto-finding::{{\"rule_id\":\"R{i:02}\",\"level\":\"error\",\"message\":\"m\"}}'\n"
            )
        })
        .collect();
    let cmd = env.script("lint.sh", &format!("{many}exit 1\n"));
    env.init(&gates_template(&[("lint", &cmd)]));
    env.next();
    let event = &env.gate_events("check", "lint")[0];
    assert_eq!(event["rule_counts"].as_object().unwrap().len(), 50);
    assert_eq!(event["rule_counts_truncated"], true);
    assert_eq!(event["rule_counts"]["R00"], count(1, 1));
    assert!(event["rule_counts"].get("R50").is_none());

    // An error printed after 55 warnings leads the capped `findings`, and
    // is counted.
    let env = Env::new();
    let warnings: String = (0..55).map(|_| format!("{W291}\n")).collect();
    let cmd = env.script(
        "lint.sh",
        &format!(
            "{warnings}echo '::koto-finding::{{\"rule_id\":\"E999\",\"level\":\"error\",\"message\":\"late\"}}'\nexit 1\n"
        ),
    );
    env.init(&gates_template(&[("lint", &cmd)]));
    env.next();
    let event = &env.gate_events("check", "lint")[0];
    let findings = event["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 50);
    assert_eq!(findings[0]["rule_id"], "E999");
    assert!(findings[1..].iter().all(|f| f["rule_id"] == "W291"));
    assert_eq!(event["findings_truncated"], true);
    assert_eq!(event["rule_counts"], json!({ "E999": count(1, 1) }));
    assert!(event.get("rule_counts_truncated").is_none());
}

// ---------------------------------------------------------------------------
// findings on the log
// ---------------------------------------------------------------------------

#[test]
fn a_passing_check_logs_its_findings_and_raises_no_count() {
    let env = Env::new();
    let cmd = env.script("lint.sh", &format!("{E501}\n{W291}\ntest ! -f fail\n"));
    env.init(&loop_template(&cmd));
    let resp = env.submit("leave");
    assert_eq!(resp["state"], "other", "the passing gate advances: {resp}");
    assert!(resp.get("attempts").is_none(), "{resp}");
    let passed = &env.gate_events("check", "lint")[0];
    assert_eq!(passed["outcome"], "passed");
    let findings = passed["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2);
    assert!(findings.iter().all(|f| f["message_source"] == "check"));
    assert_eq!(findings[0]["level"], "error");
    assert_eq!(findings[1]["level"], "warning");
    for absent in ["rule_counts", "stdout", "stderr", "findings_truncated"] {
        assert!(passed.get(absent).is_none(), "{absent}: {passed}");
    }

    env.touch("fail");
    let resp = env.submit("back");
    assert_eq!(resp["state"], "check", "{resp}");
    let failed = &env.gate_events("check", "lint")[1];
    assert_eq!(failed["rule_counts"], json!({ "E501": count(1, 1) }));
}

#[test]
fn a_default_action_that_passes_logs_its_warning() {
    let env = Env::new();
    let cmd = env.script("act.sh", &format!("{W291}\nexit 0\n"));
    env.init(&action_template(&cmd, ""));
    let resp = env.next();
    assert_eq!(resp["state"], "done", "{resp}");
    let event = &env.action_events("run")[0];
    assert_eq!(stamps(std::slice::from_ref(event)), vec![(1, 1)]);
    assert_eq!(event["findings"][0]["rule_id"], "W291");
    assert_eq!(event["findings"][0]["effect_landed"], true);
    assert!(event.get("rule_counts").is_none(), "{event}");
    assert!(event["duration_ms"].is_u64(), "{event}");
}

#[test]
fn default_action_executed_carries_the_stamp_findings_counts_and_duration() {
    let env = Env::new();
    let cmd = env.script("act.sh", "echo boom\nexit 3\n");
    env.init(&action_template(&cmd, ""));
    env.next();
    let resp = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    let events = env.action_events("run");
    assert_eq!(stamps(&events), vec![(1, 1), (2, 2)]);
    let last = &events[1];
    // The existing fields keep their meaning.
    assert_eq!(last["exit_code"], 3);
    assert_eq!(last["stdout"], "boom\n");
    assert_eq!(last["stderr"], "");
    assert_eq!(last["truncated"], false);
    assert_eq!(last["findings"][0]["rule_id"], "__action__");
    assert_eq!(last["findings"][0]["message"], "boom");
    assert_eq!(last["rule_counts"], json!({ "__action__": count(2, 2) }));
    assert!(last["duration_ms"].is_u64());
    assert_eq!(
        resp["attempts"],
        json!({ "visit": 2, "session": 2, "rules": { "__action__": { "__action__": count(2, 2) } } })
    );
}

#[test]
fn a_capture_failure_s_finding_reaches_default_action_executed() {
    let env = Env::new();
    let cmd = env.script("act.sh", "exit 0\n");
    env.init(&action_template(&cmd, "      capture_stdout_as: BRANCH\n"));
    let resp = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    let event = &env.action_events("run")[0];
    assert_eq!(event["exit_code"], 0);
    let finding = &event["findings"][0];
    assert_eq!(finding["rule_id"], "__action__");
    assert_eq!(finding["effect_landed"], false);
    assert!(
        finding["message"]
            .as_str()
            .unwrap()
            .contains("could not be delivered as BRANCH"),
        "{finding}"
    );
    assert_eq!(event["rule_counts"], json!({ "__action__": count(1, 1) }));
}

#[test]
fn default_action_executed_still_comes_before_the_capture_it_delivers() {
    let env = Env::new();
    let cmd = env.script("act.sh", "echo main\n");
    env.init(&action_template(&cmd, "      capture_stdout_as: BRANCH\n"));
    env.next();
    let kinds: Vec<String> = env
        .events()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .filter(|t| t == "default_action_executed" || t == "variable_captured")
        .collect();
    assert_eq!(kinds, vec!["default_action_executed", "variable_captured"]);
}

#[test]
fn a_working_dir_rejection_records_no_attempt() {
    let env = Env::new();
    env.init(&action_template("true", "      working_dir: ../outside\n"));
    let resp = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert!(env.check_events().is_empty(), "{:?}", env.check_events());
    assert!(resp.get("attempts").is_none(), "{resp}");
}

// ---------------------------------------------------------------------------
// captured output on gate_evaluated
// ---------------------------------------------------------------------------

#[test]
fn a_failing_gate_logs_the_leading_4_kib_of_stdout() {
    let env = Env::new();
    // One ASCII byte, then two-byte characters: byte 4,096 falls inside one.
    let cmd = env.script(
        "lint.sh",
        "printf a\ni=0\nwhile [ $i -lt 5120 ]; do printf '\\303\\251'; i=$((i+1)); done\nexit 1\n",
    );
    env.init(&gates_template(&[("lint", &cmd)]));
    env.next();
    let event = &env.gate_events("check", "lint")[0];
    let stdout = event["stdout"].as_str().unwrap();
    assert_eq!(stdout.len(), 4095);
    assert!(stdout.ends_with('\u{e9}'));
    assert_eq!(event["stdout_truncated"], true);
    assert_eq!(event["stderr"], "");
    assert!(event.get("stderr_truncated").is_none(), "{event}");
    assert_eq!(event["output"], json!({ "exit_code": 1, "error": "" }));
    assert!(event["duration_ms"].is_u64());
}

#[test]
fn the_4_kib_cut_never_splits_a_redaction_marker() {
    let mut env = Env::new();
    let token = format!("ghp-attempt-counts-{}", std::process::id());
    env.set("GH_TOKEN", &token);
    let pad = "x".repeat(4090);
    let cmd = env.script(
        "lint.sh",
        &format!("printf '%s' '{pad}{token}tail'\nexit 1\n"),
    );
    env.init(&gates_template(&[("lint", &cmd)]));
    env.next();
    let raw = std::fs::read_to_string(env.log_path()).unwrap();
    assert!(!raw.contains(&token));
    let event = &env.gate_events("check", "lint")[0];
    assert_eq!(event["stdout"], pad, "the marker would straddle the cut");
    assert_eq!(event["stdout_truncated"], true);
}

#[test]
fn a_gate_printing_60_findings_logs_50_with_the_fallback_first() {
    let env = Env::new();
    let warnings: String = (0..60).map(|_| format!("{W291}\n")).collect();
    let cmd = env.script("lint.sh", &format!("{warnings}exit 1\n"));
    env.init(&gates_template(&[("lint", &cmd)]));
    env.next();
    let event = &env.gate_events("check", "lint")[0];
    let findings = event["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 50);
    assert_eq!(findings[0]["rule_id"], "lint");
    assert_ne!(findings[0]["message_source"], "check");
    assert!(findings[1..].iter().all(|f| f["rule_id"] == "W291"));
    assert_eq!(event["findings_truncated"], true);
    assert_eq!(event["rule_counts"], json!({ "lint": count(1, 1) }));
}

#[test]
fn a_context_gate_logs_its_finding_but_no_duration_or_streams() {
    let env = Env::new();
    env.init(
        r#"---
name: ctx
version: "1.0"
initial_state: check
states:
  check:
    gates:
      note:
        type: context-exists
        key: review_note
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check.

## done

Done.
"#,
    );
    env.next();
    let event = &env.gate_events("check", "note")[0];
    assert_eq!(stamps(std::slice::from_ref(event)), vec![(1, 1)]);
    assert_eq!(event["findings"][0]["message_source"], "koto");
    assert_eq!(event["rule_counts"], json!({ "note": count(1, 1) }));
    for absent in ["duration_ms", "stdout", "stderr"] {
        assert!(event.get(absent).is_none(), "{absent}: {event}");
    }
}

// ---------------------------------------------------------------------------
// the response's `attempts`
// ---------------------------------------------------------------------------

#[test]
fn attempts_keep_earlier_rules_of_the_visit_and_drop_them_on_return() {
    let env = Env::new();
    let cmd = env.script(
        "lint.sh",
        &format!("if [ -f second ]; then {F401}; else {E501}; fi\nexit 1\n"),
    );
    env.init(&loop_template(&cmd));

    let resp = env.next();
    assert_eq!(
        resp["attempts"]["rules"],
        json!({ "lint": { "E501": count(1, 1) } })
    );
    // `attempts` sits at the top level, beside the blocking conditions.
    assert!(resp["blocking_conditions"]
        .as_array()
        .is_some_and(|b| !b.is_empty()));

    env.touch("second");
    let resp = env.next();
    assert_eq!(
        resp["attempts"],
        json!({
            "visit": 2,
            "session": 2,
            "rules": { "lint": { "E501": count(1, 1), "F401": count(1, 1) } }
        })
    );

    // Leave and come back: the new visit lists only what it has reported.
    let resp = env.next_with(&["--to", "other"]);
    assert!(resp.get("attempts").is_none(), "{resp}");
    let resp = env.submit("back");
    assert_eq!(resp["attempts"]["visit"], 1);
    assert_eq!(resp["attempts"]["session"], 3);
    assert_eq!(
        resp["attempts"]["rules"],
        json!({ "lint": { "F401": count(1, 2) } })
    );

    env.remove("second");
    let resp = env.next();
    assert_eq!(
        resp["attempts"]["rules"],
        json!({ "lint": { "E501": count(1, 2), "F401": count(1, 2) } })
    );
}
