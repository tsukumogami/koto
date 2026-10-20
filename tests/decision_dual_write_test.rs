//! The gate-passing commands and `koto decisions record <name>` writing
//! decisions beside their own events, the repair of a lost append, and
//! `koto decisions import`.
//!
//! Every test runs koto from inside a fixture workspace whose session map
//! makes COORD a root and W its worker. `HOME` points at the test's own
//! directory, so the decision store is private to it.

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const W: &str = "9c1e0000-0000-4000-8000-0000000000a1";
const COORD: &str = "4b2d0000-0000-4000-8000-0000000000ac";
const FAULT_ENV: &str = "KOTO_TEST_DECISION_STORE_FAULT";
const LOCK_WAIT_ENV: &str = "KOTO_TEST_DECISION_LOCK_WAIT_MS";

struct Ws {
    dir: TempDir,
}

impl Ws {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("ws/.niwa/sessions")).unwrap();
        std::fs::create_dir_all(dir.path().join("ws/repo")).unwrap();
        std::fs::write(dir.path().join("ws/.niwa/workspace.toml"), "").unwrap();
        let ws = Ws { dir };
        ws.entry(
            COORD,
            json!({"lineage": {"v": 1, "parent": "none", "source": {"parent": "observed"}}}),
        );
        ws.entry(
            W,
            json!({"lineage": {"v": 1, "parent": format!("session:{}", COORD),
                "chain": [{"session": COORD, "parent": "none"}],
                "source": {"parent": "observed", "chain": "observed"}}}),
        );
        ws
    }

    fn entry(&self, id: &str, body: Value) {
        let p = self
            .dir
            .path()
            .join("ws/.niwa/sessions")
            .join(format!("{}.json", id));
        std::fs::write(p, body.to_string()).unwrap();
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn cwd(&self) -> PathBuf {
        self.dir.path().join("ws/repo")
    }

    fn store(&self) -> PathBuf {
        self.home().join(".koto/decisions")
    }

    fn sessions(&self) -> PathBuf {
        self.home().join("sessions")
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    fn std_cmd(&self, cwd: &Path, session: Option<&str>) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"));
        cmd.current_dir(cwd);
        cmd.env("HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for name in [
            "CLAUDE_CODE_SESSION_ID",
            "KOTO_AGENT_ID",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "KOTO_WORKFLOWS_DIR",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            FAULT_ENV,
            LOCK_WAIT_ENV,
        ] {
            cmd.env_remove(name);
        }
        if let Some(s) = session {
            cmd.env("CLAUDE_CODE_SESSION_ID", s);
        }
        cmd
    }

    /// Run koto and return its exit code, its last JSON line and stderr.
    fn run_env(
        &self,
        session: Option<&str>,
        envs: &[(&str, &str)],
        args: &[&str],
    ) -> (i32, Value, String) {
        self.run_in(&self.cwd(), session, envs, args)
    }

    fn run_in(
        &self,
        cwd: &Path,
        session: Option<&str>,
        envs: &[(&str, &str)],
        args: &[&str],
    ) -> (i32, Value, String) {
        let mut cmd = self.std_cmd(cwd, session);
        cmd.envs(envs.iter().copied());
        let out = Command::from_std(cmd).args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        let json = serde_json::from_str(last).unwrap_or(Value::Null);
        (out.status.code().unwrap_or(-1), json, stderr)
    }

    fn run(&self, session: Option<&str>, args: &[&str]) -> (i32, Value) {
        let (code, json, _) = self.run_env(session, &[], args);
        (code, json)
    }

    fn ok(&self, session: Option<&str>, args: &[&str]) -> Value {
        let (code, json, stderr) = self.run_env(session, &[], args);
        assert_eq!(code, 0, "koto {:?} failed: {} {}", args, json, stderr);
        json
    }

    fn init(&self, name: &str, template: &str) {
        let tpl = self.cwd().join(format!("{}.md", name));
        std::fs::write(&tpl, template).unwrap();
        self.ok(
            Some(W),
            &["init", name, "--template", tpl.to_str().unwrap()],
        );
    }

    fn list(&self, args: &[&str]) -> Vec<Value> {
        let mut full = vec!["decisions", "list"];
        full.extend_from_slice(args);
        self.ok(Some(W), &full)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["decision"].clone())
            .collect()
    }

    fn show(&self, id: &str) -> Value {
        self.ok(Some(W), &["decisions", "show", id])
    }

    fn events(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap()
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn header_session_id(&self, name: &str) -> String {
        let text = std::fs::read_to_string(self.state_path(name)).unwrap();
        let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        header["session_id"].as_str().unwrap().to_string()
    }

    /// Every line in the store's month files.
    fn month_lines(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.store()) else {
            return out;
        };
        for e in entries {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let month = name.strip_suffix(".jsonl").unwrap_or_default();
            if month.len() == 7
                && month.as_bytes()[4] == b'-'
                && month.as_bytes()[0].is_ascii_digit()
            {
                for l in std::fs::read_to_string(&p).unwrap().lines() {
                    out.push(serde_json::from_str(l).unwrap());
                }
            }
        }
        out
    }

    /// Every run-journal line, wherever this setup's journal lives.
    fn journal(&self) -> String {
        [
            self.sessions().join("_run_journal.jsonl"),
            self.home().join(".koto/_run_journal.jsonl"),
        ]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("")
    }
}

