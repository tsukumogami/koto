//! `directive_rules` and `capture_allow_empty` end to end, through the built
//! `koto`.
//!
//! A state that lists `directive_rules` gets one `rule_delivered` event
//! (`via: directive`, `delivery: printed`) per entry whose `when_set` value is
//! non-empty, each time `koto next` prints its directive, appended after the
//! print. `capture_allow_empty` lets a `default_action` deliver empty output
//! as the empty string, which is how a template carries a rule in one run and
//! nothing in another. Both are telemetry: the response is unchanged.
//!
//! Every test runs koto with `HOME` and `KOTO_SESSIONS_BASE` in its own
//! temporary directory and `CLAUDE_CODE_SESSION_ID` cleared, so the store and
//! the journal are the test's own.

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

/// `probe` captures `near.txt` as `NEAR`, empty allowed. `work` carries two
/// rules: one set by `NEAR`, one by the declared variable `FAR`. Its gate
/// `g` exits with the code in `code.txt` and prints `out.txt`: 0 moves on
/// to `done`, 5 loops back to `work`, anything else holds. `note` is a
/// free-text field.
const TEMPLATE: &str = r#"---
name: directive-rules
version: "1.0"
initial_state: probe
variables:
  FAR:
    default: ""
states:
  probe:
    default_action:
      command: ./probe.sh
      capture_stdout_as: NEAR
      capture_allow_empty: true
    transitions:
      - target: work
  work:
    directive_rules:
      - rule_id: probe/ack-value
        check: probe_ack
        extent: full
        when_set: NEAR
      - rule_id: probe/far
        check: probe_ack
        extent: summary
        when_set: FAR
    gates:
      g:
        type: command
        command: ./g.sh
    accepts:
      note:
        type: string
        required: false
    transitions:
      - target: done
        when:
          gates.g.exit_code: 0
      - target: work
        when:
          gates.g.exit_code: 5
  done:
    terminal: true
---

## probe

Probe.

## work

Do the work.

{{NEAR}}

## done

Done.
"#;

const PROBE: &str = "#!/bin/sh\ncat near.txt 2>/dev/null\n";
const GATE: &str =
    "#!/bin/sh\ncat out.txt 2>/dev/null\nexit \"$(cat code.txt 2>/dev/null || echo 1)\"\n";

struct Env {
    _tmp: TempDir,
    dir: PathBuf,
}

impl Env {
    /// A session `wf`, initialised with `vars`, whose capture will be `near`.
    fn new(near: &str, vars: &[&str]) -> Self {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("t.md"), TEMPLATE).unwrap();
        for (name, body) in [("probe.sh", PROBE), ("g.sh", GATE)] {
            let script = dir.join(name);
            std::fs::write(&script, body).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.join("near.txt"), near).unwrap();
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let env = Env { _tmp: tmp, dir };
        env.set_check(1, "");
        let mut args = vec!["init", "wf", "--template", "t.md"];
        for v in vars {
            args.extend(["--var", v]);
        }
        env.koto(&args);
        env
    }

    fn cmd(&self) -> Command {
        let mut std_cmd = koto_command::koto_std();
        self.scope(&mut std_cmd);
        Command::from_std(std_cmd)
    }

    fn scope(&self, cmd: &mut std::process::Command) {
        cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        cmd.env_remove("KOTO_HOME");
        cmd.current_dir(&self.dir);
        cmd.env("KOTO_SESSIONS_BASE", self.dir.join("sessions"));
        cmd.env("HOME", &self.dir);
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

    fn raw_journal(&self) -> String {
        std::fs::read_to_string(self.dir.join("sessions").join("_run_journal.jsonl"))
            .unwrap_or_default()
    }

    fn journal(&self) -> Vec<Value> {
        self.raw_journal()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
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

    /// The `rule_delivered` events in the session log with `via: directive`.
    fn directive_events(&self) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == "rule_delivered" && e["payload"]["via"] == "directive")
            .collect()
    }
}

