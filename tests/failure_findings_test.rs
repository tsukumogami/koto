//! Findings and captured output on a failed check's `failure` object
//! (DESIGN-koto-failure-reporting.md, Decisions 1 and 3).
//!
//! Every case runs the built `koto` against a template whose check is a
//! script written into the test's temporary directory, and reads the
//! `koto next` response the agent would get. Each `koto` runs with a cleared
//! environment, so nothing from the developer's shell reaches a script.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const MAX_CAPTURE: usize = 65_536;

/// The guide's two-finding example, exactly as printed.
const GUIDE_E501: &str = r#"::koto-finding::{"rule_id":"E501","level":"error","message":"line too long (104 > 88)","path":"src/app.py","line":12,"column":89,"rule_ref":"https://docs.example.org/rules/E501"}"#;
const GUIDE_W291: &str = r#"::koto-finding::{"rule_id":"W291","level":"warning","message":"trailing whitespace","path":"src/app.py","line":40}"#;

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

    /// Write `body` as an executable-by-`sh` script and return the command
    /// that runs it.
    fn script(&self, name: &str, body: &str) -> String {
        let path: PathBuf = self.path().join(name);
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
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

    fn next_with(&self, extra: &[&str]) -> Value {
        let out = self
            .koto()
            .args(["next", "wf"])
            .args(extra)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("next should print JSON: {}", describe(&out)))
    }

    fn next(&self) -> Value {
        self.next_with(&[])
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

/// The blocking condition named `name`.
fn condition<'a>(resp: &'a Value, name: &str) -> &'a Value {
    resp["blocking_conditions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected blocking_conditions: {resp}"))
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no condition {name}: {resp}"))
}

fn failure<'a>(resp: &'a Value, name: &str) -> &'a Value {
    let f = &condition(resp, name)["failure"];
    assert!(f.is_object(), "{name} should carry a failure: {resp}");
    f
}

fn findings(resp: &Value, name: &str) -> Vec<Value> {
    failure(resp, name)["findings"].as_array().unwrap().clone()
}

/// A single state gated by one command gate named `lint`.
fn gate_template(command: &str, timeout: Option<u32>) -> String {
    let timeout = timeout
        .map(|t| format!("        timeout: {t}\n"))
        .unwrap_or_default();
    format!(
        r#"---
name: gated
version: "1.0"
initial_state: check
states:
  check:
    gates:
      lint:
        type: command
        command: '{command}'
{timeout}    transitions:
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

/// A single state running `command` as its `default_action`.
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

fn gate_run(env: &Env, body: &str) -> Value {
    let cmd = env.script("check.sh", body);
    env.init(&gate_template(&cmd, None));
    let resp = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    resp
}

// ---------------------------------------------------------------------------
// the fallback finding and the finding format
// ---------------------------------------------------------------------------

#[test]
fn plain_output_becomes_one_fallback_finding() {
    let env = Env::new();
    let resp = gate_run(&env, "echo boom\nexit 1\n");
    let cond = condition(&resp, "lint");
    assert_eq!(cond["output"], json!({"exit_code": 1, "error": ""}));
    assert_eq!(
        findings(&resp, "lint"),
        vec![json!({
            "rule_id": "lint",
            "level": "error",
            "message": "boom",
            "effect_landed": false,
            "message_source": "output",
        })]
    );
    let captured = &failure(&resp, "lint")["captured"];
    assert!(captured["stdout"].as_str().unwrap().contains("boom"));
    assert_eq!(captured["stdout_truncated"], false);
    assert_eq!(captured["stderr_truncated"], false);
    assert_eq!(failure(&resp, "lint")["findings_truncated"], false);
}

#[test]
fn the_guide_example_yields_both_findings_field_for_field() {
    let env = Env::new();
    let body = format!("cat <<'EOF'\n{GUIDE_E501}\n{GUIDE_W291}\nFound 1 error.\nEOF\nexit 1\n");
    let resp = gate_run(&env, &body);
    assert_eq!(
        findings(&resp, "lint"),
        vec![
            json!({
                "rule_id": "E501",
                "level": "error",
                "message": "line too long (104 > 88)",
                "path": "src/app.py",
                "line": 12,
                "column": 89,
                "rule_ref": "https://docs.example.org/rules/E501",
                "effect_landed": false,
                "message_source": "check",
            }),
            json!({
                "rule_id": "W291",
                "level": "warning",
                "message": "trailing whitespace",
                "path": "src/app.py",
                "line": 40,
                "effect_landed": false,
                "message_source": "check",
            }),
        ],
        "no fallback when the check reported an error"
    );
    // Finding lines stay in the captured text the agent sees.
    let stdout = failure(&resp, "lint")["captured"]["stdout"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        stdout,
        format!("{GUIDE_E501}\n{GUIDE_W291}\nFound 1 error.\n")
    );
}

#[test]
fn stderr_is_preferred_over_stdout_for_the_fallback_message() {
    let env = Env::new();
    let resp = gate_run(&env, "echo out-line\necho err-line >&2\nexit 1\n");
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0]["message"], "err-line");
    assert_eq!(f[0]["message_source"], "output");
}