/// One gate that fails until `ok.flag` exists, then a state that waits for
/// evidence, then a terminal.
const GATED: &str = r#"---
name: gated
version: "1.0"
initial_state: gated
states:
  gated:
    gates:
      g:
        type: command
        command: "test -f ok.flag"
    transitions:
      - target: after
  after:
    accepts:
      go:
        type: enum
        required: true
        values: [yes, no]
    transitions:
      - target: done
        when:
          go: yes
      - target: gated
        when:
          go: no
  done:
    terminal: true
---

## gated

Pass the gate.

## after

Carry on.

## done

Done.
"#;

/// A gated state that also waits for evidence, so a passing gate leaves the
/// workflow standing in it.
const GATED_ACCEPTS: &str = r#"---
name: gated-accepts
version: "1.0"
initial_state: gated
states:
  gated:
    gates:
      g:
        type: command
        command: "test -f ok.flag"
    accepts:
      go:
        type: enum
        required: true
        values: [yes]
    transitions:
      - target: after
        when:
          go: yes
      - target: gated
  after:
    terminal: true
---

## gated

Pass the gate and say go.

## after

Done.
"#;

/// A gate straight before a terminal.
const GATE_TO_DONE: &str = r#"---
name: gate-to-done
version: "1.0"
initial_state: gated
states:
  gated:
    gates:
      g:
        type: command
        command: "test -f ok.flag"
    transitions:
      - target: done
  done:
    terminal: true
---

## gated

Pass the gate.

## done

Done.
"#;

const ACCEPT: &[&str] = &[
    "decisions",
    "record",
    "--kind",
    "accept",
    "--outcome",
    "accepted",
    "--of",
    "document",
    "--repo",
    "o/r",
    "--path",
    "docs/a.md",
    "--commit",
    "abc",
];
const MERGE: &[&str] = &[
    "decisions",
    "record",
    "--kind",
    "merge",
    "--repo",
    "o/r",
    "--pr",
    "12",
    "--commit",
    "abc",
];

fn override_args<'a>(name: &'a str, rationale: &'a str) -> Vec<&'a str> {
    vec![
        "overrides",
        "record",
        name,
        "--gate",
        "g",
        "--rationale",
        rationale,
    ]
}

fn assert_defaulted_gate_pass(d: &Value, ws: &Ws, name: &str, session: &str) {
    assert_eq!(d["kind"], "gate-pass", "{}", d);
    assert_eq!(d["outcome"], "passed");
    assert_eq!(d["decided_by"]["kind"], "session");
    assert_eq!(d["decided_by"]["defaulted"], true);
    assert_eq!(
        d["decided_by"]["session"],
        json!([{"value": session, "source": "observed"}])
    );
    assert_eq!(
        d["subject"],
        json!({
            "workflow": name,
            "workflow_session_id": ws.header_session_id(name),
            "state": "gated",
            "gate": "g",
        })
    );
}

// ----- An unattended run, read after it is gone -----

