//! Known credentials never leave koto through captured command output
//! (DESIGN-koto-failure-reporting.md, Decision 5).
//!
//! Every secret here is generated at run time and handed to koto through the
//! environment; templates only ever name the variable (`$GH_TOKEN`), never a
//! value. Each `koto` runs with a cleared environment, so a credential in the
//! developer's own environment can't reach a test or be printed by one.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use koto::action::{run_shell_command, CommandEnv, FailureKind, MAX_ACTION_OUTPUT_BYTES};
use koto::redact::{Redactor, MIN_VALUE_LEN};
use serde_json::Value;

/// The CLI's truncation note without its leading newline, so `ends_with`
/// matches however the stream's last line ended.
const TRUNCATION_NOTE: &str = "... [output truncated]";

/// A value unique to this run, safe in a JSON string and a shell word.
fn secret(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{tag}-{nanos:x}-{}", std::process::id())
}

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

    fn init(&self, template: &str, extra: &[&str]) {
        let tpl = self.path().join("template.md");
        std::fs::write(&tpl, template).unwrap();
        let out = self
            .koto()
            .args(["init", "wf", "--template", tpl.to_str().unwrap()])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "init: {}", describe(&out));
    }

    fn next(&self) -> (Output, Value) {
        let out = self.koto().args(["next", "wf"]).output().unwrap();
        let body = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("next should print JSON: {}", describe(&out)));
        (out, body)
    }

    fn log_path(&self) -> PathBuf {
        self.path()
            .join("sessions")
            .join("wf")
            .join("koto-wf.state.jsonl")
    }

    fn raw_log(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap()
    }

    fn events_of(&self, ty: &str) -> Vec<Value> {
        self.raw_log()
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|e| e["type"] == ty)
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

/// The single `__action__` condition's output in a blocked response.
fn action_output(resp: &Value) -> &Value {
    let conditions = resp["blocking_conditions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected blocking_conditions: {resp}"));
    let found: Vec<&Value> = conditions
        .iter()
        .filter(|c| c["name"] == "__action__")
        .collect();
    assert_eq!(found.len(), 1, "{resp}");
    &found[0]["output"]
}

/// Assert no run of `MIN_VALUE_LEN` or more bytes of any value appears in
/// `text`.
fn assert_absent(text: &str, values: &[&str], site: &str) {
    for value in values {
        for len in (MIN_VALUE_LEN..=value.len()).rev() {
            for start in 0..=value.len() - len {
                let frag = &value[start..start + len];
                assert!(
                    !text.contains(frag),
                    "{site}: a fragment of a known value survived"
                );
            }
        }
    }
}

/// Echo each named variable to stdout and stderr, then fail.
fn echo_both(names: &[&str]) -> String {
    let mut cmd = String::new();
    for n in names {
        cmd.push_str(&format!("echo \"out ${n}\"; echo \"err ${n}\" >&2; "));
    }
    cmd.push_str("exit 1");
    cmd
}