#[test]
fn a_long_stderr_line_folds_to_500_characters() {
    let env = Env::new();
    let resp = gate_run(
        &env,
        &format!("printf '%s\\n' '{}' >&2\nexit 1\n", "y".repeat(600)),
    );
    let message = findings(&resp, "lint")[0]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(message.chars().count(), 500);
    assert!(message.ends_with("..."), "{message}");
}

#[test]
fn a_malformed_finding_line_is_ordinary_output() {
    for bad in [
        r#"::koto-finding::{"rule_id":"","level":"error","message":"empty id"}"#,
        r#"::koto-finding::{"rule_id":"R1","level":"fatal","message":"unknown level"}"#,
    ] {
        let env = Env::new();
        let resp = gate_run(&env, &format!("cat <<'EOF'\n{bad}\nEOF\nexit 1\n"));
        let f = findings(&resp, "lint");
        assert_eq!(f.len(), 1, "only the fallback: {resp}");
        assert_eq!(f[0]["rule_id"], "lint");
        assert_eq!(f[0]["level"], "error");
        let stdout = failure(&resp, "lint")["captured"]["stdout"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(stdout.contains(bad), "{stdout}");
    }
}

#[test]
fn a_warning_only_failure_also_gets_the_fallback_error() {
    let env = Env::new();
    let resp = gate_run(&env, &format!("cat <<'EOF'\n{GUIDE_W291}\nEOF\nexit 1\n"));
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 2, "{resp}");
    assert_eq!(f[0]["rule_id"], "W291");
    assert_eq!(f[1]["rule_id"], "lint");
    assert_eq!(f[1]["level"], "error");
    assert_eq!(f[1]["message"], "command exited with status 1");
    assert_eq!(f[1]["message_source"], "koto");
}

#[test]
fn an_error_finding_on_a_passing_check_adds_no_failure() {
    let env = Env::new();
    let cmd = env.script(
        "check.sh",
        &format!("cat <<'EOF'\n{GUIDE_E501}\nEOF\nexit 0\n"),
    );
    env.init(&gate_template(&cmd, None));
    let resp = env.next();
    assert_eq!(resp["state"], "done", "the check passed: {resp}");
    assert!(
        !resp.to_string().contains("\"failure\""),
        "a passing check has no failure: {resp}"
    );
}

#[test]
fn the_response_keeps_100_findings_with_the_fallback_last() {
    let env = Env::new();
    let body = "i=0\nwhile [ $i -lt 150 ]; do\n  printf '::koto-finding::{\"rule_id\":\"W%d\",\"level\":\"warning\",\"message\":\"w\"}\\n' $i\n  i=$((i+1))\ndone\nexit 1\n";
    let resp = gate_run(&env, body);
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 100);
    assert_eq!(failure(&resp, "lint")["findings_truncated"], true);
    assert_eq!(f[98]["rule_id"], "W98");
    assert_eq!(f[99]["rule_id"], "lint");
    assert_eq!(f[99]["level"], "error");
}