fn rule_ids(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["payload"]["rule_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_printed_directive_records_each_rule_whose_value_is_set() {
    let env = Env::new("ack with plantedvalue7a2c", &["FAR=far"]);
    let resp = env.koto(&["next", "wf"]);
    assert_eq!(resp["state"], "work", "{resp}");
    assert!(resp["directive"]
        .as_str()
        .unwrap()
        .contains("ack with plantedvalue7a2c"));

    let records = env.of_kind("rule_delivered");
    assert_eq!(records.len(), 2, "{records:#?}");
    let near = &records[0];
    assert_eq!(near["koto.state"], "work");
    assert_eq!(near["koto.check.name"], "probe_ack");
    assert_eq!(near["koto.rule.id"], "probe/ack-value");
    assert_eq!(near["koto.rule.extent"], "full");
    assert_eq!(near["koto.rule.via"], "directive");
    assert_eq!(near["koto.rule.delivery"], "printed");
    assert_eq!(records[1]["koto.rule.id"], "probe/far");
    assert_eq!(records[1]["koto.rule.extent"], "summary");
    assert_eq!(records[1]["koto.rule.via"], "directive");
    // A directive delivery is never in unit mode: the count is left out,
    // not written as zero, in the journal and in the session log.
    for r in &records {
        assert!(r.get("koto.rule.unit_count").is_none(), "{r}");
    }
    for e in env.directive_events() {
        assert!(e["payload"].get("unit_count").is_none(), "{e}");
    }
}

#[test]
fn an_empty_value_records_nothing_for_its_entry_only() {
    // The capture is empty, so the directive renders it as nothing and its
    // rule isn't recorded; FAR is set in the same directive and still is.
    let env = Env::new("", &["FAR=far"]);
    let resp = env.koto(&["next", "wf"]);
    assert_eq!(resp["state"], "work", "{resp}");
    let directive = resp["directive"].as_str().unwrap();
    assert_eq!(directive.trim_end(), "Do the work.", "{directive:?}");
    assert_eq!(rule_ids(&env.directive_events()), vec!["probe/far"]);

    // Neither set: nothing recorded at all.
    let none = Env::new("", &[]);
    none.koto(&["next", "wf"]);
    assert!(none.directive_events().is_empty());
    assert!(none.of_kind("rule_delivered").is_empty());
}

#[test]
fn an_empty_capture_in_one_run_and_a_value_in_another() {
    // The same template, the capture empty in one run and set in another:
    // the rule records only where the directive carried it.
    let empty = Env::new("\n", &[]);
    let set = Env::new("the rule\n", &[]);
    let a = empty.koto(&["next", "wf"]);
    let b = set.koto(&["next", "wf"]);
    assert_eq!(a["state"], "work");
    assert_eq!(b["state"], "work");
    assert!(!a["directive"].as_str().unwrap().contains("the rule"));
    assert!(b["directive"].as_str().unwrap().contains("the rule"));
    assert!(empty.directive_events().is_empty());
    assert_eq!(rule_ids(&set.directive_events()), vec!["probe/ack-value"]);
    // The empty value was still delivered, as the empty string.
    let captured: Vec<Value> = empty
        .events()
        .into_iter()
        .filter(|e| e["type"] == "variable_captured")
        .collect();
    assert_eq!(captured.len(), 1, "{captured:#?}");
    assert_eq!(captured[0]["payload"]["value"], "");
}

#[test]
fn every_print_records_again_and_status_records_nothing() {
    let env = Env::new("rule", &[]);
    env.koto(&["next", "wf"]);
    assert_eq!(env.directive_events().len(), 1);

    // A second tick that holds on the gate prints the directive again.
    env.koto(&["next", "wf"]);
    assert_eq!(env.directive_events().len(), 2);

    // `koto status` reads; it never records a delivery.
    for _ in 0..3 {
        env.koto(&["status", "wf"]);
    }
    assert_eq!(env.directive_events().len(), 2);

    // A `--to` move that re-enters the state prints it again.
    let resp = env.koto(&["next", "wf", "--to", "work"]);
    assert_eq!(resp["state"], "work", "{resp}");
    assert_eq!(env.directive_events().len(), 3);

    // Leave for `done`, rewind into `work`, and print it once more.
    env.set_check(0, "");
    let resp = env.koto(&["next", "wf", "--no-cleanup"]);
    assert_eq!(resp["state"], "done", "{resp}");
    let at_done = env.directive_events().len();
    assert_eq!(at_done, 3, "done carries no rules");
    env.koto(&["rewind", "wf"]);
    env.set_check(1, "");
    let resp = env.koto(&["next", "wf"]);
    assert_eq!(resp["state"], "work", "{resp}");
    assert_eq!(env.directive_events().len(), 4);
    assert_eq!(env.of_kind("rule_delivered").len(), 4);
}

#[test]
fn a_failed_print_appends_no_delivery() {
    let env = Env::new("rule", &[]);
    env.koto(&["next", "wf"]);
    assert_eq!(env.directive_events().len(), 1);

    // stdout is a pipe whose read end is already closed, so the print fails.
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    unsafe { libc::close(fds[0]) };
    let write_end =
        unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fds[1]) };
    let mut cmd = koto_command::koto_std();
    env.scope(&mut cmd);
    let status = cmd
        .args(["next", "wf"])
        .stdout(std::process::Stdio::from(write_end))
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "the print should have failed");

    // The tick ran its gate, but nothing it printed was recorded.
    let gate_runs = env
        .events()
        .iter()
        .filter(|e| e["type"] == "gate_evaluated")
        .count();
    assert_eq!(gate_runs, 2, "the failed tick still evaluated its gate");
    assert_eq!(env.directive_events().len(), 1);
}