fn action_template(pass_env: &[&str], command: &str, capture: Option<&str>) -> String {
    let pass = if pass_env.is_empty() {
        String::new()
    } else {
        format!(
            "pass_env:\n{}",
            pass_env
                .iter()
                .map(|n| format!("  - {n}\n"))
                .collect::<String>()
        )
    };
    let capture = capture
        .map(|k| format!("      capture_stdout_as: {k}\n"))
        .unwrap_or_default();
    format!(
        r#"---
name: leak
version: "1.0"
initial_state: run
{pass}states:
  run:
    default_action:
      command: '{command}'
{capture}    transitions:
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

fn gate_template(pass_env: &[&str], command: &str) -> String {
    let pass = format!(
        "pass_env:\n{}",
        pass_env
            .iter()
            .map(|n| format!("  - {n}\n"))
            .collect::<String>()
    );
    format!(
        r#"---
name: gated
version: "1.0"
initial_state: check
{pass}states:
  check:
    gates:
      leaky:
        type: command
        command: '{command}'
    transitions:
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

const PASSED: [&str; 4] = [
    "KOTO_TEST_PASS",
    "KOTO_DECIDER_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
];

/// Five credentials, each set on the koto process under its own name.
fn five_secrets(env: &mut Env) -> Vec<(&'static str, String)> {
    let names = ["GH_TOKEN"].into_iter().chain(PASSED);
    let pairs: Vec<(&'static str, String)> = names
        .map(|n| (n, secret(&n.to_lowercase().replace('_', ""))))
        .collect();
    for (n, v) in &pairs {
        env.set(n, v);
    }
    pairs
}

#[test]
fn credentials_echoed_by_a_default_action_are_redacted_in_the_response_and_the_log() {
    let mut env = Env::new();
    let pairs = five_secrets(&mut env);
    let names: Vec<&str> = pairs.iter().map(|(n, _)| *n).collect();
    let values: Vec<&str> = pairs.iter().map(|(_, v)| v.as_str()).collect();
    env.init(&action_template(&PASSED, &echo_both(&names), None), &[]);

    let (out, resp) = env.next();
    let output = action_output(&resp);
    for name in &names {
        let marker = format!("[REDACTED:{name}]");
        assert!(
            output["stdout"]
                .as_str()
                .unwrap()
                .contains(&format!("out {marker}")),
            "{output}"
        );
        assert!(
            output["stderr"]
                .as_str()
                .unwrap()
                .contains(&format!("err {marker}")),
            "{output}"
        );
    }
    assert_absent(&String::from_utf8_lossy(&out.stdout), &values, "response");
    assert_absent(&String::from_utf8_lossy(&out.stderr), &values, "stderr");
    assert_absent(&env.raw_log(), &values, "session log");

    let executed = env.events_of("default_action_executed");
    assert_eq!(executed.len(), 1);
    let payload = &executed[0]["payload"];
    for name in &names {
        let marker = format!("[REDACTED:{name}]");
        assert!(
            payload["stdout"].as_str().unwrap().contains(&marker),
            "{payload}"
        );
        assert!(
            payload["stderr"].as_str().unwrap().contains(&marker),
            "{payload}"
        );
    }
}

/// A failing gate's evidence doesn't carry its streams; its `failure` does,
/// and holds a marker at each site.
#[test]
fn credentials_echoed_by_a_failing_command_gate_never_reach_the_response_or_the_log() {
    let mut env = Env::new();
    let pairs = five_secrets(&mut env);
    let names: Vec<&str> = pairs.iter().map(|(n, _)| *n).collect();
    let values: Vec<&str> = pairs.iter().map(|(_, v)| v.as_str()).collect();
    env.init(&gate_template(&PASSED, &echo_both(&names)), &[]);

    let (out, resp) = env.next();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    let captured = &resp["blocking_conditions"][0]["failure"]["captured"];
    for name in &names {
        let marker = format!("[REDACTED:{name}]");
        for (stream, prefix) in [("stdout", "out"), ("stderr", "err")] {
            assert!(
                captured[stream]
                    .as_str()
                    .unwrap_or_else(|| panic!("no captured {stream}: {resp}"))
                    .contains(&format!("{prefix} {marker}")),
                "{resp}"
            );
        }
    }
    assert_absent(&String::from_utf8_lossy(&out.stdout), &values, "response");
    assert_absent(&String::from_utf8_lossy(&out.stderr), &values, "stderr");
    assert_absent(&env.raw_log(), &values, "session log");
}

#[test]
fn config_file_keys_and_the_env_values_overriding_them_are_all_redacted() {
    let mut env = Env::new();
    let file_decider = secret("filedecider");
    let file_access = secret("fileaccess");
    let file_secret = secret("filesecret");
    let env_decider = secret("envdecider");
    let env_access = secret("envaccess");
    std::fs::create_dir_all(env.path().join(".koto")).unwrap();
    std::fs::write(
        env.path().join(".koto").join("config.toml"),
        format!(
            "[session.cloud]\naccess_key = \"{file_access}\"\nsecret_key = \"{file_secret}\"\n\n\
             [decider]\napi_key = \"{file_decider}\"\n"
        ),
    )
    .unwrap();
    env.set("KOTO_DECIDER_API_KEY", &env_decider);
    env.set("AWS_ACCESS_KEY_ID", &env_access);
    // The file values also sit in test-only variables, so a command in a
    // legacy session (which inherits everything) can print them by name.
    env.set("KOTO_T_FILE_DECIDER", &file_decider);
    env.set("KOTO_T_FILE_ACCESS", &file_access);
    env.set("KOTO_T_FILE_SECRET", &file_secret);
    let printed = [
        "KOTO_T_FILE_DECIDER",
        "KOTO_T_FILE_ACCESS",
        "KOTO_T_FILE_SECRET",
        "KOTO_DECIDER_API_KEY",
        "AWS_ACCESS_KEY_ID",
    ];
    env.init(
        &action_template(&[], &echo_both(&printed), None),
        &["--legacy-environment"],
    );

    let (out, resp) = env.next();
    let stdout = action_output(&resp)["stdout"].as_str().unwrap().to_string();
    for source in [
        "decider.api_key",
        "session.cloud.access_key",
        "session.cloud.secret_key",
        "KOTO_DECIDER_API_KEY",
        "AWS_ACCESS_KEY_ID",
    ] {
        assert!(
            stdout.contains(&format!("[REDACTED:{source}]")),
            "{source}: {stdout}"
        );
    }
    let values = [
        file_decider.as_str(),
        file_access.as_str(),
        file_secret.as_str(),
        env_decider.as_str(),
        env_access.as_str(),
    ];
    assert_absent(&String::from_utf8_lossy(&out.stdout), &values, "response");
    assert_absent(&env.raw_log(), &values, "session log");
}

#[test]
fn a_json_escaped_spelling_is_caught() {
    let mut env = Env::new();
    let value = format!("pa/ss\"{}", secret("word"));
    env.set("KOTO_TEST_PASS", &value);
    // One line as serde_json escapes it, one with `/` written `\/` as well.
    let command = "printf \"%s\\n\" \"$KOTO_TEST_PASS\" | sed -e \"s/\\\"/\\\\\\\\\\\"/g\"; \
                   printf \"%s\\n\" \"$KOTO_TEST_PASS\" | sed -e \"s/\\\"/\\\\\\\\\\\"/g\" -e \"s|/|\\\\\\\\/|g\"; \
                   exit 1";
    let template = format!(
        r#"---
name: escaped
version: "1.0"
initial_state: run
pass_env:
  - KOTO_TEST_PASS
states:
  run:
    default_action:
      command: {}
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run it.

## done

Done.
"#,
        serde_json::to_string(command).unwrap()
    );
    env.init(&template, &[]);
    let (out, resp) = env.next();
    let stdout = action_output(&resp)["stdout"].as_str().unwrap();
    assert_eq!(
        stdout, "[REDACTED:KOTO_TEST_PASS]\n[REDACTED:KOTO_TEST_PASS]\n",
        "{resp}"
    );
    let escaped = serde_json::to_string(&value).unwrap();
    let escaped = &escaped[1..escaped.len() - 1];
    let slashed = escaped.replace('/', "\\/");
    let raw_response = String::from_utf8_lossy(&out.stdout).to_string();
    for spelling in [value.as_str(), escaped, slashed.as_str()] {
        assert!(!raw_response.contains(spelling), "response");
        assert!(!env.raw_log().contains(spelling), "session log");
    }
}

#[test]
fn a_capture_holding_a_marker_is_refused_naming_the_source() {
    let mut env = Env::new();
    let token = secret("ghp");
    env.set("GH_TOKEN", &token);
    env.init(
        &action_template(&[], "echo \"$GH_TOKEN\"", Some("TOKEN")),
        &[],
    );
    let (out, resp) = env.next();
    let output = action_output(&resp);
    assert_eq!(output["failure_kind"], "capture_failed", "{resp}");
    assert_eq!(
        output["capture_error"],
        serde_json::json!({"key": "TOKEN", "case": "redacted", "source": "GH_TOKEN"})
    );
    assert!(env.events_of("variable_captured").is_empty());
    assert_absent(&String::from_utf8_lossy(&out.stdout), &[&token], "response");
    assert_absent(&env.raw_log(), &[&token], "session log");
}

#[test]
fn an_uncut_stderr_near_the_bound_is_not_marked_when_stdout_was_cut() {
    let env = Env::new();
    let command = format!(
        "head -c {} /dev/zero | tr \"\\000\" a; head -c {} /dev/zero | tr \"\\000\" b >&2; exit 1",
        MAX_ACTION_OUTPUT_BYTES + 4096,
        MAX_ACTION_OUTPUT_BYTES - 2
    );
    env.init(&action_template(&[], &command, None), &[]);
    let (_, resp) = env.next();
    let output = action_output(&resp);
    let stdout = output["stdout"].as_str().unwrap();
    let stderr = output["stderr"].as_str().unwrap();
    assert!(stdout.ends_with(TRUNCATION_NOTE));
    assert_eq!(stderr.len(), MAX_ACTION_OUTPUT_BYTES - 2);
    assert!(!stderr.ends_with(TRUNCATION_NOTE));
    assert_eq!(output["truncated"], true);
}

// ---------------------------------------------------------------------------
// the runner, directly
// ---------------------------------------------------------------------------

fn env_with(name: &str, value: &str) -> CommandEnv {
    CommandEnv::cleared(
        vec![
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            (name.to_string(), value.to_string()),
        ],
        Redactor::new([(name.to_string(), value.to_string())]),
    )
}

#[test]
fn a_value_at_every_offset_up_to_the_bound_leaves_no_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let value = secret("ghp");
    let env = env_with("KOTO_T_V", &value);
    let l = value.len();
    for offset in (MAX_ACTION_OUTPUT_BYTES - l)..=MAX_ACTION_OUTPUT_BYTES {
        let command = format!(
            "head -c {offset} /dev/zero | tr '\\000' x; printf '%s' \"$KOTO_T_V\"; \
             head -c 300 /dev/zero | tr '\\000' y"
        );
        let out = run_shell_command(&command, dir.path(), 10, &env);
        assert_eq!(out.failure_kind, None, "offset {offset}");
        assert!(out.stdout_truncated, "offset {offset}");
        assert!(!out.stderr_truncated, "offset {offset}");
        assert!(
            out.stdout.len() <= MAX_ACTION_OUTPUT_BYTES,
            "offset {offset}"
        );
        assert_absent(&out.stdout, &[&value], &format!("offset {offset}"));
    }
}

#[test]
fn a_timeout_kill_mid_value_masks_the_head_it_left() {
    let dir = tempfile::tempdir().unwrap();
    let value = secret("ghp");
    let env = env_with("KOTO_T_V", &value);
    let out = run_shell_command(
        "printf 'partial '; printf '%s' \"$KOTO_T_V\" | head -c 12; sleep 60",
        dir.path(),
        1,
        &env,
    );
    assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
    assert_eq!(out.stdout, "partial [REDACTED:KOTO_T_V]");
    assert!(out.stderr.contains("timed out"));
}

#[test]
fn a_seven_byte_value_is_left_and_an_eight_byte_one_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shell_command(
        "printf '%s' \"$KOTO_T_V\"",
        dir.path(),
        5,
        &env_with("KOTO_T_V", "abc1234"),
    );
    assert_eq!(out.stdout, "abc1234");
    let out = run_shell_command(
        "printf '%s' \"$KOTO_T_V\"",
        dir.path(),
        5,
        &env_with("KOTO_T_V", "abc12345"),
    );
    assert_eq!(out.stdout, "[REDACTED:KOTO_T_V]");
}

#[test]
fn with_no_known_values_exactly_the_bound_is_returned_uncut() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shell_command(
        &format!(
            "head -c {} /dev/zero | tr '\\000' z",
            MAX_ACTION_OUTPUT_BYTES
        ),
        dir.path(),
        10,
        &CommandEnv::cleared(
            vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            Redactor::empty(),
        ),
    );
    assert_eq!(out.stdout.len(), 65_536);
    assert!(out.stdout.bytes().all(|b| b == b'z'));
    assert!(!out.stdout_truncated);
    assert!(!out.truncated);
}