/// The fallback is judged over every parsed finding, so an error printed
/// after 100 warnings suppresses it even though the cap drops that error:
/// the agent sees warnings only, truncated, on a failed check. This is why
/// the guide says to print errors first.
#[test]
fn an_error_after_100_warnings_is_cut_and_adds_no_fallback() {
    let env = Env::new();
    let body = "i=0\nwhile [ $i -lt 100 ]; do\n  printf '::koto-finding::{\"rule_id\":\"W%d\",\"level\":\"warning\",\"message\":\"w\"}\\n' $i\n  i=$((i+1))\ndone\necho '::koto-finding::{\"rule_id\":\"E1\",\"level\":\"error\",\"message\":\"e\"}'\nexit 1\n";
    let resp = gate_run(&env, body);
    assert_eq!(condition(&resp, "lint")["status"], "failed");
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 100);
    assert_eq!(failure(&resp, "lint")["findings_truncated"], true);
    assert!(
        f.iter().all(|x| x["level"] == "warning"),
        "no error and no fallback reach the response: {resp}"
    );
    assert_eq!(f[99]["rule_id"], "W99");
    // The error is still in the captured output.
    let stdout = failure(&resp, "lint")["captured"]["stdout"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(stdout.contains(r#""rule_id":"E1""#), "{stdout}");
}

#[test]
fn a_finding_line_on_stderr_is_not_parsed() {
    let env = Env::new();
    let resp = gate_run(&env, &format!("echo '{GUIDE_E501}' >&2\nexit 1\n"));
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 1, "only koto's fallback: {resp}");
    assert_eq!(f[0]["rule_id"], "lint");
    assert_eq!(f[0]["message_source"], "output");
    assert_eq!(f[0]["message"], GUIDE_E501);
}

#[test]
fn a_known_value_spelled_with_a_json_escape_does_not_reach_a_finding() {
    let mut env = Env::new();
    let value = format!("sekrit-{}-value", std::process::id());
    env.set("KOTO_TEST_PASS", &value);
    // `\u0073` spells the value's leading `s`, so the raw capture never holds
    // the value itself; the decoded finding does.
    let body = "rest=${KOTO_TEST_PASS#s}\n\
                printf '::koto-finding::{\"rule_id\":\"R1\",\"level\":\"error\",\"message\":\"m \\\\u0073%s\",\"path\":\"p/\\\\u0073%s\",\"rule_ref\":\"u/\\\\u0073%s\"}\\n' \"$rest\" \"$rest\" \"$rest\"\n\
                exit 1\n";
    let cmd = env.script("check.sh", body);
    let template = gate_template(&cmd, None).replace(
        "initial_state: check\n",
        "initial_state: check\npass_env:\n  - KOTO_TEST_PASS\n",
    );
    env.init(&template);
    let resp = env.next();
    let f = &findings(&resp, "lint")[0];
    assert_eq!(f["rule_id"], "R1", "{resp}");
    for field in ["message", "path", "rule_ref"] {
        let text = f[field].as_str().unwrap();
        assert!(!text.contains(&value), "{field}: {text}");
        assert!(
            text.contains("[REDACTED:KOTO_TEST_PASS]"),
            "{field}: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// timeouts, spawn failures and the capture bound
// ---------------------------------------------------------------------------

#[test]
fn a_timed_out_gate_returns_both_partial_streams_and_koto_s_note() {
    let env = Env::new();
    let cmd = env.script(
        "check.sh",
        "echo partial-out\necho partial-err >&2\nsleep 5\n",
    );
    env.init(&gate_template(&cmd, Some(1)));
    let resp = env.next();
    let cond = condition(&resp, "lint");
    assert_eq!(cond["status"], "timed_out");
    // Byte-identical to the output koto wrote before findings existed.
    assert_eq!(
        serde_json::to_string(&cond["output"]).unwrap(),
        r#"{"error":"timed_out","exit_code":-1,"failure_kind":"timed_out"}"#
    );
    let captured = &failure(&resp, "lint")["captured"];
    assert_eq!(captured["stdout"], "partial-out\n");
    assert!(captured["stderr"]
        .as_str()
        .unwrap()
        .starts_with("partial-err\n"));
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0]["message"], "command timed out after 1 seconds");
    assert_eq!(f[0]["message_source"], "koto");
}

#[test]
fn a_failing_gate_returns_exactly_the_capture_bound() {
    let env = Env::new();
    let resp = gate_run(&env, "head -c 102400 /dev/zero | tr '\\0' x\nexit 1\n");
    let captured = &failure(&resp, "lint")["captured"];
    assert_eq!(captured["stdout"].as_str().unwrap().len(), MAX_CAPTURE);
    assert_eq!(captured["stdout_truncated"], true);
    assert_eq!(captured["stderr_truncated"], false);

    // A two-byte character straddling the bound is dropped whole.
    let env = Env::new();
    let resp = gate_run(
        &env,
        "head -c 65535 /dev/zero | tr '\\0' x\nprintf '\\303\\251tail'\nexit 1\n",
    );
    let captured = &failure(&resp, "lint")["captured"];
    let stdout = captured["stdout"].as_str().unwrap();
    assert_eq!(stdout.len(), MAX_CAPTURE - 1);
    assert!(stdout.bytes().all(|b| b == b'x'));
    assert_eq!(captured["stdout_truncated"], true);
}

#[test]
fn a_default_action_that_cannot_be_spawned_reports_koto_s_error() {
    let env = Env::new();
    env.init(&action_template(
        "echo never",
        "      working_dir: no-such-directory\n",
    ));
    let resp = env.next();
    let cond = condition(&resp, "__action__");
    assert_eq!(cond["output"]["failure_kind"], "spawn_failed");
    let stderr = cond["output"]["stderr"].as_str().unwrap().to_string();
    let f = findings(&resp, "__action__");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0]["rule_id"], "__action__");
    assert_eq!(f[0]["message_source"], "koto");
    assert!(stderr.starts_with("failed to spawn command"), "{stderr}");
    assert_eq!(f[0]["message"], stderr.trim());
    let captured = &failure(&resp, "__action__")["captured"];
    assert_eq!(captured["stderr"], stderr.as_str());
    assert_eq!(captured["stdout"], "");
}