#[test]
fn a_command_checks_deliveries_follow_its_check_evaluated_record() {
    let env = Env::new("rule", &[]);
    env.set_check(
        1,
        "::koto-rule-delivered::{\"rule_id\":\"probe/ack-value\",\"extent\":\"summary\",\"delivery\":\"printed\"}\n",
    );
    env.koto(&["next", "wf"]);

    let journal = env.journal();
    let kinds: Vec<&str> = journal
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    let evaluated = kinds.iter().position(|k| *k == "check_evaluated").unwrap();
    let delivered: Vec<(usize, &Value)> = journal
        .iter()
        .enumerate()
        .filter(|(_, r)| r["kind"] == "rule_delivered")
        .collect();
    assert_eq!(delivered.len(), 2, "{kinds:?}");
    let (check_at, check) = delivered[0];
    assert_eq!(check["koto.rule.via"], "check_output");
    assert_eq!(check["koto.check.name"], "g");
    assert!(evaluated < check_at, "{kinds:?}");
    // The directive's own delivery comes after the check's.
    let (directive_at, directive) = delivered[1];
    assert_eq!(directive["koto.rule.via"], "directive");
    assert!(check_at < directive_at, "{kinds:?}");
}

#[test]
fn no_text_reaches_the_journal() {
    // Each value is planted with a marker of its own: a finding's message,
    // a string evidence field, a captured value and a gate's stdout.
    let env = Env::new("plantedcapture4e1b", &["FAR=far"]);
    env.set_check(
        1,
        "plantedstdout4e1b\n\
         ::koto-finding::{\"rule_id\":\"probe/ack-value\",\"level\":\"error\",\"message\":\"plantedmessage4e1b\"}\n",
    );
    env.koto(&["next", "wf"]);
    env.koto(&[
        "next",
        "wf",
        "--with-data",
        r#"{"note":"plantedevidence4e1b"}"#,
    ]);
    env.set_check(0, "");
    env.koto(&["next", "wf", "--no-cleanup"]);

    // The session log holds them, so the markers did reach koto.
    let log = env
        .events()
        .iter()
        .map(|e| e.to_string())
        .collect::<String>();
    for marker in [
        "plantedcapture4e1b",
        "plantedstdout4e1b",
        "plantedmessage4e1b",
        "plantedevidence4e1b",
    ] {
        assert!(log.contains(marker), "{marker} never reached the log");
    }
    let raw = env.raw_journal();
    assert!(!env.of_kind("rule_delivered").is_empty());
    for marker in [
        "plantedcapture4e1b",
        "plantedstdout4e1b",
        "plantedmessage4e1b",
        "plantedevidence4e1b",
    ] {
        assert!(!raw.contains(marker), "{marker} reached the journal: {raw}");
    }
}
