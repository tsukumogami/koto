//! `koto review record`: document review events recorded as telemetry,
//! through the built binary.
//!
//! Every case runs `koto` with a cleared environment and its sessions (and
//! so its run journal) under the test's temporary directory.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// A distinctive string fed through every refusal path; no refusal may
/// print it back.
const SENTINEL: &str = "PLANTED-DOC-TEXT-91c4";

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
        let mut cmd = koto_command::koto_std();
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

    /// Start `name` on [`template`] with a gate exiting `gate_exit`.
    fn init(&self, name: &str, gate_exit: i32) {
        let script = self.path().join(format!("{name}-lint.sh"));
        std::fs::write(
            &script,
            format!(
                "echo '::koto-finding::{{\"rule_id\":\"L1\",\"level\":\"error\",\"message\":\"lint\"}}'\nexit {gate_exit}\n"
            ),
        )
        .unwrap();
        let tpl = self.path().join(format!("{name}.md"));
        std::fs::write(&tpl, template(&format!("sh {}", script.display()))).unwrap();
        let out = self.run(&["init", name, "--template", tpl.to_str().unwrap()]);
        assert!(out.status.success(), "init: {}", describe(&out));
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("koto {args:?} should print JSON: {}", describe(&out)))
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.path()
            .join("sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.log_path(name)).unwrap()
    }

    fn events(&self, name: &str, event_type: &str) -> Vec<Value> {
        self.log(name)
            .lines()
            .skip(1)
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["type"] == event_type)
            .collect()
    }

    fn journal_path(&self) -> PathBuf {
        self.path().join("sessions").join("_run_journal.jsonl")
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.journal_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// `koto review record <name> <args>`.
    fn record(&self, name: &str, args: &[&str]) -> Output {
        self.koto()
            .args(["review", "record", name])
            .args(args)
            .output()
            .unwrap()
    }

    /// Write `content` to a file under the test directory and return its
    /// path.
    fn file(&self, name: &str, content: &str) -> String {
        let path = self.path().join(name);
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
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

/// `draft` holds a command gate (`lint`) and waits for a verdict; `done` is
/// terminal. The template declares two seats and three sections.
fn template(lint: &str) -> String {
    format!(
        r#"---
name: review-record
version: "1.0"
initial_state: draft
review_vocabulary:
  seats: [clarity, scope]
  sections: [goals, frontmatter, document]
states:
  draft:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    gates:
      lint:
        type: command
        command: '{lint}'
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## draft

Draft.

## done

Done.
"#
    )
}

/// A three-seat round's verdicts: two rules over two sections per seat,
/// one seat the template doesn't declare and one section it doesn't.
fn verdicts() -> Value {
    let mut entries = Vec::new();
    for seat in ["clarity", "scope", "intruder"] {
        for (rule, section) in [("rs-001", "goals"), ("rs-002", "not-a-section")] {
            entries.push(json!({
                "rule_id": rule,
                "section": section,
                "verdict": if seat == "scope" { "FAIL" } else { "PASS" },
                "seat": seat,
                "round": 1,
                "doc_type": "prd",
                "schema": "prd/v1",
            }));
        }
    }
    Value::Array(entries)
}

fn records<'j>(journal: &'j [Value], kind: &str) -> Vec<&'j Value> {
    journal.iter().filter(|r| r["kind"] == kind).collect()
}