#[test]
fn a_working_dir_refusal_reports_the_refusal() {
    let env = Env::new();
    env.init(&action_template("echo never", "      working_dir: ../..\n"));
    let resp = env.next();
    let f = findings(&resp, "__action__");
    assert_eq!(f[0]["message_source"], "koto");
    assert!(
        f[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("default_action working_dir '../..' resolves to"),
        "{resp}"
    );
}

// ---------------------------------------------------------------------------
// the fallback message for each failing check
// ---------------------------------------------------------------------------

#[test]
fn a_default_action_failure_gets_the_same_payload_shape() {
    let env = Env::new();
    env.init(&action_template("exit 3", ""));
    let resp = env.next();
    let f = failure(&resp, "__action__");
    assert_eq!(
        f["findings"],
        json!([{
            "rule_id": "__action__",
            "level": "error",
            "message": "command exited with status 3",
            "effect_landed": false,
            "message_source": "koto",
        }])
    );
    assert_eq!(
        f["captured"],
        json!({"stdout": "", "stderr": "", "stdout_truncated": false, "stderr_truncated": false})
    );

    let env = Env::new();
    env.init(&action_template(
        "echo act-out; echo act-err >&2; exit 3",
        "",
    ));
    let resp = env.next();
    assert_eq!(findings(&resp, "__action__")[0]["message"], "act-err");
}

#[test]
fn a_capture_failure_after_exit_0_says_so_and_landed_nothing() {
    let env = Env::new();
    env.init(&action_template(
        "true",
        "      capture_stdout_as: BRANCH\n",
    ));
    let resp = env.next();
    let cond = condition(&resp, "__action__");
    assert_eq!(cond["output"]["failure_kind"], "capture_failed");
    let f = findings(&resp, "__action__");
    assert_eq!(
        f[0]["message"],
        "command exited 0 but its output could not be delivered as BRANCH"
    );
    assert_eq!(f[0]["message_source"], "koto");
    assert_eq!(f[0]["effect_landed"], false);
}

#[test]
fn the_truncation_note_is_never_the_message() {
    let env = Env::new();
    env.init(&action_template(
        "head -c 70000 /dev/zero | tr \"\\0\" x; exit 1",
        "",
    ));
    let resp = env.next();
    let cond = condition(&resp, "__action__");
    assert!(cond["output"]["stdout"]
        .as_str()
        .unwrap()
        .ends_with("... [output truncated]"));
    let message = findings(&resp, "__action__")[0]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!message.contains("output truncated"), "{message}");
    assert!(message.starts_with("xxx"), "{message}");
    let captured = &failure(&resp, "__action__")["captured"];
    assert_eq!(captured["stdout"].as_str().unwrap().len(), MAX_CAPTURE);
    assert!(!captured["stdout"]
        .as_str()
        .unwrap()
        .contains("output truncated"));
}

