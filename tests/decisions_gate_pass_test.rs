//! Gate passes and the other commands that record decisions with their
//! events (decision record, PRD R24, R25, R33).

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use serde_json::Value;

const IDENTITY_ENV: [&str; 5] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
];

/// An id-shaped harness session identifier.
const HARNESS: (&str, &str) = (
    "CLAUDE_CODE_SESSION_ID",
    "0f0e0d0c-1111-4222-8333-444455556666",
);

/// One state with a failing, overridable command gate the unconditional
/// edge to `done` depends on.
const TEMPLATE: &str = r#"---
name: gated
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: "exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    root: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("gated.md"), TEMPLATE).unwrap();
        Env { _tmp: tmp, root }
    }

    fn run(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in IDENTITY_ENV {
            cmd.env_remove(name);
        }
        cmd.env_remove("KOTO_TEST_DECISION_FAULT");
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.root.join("sessions"));
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let out = self.run(envs, args);
        assert_eq!(out.code, 0, "{args:?}\n{}\n{}", out.stdout, out.stderr);
        out
    }

    /// A session at `work` whose `ci` gate has failed once.
    fn blocked(&self, name: &str) {
        let template = self.root.join("gated.md");
        self.ok(
            &[],
            &["init", name, "--template", template.to_str().unwrap()],
        );
        self.run(&[], &["next", name]);
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.root
            .join("sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn events(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.log_path(name))
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn decision_dir(&self, name: &str) -> PathBuf {
        self.root
            .join("sessions")
            .join(name)
            .join("ctx/koto/decisions")
    }

    fn list(&self, name: &str, extra: &[&str]) -> Vec<Value> {
        let mut args = vec!["decisions", "list", name, "--format", "v1"];
        args.extend_from_slice(extra);
        let out = self.ok(&[], &args).json();
        out["decisions"]
            .as_array()
            .unwrap_or_else(|| panic!("no decisions array: {out}"))
            .iter()
            .map(|d| d.get("decision").cloned().unwrap_or_else(|| d.clone()))
            .collect()
    }
}

fn override_args(name: &str) -> Vec<&str> {
    vec![
        "overrides",
        "record",
        name,
        "--gate",
        "ci",
        "--rationale",
        "flaky",
    ]
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn an_override_with_no_by_is_a_defaulted_session_gate_pass_in_one_record() {
    let env = Env::new();
    env.blocked("s");
    let out = env.ok(&[HARNESS], &override_args("s"));
    assert_eq!(out.json(), serde_json::json!({"status": "recorded"}));

    let listed = env.list("s", &["--defaulted", "true"]);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let d = &listed[0];
    assert_eq!(d["kind"], "gate-pass");
    assert_eq!(d["outcome"], "passed");
    assert_eq!(d["subject"]["workflow"], "s");
    assert_eq!(d["subject"]["state"], "work");
    assert_eq!(d["subject"]["gate"], "ci");
    assert_eq!(d["decider"]["kind"], "session");

    // The event carries the same body, every existing key where it was.
    let event = env
        .events("s")
        .into_iter()
        .find(|e| e["type"] == "gate_override_recorded")
        .unwrap();
    for key in [
        "state",
        "gate",
        "rationale",
        "override_applied",
        "actual_output",
        "timestamp",
    ] {
        assert!(
            event["payload"].get(key).is_some(),
            "{key} missing: {event}"
        );
    }
    assert_eq!(event["payload"]["decision"]["id"], d["id"]);
}

#[test]
fn a_person_named_with_by_is_handed_and_not_defaulted() {
    let env = Env::new();
    env.blocked("s");
    let mut args = override_args("s");
    args.extend(["--by", "person:never-seen-before", "--surface", "terminal"]);
    env.ok(&[], &args);
    let listed = env.list("s", &["--kind", "gate-pass"]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["decider"]["kind"], "person");
    assert_eq!(listed[0]["decider"]["handle"]["value"], "never-seen-before");
    assert_eq!(listed[0]["decider"]["handle"]["source"], "handed");
    assert!(env.list("s", &["--defaulted", "true"]).is_empty());
}

#[test]
fn with_no_decider_each_gate_pass_is_refused_and_recorded_as_refused() {
    let env = Env::new();
    for (name, args) in [
        ("o", override_args("o")),
        ("t", vec!["next", "t", "--to", "done", "--no-cleanup"]),
    ] {
        env.blocked(name);
        let before = std::fs::read(env.log_path(name)).unwrap();
        let out = env.run(&[], &args);
        assert_ne!(out.code, 0, "{name}: {}", out.stdout);
        assert!(
            out.stdout.contains("decider_missing"),
            "{name}: {}",
            out.stdout
        );
        assert_eq!(std::fs::read(env.log_path(name)).unwrap(), before, "{name}");

        let refused = env.list(name, &["--kind", "refused"]);
        assert_eq!(refused.len(), 1, "{name}: {refused:?}");
        let d = &refused[0];
        assert_eq!(d["subject"]["kind"], "gate-pass");
        assert_eq!(d["decider"]["unestablished"], true);
        assert_eq!(d["decider"]["agent"]["value"], "unknown");
        assert_eq!(d["decider"]["harness"]["value"], "");
        assert_eq!(d["decider"]["anchor"]["value"], "");
    }
}

#[test]
fn a_to_past_an_unmet_gate_records_a_gate_pass_with_its_transition() {
    let env = Env::new();
    env.blocked("s");
    env.ok(&[HARNESS], &["next", "s", "--to", "done", "--no-cleanup"]);
    let listed = env.list("s", &["--defaulted", "true"]);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["subject"]["gate"], "ci");
    let event = env
        .events("s")
        .into_iter()
        .find(|e| e["type"] == "directed_transition")
        .unwrap();
    assert_eq!(event["payload"]["decision"]["id"], listed[0]["id"]);

    // An overridden gate isn't passed again by the --to that follows.
    env.blocked("o");
    env.ok(&[HARNESS], &override_args("o"));
    env.ok(&[], &["next", "o", "--to", "done", "--no-cleanup"]);
    assert_eq!(env.list("o", &["--kind", "gate-pass"]).len(), 1);
}

#[test]
fn a_writer_stopped_after_the_event_is_repaired_by_the_next_tick() {
    let env = Env::new();
    for (name, args) in [
        ("o", override_args("o")),
        ("t", vec!["next", "t", "--to", "done", "--no-cleanup"]),
    ] {
        env.blocked(name);
        let out = env.run(
            &[HARNESS, ("KOTO_TEST_DECISION_FAULT", "after-event")],
            &args,
        );
        assert_eq!(out.code, 86, "{name}: {}\n{}", out.stdout, out.stderr);
        let embedded = env
            .events(name)
            .into_iter()
            .find_map(|e| e["payload"].get("decision").cloned())
            .unwrap_or_else(|| panic!("{name}: no event carries a decision"));
        assert!(files_in(&env.decision_dir(name)).is_empty(), "{name}");

        env.run(&[], &["next", name, "--no-cleanup"]);
        let id = embedded["id"].as_str().unwrap();
        assert_eq!(files_in(&env.decision_dir(name)), vec![id.to_string()]);
        let stored: Value =
            serde_json::from_slice(&std::fs::read(env.decision_dir(name).join(id)).unwrap())
                .unwrap();
        assert_eq!(stored, embedded, "{name}");
        // The repaired body reads back as a well-formed decision.
        let shown = env.ok(&[], &["decisions", "show", name, id]).json();
        assert_eq!(shown["decision"]["id"], id, "{shown}");
    }
}

#[test]
fn disputes_and_with_data_choices_also_record_judgments_with_unchanged_output() {
    let env = Env::new();
    env.blocked("s");
    let out = env.ok(
        &[],
        &[
            "disputes", "record", "s", "--gate", "ci", "--rule", "r1", "--reason", "wrong", "--by",
            "person",
        ],
    );
    assert_eq!(out.json(), serde_json::json!({"status": "recorded"}));
    let judgments = env.list("s", &["--kind", "judgment"]);
    assert_eq!(judgments.len(), 1, "{judgments:?}");
    assert_eq!(judgments[0]["decider"]["kind"], "person");
    assert_eq!(judgments[0]["evidence"][0]["kind"], "event");

    env.ok(
        &[],
        &[
            "decisions",
            "record",
            "s",
            "--with-data",
            r#"{"choice":"retry","rationale":"flaky"}"#,
        ],
    );
    let judgments = env.list("s", &["--kind", "judgment"]);
    assert_eq!(judgments.len(), 2);
    assert!(judgments.iter().any(|j| j["subject"]["what"] == "retry"));
}