#[test]
fn an_unattended_run_lists_its_three_decisions_after_cleanup_and_workspace_removal() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    let envs: &[(&str, &str)] = &[
        ("KOTO_AGENT_ID", "main"),
        ("CLAUDE_CODE_SESSION_ATTENDED", "0"),
    ];
    let ok = |args: &[&str]| {
        let (code, json, stderr) = ws.run_env(Some(W), envs, args);
        assert_eq!(code, 0, "koto {:?}: {} {}", args, json, stderr);
        json
    };
    ok(ACCEPT);
    let blocked = ok(&["next", "wf"]);
    assert_eq!(blocked["state"], "gated", "{}", blocked);
    ok(&override_args("wf", "the flag is checked by hand"));
    assert_eq!(ok(&["next", "wf"])["state"], "after");
    ok(MERGE);
    let done = ok(&["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert_eq!(done["action"], "done", "{}", done);
    assert!(
        !ws.sessions().join("wf").exists(),
        "a successful end removes the session"
    );

    let check = |items: &[Value]| {
        // All three may share a second, which the listing orders by
        // identifier, so compare the set.
        let mut kinds: Vec<&str> = items.iter().map(|d| d["kind"].as_str().unwrap()).collect();
        kinds.sort();
        assert_eq!(kinds, vec!["accept", "gate-pass", "merge"]);
        for d in items {
            assert_eq!(d["v"], 1);
            assert!(d["id"].as_str().unwrap().starts_with("d-"));
            let t = d["time"].as_str().unwrap();
            assert!(t.ends_with('Z') && t.len() == 20, "{}", t);
            assert!(d["outcome"].is_string());
            assert!(d["subject"].is_object());
            assert_eq!(d["decided_by"]["kind"], "session");
            assert_eq!(d["decided_by"]["session"][0]["value"], W);
            assert_eq!(
                d["decided_by"]["agent"],
                json!([{"value": "main", "source": "observed"}])
            );
            assert_eq!(d["authority"]["kind"], "dispatch");
            assert_eq!(
                d["authority"]["run_mode"],
                json!([{"value": "unattended", "source": "observed"}])
            );
            assert_eq!(d["chain"]["incomplete"], false, "{}", d);
            assert_eq!(
                d["chain"]["links"][0][0]["value"],
                format!("session:{}", COORD)
            );
        }
    };
    let items = ws.list(&["--session", W]);
    check(&items);

    // The workspace that ran the session is reclaimed; the record isn't.
    std::fs::remove_dir_all(ws.home().join("ws")).unwrap();
    let (code, out, stderr) = ws.run_in(
        ws.home(),
        Some(W),
        &[],
        &["decisions", "list", "--session", W],
    );
    assert_eq!(code, 0, "{} {}", out, stderr);
    let items: Vec<Value> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["decision"].clone())
        .collect();
    check(&items);
}

// ----- Gate passes -----

#[test]
fn an_override_records_a_defaulted_gate_pass_beside_its_event() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let out = ws.ok(Some(W), &override_args("wf", "checked by hand"));
    assert_eq!(out["status"], "recorded");
    let id = out["decision"].as_str().unwrap().to_string();

    let shown = ws.show(&id);
    let d = &shown["decision"];
    assert_defaulted_gate_pass(d, &ws, "wf", W);
    assert_eq!(d["reason"], "checked by hand");
    assert_eq!(shown["status"], "in_effect");

    // The override is still listed, and its event carries the decision.
    let list = ws.ok(Some(W), &["overrides", "list", "wf"]);
    assert_eq!(list["overrides"]["count"], 1);
    let ev = ws
        .events("wf")
        .into_iter()
        .find(|e| e["type"] == "gate_override_recorded")
        .unwrap();
    assert_eq!(ev["payload"]["decision_records"][0]["id"], json!(id));
    assert_eq!(&ev["payload"]["decision_records"][0], d);
}

#[test]
fn a_directed_transition_past_an_unmet_gate_records_a_defaulted_gate_pass() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    let out = ws.ok(
        Some(W),
        &[
            "next",
            "wf",
            "--to",
            "after",
            "--rationale",
            "skip the flag",
        ],
    );
    assert_eq!(out["state"], "after", "{}", out);
    let passes = ws.list(&["--kind", "gate-pass"]);
    assert_eq!(passes.len(), 1);
    assert_defaulted_gate_pass(&passes[0], &ws, "wf", W);
    assert_eq!(passes[0]["reason"], "skip the flag");
    let ev = ws
        .events("wf")
        .into_iter()
        .find(|e| e["type"] == "directed_transition")
        .unwrap();
    assert_eq!(ev["payload"]["decision_records"][0]["id"], passes[0]["id"]);
}