fn context_template(gate: &str) -> String {
    format!(
        r#"---
name: ctx
version: "1.0"
initial_state: check
states:
  check:
    gates:
      note:
{gate}    transitions:
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

#[test]
fn context_gate_failures_name_the_key_and_pattern() {
    let exists = "        type: context-exists\n        key: review_note\n";
    let matches =
        "        type: context-matches\n        key: review_note\n        pattern: \"^approved\"\n";

    let env = Env::new();
    env.init(&context_template(exists));
    let resp = env.next();
    let f = failure(&resp, "note");
    assert!(f.get("captured").is_none(), "{resp}");
    assert_eq!(
        f["findings"],
        json!([{
            "rule_id": "note",
            "level": "error",
            "message": "context key 'review_note' is not set",
            "effect_landed": false,
            "message_source": "koto",
        }])
    );

    let env = Env::new();
    env.init(&context_template(matches));
    let resp = env.next();
    assert_eq!(
        findings(&resp, "note")[0]["message"],
        "context key 'review_note' is not set"
    );

    let env = Env::new();
    env.init(&context_template(matches));
    let body = env.path().join("note.txt");
    std::fs::write(&body, "rejected").unwrap();
    let out = env
        .koto()
        .args([
            "context",
            "add",
            "wf",
            "review_note",
            "--from-file",
            body.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let resp = env.next();
    assert_eq!(
        findings(&resp, "note")[0]["message"],
        "context key 'review_note' does not match pattern '^approved'"
    );
    assert_eq!(
        condition(&resp, "note")["output"],
        json!({"matches": false, "error": ""})
    );
}

// ---------------------------------------------------------------------------
// effect_landed
// ---------------------------------------------------------------------------

/// A state that accepts evidence and gates on a failing command; the
/// submitted `retry` routes nowhere, so the state stops with the gate's
/// failure on the evidence-required response.
fn evidence_template(command: &str) -> String {
    format!(
        r#"---
name: landed
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      decision:
        type: enum
        required: true
        values: [approve, retry]
    gates:
      lint:
        type: command
        command: '{command}'
    transitions:
      - target: done
        when:
          decision: approve
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#
    )
}

#[test]
fn effect_landed_is_true_only_when_this_invocation_recorded_evidence() {
    let env = Env::new();
    let stated = r#"::koto-finding::{"rule_id":"S1","level":"warning","message":"stated","effect_landed":false}"#;
    let cmd = env.script("check.sh", &format!("cat <<'EOF'\n{stated}\nEOF\nexit 1\n"));
    env.init(&evidence_template(&cmd));

    let submitted = env.next_with(&["--with-data", r#"{"decision":"retry"}"#]);
    let f = findings(&submitted, "lint");
    assert_eq!(f.len(), 2, "{submitted}");
    assert_eq!(f[0]["effect_landed"], false, "a stated value is kept");
    assert_eq!(f[1]["rule_id"], "lint");
    assert_eq!(f[1]["effect_landed"], true, "{submitted}");

    // The evidence is still on the state, but this invocation recorded none.
    let again = env.next();
    let f = findings(&again, "lint");
    assert_eq!(f[1]["effect_landed"], false, "{again}");
}

/// Evidence counts toward `effect_landed` only before the tick's first
/// transition: evidence that moves the session on says nothing about the
/// next state's checks.
#[test]
fn evidence_that_transitions_does_not_land_the_next_state_s_effect() {
    let env = Env::new();
    let cmd = env.script("check.sh", "echo still-broken\nexit 1\n");
    let template = format!(
        r#"---
name: moved
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      decision:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: work
        when:
          decision: go
  work:
    gates:
      lint:
        type: command
        command: '{cmd}'
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Start.

## work

Work.

## done

Done.
"#
    );
    env.init(&template);
    let resp = env.next_with(&["--with-data", r#"{"decision":"go"}"#]);
    assert_eq!(resp["state"], "work", "{resp}");
    let f = findings(&resp, "lint");
    assert_eq!(f.len(), 1, "{resp}");
    assert_eq!(f[0]["effect_landed"], false, "{resp}");
}

#[test]
fn effect_landed_is_true_after_a_default_action_delivered_its_capture() {
    let env = Env::new();
    let gate = env.script("check.sh", "echo gate-failed\nexit 1\n");
    let template = format!(
        r#"---
name: captured
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: "echo main"
      capture_stdout_as: BRANCH
    gates:
      lint:
        type: command
        command: '{gate}'
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run.

## done

Done.
"#
    );
    env.init(&template);
    let resp = env.next();
    let f = findings(&resp, "lint");
    assert_eq!(f[0]["message"], "gate-failed");
    assert_eq!(f[0]["effect_landed"], true, "{resp}");
}

// ---------------------------------------------------------------------------
// routing stays put
// ---------------------------------------------------------------------------

fn compile(env: &Env, template: &str) -> anyhow::Result<()> {
    let path = env.path().join("compile.md");
    std::fs::write(&path, template).unwrap();
    koto::template::compile::compile(&path, false).map(|_| ())
}

#[test]
fn routing_and_overrides_cannot_see_the_failure() {
    let env = Env::new();
    let with_default = gate_template("exit 1", None).replace(
        "        command: 'exit 1'\n",
        "        command: 'exit 1'\n        override_default:\n          exit_code: 0\n          error: \"\"\n",
    );
    compile(&env, &with_default).expect("an override_default of today's shape still compiles");

    let routed = gate_template("exit 1", None).replace(
        "      - target: done\n",
        "      - target: done\n        when:\n          gates.lint.failure: x\n",
    );
    let err = compile(&env, &routed).expect_err("a when clause on `failure` is refused");
    assert!(format!("{err:#}").contains("failure"), "{err:#}");

    let fields: Vec<&str> = koto::template::types::gate_type_schema("command")
        .unwrap()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(fields, ["exit_code", "error"]);
}
