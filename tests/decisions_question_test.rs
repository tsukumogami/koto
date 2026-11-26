//! Integration tests for questions under the local backend: the typed
//! recipient on `escalate`, `koto decisions answer`, `koto decisions open
//! --person` and the `questions` array on `koto next`.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use serde_json::Value;

/// Variables koto reads an identity, a run mode or an authority from.
const CLEARED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
];

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
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

/// A recorded escalation: its session, id and the session's id.
#[derive(Clone)]
struct Rec {
    session: String,
    id: String,
    session_id: String,
}

impl Rec {
    fn identity(&self) -> String {
        format!("{}/{}", self.session_id, self.id)
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_env(&[], args)
    }

    fn run_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn escalate(&self, name: &str, question: &str, recipient: &str) -> Rec {
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let out = self
            .ok(&[
                "decisions",
                "record",
                name,
                "--kind",
                "escalate",
                "--outcome",
                "escalated",
                "--subject",
                &q,
                "--subject",
                &t,
                "--subject",
                &r,
                "--by",
                "session",
            ])
            .json();
        Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        }
    }

    fn answer_run(&self, q: &Rec, in_session: &str, text: &str, by: &str) -> Out {
        self.run(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--text",
            text,
            "--by",
            by,
        ])
    }

    /// Answer `q` from `in_session`; returns the answer's id.
    fn answer(&self, q: &Rec, in_session: &str, text: &str) -> String {
        let out = self.answer_run(q, in_session, text, "session");
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn open(&self, person: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--person", person]).json();
        out["questions"].as_array().unwrap().clone()
    }

    /// `koto next <name> --no-cleanup`, so a terminal session stays.
    fn next(&self, name: &str) -> Value {
        self.ok(&["next", name, "--no-cleanup"]).json()
    }

    /// Every file under a session's directory with its bytes, lock files
    /// aside.
    fn snapshot(&self, name: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else if !path.to_string_lossy().ends_with(".lock") {
                    out.insert(
                        path.strip_prefix(base).unwrap().to_path_buf(),
                        std::fs::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        let dir = self.sessions().join(name);
        walk(&dir, &dir, &mut out);
        out
    }

    fn aimed_files(&self, name: &str, target_id: &str) -> Vec<String> {
        let dir = self
            .sessions()
            .join(name)
            .join("ctx/koto/aimed")
            .join(target_id);
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn questions(next: &Value) -> Vec<Value> {
    next.get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn names(qs: &[Value]) -> Vec<&str> {
    qs.iter().map(|q| q["question"].as_str().unwrap()).collect()
}

#[test]
fn a_typed_recipient_reads_back_as_its_kind_and_name() {
    let env = Env::new();
    env.init("a", &[]);
    let person = env.escalate("a", "q1", "person:owner");
    let session = env.escalate("a", "q2", "session:coord");
    let anchor = env.escalate("a", "q3", "anchor:boss");
    let untyped = env.escalate("a", "q4", "coord");

    let show = |r: &Rec| env.ok(&["decisions", "show", "a", &r.id]).json()["recipient"].clone();
    assert_eq!(
        show(&person),
        serde_json::json!({"typed": true, "kind": "person", "handle": "owner"})
    );
    assert_eq!(
        show(&session),
        serde_json::json!({"typed": true, "kind": "session", "name": "coord"})
    );
    assert_eq!(
        show(&anchor),
        serde_json::json!({"typed": true, "kind": "anchor", "name": "boss"})
    );
    assert_eq!(
        show(&untyped),
        serde_json::json!({"typed": false, "value": "coord"})
    );

    // The untyped one lists with the value it was stored with.
    let listing = env.ok(&["decisions", "list", "a", "--format", "v1"]).json();
    let entry = listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == untyped.id.as_str())
        .unwrap()
        .clone();
    assert_eq!(entry["decision"]["subject"]["recipient"], "coord");
    assert_eq!(entry["recipient"]["typed"], false);

    // Only the person's question is open for that person.
    assert_eq!(names(&env.open("owner")), vec!["q1"]);
    assert!(env.open("coord").is_empty());

    // A typed prefix with a name koto doesn't accept is refused.
    let out = env.run(&[
        "decisions",
        "record",
        "a",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q5",
        "--subject",
        "text=t",
        "--subject",
        "recipient=session:not a name",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["field"], "subject.recipient");
}

#[test]
fn a_question_is_open_answered_elsewhere_and_reported_until_cited() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "person:owner");

    let open = env.open("owner");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["identity"], q.identity().as_str());
    assert_eq!(open[0]["session"], "a");
    assert_eq!(open[0]["question"], "q1");
    assert_eq!(open[0]["recipient"]["kind"], "person");
    assert_eq!(open[0]["recipient"]["handle"], "owner");
    assert!(env.open("someone-else").is_empty());

    let before = env.snapshot("a");
    let out = env.answer_run(&q, "b", "SQLite", "session");
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let answer = out.json();
    let decision = &answer["decision"];
    assert_eq!(decision["kind"], "answer");
    assert_eq!(decision["outcome"], "answered");
    assert_eq!(decision["target"]["id"], q.session_id.as_str());
    assert_eq!(decision["target"]["decision"], q.id.as_str());
    assert_eq!(decision["subject"]["text"], "SQLite");
    assert_eq!(decision["decider"]["kind"], "session");
    assert!(decision["authority"]["run_mode"].is_object());
    let answer_id = answer["id"].as_str().unwrap().to_string();
    assert_eq!(env.aimed_files("b", &q.session_id), vec![answer_id.clone()]);
    assert_eq!(env.snapshot("a"), before, "the asker's session changed");

    assert!(env.open("owner").is_empty());

    let listing = env.ok(&["decisions", "list", "b", "--format", "v1"]).json();
    let listed = &listing["decisions"].as_array().unwrap()[0];
    assert_eq!(listed["status"], "in-effect");
    assert_eq!(listed["decision"]["kind"], "answer");

    let reported = questions(&env.next("a"));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0]["identity"], q.identity().as_str());
    assert_eq!(reported[0]["state"], "answered");
    assert_eq!(reported[0]["answer"]["text"], "SQLite");
    assert_eq!(reported[0]["answer"]["session"], "b");
    assert_eq!(reported[0]["answer"]["decider"]["kind"], "session");
    assert_eq!(questions(&env.next("a")), reported, "a repeat differs");

    // Ended: the terminal response carries the same array, and a tick
    // that only reports writes nothing in either session.
    let ended = env.ok(&["next", "a", "--with-data", r#"{"go":"go"}"#, "--no-cleanup"]);
    assert_eq!(questions(&ended.json()), reported);
    let (a0, b0) = (env.snapshot("a"), env.snapshot("b"));
    assert_eq!(questions(&env.next("a")), reported);
    assert_eq!(env.snapshot("a"), a0);
    assert_eq!(env.snapshot("b"), b0);

    // A decision that doesn't cite the answer leaves the report; one that
    // does ends it.
    env.ok(&[
        "decisions",
        "record",
        "a",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=unrelated",
        "--by",
        "session",
    ]);
    assert_eq!(questions(&env.next("a")), reported);
    let basis = format!("b:{answer_id}");
    env.ok(&[
        "decisions",
        "record",
        "a",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=use SQLite",
        "--by",
        "session",
        "--basis",
        &basis,
    ]);
    assert!(env.next("a").get("questions").is_none());
}

#[test]
fn an_answer_closes_only_its_own_question() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q1 = env.escalate("a", "q1", "person:owner");
    let _q2 = env.escalate("a", "q2", "person:owner");
    let _q3 = env.escalate("a", "q3", "person:someone-else");
    let answer_id = env.answer(&q1, "b", "yes to q1");

    assert_eq!(names(&env.open("owner")), vec!["q2"]);
    assert_eq!(names(&env.open("someone-else")), vec!["q3"]);
    let reported = questions(&env.next("a"));
    assert_eq!(names(&reported), vec!["q1"]);
    assert_eq!(reported[0]["answer"]["text"], "yes to q1");

    let basis = format!("b:{answer_id}");
    env.ok(&[
        "decisions",
        "record",
        "a",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=q1 settled",
        "--by",
        "session",
        "--basis",
        &basis,
    ]);
    assert_eq!(names(&env.open("owner")), vec!["q2"]);
    assert!(env.next("a").get("questions").is_none());
}

#[test]
fn an_answer_is_refused_and_writes_nothing_when_its_target_is_wrong() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    env.init("c", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    let judgment = env
        .ok(&[
            "decisions",
            "record",
            "a",
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=x",
            "--by",
            "session",
        ])
        .json();
    // A decision id that exists only in another session: `c` holds three
    // decisions, `a` two.
    env.escalate("c", "qc1", "session:c");
    env.escalate("c", "qc2", "session:c");
    let elsewhere = env.escalate("c", "qc", "person:owner");
    assert_eq!(elsewhere.id, "000000000003");
    let (a0, b0) = (env.snapshot("a"), env.snapshot("b"));

    let missing = Rec {
        id: "000000000099".to_string(),
        ..q.clone()
    };
    let out = env.answer_run(&missing, "b", "x", "session");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["code"], "decision_not_found");

    let not_escalate = Rec {
        id: judgment["id"].as_str().unwrap().to_string(),
        ..q.clone()
    };
    let out = env.answer_run(&not_escalate, "b", "x", "session");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["code"], "invalid_decision");

    // The target pairs the question session's id with the decision id, so
    // a decision id found only in another session isn't found.
    let out = env.answer_run(
        &Rec {
            id: elsewhere.id.clone(),
            ..q.clone()
        },
        "b",
        "x",
        "session",
    );
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["code"], "decision_not_found");

    let out = env.answer_run(&q, "b", "x", "person:owner");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("a person's answer can't be recorded yet"),
        "{}",
        out.stdout
    );

    let out = env.answer_run(&q, "a", "x", "session");
    assert_eq!(out.code, 2, "{}", out.stdout);

    let out = env.run(&[
        "decisions",
        "record",
        "b",
        "--kind",
        "answer",
        "--outcome",
        "answered",
        "--subject",
        "session=a",
        "--subject",
        "id=000000000001",
        "--subject",
        "text=x",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);

    assert_eq!(env.snapshot("a"), a0);
    assert_eq!(env.snapshot("b"), b0);
    assert_eq!(names(&env.open("owner")), vec!["q1", "qc"]);
}