#[test]
fn a_named_decider_is_recorded_as_handed_and_not_defaulted() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(
        Some(W),
        &[
            "next",
            "wf",
            "--to",
            "after",
            "--by",
            "person:ana",
            "--surface",
            "terminal",
        ],
    );
    let d = &ws.list(&["--kind", "gate-pass"])[0];
    assert_eq!(d["decided_by"]["kind"], "person");
    assert_eq!(d["decided_by"]["defaulted"], false);
    assert_eq!(
        d["decided_by"]["handle"],
        json!([{"value": "ana", "source": "handed"}])
    );
    // Decider flags on a tick that isn't directed are refused.
    let (code, out) = ws.run(Some(W), &["next", "wf", "--by", "person:ana"]);
    assert_eq!(code, 2, "{}", out);
}

#[test]
fn with_no_decider_a_gate_pass_is_refused_and_nothing_else_is_written() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let before = std::fs::read_to_string(ws.state_path("wf")).unwrap();

    let (code, out) = ws.run(None, &override_args("wf", "checked by hand"));
    assert_eq!(code, 2, "{}", out);
    assert_eq!(out["reason"], "decider_missing");
    assert_eq!(out["field"], "by");
    let refused_id = out["refused_id"].as_str().unwrap().to_string();

    let (code, out) = ws.run(None, &["next", "wf", "--to", "after"]);
    assert_eq!(code, 2, "{}", out);
    assert_eq!(out["error"]["code"], "decider_missing");

    assert_eq!(
        std::fs::read_to_string(ws.state_path("wf")).unwrap(),
        before,
        "the session is untouched"
    );
    assert_eq!(
        ws.ok(None, &["overrides", "list", "wf"])["overrides"]["count"],
        0
    );
    assert!(ws.list(&["--kind", "gate-pass"]).is_empty());
    let refused = ws.list(&["--kind", "refused"]);
    assert_eq!(refused.len(), 2);
    assert!(refused.iter().any(|r| r["id"] == json!(refused_id)));
    for r in &refused {
        assert_eq!(r["subject"]["attempted_kind"], "gate-pass");
        assert_eq!(r["subject"]["refusal_reason"], "decider_missing");
        assert_eq!(r["subject"]["attempted"]["workflow"], "wf");
        assert_eq!(r["subject"]["attempted"]["gate"], "g");
        assert_eq!(r["decided_by"]["session"], Value::Null);
    }
}

#[test]
fn a_directed_transition_past_a_gate_that_passed_records_no_gate_pass() {
    let ws = Ws::new();
    ws.init("wf", GATED_ACCEPTS);
    std::fs::write(ws.cwd().join("ok.flag"), "").unwrap();
    let out = ws.ok(Some(W), &["next", "wf"]);
    assert_eq!(out["state"], "gated", "{}", out);
    // A transition back into the same state passes nothing either. It
    // opens a new visit, so the gate is evaluated again before leaving.
    ws.ok(None, &["next", "wf", "--to", "gated"]);
    assert_eq!(ws.ok(Some(W), &["next", "wf"])["state"], "gated");
    let out = ws.ok(None, &["next", "wf", "--to", "after"]);
    assert_eq!(out["action"], "done", "{}", out);
    assert!(ws.list(&["--kind", "gate-pass"]).is_empty());
    assert!(ws.list(&["--kind", "refused"]).is_empty());
}

