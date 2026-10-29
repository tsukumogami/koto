//! `koto review record ... code-round`: a code review round (a pull request
//! review panel) recorded as kinds of its own, through the built binary.
//!
//! Every case runs `koto` with a cleared environment, so no `KOTO_*`
//! variable reaches it, and `HOME` set to the test's temporary directory.
//! Before recording, each case asks `koto session dir` where the session
//! lives and requires the answer to be under that directory, so the
//! session, its log and the run journal beside it are the test's own.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// A distinctive string fed through the refusal paths; no refusal may print
/// it back.
const SENTINEL: &str = "PLANTED-PANEL-TEXT-5e2b";

/// A commit id; only its digest may be stored.
const HEAD: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    /// The sessions directory `koto session dir` reported at [`Env::init`].
    sessions: std::cell::OnceCell<PathBuf>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The resolved path, so it compares with what koto prints.
        let home = dir.path().canonicalize().unwrap();
        Env {
            _dir: dir,
            home,
            sessions: std::cell::OnceCell::new(),
        }
    }

    fn home(&self) -> &Path {
        &self.home
    }

    /// koto with nothing inherited: no `KOTO_*` variable, `HOME` the
    /// scratch directory.
    fn koto(&self) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(self.home())
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.koto().args(args).output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("koto {args:?} should print JSON: {}", describe(&out)))
    }

    /// Start `name` on [`TEMPLATE`], then require `koto session dir` to put
    /// it under the scratch home.
    fn init(&self, name: &str) {
        let tpl = self.home().join(format!("{name}.md"));
        std::fs::write(&tpl, TEMPLATE).unwrap();
        let out = self.run(&["init", name, "--template", tpl.to_str().unwrap()]);
        assert!(out.status.success(), "init: {}", describe(&out));
        let out = self.run(&["session", "dir", name]);
        assert!(out.status.success(), "session dir: {}", describe(&out));
        let dir = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        assert!(
            dir.starts_with(self.home()),
            "koto session dir {} is not under the scratch home {}",
            dir.display(),
            self.home().display()
        );
        let sessions = dir.parent().unwrap().to_path_buf();
        assert_eq!(self.sessions.get_or_init(|| sessions.clone()), &sessions);
        assert!(self.log_path(name).is_file(), "{}", dir.display());
        // The session's start is already in the journal, so the journal
        // read below is the one koto writes.
        assert!(
            self.journal_text().contains("\"session_started\""),
            "no journal at {}",
            self.journal_path().display()
        );
    }

    /// The sessions directory, as `koto session dir` reported it.
    fn sessions(&self) -> &Path {
        self.sessions.get().expect("init ran")
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.sessions()
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

    /// The run journal, in the koto root that holds the sessions directory
    /// (`$HOME/.koto` here, with no `KOTO_HOME`).
    fn journal_path(&self) -> PathBuf {
        self.sessions().parent().unwrap().join("_run_journal.jsonl")
    }

    fn journal_text(&self) -> String {
        std::fs::read_to_string(self.journal_path()).unwrap_or_default()
    }

    fn journal(&self) -> Vec<Value> {
        self.journal_text()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Write `content` to a file under the scratch directory and return its
    /// path.
    fn file(&self, name: &str, content: &str) -> String {
        let path = self.home().join(name);
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    /// `koto review record <name> code-round --round <round> --head <head>
    /// --file <batch>`.
    fn code_round(&self, name: &str, round: &str, head: &str, batch: &str) -> Output {
        let file = self.file("seats.json", batch);
        self.koto()
            .args(["review", "record", name, "code-round"])
            .args(["--round", round, "--head", head, "--file", &file])
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

/// `review` waits for a verdict; `done` is terminal. The template declares
/// three seats.
const TEMPLATE: &str = r#"---
name: code-review-record
version: "1.0"
initial_state: review
review_vocabulary:
  seats: [correctness, security, maintainability]
states:
  review:
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

## review

Review.

## done

Done.
"#;

fn seat(seat: &str, model: &str, verdict: &str) -> Value {
    json!({"seat": seat, "model": model, "verdict": verdict})
}

/// A three-seat panel, every seat declared.
fn three_seats() -> String {
    json!([
        seat("correctness", "claude-opus-4-5", "pass"),
        seat("security", "claude-opus-4-5", "FAIL"),
        seat("maintainability", "gpt-5.1", "pass"),
    ])
    .to_string()
}

fn records<'j>(journal: &'j [Value], kind: &str) -> Vec<&'j Value> {
    journal.iter().filter(|r| r["kind"] == kind).collect()
}

/// The fields every relayed record carries.
const RELAY_FIELDS: [&str; 8] = [
    "kind",
    "v",
    "at",
    "session",
    "koto.session.id",
    "koto.run.id",
    "koto.seq",
    "koto.visit.seq",
];

/// A record's fields beyond [`RELAY_FIELDS`].
fn own_keys(record: &Value) -> Vec<String> {
    let mut keys: Vec<String> = record
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !RELAY_FIELDS.contains(&k.as_str()))
        .cloned()
        .collect();
    keys.sort();
    keys
}

#[test]
fn a_three_seat_round_writes_one_round_record_and_three_verdict_records() {
    let env = Env::new();
    env.init("wf");

    let out = env.code_round("wf", "2", HEAD, &three_seats());
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({"status": "recorded", "event": "code-round", "count": 4})
    );

    // One session event, in koto's words, the head only as its digest.
    let events = env.events("wf", "code_review_round_recorded");
    assert_eq!(events.len(), 1);
    let payload = &events[0]["payload"];
    let head = payload["head"].as_str().unwrap().to_string();
    assert!(
        head.starts_with("sha256:") && head.len() == "sha256:".len() + 32,
        "{head}"
    );
    assert_eq!(
        *payload,
        json!({
            "state": "review",
            "round": 2,
            "head": head,
            "entries": [
                {"seat": "correctness", "model": "claude-opus-4-5", "verdict": "pass"},
                {"seat": "security", "model": "claude-opus-4-5", "verdict": "fail"},
                {"seat": "maintainability", "model": "gpt-5.1", "verdict": "pass"},
            ],
        })
    );
    let seq = events[0]["seq"].as_u64().unwrap();

    // One round record and three verdict records, all under the event's
    // seq, and none of the document review kinds.
    let journal = env.journal();
    let round = records(&journal, "code_review_round");
    assert_eq!(round.len(), 1);
    let r = round[0];
    assert_eq!(r["session"], "wf");
    assert_eq!(r["koto.seq"], seq);
    assert_eq!(r["koto.state"], "review");
    assert_eq!(r["koto.code_review.round"], 2);
    assert_eq!(r["koto.code_review.seat_count"], 3);
    assert_eq!(r["koto.code_review.head"], head.as_str());
    assert_eq!(
        own_keys(r),
        [
            "koto.code_review.head",
            "koto.code_review.round",
            "koto.code_review.seat_count",
            "koto.state",
        ]
    );

    let verdicts = records(&journal, "code_review_verdict");
    let got: Vec<(&str, &str, &str)> = verdicts
        .iter()
        .map(|v| {
            (
                v["koto.code_review.seat"].as_str().unwrap(),
                v["koto.code_review.model"].as_str().unwrap(),
                v["koto.code_review.verdict"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("correctness", "claude-opus-4-5", "pass"),
            ("security", "claude-opus-4-5", "fail"),
            ("maintainability", "gpt-5.1", "pass"),
        ]
    );
    for v in &verdicts {
        assert_eq!(v["session"], "wf");
        assert_eq!(v["koto.seq"], seq, "tied to its round");
        assert_eq!(v["koto.visit.seq"], r["koto.visit.seq"]);
        assert_eq!(v["koto.code_review.round"], 2);
        assert_eq!(v["koto.state"], "review");
        assert_eq!(
            own_keys(v),
            [
                "koto.code_review.model",
                "koto.code_review.round",
                "koto.code_review.seat",
                "koto.code_review.verdict",
                "koto.state",
            ]
        );
    }
    // The round record comes before its verdicts.
    let position = |kind: &str| journal.iter().position(|j| j["kind"] == kind).unwrap();
    assert!(position("code_review_round") < position("code_review_verdict"));

    for kind in [
        "review_round",
        "review_verdict",
        "review_decision",
        "doc_checked",
    ] {
        assert!(records(&journal, kind).is_empty(), "{kind}");
    }
    assert!(env.events("wf", "review_round_started").is_empty());
    assert!(env.events("wf", "review_verdict_recorded").is_empty());

    // The commit id itself is in neither the log nor the journal.
    assert!(!env.log("wf").contains(HEAD));
    assert!(!env.journal_text().contains(HEAD));

    // The same head in another round gives the same digest.
    let out = env.code_round("wf", "3", HEAD, &three_seats());
    assert!(out.status.success(), "{}", describe(&out));
    let journal = env.journal();
    let rounds = records(&journal, "code_review_round");
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[1]["koto.code_review.round"], 3);
    assert_eq!(rounds[1]["koto.code_review.head"], head.as_str());
    assert_eq!(records(&journal, "code_review_verdict").len(), 6);
}

#[test]
fn an_undeclared_seat_is_recorded_as_other() {
    let env = Env::new();
    env.init("wf");
    let batch = json!([
        seat("correctness", "claude-opus-4-5", "pass"),
        seat("intruder", "claude-opus-4-5", "fail"),
        seat("Security", "gpt-5.1", "pass"),
    ])
    .to_string();
    let out = env.code_round("wf", "1", HEAD, &batch);
    assert!(out.status.success(), "{}", describe(&out));

    let stored: Vec<Value> = env.events("wf", "code_review_round_recorded")[0]["payload"]
        ["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seat"].clone())
        .collect();
    assert_eq!(
        stored,
        [json!("correctness"), json!("other"), json!("other")]
    );

    let journal = env.journal();
    let seats: Vec<&str> = records(&journal, "code_review_verdict")
        .iter()
        .map(|v| v["koto.code_review.seat"].as_str().unwrap())
        .collect();
    assert_eq!(seats, ["correctness", "other", "other"]);
    assert!(!env.journal_text().contains("intruder"));
    assert!(!env.log("wf").contains("intruder"));
}