#[test]
fn an_answer_is_not_reversed() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    let answer_id = env.answer(&q, "b", "yes");
    let out = env.run(&[
        "decisions",
        "reverse",
        "b",
        &answer_id,
        "--in",
        "b",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["reason"], "reverses_answer");
    assert!(env.open("owner").is_empty());
}

#[test]
fn the_answer_follows_the_question_id_not_the_session_name() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    env.answer(&q, "b", "old answer");

    std::fs::remove_dir_all(env.sessions().join("a")).unwrap();
    env.init("a", &[]);
    let fresh = env.escalate("a", "q1", "person:owner");
    assert_ne!(fresh.session_id, q.session_id);

    assert_eq!(names(&env.open("owner")), vec!["q1"]);
    assert_eq!(env.open("owner")[0]["identity"], fresh.identity().as_str());
    assert!(env.next("a").get("questions").is_none());
}

#[test]
fn a_successor_gets_the_answer_to_its_predecessors_question() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    env.answer(&q, "b", "SQLite");
    env.init("s", &["--continues", "a", "--supersede"]);

    let reported = questions(&env.next("s"));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0]["identity"], q.identity().as_str());
    assert_eq!(reported[0]["answer"]["text"], "SQLite");
}

#[test]
fn a_session_with_no_question_carries_no_questions_key() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("b", "q1", "person:owner");
    env.answer(&q, "a", "answered from a");
    // `a` holds an answer, not a question: nothing to report.
    assert!(env.next("a").get("questions").is_none());
}