#[test]
fn reversing_a_commands_gate_pass_blocks_the_next_tick() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let id = ws.ok(Some(W), &override_args("wf", "checked by hand"))["decision"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(ws.ok(Some(W), &["next", "wf"])["state"], "after");
    let (code, out) = ws.run(Some(COORD), &["decisions", "reverse", &id]);
    assert_eq!(code, 0, "{}", out);
    let out = ws.ok(Some(W), &["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert_eq!(out["action"], "gate_blocked", "{}", out);
    assert_eq!(
        out["blocking_conditions"][0]["category"],
        "decision_reversed"
    );
    assert_eq!(
        out["blocking_conditions"][0]["output"]["reversal"],
        out_id(&ws, &id)
    );
}

#[test]
fn reversing_a_directed_transitions_gate_pass_blocks_the_next_tick() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    assert_eq!(
        ws.ok(Some(W), &["next", "wf", "--to", "after"])["state"],
        "after"
    );
    let id = ws.list(&["--kind", "gate-pass"])[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, out) = ws.run(Some(COORD), &["decisions", "reverse", &id]);
    assert_eq!(code, 0, "{}", out);
    let out = ws.ok(Some(W), &["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert_eq!(out["action"], "gate_blocked", "{}", out);
    assert_eq!(
        out["blocking_conditions"][0]["category"],
        "decision_reversed"
    );
}

fn out_id(ws: &Ws, reversed: &str) -> Value {
    ws.show(reversed)["reversed_by"].clone()
}

// ----- Existing records -----

#[test]
fn todays_decisions_record_writes_a_judgment_and_still_lists_the_choice() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    let out = ws.ok(
        Some(W),
        &[
            "decisions",
            "record",
            "wf",
            "--with-data",
            r#"{"choice":"keep the cache","rationale":"it is cheap"}"#,
        ],
    );
    assert_eq!(out["decisions_recorded"], 1);
    let id = out["decision"].as_str().unwrap().to_string();
    let d = ws.show(&id)["decision"].clone();
    assert_eq!(d["kind"], "judgment");
    assert_eq!(d["outcome"], "settled");
    assert_eq!(d["subject"]["what"], "keep the cache");
    assert_eq!(d["reason"], "it is cheap");
    assert_eq!(d["decided_by"]["kind"], "session");
    assert_eq!(d["decided_by"]["session"][0]["value"], W);
    let list = ws.ok(Some(W), &["decisions", "list", "wf"]);
    assert_eq!(list["decisions"]["items"][0]["choice"], "keep the cache");
    let ev = ws
        .events("wf")
        .into_iter()
        .find(|e| e["type"] == "decision_recorded")
        .unwrap();
    assert_eq!(ev["payload"]["decision_records"][0]["id"], json!(id));
    assert_eq!(ev["payload"]["decision"]["choice"], "keep the cache");

    // With no decider the event is written alone, as before.
    let out = ws.ok(
        None,
        &[
            "decisions",
            "record",
            "wf",
            "--with-data",
            r#"{"choice":"b","rationale":"c"}"#,
        ],
    );
    assert_eq!(out["decisions_recorded"], 2);
    assert!(out.get("decision").is_none());
    assert_eq!(ws.list(&["--kind", "judgment"]).len(), 1);
}

// ----- Disputes -----

fn dispute_args<'a>(by: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "disputes",
        "record",
        "wf",
        "--gate",
        "g",
        "--rule",
        "R1",
        "--reason",
        "the flag file is created later",
    ];
    args.extend_from_slice(by);
    args
}

/// A dispute is a `judgment` about the disputed finding, written beside its
/// `finding_disputed` event, whichever decider form names who disputed it.
#[test]
fn a_dispute_records_a_judgment_about_the_disputed_finding() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);

    // No --by: the observed session, defaulted.
    let out = ws.ok(Some(W), &dispute_args(&[]));
    assert_eq!(out["status"], "recorded");
    let id = out["decision"].as_str().unwrap().to_string();
    let d = ws.show(&id)["decision"].clone();
    assert_eq!(d["kind"], "judgment");
    assert_eq!(d["outcome"], "settled");
    assert_eq!(
        d["subject"],
        json!({
            "what": "finding of rule 'R1' at gate 'g' in state 'gated' disputed",
            "pointer": "workflow:wf/gated",
        })
    );
    assert_eq!(d["evidence"], json!(["gate:g", "rule:R1"]));
    assert_eq!(d["reason"], "the flag file is created later");
    assert_eq!(d["decided_by"]["kind"], "session");
    assert_eq!(d["decided_by"]["defaulted"], true);
    assert_eq!(
        d["decided_by"]["session"],
        json!([{"value": W, "source": "observed"}])
    );
    let ev = ws
        .events("wf")
        .into_iter()
        .rfind(|e| e["type"] == "finding_disputed")
        .unwrap();
    assert_eq!(ev["payload"]["source"], "agent");
    assert_eq!(ev["payload"]["rule_id"], "R1");
    assert_eq!(ev["payload"]["decision_records"][0]["id"], json!(id));

    // --by agent is an alias of --by session: named, not defaulted.
    let id = ws.ok(Some(W), &dispute_args(&["--by", "agent"]))["decision"]
        .as_str()
        .unwrap()
        .to_string();
    let d = ws.show(&id)["decision"].clone();
    assert_eq!(d["decided_by"]["kind"], "session");
    assert_eq!(d["decided_by"]["defaulted"], false);
    assert_eq!(d["decided_by"]["session"][0]["value"], W);

    // A person, with the surface the decision came through.
    let id = ws.ok(
        Some(W),
        &dispute_args(&["--by", "person:ana", "--surface", "terminal"]),
    )["decision"]
        .as_str()
        .unwrap()
        .to_string();
    let d = ws.show(&id)["decision"].clone();
    assert_eq!(d["decided_by"]["kind"], "person");
    assert_eq!(d["decided_by"]["handle"][0]["value"], "ana");
    assert_eq!(d["decided_by"]["surface"], "terminal");
    let ev = ws
        .events("wf")
        .into_iter()
        .rfind(|e| e["type"] == "finding_disputed")
        .unwrap();
    assert_eq!(ev["payload"]["source"], "person");

    // A handed session and a check.
    let id = ws.ok(
        None,
        &dispute_args(&["--by", &format!("session:{}", COORD)]),
    )["decision"]
        .as_str()
        .unwrap()
        .to_string();
    let d = ws.show(&id)["decision"].clone();
    assert_eq!(
        d["decided_by"]["session"],
        json!([{"value": COORD, "source": "handed"}])
    );
    let id = ws.ok(Some(W), &dispute_args(&["--by", "check:lint-audit"]))["decision"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(ws.show(&id)["decision"]["decided_by"]["kind"], "check");

    // A person with no surface is refused before anything is written.
    let before = ws.events("wf").len();
    let (code, body) = ws.run(Some(W), &dispute_args(&["--by", "person:ana"]));
    assert_eq!(code, 2, "{}", body);
    assert_eq!(body["field"], "surface");
    assert_eq!(ws.events("wf").len(), before);

    assert_eq!(ws.list(&["--kind", "judgment"]).len(), 5);
}