/// Exit 2 with `code` and `field`, nothing appended to the session's log or
/// the journal, and the sentinel printed on neither stream.
fn refused(env: &Env, round: &str, head: &str, batch: &str, code: &str, field: Option<&str>) {
    let log = env.log("wf");
    let journal = env.journal_text();
    let out = env.code_round("wf", round, head, batch);
    assert_eq!(out.status.code(), Some(2), "{batch}: {}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["command"], "review record", "{body}");
    assert_eq!(body["error"]["code"], code, "{batch}: {body}");
    if let Some(field) = field {
        assert_eq!(body["error"]["field"], field, "{batch}: {body}");
    }
    assert!(!describe(&out).contains(SENTINEL), "{}", describe(&out));
    assert_eq!(env.log("wf"), log, "{batch} appended to the log");
    assert_eq!(env.journal_text(), journal, "{batch} wrote to the journal");
}

#[test]
fn a_malformed_batch_exits_non_zero_and_writes_nothing() {
    let env = Env::new();
    env.init("wf");
    let good = seat("correctness", "claude-opus-4-5", "pass");
    let batch = |bad: Value| Value::Array(vec![good.clone(), good.clone(), bad]).to_string();

    // A missing seat.
    let mut no_seat = good.clone();
    no_seat.as_object_mut().unwrap().remove("seat");
    refused(
        &env,
        "1",
        HEAD,
        &batch(no_seat),
        "invalid_entry",
        Some("seat"),
    );
    // An unknown verdict value.
    refused(
        &env,
        "1",
        HEAD,
        &batch(seat("security", "claude-opus-4-5", SENTINEL)),
        "invalid_value",
        Some("verdict"),
    );
    refused(
        &env,
        "1",
        HEAD,
        &batch(seat("security", "claude-opus-4-5", "approve")),
        "invalid_value",
        Some("verdict"),
    );
    // Over the cap: 201 seats.
    let over = Value::Array(vec![good.clone(); 201]).to_string();
    refused(&env, "1", HEAD, &over, "batch_too_large", Some("--file"));

    // The rest of the checks refuse the whole call the same way.
    let mut no_model = good.clone();
    no_model.as_object_mut().unwrap().remove("model");
    refused(
        &env,
        "1",
        HEAD,
        &batch(no_model),
        "invalid_entry",
        Some("model"),
    );
    refused(
        &env,
        "1",
        HEAD,
        &batch(seat("security", &format!("{SENTINEL} model"), "pass")),
        "invalid_value",
        Some("model"),
    );
    let mut extra = good.clone();
    extra[SENTINEL] = Value::from(SENTINEL);
    refused(&env, "1", HEAD, &batch(extra), "invalid_entry", None);
    refused(
        &env,
        "1",
        HEAD,
        &format!("{{\"{SENTINEL}\":"),
        "invalid_file",
        Some("--file"),
    );
    refused(
        &env,
        "1",
        HEAD,
        &json!({"seats": [good.clone()]}).to_string(),
        "invalid_file",
        Some("--file"),
    );
    refused(
        &env,
        "0",
        HEAD,
        &batch(good.clone()),
        "invalid_value",
        Some("--round"),
    );
    refused(
        &env,
        SENTINEL,
        HEAD,
        &batch(good.clone()),
        "invalid_value",
        Some("--round"),
    );
    refused(
        &env,
        "1",
        "",
        &batch(good.clone()),
        "invalid_value",
        Some("--head"),
    );
    refused(
        &env,
        "1",
        &SENTINEL.repeat(20),
        &batch(good.clone()),
        "invalid_value",
        Some("--head"),
    );

    assert!(env.events("wf", "code_review_round_recorded").is_empty());

    // The cap itself is taken.
    let full = Value::Array(vec![good.clone(); 200]).to_string();
    let out = env.code_round("wf", "1", HEAD, &full);
    assert!(out.status.success(), "{}", describe(&out));
    let journal = env.journal();
    assert_eq!(records(&journal, "code_review_round").len(), 1);
    assert_eq!(records(&journal, "code_review_verdict").len(), 200);

    // An empty batch records nothing and succeeds.
    let log = env.log("wf");
    let out = env.code_round("wf", "2", HEAD, "[]");
    assert!(out.status.success(), "{}", describe(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({"status": "recorded", "event": "code-round", "count": 0})
    );
    assert_eq!(env.log("wf"), log);
}

/// A round counts one review entry for itself and one per seat: ten
/// rounds of 199 seats fill the session's 2,000, and the next round is
/// refused with nothing appended.
#[test]
fn code_rounds_count_against_the_session_cap() {
    let env = Env::new();
    env.init("wf");
    let batch = Value::Array(vec![seat("correctness", "m", "pass"); 199]).to_string();
    for round in 1..=10 {
        let out = env.code_round("wf", &round.to_string(), HEAD, &batch);
        assert!(out.status.success(), "round {round}: {}", describe(&out));
    }
    let one = json!([seat("correctness", "m", "pass")]).to_string();
    refused(&env, "11", HEAD, &one, "cap_reached", None);
}

/// Recording a round changes no response, state or route: the next
/// `koto next` prints what it printed before.
#[test]
fn a_code_round_changes_no_response_or_state() {
    let env = Env::new();
    env.init("wf");
    // A second call settles what a repeated call prints.
    env.json(&["next", "wf"]);
    let before = env.json(&["next", "wf"]);
    let status_before = env.json(&["status", "wf"]);

    let out = env.code_round("wf", "1", HEAD, &three_seats());
    assert!(out.status.success(), "{}", describe(&out));

    assert_eq!(env.json(&["next", "wf"]), before);
    let status_after = env.json(&["status", "wf"]);
    assert_eq!(
        status_after["current_state"],
        status_before["current_state"]
    );
    assert_eq!(status_after["current_state"], "review");
}

/// A finished session is refused, as for the document review verbs.
#[test]
fn a_cancelled_session_is_refused() {
    let env = Env::new();
    env.init("wf");
    let out = env.run(&["cancel", "wf"]);
    assert!(out.status.success(), "{}", describe(&out));
    let log = env.log("wf");
    let out = env.code_round("wf", "1", HEAD, &three_seats());
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "session_terminal");
    assert_eq!(env.log("wf"), log);
}

#[test]
fn the_help_lists_code_round() {
    let env = Env::new();
    let out = env.run(&["review", "record", "--help"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("code-round"),
        "{}",
        describe(&out)
    );
    let out = env.run(&["review", "record", "x", "code-round", "--help"]);
    assert!(out.status.success(), "{}", describe(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--round", "--head", "--file"] {
        assert!(help.contains(flag), "{flag} missing from: {help}");
    }
}