/// `koto next <name> --no-cleanup` with the scan probe set: the response,
/// and whether the questions read scanned the store.
fn next_probed(env: &Env, name: &str) -> (Value, bool) {
    let out = env.run_env(
        &[(SCAN_PROBE.0, SCAN_PROBE.1)],
        &["next", name, "--no-cleanup"],
    );
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    (out.json(), out.stderr.contains("questions: store scanned"))
}

/// The variable that has the questions read say when it scans the store.
const SCAN_PROBE: (&str, &str) = ("KOTO_TEST_QUESTIONS_SCAN_PROBE", "1");

fn cite(env: &Env, name: &str, basis: &str) {
    env.ok(&[
        "decisions",
        "record",
        name,
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=cited",
        "--by",
        "session",
        "--basis",
        basis,
    ]);
}

#[test]
fn a_recreated_answering_session_reusing_an_answer_id_isnt_taken_as_cited() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q1 = env.escalate("a", "q1", "person:owner");
    let q2 = env.escalate("a", "q2", "person:owner");
    let first = env.answer(&q1, "b", "yes to q1");
    cite(&env, "a", &format!("b:{first}"));
    assert!(env.next("a").get("questions").is_none());

    // `b` goes, a new `b` answers q2, and its answer gets the same id.
    std::fs::remove_dir_all(env.sessions().join("b")).unwrap();
    env.init("b", &[]);
    let second = env.answer(&q2, "b", "yes to q2");
    assert_eq!(second, first, "the ids must collide for this test");

    let reported = questions(&env.next("a"));
    assert_eq!(names(&reported), vec!["q2"], "{reported:?}");
    assert_eq!(reported[0]["identity"], q2.identity().as_str());
    assert_eq!(reported[0]["answer"]["text"], "yes to q2");
    // q2 is answered, so it isn't open; the new answer closes nothing but
    // the question it targets.
    assert!(!names(&env.open("owner")).contains(&"q2"));

    // Citing it now ends the report.
    cite(&env, "a", &format!("b:{second}"));
    assert!(env.next("a").get("questions").is_none());
}