#[test]
fn a_disputes_lost_append_is_repaired_by_the_next_tick() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the decision store's test variables");
        return;
    }
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let (code, out, stderr) = ws.run_env(Some(W), &[(FAULT_ENV, "append")], &dispute_args(&[]));
    assert_eq!(code, 0, "{} {}", out, stderr);
    let id = out["decision"].as_str().unwrap().to_string();
    assert!(ws.month_lines().is_empty(), "the append was lost");
    ws.ok(Some(W), &["next", "wf"]);
    let lines = ws.month_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["id"], json!(id));
    assert_eq!(lines[0]["kind"], "judgment");
    assert_eq!(lines[0]["repaired_from"], "wf");
}

// ----- Repair -----

#[test]
fn a_lost_append_is_repaired_by_the_next_tick() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the decision store's test variables");
        return;
    }
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let (code, out, stderr) = ws.run_env(
        Some(W),
        &[(FAULT_ENV, "append")],
        &override_args("wf", "checked by hand"),
    );
    assert_eq!(code, 0, "{} {}", out, stderr);
    assert!(
        stderr.contains("was not written to the decision store"),
        "{}",
        stderr
    );
    let id = out["decision"].as_str().unwrap().to_string();
    assert!(ws.month_lines().is_empty(), "the append was lost");

    let next = ws.ok(Some(W), &["next", "wf"]);
    assert_eq!(next["state"], "after");
    let lines = ws.month_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["id"], json!(id));
    assert_eq!(lines[0]["repaired_from"], "wf");
    let ev = ws
        .events("wf")
        .into_iter()
        .find(|e| e["type"] == "gate_override_recorded")
        .unwrap();
    let mut expected = ev["payload"]["decision_records"][0].clone();
    expected["repaired_from"] = json!("wf");
    assert_eq!(lines[0], expected, "the identical decision, marked");

    // Later ticks find it present.
    ws.ok(Some(W), &["next", "wf"]);
    assert_eq!(ws.month_lines().len(), 1);
}

#[test]
fn a_decision_lost_on_the_terminal_tick_survives_the_cleanup() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the decision store's test variables");
        return;
    }
    let ws = Ws::new();
    ws.init("wf", GATE_TO_DONE);
    let (code, out, stderr) = ws.run_env(
        Some(W),
        &[(FAULT_ENV, "append")],
        &["next", "wf", "--to", "done"],
    );
    assert_eq!(code, 0, "{} {}", out, stderr);
    assert_eq!(out["action"], "done", "{}", out);
    assert!(
        !ws.sessions().join("wf").exists(),
        "the session was removed"
    );
    let lines = ws.month_lines();
    assert_eq!(lines.len(), 1, "{}", stderr);
    assert_eq!(lines[0]["kind"], "gate-pass");
    assert_eq!(lines[0]["repaired_from"], "wf");
}