#[test]
fn a_full_round_appends_its_events_and_the_journal_relays_each() {
    let env = Env::new();
    env.init("wf", 1);

    let out = env.record(
        "wf",
        &[
            "round-started",
            "--doc-type",
            "prd",
            "--schema",
            "prd/v1",
            "--round",
            "1",
            "--seat-count",
            "3",
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({"status": "recorded", "event": "round-started", "count": 1})
    );

    let file = env.file("verdicts.json", &verdicts().to_string());
    let out = env.record("wf", &["verdicts", "--file", &file]);
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["count"], 6);

    let out = env.record(
        "wf",
        &[
            "decision",
            "--doc-type",
            "prd",
            "--schema",
            "prd/v1",
            "--round",
            "1",
            "--decision",
            "rerun",
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));

    let checks = json!([
        {"rule_id": "fm-schema", "outcome": "passed", "doc_type": "prd", "schema": "prd/v9"},
        {"rule_id": "ac-binary", "outcome": "failed", "doc_type": "prd"},
    ]);
    let file = env.file("checks.json", &checks.to_string());
    let out = env.record("wf", &["doc-checks", "--file", &file]);
    assert!(out.status.success(), "{}", describe(&out));

    // Session events, in koto's words.
    let round = env.events("wf", "review_round_started");
    assert_eq!(round.len(), 1);
    assert_eq!(
        round[0]["payload"],
        json!({"doc_type": "prd", "schema": "prd/v1", "round": 1, "seat_count": 3, "fallback": false})
    );
    let v = env.events("wf", "review_verdict_recorded");
    assert_eq!(v.len(), 6);
    let stored: Vec<(String, String, String)> = v
        .iter()
        .map(|e| {
            (
                e["payload"]["seat"].as_str().unwrap().to_string(),
                e["payload"]["section"].as_str().unwrap().to_string(),
                e["payload"]["verdict"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        stored[..4],
        [
            ("clarity".into(), "goals".into(), "pass".into()),
            ("clarity".into(), "other".into(), "pass".into()),
            ("scope".into(), "goals".into(), "fail".into()),
            ("scope".into(), "other".into(), "fail".into()),
        ]
    );
    assert_eq!(stored[4].0, "other");
    let d = env.events("wf", "review_decision_recorded");
    assert_eq!(
        d[0]["payload"],
        json!({"doc_type": "prd", "schema": "prd/v1", "round": 1, "decision": "rerun"})
    );
    let c = env.events("wf", "doc_check_recorded");
    assert_eq!(c.len(), 2);
    assert_eq!(c[0]["payload"]["schema"], "other");
    assert_eq!(c[1]["payload"]["schema"], "absent");
    assert_eq!(c[1]["payload"]["outcome"], "failed");

    // One journal record per event, each on its own koto.seq, which is the
    // event's seq in the log.
    let journal = env.journal();
    let kinds = [
        ("review_round", "review_round_started", 1),
        ("review_verdict", "review_verdict_recorded", 6),
        ("review_decision", "review_decision_recorded", 1),
        ("doc_checked", "doc_check_recorded", 2),
    ];
    let mut seqs = Vec::new();
    for (kind, event, count) in kinds {
        let r = records(&journal, kind);
        assert_eq!(r.len(), count, "{kind}");
        let logged: Vec<u64> = env
            .events("wf", event)
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect();
        let relayed: Vec<u64> = r.iter().map(|r| r["koto.seq"].as_u64().unwrap()).collect();
        assert_eq!(relayed, logged, "{kind}");
        seqs.extend(relayed);
    }
    let distinct: std::collections::BTreeSet<u64> = seqs.iter().copied().collect();
    assert_eq!(distinct.len(), seqs.len());

    let r = records(&journal, "review_round")[0];
    assert_eq!(r["koto.doc.type"], "prd");
    assert_eq!(r["koto.doc.schema"], "prd/v1");
    assert_eq!(r["koto.review.round"], 1);
    assert_eq!(r["koto.review.seat_count"], 3);
    assert_eq!(r["koto.review.fallback"], false);
    let r = records(&journal, "review_verdict");
    assert_eq!(r[0]["koto.review.seat"], "clarity");
    assert_eq!(r[0]["koto.rule.id"], "rs-001");
    assert_eq!(r[0]["koto.review.section"], "goals");
    assert_eq!(r[0]["koto.review.verdict"], "pass");
    assert_eq!(r[5]["koto.review.seat"], "other");
    assert_eq!(r[5]["koto.review.section"], "other");
    assert_eq!(
        records(&journal, "review_decision")[0]["koto.review.decision"],
        "rerun"
    );
    let r = records(&journal, "doc_checked");
    assert_eq!(r[0]["koto.doc.check_outcome"], "passed");
    assert_eq!(r[0]["koto.doc.schema"], "other");
    assert_eq!(r[1]["koto.rule.id"], "ac-binary");

    // Nothing the caller wrote outside a declared or closed value reaches the
    // log or the journal.
    let raw = std::fs::read_to_string(env.journal_path()).unwrap();
    for absent in ["intruder", "not-a-section", "prd/v9"] {
        assert!(!raw.contains(absent), "{absent} in the journal");
        assert!(!env.log("wf").contains(absent), "{absent} in the log");
    }
}

#[test]
fn a_fallback_round_is_recorded_as_one() {
    let env = Env::new();
    env.init("wf", 1);
    let out = env.record(
        "wf",
        &[
            "round-started",
            "--doc-type",
            "plan",
            "--round",
            "2",
            "--seat-count",
            "1",
            "--fallback",
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let e = &env.events("wf", "review_round_started")[0]["payload"];
    assert_eq!(e["fallback"], true);
    assert_eq!(e["schema"], "absent");
    assert_eq!(
        records(&env.journal(), "review_round")[0]["koto.review.fallback"],
        true
    );
}

/// Recording review events changes nothing `koto next` shows, no gate
/// decision and no state, and leaves the decider's mode as it was.
#[test]
fn review_events_change_no_response_gate_decision_or_state() {
    let env = Env::new();
    env.init("plain", 1);
    env.init("reviewed", 1);
    let decider_mode = |env: &Env| env.run(&["config", "get", "decider.mode"]);
    let mode_before = decider_mode(&env);

    // A second call settles what a repeated call prints.
    env.json(&["next", "plain"]);
    env.json(&["next", "reviewed"]);
    let file = env.file("verdicts.json", &verdicts().to_string());
    let out = env.record("reviewed", &["verdicts", "--file", &file]);
    assert!(out.status.success(), "{}", describe(&out));
    let out = env.record(
        "reviewed",
        &[
            "decision",
            "--doc-type",
            "prd",
            "--round",
            "1",
            "--decision",
            "stop",
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));

    let plain = env.json(&["next", "plain"]);
    let reviewed = env.json(&["next", "reviewed"]);
    assert_eq!(reviewed["state"], "draft");
    assert_eq!(
        strip_names(reviewed.clone()),
        strip_names(plain.clone()),
        "the response is the one a session without review events gets"
    );
    let outcomes = |name: &str| -> Vec<Value> {
        env.events(name, "gate_evaluated")
            .iter()
            .map(|e| json!([e["payload"]["gate"], e["payload"]["outcome"]]))
            .collect()
    };
    assert_eq!(outcomes("reviewed"), outcomes("plain"));
    let status = |name: &str| env.json(&["status", name])["current_state"].clone();
    assert_eq!(status("reviewed"), status("plain"));
    let mode_after = decider_mode(&env);
    assert_eq!(mode_after.stdout, mode_before.stdout);
    assert_eq!(mode_after.status.code(), mode_before.status.code());
}

/// A response with the session's name taken out, so two sessions' responses
/// compare.
fn strip_names(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::String(s) => {
                *s = s.replace("reviewed", "S").replace("plain", "S");
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            Value::Object(o) => o.values_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[test]
fn an_invalid_entry_mid_batch_leaves_the_log_byte_for_byte_unchanged() {
    let env = Env::new();
    env.init("wf", 1);
    let before = env.log("wf");
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();
    let mut batch = verdicts();
    batch[3]["verdict"] = Value::from("MAYBE");
    let file = env.file("verdicts.json", &batch.to_string());
    let out = env.record("wf", &["verdicts", "--file", &file]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["command"], "review record");
    assert_eq!(body["error"]["code"], "invalid_value");
    assert_eq!(body["error"]["entry"], 3);
    assert_eq!(body["error"]["field"], "verdict");
    assert_eq!(env.log("wf"), before);
    assert_eq!(
        std::fs::read_to_string(env.journal_path()).unwrap(),
        journal_before
    );
}

/// Every refusal path through the binary, each fed [`SENTINEL`]; none
/// appends anything or prints the sentinel on either stream.
#[test]
fn refusals_exit_two_append_nothing_and_never_echo_what_they_rejected() {
    let env = Env::new();
    env.init("wf", 1);
    let before = env.log("wf");

    let entry = |field: &str, value: Value| {
        let mut e = verdicts()[0].clone();
        e[field] = value;
        Value::Array(vec![e]).to_string()
    };
    let check = |field: &str, value: Value| {
        let mut e = json!({"rule_id": "r", "outcome": "passed", "doc_type": "plan"});
        e[field] = value;
        Value::Array(vec![e]).to_string()
    };
    let too_many = Value::Array(vec![verdicts()[0].clone(); 201]).to_string();
    let file_cases: Vec<(&str, String, &str)> = vec![
        ("verdicts", format!("{{\"{SENTINEL}\":"), "invalid_file"),
        ("verdicts", format!("\"{SENTINEL}\""), "invalid_file"),
        ("verdicts", format!("[\"{SENTINEL}\"]"), "invalid_entry"),
        ("verdicts", entry(SENTINEL, Value::from(1)), "invalid_entry"),
        (
            "verdicts",
            entry("verdict", Value::from(SENTINEL)),
            "invalid_value",
        ),
        (
            "verdicts",
            entry("doc_type", Value::from(SENTINEL)),
            "invalid_value",
        ),
        (
            "verdicts",
            entry("rule_id", Value::from(format!("{SENTINEL} x"))),
            "invalid_value",
        ),
        (
            "verdicts",
            entry("round", Value::from(SENTINEL)),
            "invalid_value",
        ),
        (
            "verdicts",
            entry("seat", json!([SENTINEL])),
            "invalid_entry",
        ),
        (
            "verdicts",
            entry("schema", json!({ SENTINEL: 1 })),
            "invalid_entry",
        ),
        ("verdicts", too_many, "batch_too_large"),
        (
            "doc-checks",
            check("outcome", Value::from(SENTINEL)),
            "invalid_value",
        ),
        (
            "doc-checks",
            check("schema", json!([SENTINEL])),
            "invalid_entry",
        ),
        ("doc-checks", check(SENTINEL, Value::Null), "invalid_entry"),
    ];
    for (i, (event, content, code)) in file_cases.iter().enumerate() {
        let file = env.file(&format!("case-{i}.json"), content);
        let out = env.record("wf", &[event, "--file", &file]);
        assert_eq!(out.status.code(), Some(2), "case {i}: {}", describe(&out));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(body["error"]["code"], *code, "case {i}: {body}");
        assert!(
            !describe(&out).contains(SENTINEL),
            "case {i}: {}",
            describe(&out)
        );
    }

    // An unreadable file and one that isn't UTF-8.
    let missing = env.path().join(SENTINEL);
    let mut raw = SENTINEL.as_bytes().to_vec();
    raw.extend([0xff, 0xfe]);
    let not_utf8 = env.path().join("not-utf8.json");
    std::fs::write(&not_utf8, raw).unwrap();
    for path in [missing, not_utf8] {
        let out = env.record("wf", &["verdicts", "--file", path.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(body["error"]["code"], "invalid_file");
        assert!(!describe(&out).contains(SENTINEL), "{}", describe(&out));
    }

    // Flags outside their sets.
    for args in [
        vec![
            "round-started",
            "--doc-type",
            SENTINEL,
            "--round",
            "1",
            "--seat-count",
            "1",
        ],
        vec![
            "round-started",
            "--doc-type",
            "prd",
            "--round",
            SENTINEL,
            "--seat-count",
            "1",
        ],
        vec![
            "round-started",
            "--doc-type",
            "prd",
            "--round",
            "0",
            "--seat-count",
            "1",
        ],
        vec![
            "round-started",
            "--doc-type",
            "prd",
            "--round",
            "1",
            "--seat-count",
            SENTINEL,
        ],
        vec![
            "decision",
            "--doc-type",
            "prd",
            "--round",
            "1",
            "--decision",
            SENTINEL,
        ],
    ] {
        let out = env.record("wf", &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", describe(&out));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(body["error"]["code"], "invalid_value", "{args:?}");
        assert!(
            !describe(&out).contains(SENTINEL),
            "{args:?}: {}",
            describe(&out)
        );
    }

    assert_eq!(env.log("wf"), before, "no refusal appended anything");
}

#[test]
fn a_missing_session_exits_one_and_a_finished_one_is_refused() {
    let env = Env::new();
    let args = [
        "decision",
        "--doc-type",
        "prd",
        "--round",
        "1",
        "--decision",
        "proceed",
    ];
    let out = env.record("wf", &args);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!env.path().join("sessions").join("wf").exists());

    // A session at its terminal state, kept on disk.
    env.init("done", 0);
    let resp = env.json(&[
        "next",
        "done",
        "--with-data",
        r#"{"verdict":"finish"}"#,
        "--no-cleanup",
    ]);
    assert_eq!(resp["state"], "done", "{resp}");
    let before = env.log("done");
    let out = env.record("done", &args);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "session_terminal");
    assert_eq!(env.log("done"), before);

    // A cancelled session.
    env.init("gone", 1);
    let out = env.run(&["cancel", "gone"]);
    assert!(out.status.success(), "{}", describe(&out));
    let before = env.log("gone");
    let out = env.record("gone", &args);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "session_terminal");
    assert_eq!(env.log("gone"), before);
}

/// A journal that can't be written changes neither the verb's output nor
/// the events it appends.
#[test]
fn a_journal_write_failure_changes_nothing_the_verb_reports() {
    let ok = Env::new();
    ok.init("wf", 1);
    let broken = Env::new();
    broken.init("wf", 1);
    // A directory where the journal file goes: every write fails.
    std::fs::remove_file(broken.journal_path()).unwrap();
    std::fs::create_dir(broken.journal_path()).unwrap();

    let args = [
        "round-started",
        "--doc-type",
        "design",
        "--schema",
        "design/v1",
        "--round",
        "1",
        "--seat-count",
        "3",
    ];
    let a = ok.record("wf", &args);
    let b = broken.record("wf", &args);
    assert!(b.status.success(), "{}", describe(&b));
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(a.status.code(), b.status.code());
    let payload = |env: &Env| env.events("wf", "review_round_started")[0]["payload"].clone();
    assert_eq!(payload(&ok), payload(&broken));
    assert_eq!(records(&ok.journal(), "review_round").len(), 1);
}

/// The per-session cap through the binary: ten full batches fill it, the
/// next entry is refused with `cap_reached`, and the log is left as it was.
#[test]
fn a_call_past_the_session_cap_is_refused_and_appends_nothing() {
    let env = Env::new();
    env.init("wf", 1);
    let batch = Value::Array(vec![verdicts()[0].clone(); 200]).to_string();
    let file = env.file("full.json", &batch);
    for i in 0..10 {
        let out = env.record("wf", &["verdicts", "--file", &file]);
        assert!(out.status.success(), "batch {i}: {}", describe(&out));
    }
    assert_eq!(env.events("wf", "review_verdict_recorded").len(), 2000);
    let before = env.log("wf");
    let out = env.record(
        "wf",
        &[
            "decision",
            "--doc-type",
            "prd",
            "--round",
            "1",
            "--decision",
            "stop",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "cap_reached");
    assert_eq!(env.log("wf"), before);
}

#[test]
fn an_empty_batch_records_nothing_and_succeeds() {
    let env = Env::new();
    env.init("wf", 1);
    let before = env.log("wf");
    let file = env.file("empty.json", "[]");
    let out = env.record("wf", &["doc-checks", "--file", &file]);
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({"status": "recorded", "event": "doc-checks", "count": 0})
    );
    assert_eq!(env.log("wf"), before);
}

#[test]
fn a_missing_workflow_name_is_refused_with_exit_two() {
    let env = Env::new();
    let out = env.run(&[
        "review",
        "record",
        "decision",
        "--doc-type",
        "prd",
        "--round",
        "1",
        "--decision",
        "stop",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid_value");
    assert_eq!(body["error"]["field"], "<NAME>");
}

#[test]
fn the_help_lists_the_four_events_as_telemetry() {
    let env = Env::new();
    let out = env.run(&["review", "record", "--help"]);
    assert!(out.status.success(), "{}", describe(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    for sub in ["round-started", "verdicts", "decision", "doc-checks"] {
        assert!(help.contains(sub), "{sub} missing from: {help}");
    }
    let out = env.run(&["review", "--help"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("telemetry"),
        "{}",
        describe(&out)
    );
}

/// A log written before review events existed, and one an older koto reads
/// with review events in it, both read back: an unreadable review payload
/// degrades to an unknown event instead of breaking the log.
#[test]
fn logs_with_and_without_review_events_still_read() {
    let env = Env::new();
    env.init("wf", 1);
    let status_before = env.json(&["status", "wf"]);
    let mut log = env.log("wf");
    let next_seq = log.lines().count();
    log.push_str(&format!(
        "{}\n",
        json!({
            "seq": next_seq,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "review_verdict_recorded",
            "payload": {"from_a_future_koto": true},
        })
    ));
    std::fs::write(env.log_path("wf"), log).unwrap();
    let status_after = env.json(&["status", "wf"]);
    assert_eq!(
        status_after["current_state"],
        status_before["current_state"]
    );
    assert_eq!(status_after["is_terminal"], false);
}