#[test]
fn a_question_not_in_effect_isnt_answered() {
    let env = Env::new();
    env.init("coord", &[]);
    env.init("w", &["--dispatched-by", "coord"]);
    env.init("b", &[]);
    let q = env.escalate("w", "q1", "person:owner");
    let out = env.run(&[
        "decisions",
        "reverse",
        "w",
        &q.id,
        "--in",
        "coord",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let (w0, b0) = (env.snapshot("w"), env.snapshot("b"));

    let out = env.answer_run(&q, "b", "too late", "session");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["code"], "invalid_decision");
    assert!(
        out.json()["error"].as_str().unwrap().contains("reversed"),
        "{}",
        out.stdout
    );
    assert_eq!(env.snapshot("w"), w0);
    assert_eq!(env.snapshot("b"), b0);
    assert!(env.open("owner").is_empty());
}

#[test]
fn the_store_is_scanned_only_while_a_question_is_uncited() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);

    // No decision, then only an untyped escalation: no scan, no key.
    let (resp, scanned) = next_probed(&env, "a");
    assert!(!scanned && resp.get("questions").is_none());
    let legacy = env.escalate("a", "q0", "coord");
    let (resp, scanned) = next_probed(&env, "a");
    assert!(!scanned && resp.get("questions").is_none());
    // An untyped escalation isn't a question this version answers.
    let out = env.answer_run(&legacy, "b", "x", "session");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["code"], "invalid_decision");

    // An open question, then an answered one: the store is scanned.
    let q = env.escalate("a", "q1", "person:owner");
    let (resp, scanned) = next_probed(&env, "a");
    assert!(scanned && resp.get("questions").is_none());
    let answer_id = env.answer(&q, "b", "yes");
    let (resp, scanned) = next_probed(&env, "a");
    assert!(scanned);
    assert_eq!(names(&questions(&resp)), vec!["q1"]);

    // Once every question is cited, the scan stops.
    cite(&env, "a", &format!("b:{answer_id}"));
    let (resp, scanned) = next_probed(&env, "a");
    assert!(!scanned && resp.get("questions").is_none());
}

#[test]
fn a_stray_file_beside_the_question_doesnt_stop_its_answer() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    let dir = env.sessions().join("a/ctx/koto/decisions");
    std::fs::write(dir.join(".DS_Store"), b"junk").unwrap();
    let out = env.answer_run(&q, "b", "yes", "session");
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(env.open("owner").is_empty());
}