#[cfg(unix)]
#[test]
fn a_store_lock_timeout_fails_neither_the_tick_nor_the_gate_and_is_retried() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the decision store's test variables");
        return;
    }
    use std::os::unix::io::AsRawFd;
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let (code, out, _) = ws.run_env(
        Some(W),
        &[(FAULT_ENV, "append")],
        &override_args("wf", "checked by hand"),
    );
    assert_eq!(code, 0, "{}", out);

    std::fs::create_dir_all(ws.store()).unwrap();
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ws.store().join("store.lock"))
        .unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let (code, next, stderr) = ws.run_env(Some(W), &[(LOCK_WAIT_ENV, "100")], &["next", "wf"]);
    assert_eq!(code, 0, "{} {}", next, stderr);
    assert_eq!(next["state"], "after", "the tick goes on: {}", next);
    assert!(stderr.contains("repairing the decisions"), "{}", stderr);
    assert!(ws.month_lines().is_empty());
    drop(holder);

    ws.ok(Some(W), &["next", "wf"]);
    assert_eq!(ws.month_lines().len(), 1, "the next tick repairs it");
}

// ----- Import -----

fn coordinator_source(ws: &Ws) -> PathBuf {
    let p = ws.home().join("table.json");
    std::fs::write(
        &p,
        json!({
            "v": 1,
            "format": "coordinator-decisions",
            "source": "record:o/r#12",
            "coordinator": COORD,
            "entries": [
                {"entry": "1", "time": "2026-09-30T12:00Z", "kind": "judgment",
                 "question": "Postgres or SQLite?", "decided_by": "coordinator",
                 "reason": "smaller footprint", "dispatch_id": "disp-1"},
                {"entry": "2", "time": "2026-10-01T09:30Z", "kind": "escalate",
                 "question": "Drop the cache?", "recipient": "person:ana",
                 "decided_by": "coordinator"},
                {"entry": "3", "time": "2026-10-02T08:00Z", "kind": "judgment",
                 "question": "Ship today?", "decided_by": "person:ana",
                 "surface": "terminal"},
            ],
        })
        .to_string(),
    )
    .unwrap();
    p
}

#[test]
fn an_import_keeps_handed_deciders_and_adds_nothing_the_second_time() {
    let ws = Ws::new();
    let src = coordinator_source(&ws);
    let first = ws.ok(Some(W), &["decisions", "import", src.to_str().unwrap()]);
    let imported = first["imported"].as_array().unwrap();
    assert_eq!(imported.len(), 3, "{}", first);
    let id = imported[0]["id"].as_str().unwrap();
    let d = ws.show(id)["decision"].clone();
    assert_eq!(d["time"], "2026-09-30T12:00:00Z");
    assert!(id.starts_with("d-202609-"));
    assert!(d["recorded_at"].is_string());
    assert_eq!(d["import_source"], "record:o/r#12#1");
    assert_eq!(
        d["decided_by"]["session"],
        json!([{"value": COORD, "source": "handed"}])
    );
    assert_eq!(
        d["authority"]["dispatch_id"],
        json!([{"value": "disp-1", "source": "handed"}])
    );
    assert_eq!(d["chain"]["incomplete"], true);
    let person = ws.show(imported[2]["id"].as_str().unwrap())["decision"].clone();
    assert_eq!(person["decided_by"]["kind"], "person");
    assert_eq!(person["decided_by"]["handle"][0]["source"], "handed");

    let again = ws.ok(Some(W), &["decisions", "import", src.to_str().unwrap()]);
    assert_eq!(again["imported"], json!([]));
    assert_eq!(again["already_imported"], first["imported"]);
    assert_eq!(ws.month_lines().len(), 3);
    let ledger = std::fs::read_to_string(ws.store().join("imports.jsonl")).unwrap();
    assert_eq!(ledger.lines().count(), 3);
}

#[test]
fn a_reversal_racing_a_repeat_import_leaves_one_decision_and_one_reversal() {
    let ws = Ws::new();
    let src = coordinator_source(&ws);
    let first = ws.ok(Some(W), &["decisions", "import", src.to_str().unwrap()]);
    let id = first["imported"][0]["id"].as_str().unwrap().to_string();
    for _ in 0..3 {
        let spawn = |args: &[&str], session: &str| {
            ws.std_cmd(&ws.cwd(), Some(session))
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        let import = spawn(&["decisions", "import", src.to_str().unwrap()], W);
        let reverse = spawn(&["decisions", "reverse", &id], COORD);
        let import = import.wait_with_output().unwrap();
        let reverse = reverse.wait_with_output().unwrap();
        assert!(import.status.success());
        let r: Value = serde_json::from_slice(&reverse.stdout).unwrap();
        // The first round reverses; later rounds are refused as already
        // reversed. Either way exactly one reversal exists.
        assert!(matches!(reverse.status.code(), Some(0) | Some(1)), "{}", r);
    }
    assert_eq!(
        ws.month_lines()
            .iter()
            .filter(|d| d["kind"] != "refused")
            .count(),
        3
    );
    let reversals = std::fs::read_to_string(ws.store().join("reversals.jsonl")).unwrap();
    assert_eq!(reversals.lines().count(), 1);
    assert_eq!(ws.show(&id)["status"], "reversed");
}

#[test]
fn an_import_source_that_is_a_symlink_or_malformed_is_refused() {
    let ws = Ws::new();
    let src = coordinator_source(&ws);
    let link = ws.home().join("link.json");
    std::os::unix::fs::symlink(&src, &link).unwrap();
    let (code, out) = ws.run(Some(W), &["decisions", "import", link.to_str().unwrap()]);
    assert_eq!(code, 2, "{}", out);
    assert_eq!(out["field"], "source");
    let bad = ws.home().join("bad.json");
    std::fs::write(&bad, r#"{"v": 1, "format": "coordinator-decisions", "source": "s", "entries": [{"entry": "1", "time": "2026-10-01T00:00Z", "kind": "reverse", "decided_by": "session:x"}]}"#).unwrap();
    let (code, out) = ws.run(Some(W), &["decisions", "import", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{}", out);
    assert_eq!(out["field"], "kind");
    assert!(!ws.store().exists(), "nothing was written");
}

// ----- Privacy -----

#[test]
fn the_run_journal_carries_decision_identifiers_and_kinds_never_reasons() {
    let ws = Ws::new();
    ws.init("wf", GATED);
    ws.ok(Some(W), &["next", "wf"]);
    let reason = "planted-reason-7f3a";
    let id = ws.ok(Some(W), &override_args("wf", reason))["decision"]
        .as_str()
        .unwrap()
        .to_string();
    ws.ok(
        Some(W),
        &[
            "decisions",
            "record",
            "wf",
            "--with-data",
            &json!({"choice": "planted-choice-7f3a", "rationale": reason}).to_string(),
        ],
    );
    let journal = ws.journal();
    assert!(!journal.is_empty());
    assert!(!journal.contains("planted"), "{}", journal);
    let records: Vec<Value> = journal
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["kind"] == "decision_recorded")
        .collect();
    assert_eq!(records.len(), 2, "{}", journal);
    assert_eq!(records[0]["koto.decision.id"], json!(id));
    assert_eq!(records[0]["koto.decision.kind"], "gate-pass");
    assert_eq!(records[1]["koto.decision.kind"], "judgment");
    for r in &records {
        let keys: Vec<&str> = r
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| k.starts_with("koto.decision"))
            .collect();
        assert_eq!(keys.len(), 2, "{}", r);
    }
}

// ----- Detection -----

#[test]
fn koto_version_names_the_decision_store_format() {
    let ws = Ws::new();
    let v = ws.ok(None, &["version", "--json"]);
    assert_eq!(v["decision_store"], 1, "{}", v);
}

#[cfg(unix)]
#[test]
fn a_terminal_tick_that_cant_repair_keeps_the_session_for_the_next_tick() {
    use std::os::unix::io::AsRawFd;
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the decision store's test variables");
        return;
    }
    let ws = Ws::new();
    ws.init("wf", GATE_TO_DONE);
    std::fs::create_dir_all(ws.store()).unwrap();
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ws.store().join("store.lock"))
        .unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let (code, out, stderr) = ws.run_env(
        Some(W),
        &[(FAULT_ENV, "append"), (LOCK_WAIT_ENV, "100")],
        &["next", "wf", "--to", "done"],
    );
    assert_eq!(code, 0, "{} {}", out, stderr);
    assert_eq!(out["action"], "done", "{}", out);
    assert!(ws.sessions().join("wf").exists(), "kept: {}", stderr);
    assert!(ws.month_lines().is_empty());
    drop(holder);

    ws.ok(Some(W), &["next", "wf"]);
    assert_eq!(ws.month_lines().len(), 1, "the next tick repairs it");
    assert!(!ws.sessions().join("wf").exists(), "and then removes it");
}
