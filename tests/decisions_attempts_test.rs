//! Integration tests for attempts to decide and provisional judgments:
//! the `state` and `result` subject fields, `koto decisions attempts`, and
//! the review a provisional judgment waits on.
//!
//! Every test runs the real binary with `HOME`, `KOTO_HOME` and
//! `KOTO_SESSIONS_BASE` in its own temporary directory. A person's answer
//! comes from a fixture in `tests/fixtures/prompt/` installed as a harness
//! record (see `support/prompt_fixture.rs`), never from a live prompt.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use prompt_fixture as fx;
use serde_json::Value;

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
    _records: fx::Records,
    _tmp: TempDir,
    root: PathBuf,
    calls: std::cell::Cell<u32>,
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

    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// A recorded decision: its session's name and id, and its identity.
#[derive(Clone, Debug)]
struct D {
    session: String,
    id: String,
    identity: String,
    header: Option<String>,
}

impl D {
    fn basis(&self) -> String {
        format!("{}:{}", self.session, self.id)
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
        Env {
            _records: fx::Records::new(),
            _tmp: tmp,
            root,
            calls: std::cell::Cell::new(0),
        }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn command(&self, harness: Option<&str>, args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        if let Some(h) = harness {
            cmd.env("CLAUDE_CODE_SESSION_ID", h);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        cmd.args(args);
        cmd
    }

    fn run_h(&self, harness: Option<&str>, args: &[&str]) -> Out {
        let output = self.command(harness, args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_h(None, args)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(out.code, 0, "{args:?}\n{}", out.all());
        out
    }

    fn init(&self, name: &str, parent: Option<&str>) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        if let Some(p) = parent {
            args.extend_from_slice(&["--dispatched-by", p]);
        }
        self.ok(&args);
    }

    fn decision(&self, session: &str, out: &Out) -> D {
        let j = out.json();
        let id = j["id"].as_str().unwrap().to_string();
        let sid = j["decision"]["session"]["id"].as_str().unwrap().to_string();
        D {
            session: session.to_string(),
            identity: format!("{sid}/{id}"),
            id,
            header: None,
        }
    }

    /// Record a judgment in `session` with `subjects` and `basis`.
    fn judgment(&self, session: &str, subjects: &[&str], basis: &[&D]) -> D {
        let args = judgment_args(session, subjects, basis);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run(&args);
        assert_eq!(out.code, 0, "{}", out.all());
        self.decision(session, &out)
    }

    /// Raise a question in `session` to `recipient`, citing `basis`, with
    /// content stored for the accepted fixture's first question.
    fn question(&self, session: &str, recipient: &str, extra: &[&str], basis: &[&D]) -> D {
        let (question, labels) = fx::shown(&fx::lines("accepted"), 1);
        let content = fx::content(&question, &labels);
        let packet = format!("packet={}", fx::packet(&content));
        let recipient = format!("recipient={recipient}");
        let mut args = vec![
            "decisions",
            "record",
            session,
            "--kind",
            "escalate",
            "--outcome",
            "escalated",
            "--subject",
            "question=cache-store",
            "--subject",
            "text=Postgres or SQLite for the cache?",
            "--subject",
            &recipient,
            "--subject",
            &packet,
        ];
        for s in extra {
            args.extend_from_slice(&["--subject", s]);
        }
        let cites: Vec<String> = basis.iter().map(|d| d.basis()).collect();
        for c in &cites {
            args.extend_from_slice(&["--basis", c]);
        }
        args.extend_from_slice(&["--by", "session"]);
        let mut d = self.decision(session, &self.ok(&args));
        let file = self.root.join(format!("content-{session}-{}", d.id));
        std::fs::write(&file, &content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                session,
                &d.id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        d.header = Some(written["prompt_header"].as_str().unwrap().to_string());
        d
    }

    /// The owner answers `q` in `answerer` through the accepted fixture,
    /// installed under a call id of its own.
    fn owner_answers(&self, q: &D, answerer: &str) -> D {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        let call = format!("toolu_ZQXMARKERcall{n:015}");
        let lines = fx::with_call(&fx::lines("accepted"), &call);
        let header = q.header.as_deref().unwrap();
        fx::install(
            &fx::harness(),
            &fx::render(&lines, &fx::harness(), &[header]),
        );
        let pointer = format!("{}/{call}/1", fx::harness());
        let out = self.run_h(
            Some(&fx::harness()),
            &[
                "decisions",
                "answer",
                &q.session,
                &q.id,
                "--in",
                answerer,
                "--by",
                "person:owner",
                "--prompt",
                &pointer,
            ],
        );
        assert_eq!(out.code, 0, "{}", out.all());
        assert_eq!(
            out.json()["decision"]["prompt"]["mark"]["value"],
            "verified"
        );
        self.decision(answerer, &out)
    }

    fn attempts(&self, args: &[&str]) -> Value {
        let mut all = vec!["decisions", "attempts"];
        all.extend_from_slice(args);
        self.ok(&all).json()
    }

    /// Every file under the sessions directory and its bytes, so a read
    /// or a refusal can be shown to have changed nothing.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(p: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let Ok(rd) = std::fs::read_dir(p) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if !p.to_string_lossy().ends_with(".lock") {
                    out.insert(p.clone(), std::fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.sessions(), &mut out);
        out
    }
}

fn judgment_args(session: &str, subjects: &[&str], basis: &[&D]) -> Vec<String> {
    let mut args: Vec<String> = ["decisions", "record", session, "--kind", "judgment"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(["--outcome".to_string(), "settled".to_string()]);
    for s in subjects {
        args.extend(["--subject".to_string(), s.to_string()]);
    }
    for d in basis {
        args.extend(["--basis".to_string(), d.basis()]);
    }
    args.extend(["--by".to_string(), "session".to_string()]);
    args
}

fn list(v: &Value) -> &Vec<Value> {
    v["attempts"].as_array().unwrap()
}

fn ids(v: &Value) -> Vec<String> {
    list(v)
        .iter()
        .map(|e| e["identity"].as_str().unwrap().to_string())
        .collect()
}

/// director -> manager -> {worker, worker2}; other-lead -> other-worker.
fn tree(env: &Env) {
    env.init("director", None);
    env.init("manager", Some("director"));
    env.init("worker", Some("manager"));
    env.init("worker2", Some("manager"));
    env.init("other-lead", None);
    env.init("other-worker", Some("other-lead"));
}

#[test]
fn state_and_result_are_taken_on_a_judgment_and_state_on_an_escalate() {
    let env = Env::new();
    tree(&env);
    let a = env.judgment(
        "worker",
        &["what=pick the cache", "state=implement:1", "result=decided"],
        &[],
    );
    let shown = env.ok(&["decisions", "show", "worker", &a.id]).json();
    assert_eq!(shown["decision"]["subject"]["state"], "implement:1");
    assert_eq!(shown["decision"]["subject"]["result"], "decided");
    assert!(
        shown.get("review").is_none(),
        "only a provisional has a review"
    );

    let q = env.question("worker", "session:manager", &["state=implement:2"], &[]);
    let shown = env.ok(&["decisions", "show", "worker", &q.id]).json();
    assert_eq!(shown["decision"]["subject"]["state"], "implement:2");

    // Each field alone is taken too.
    env.judgment("worker", &["what=a", "result=undecided"], &[]);
    env.judgment("worker", &["what=b", "state=review:12"], &[]);
}

#[test]
fn a_malformed_or_misplaced_field_is_refused_and_writes_nothing() {
    let env = Env::new();
    tree(&env);
    let cases: [(&[&str], &str, &str); 9] = [
        (&["what=x", "result=maybe"], "judgment", "subject.result"),
        (&["what=x", "result="], "judgment", "subject.result"),
        (&["what=x", "state=implement"], "judgment", "subject.state"),
        (
            &["what=x", "state=implement:0"],
            "judgment",
            "subject.state",
        ),
        (
            &["what=x", "state=implement:01"],
            "judgment",
            "subject.state",
        ),
        (&["what=x", "state=:1"], "judgment", "subject.state"),
        (
            &["what=x", "state=implement:-1"],
            "judgment",
            "subject.state",
        ),
        (
            &["what=x", "state=impl ement:1"],
            "judgment",
            "subject.state",
        ),
        (
            &["what=x", "state=implement:1:2"],
            "judgment",
            "subject.state",
        ),
    ];
    for (subjects, kind, field) in cases {
        let before = env.snapshot();
        let mut args = vec![
            "decisions",
            "record",
            "worker",
            "--kind",
            kind,
            "--outcome",
            "settled",
        ];
        for s in subjects {
            args.extend_from_slice(&["--subject", s]);
        }
        args.extend_from_slice(&["--by", "session"]);
        let out = env.run(&args);
        assert_eq!(out.code, 2, "{subjects:?}: {}", out.all());
        assert_eq!(out.json()["field"], field, "{subjects:?}: {}", out.all());
        assert_eq!(env.snapshot(), before, "{subjects:?} wrote something");
    }

    // `result` on an escalate, and either field on a kind that takes
    // neither, are refused by the subject's shape check.
    let before = env.snapshot();
    let out = env.run(&[
        "decisions",
        "record",
        "worker",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q",
        "--subject",
        "text=t",
        "--subject",
        "recipient=session:manager",
        "--subject",
        "result=undecided",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["field"], "subject.result");
    let out = env.run(&[
        "decisions",
        "record",
        "worker",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q",
        "--subject",
        "text=t",
        "--subject",
        "recipient=session:manager",
        "--subject",
        "state=implement:x",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["field"], "subject.state");
    assert_eq!(env.snapshot(), before);
}

#[test]
fn a_judgment_without_the_fields_is_unchanged_and_isnt_an_attempt() {
    let env = Env::new();
    tree(&env);
    let plain = env.judgment("worker", &["what=pick the cache"], &[]);
    let shown = env.ok(&["decisions", "show", "worker", &plain.id]);
    let j = shown.json();
    assert!(j.get("review").is_none());
    assert!(j["decision"]["subject"].get("result").is_none());
    assert!(!shown.stdout.contains("\"result\""), "{}", shown.stdout);
    assert!(!shown.stdout.contains("\"review\""), "{}", shown.stdout);
    // A judgment carrying only `state` isn't an attempt either.
    env.judgment("worker", &["what=b", "state=implement:1"], &[]);
    assert!(list(&env.attempts(&["--in", "worker"])).is_empty());
}

#[test]
fn list_filters_on_the_new_subject_fields() {
    let env = Env::new();
    tree(&env);
    let a = env.judgment(
        "worker",
        &["what=a", "state=implement:1", "result=undecided"],
        &[],
    );
    env.judgment(
        "worker",
        &["what=b", "state=implement:2", "result=decided"],
        &[],
    );
    let listed = env
        .ok(&[
            "decisions",
            "list",
            "worker",
            "--subject",
            "result=undecided",
            "--format",
            "v1",
        ])
        .json();
    let got: Vec<&str> = listed["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(got, vec![a.id.as_str()]);
    let listed = env
        .ok(&[
            "decisions",
            "list",
            "worker",
            "--subject",
            "state=implement:2",
            "--format",
            "v1",
        ])
        .json();
    assert_eq!(listed["decisions"].as_array().unwrap().len(), 1);
}

#[test]
fn an_escalates_state_shows_on_its_open_element() {
    let env = Env::new();
    tree(&env);
    env.question("worker", "person:owner", &["state=implement:1"], &[]);
    env.question("worker2", "person:owner", &[], &[]);
    let open = env.ok(&["decisions", "open", "--person", "owner"]).json();
    let mut states: Vec<Value> = open["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.get("state").cloned().unwrap_or(Value::Null))
        .collect();
    states.sort_by_key(|v| v.to_string());
    assert_eq!(states, vec![Value::from("implement:1"), Value::Null]);
    let open = env.ok(&["decisions", "open", "--at", "director"]).json();
    let with_state: Vec<&Value> = open["questions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("state").is_some())
        .collect();
    assert_eq!(with_state.len(), 1);
    assert_eq!(with_state[0]["session"], "worker");
}

#[test]
fn attempts_in_reads_the_run_and_at_reads_what_is_below() {
    let env = Env::new();
    tree(&env);
    let w = env.judgment(
        "worker",
        &["what=w", "state=implement:1", "result=undecided"],
        &[],
    );
    let m = env.judgment("manager", &["what=m", "result=decided"], &[]);
    let d = env.judgment("director", &["what=d", "result=provisional"], &[]);
    let o = env.judgment("other-worker", &["what=o", "result=undecided"], &[]);

    let v = env.attempts(&["--in", "worker"]);
    assert_eq!(v["in"], "worker");
    assert_eq!(ids(&v), vec![w.identity.clone()]);
    assert!(list(&v)[0].get("relation").is_none());
    let e = &list(&v)[0];
    assert_eq!(e["session"], "worker");
    assert_eq!(e["id"], w.id.as_str());
    assert_eq!(e["what"], "w");
    assert_eq!(e["state"], "implement:1");
    assert_eq!(e["result"], "undecided");
    assert_eq!(e["status"], "in-effect");
    assert_eq!(e["question"], Value::Null);
    assert_eq!(e["decider"]["kind"], "session");
    assert!(e.get("review").is_none());

    let v = env.attempts(&["--at", "director"]);
    assert_eq!(v["at"], "director");
    let got = ids(&v);
    assert_eq!(got.len(), 3, "{v}");
    for x in [&w, &m, &d] {
        assert!(got.contains(&x.identity), "{v}");
    }
    assert!(!got.contains(&o.identity));
    for e in list(&v) {
        let want = if e["session"] == "director" {
            "at"
        } else {
            "below"
        };
        assert_eq!(e["relation"], want, "{e}");
    }
    // Ordered by time.
    let times: Vec<&str> = list(&v)
        .iter()
        .map(|e| e["time"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort();
    assert_eq!(times, sorted);

    assert_eq!(
        ids(&env.attempts(&["--at", "other-lead"])),
        vec![o.identity.clone()]
    );
    assert_eq!(
        ids(&env.attempts(&["--at", "worker"])),
        vec![w.identity.clone()]
    );
}

#[test]
fn attempts_in_takes_in_the_sessions_a_run_continues() {
    let env = Env::new();
    env.init("first", None);
    let a = env.judgment("first", &["what=a", "result=undecided"], &[]);
    env.ok(&[
        "decisions",
        "record",
        "first",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=done",
        "--by",
        "session",
    ]);
    // `second` continues and supersedes `first`.
    let template = env.root.join("work.md");
    let out = env.run(&[
        "init",
        "second",
        "--template",
        template.to_str().unwrap(),
        "--continues",
        "first",
        "--supersede",
    ]);
    assert_eq!(out.code, 0, "{}", out.all());
    let b = env.judgment("second", &["what=b", "result=decided"], &[]);
    let got = ids(&env.attempts(&["--in", "second"]));
    assert_eq!(got, vec![a.identity, b.identity]);
}

#[test]
fn a_question_and_an_attempt_are_tied_by_basis_either_way() {
    let env = Env::new();
    tree(&env);
    let a1 = env.judgment(
        "worker",
        &["what=a1", "state=implement:1", "result=undecided"],
        &[],
    );
    let q1 = env.question("worker", "session:manager", &["state=implement:1"], &[&a1]);
    let a2 = env.judgment("manager", &["what=a2", "result=undecided"], &[&q1]);
    // Unrelated attempt.
    env.judgment("manager", &["what=other", "result=undecided"], &[]);

    let v = env.attempts(&["--at", "director", "--question", &q1.identity]);
    assert_eq!(ids(&v), vec![a1.identity.clone(), a2.identity.clone()]);
    for e in list(&v) {
        assert_eq!(e["question"], q1.identity.as_str());
    }
    let v = env.attempts(&["--in", "worker", "--result", "undecided"]);
    assert_eq!(list(&v)[0]["question"], q1.identity.as_str());
    assert!(list(&env.attempts(&[
        "--in",
        "manager",
        "--question",
        "00000000-0000-0000-0000-000000000000/000000000001"
    ]))
    .is_empty());
}

#[test]
fn a_citation_resolves_to_the_session_recorded_before_the_citer() {
    let env = Env::new();
    tree(&env);
    let old_q = env.question("worker2", "session:manager", &[], &[]);
    let a = env.judgment("worker", &["what=a", "result=undecided"], &[&old_q]);
    assert_eq!(
        ids(&env.attempts(&["--at", "director", "--question", &old_q.identity])),
        vec![a.identity.clone()]
    );

    // worker2 goes, and a new worker2 raises a question that reuses the id.
    std::fs::remove_dir_all(env.sessions().join("worker2")).unwrap();
    env.init("worker2", Some("manager"));
    let new_q = env.question("worker2", "session:manager", &[], &[]);
    assert_eq!(new_q.id, old_q.id, "the ids must collide for this test");
    assert_ne!(new_q.identity, old_q.identity);

    // a cites a question recorded before it; the new one came after.
    let v = env.attempts(&["--in", "worker"]);
    assert_eq!(list(&v)[0]["question"], Value::Null, "{v}");
    assert_eq!(list(&v)[0]["questions"], serde_json::json!([]), "{v}");
    assert!(list(&env.attempts(&["--at", "director", "--question", &new_q.identity])).is_empty());
}

#[test]
fn the_read_refuses_bad_usage_and_writes_nothing() {
    let env = Env::new();
    tree(&env);
    env.judgment("worker", &["what=w", "result=provisional"], &[]);
    let before = env.snapshot();
    for args in [
        &["decisions", "attempts"][..],
        &[
            "decisions",
            "attempts",
            "--in",
            "worker",
            "--at",
            "director",
        ],
    ] {
        let out = env.run(args);
        assert_eq!(out.code, 2, "{args:?}: {}", out.all());
    }
    let refusals: [(&[&str], &str); 6] = [
        (&["--in", "nobody"], "in"),
        (&["--at", "nobody"], "at"),
        (&["--in", "Bad Name"], "in"),
        (&["--in", "worker", "--result", "maybe"], "result"),
        (&["--in", "worker", "--state", "implement"], "state"),
        (&["--in", "worker", "--question", "nope"], "question"),
    ];
    for (args, field) in refusals {
        let mut all = vec!["decisions", "attempts"];
        all.extend_from_slice(args);
        let out = env.run(&all);
        assert_eq!(out.code, 2, "{args:?}: {}", out.all());
        assert_eq!(out.json()["field"], field, "{args:?}: {}", out.all());
    }
    env.attempts(&[
        "--at",
        "director",
        "--result",
        "provisional",
        "--awaiting-review",
    ]);
    env.attempts(&["--in", "worker", "--state", "implement:1"]);
    assert_eq!(env.snapshot(), before, "the read wrote something");
}

#[test]
fn a_provisional_waits_until_a_person_answers_a_question_naming_it() {
    let env = Env::new();
    tree(&env);
    let p1 = env.judgment(
        "worker2",
        &["what=name the table cache_entries", "result=provisional"],
        &[],
    );
    let awaiting = |env: &Env| {
        ids(&env.attempts(&[
            "--at",
            "director",
            "--result",
            "provisional",
            "--awaiting-review",
        ]))
    };
    assert_eq!(awaiting(&env), vec![p1.identity.clone()]);
    let v = env.attempts(&["--at", "director", "--awaiting-review"]);
    assert_eq!(list(&v)[0]["review"]["state"], "awaiting");
    assert_eq!(list(&v)[0]["review"]["question"], Value::Null);
    assert!(awaiting_at(&env, "other-lead").is_empty());

    let q7 = env.question("director", "person:owner", &[], &[&p1]);
    let v = env.attempts(&["--at", "director", "--awaiting-review"]);
    assert_eq!(list(&v)[0]["review"]["question"], q7.identity.as_str());
    // director raised q7 about worker2's judgment: that names the question
    // the review waits on, but doesn't make it a question worker2 attempted.
    assert_eq!(list(&v)[0]["question"], Value::Null);

    // A session's answer to a question waiting on a person is refused.
    let out = env.run(&[
        "decisions",
        "answer",
        "director",
        &q7.id,
        "--in",
        "manager",
        "--by",
        "session",
        "--text",
        "fine by me",
    ]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(awaiting(&env), vec![p1.identity.clone()]);

    let answer = env.owner_answers(&q7, "manager");
    assert!(awaiting(&env).is_empty());
    let v = env.attempts(&["--at", "director", "--result", "provisional"]);
    let review = &list(&v)[0]["review"];
    assert_eq!(review["state"], "reviewed");
    assert_eq!(review["question"], q7.identity.as_str());
    assert_eq!(review["answer"]["session"], "manager");
    assert_eq!(review["answer"]["id"], answer.id.as_str());
    assert_eq!(review["answer"]["reading"], "verified");
    assert_eq!(review["answer"]["decider"]["kind"], "person");
    let shown = env.ok(&["decisions", "show", "worker2", &p1.id]).json();
    assert_eq!(shown["review"], *review);
    assert_eq!(shown["decision"]["subject"]["result"], "provisional");
}

#[test]
fn a_review_belongs_to_the_judgment_it_names_not_a_reused_identifier() {
    let env = Env::new();
    tree(&env);
    let old = env.judgment("worker2", &["what=old", "result=provisional"], &[]);
    let q = env.question("manager", "person:owner", &[], &[&old]);
    env.owner_answers(&q, "director");
    let v = env.attempts(&["--at", "director", "--result", "provisional"]);
    assert_eq!(list(&v)[0]["review"]["state"], "reviewed", "{v}");

    // worker2 goes, and a new worker2 records a provisional judgment that
    // reuses the id the review question names.
    std::fs::remove_dir_all(env.sessions().join("worker2")).unwrap();
    env.init("worker2", Some("manager"));
    let new = env.judgment("worker2", &["what=new", "result=provisional"], &[]);
    assert_eq!(new.id, old.id, "the ids must collide for this test");

    assert_eq!(awaiting_at(&env, "director"), vec![new.identity.clone()]);
    let shown = env.ok(&["decisions", "show", "worker2", &new.id]).json();
    assert_eq!(
        shown["review"],
        serde_json::json!({"state": "awaiting", "question": null})
    );
}

#[test]
fn the_review_reports_the_answer_that_governs_the_question() {
    let env = Env::new();
    tree(&env);
    let p1 = env.judgment(
        "worker2",
        &["what=name the table cache_entries", "result=provisional"],
        &[],
    );
    let q7 = env.question("director", "person:owner", &[], &[&p1]);
    let first = env.owner_answers(&q7, "manager");
    let second = env.owner_answers(&q7, "other-lead");
    // Of two person answers citing neither, the later governs; the earlier
    // still counts but is outranked.
    let earlier = env.ok(&["decisions", "show", "manager", &first.id]).json();
    assert_eq!(earlier["counted"], true, "{earlier}");
    assert_eq!(earlier["governs"], false, "{earlier}");
    let v = env.attempts(&["--at", "director", "--result", "provisional"]);
    let review = &list(&v)[0]["review"];
    assert_eq!(review["state"], "reviewed", "{v}");
    assert_eq!(review["question"], q7.identity.as_str());
    assert_eq!(review["answer"]["session"], "other-lead");
    assert_eq!(review["answer"]["id"], second.id.as_str());
    let shown = env.ok(&["decisions", "show", "worker2", &p1.id]).json();
    assert_eq!(shown["review"], *review);
}

#[test]
fn an_uncounted_answer_a_settle_and_a_withdrawal_dont_end_the_wait() {
    let env = Env::new();
    tree(&env);
    let p1 = env.judgment(
        "worker2",
        &["what=name the table cache_entries", "result=provisional"],
        &[],
    );
    let waiting = |env: &Env| {
        assert_eq!(awaiting_at(env, "director"), vec![p1.identity.clone()]);
        let shown = env.ok(&["decisions", "show", "worker2", &p1.id]).json();
        assert_eq!(shown["review"]["state"], "awaiting", "{shown}");
    };

    // A session's answer that governs a question waiting on that session.
    let qa = env.question("director", "session:manager", &[], &[&p1]);
    let out = env.ok(&[
        "decisions",
        "answer",
        "director",
        &qa.id,
        "--in",
        "manager",
        "--by",
        "session",
        "--text",
        "fine by me",
    ]);
    let said = out.json()["id"].as_str().unwrap().to_string();
    let e = env.ok(&["decisions", "show", "manager", &said]).json();
    assert_eq!(e["governs"], true, "{e}");
    waiting(&env);

    let qw = env.question("director", "person:owner", &[], &[&p1]);
    // The raiser withdraws a question waiting on a person.
    env.ok(&[
        "decisions",
        "answer",
        "director",
        &qw.id,
        "--in",
        "director",
        "--outcome",
        "withdrawn",
        "--reason",
        "asked elsewhere",
        "--by",
        "session",
    ]);
    waiting(&env);

    // The raiser settles another from the owner's answer to a question
    // with its label.
    let qb = env.question("other-worker", "person:owner", &["label=cache"], &[]);
    let owner = env.owner_answers(&qb, "manager");
    let qc = env.question("director", "person:owner", &["label=cache"], &[&p1]);
    let basis = owner.basis();
    env.ok(&[
        "decisions",
        "answer",
        "director",
        &qc.id,
        "--in",
        "director",
        "--outcome",
        "settled",
        "--text",
        "settled",
        "--by",
        "session",
        "--basis",
        &basis,
    ]);
    let settled = out_id(&env, "director", &qc);
    waiting(&env);

    // The owner answers the labelled question again: the settle reads
    // counted false and the wait goes on.
    env.owner_answers(&qb, "other-lead");
    let e = env.ok(&["decisions", "show", "director", &settled]).json();
    assert_eq!(e["counted"], false, "{e}");
    waiting(&env);
}

/// The id of the newest entry `session` aimed at `q`.
fn out_id(env: &Env, session: &str, q: &D) -> String {
    let v = env
        .ok(&["decisions", "list", session, "--format", "v1"])
        .json();
    v["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e["decision"]["target"]["decision"] == q.id.as_str())
        .map(|e| e["id"].as_str().unwrap().to_string())
        .unwrap()
}

fn awaiting_at(env: &Env, at: &str) -> Vec<String> {
    ids(&env.attempts(&["--at", at, "--result", "provisional", "--awaiting-review"]))
}

#[test]
fn a_withdrawn_question_or_a_citing_decision_doesnt_end_the_wait() {
    let env = Env::new();
    tree(&env);
    let p = env.judgment("worker2", &["what=p", "result=provisional"], &[]);
    // A judgment citing it is no review.
    env.judgment("manager", &["what=noted"], &[&p]);
    assert_eq!(awaiting_at(&env, "director"), vec![p.identity.clone()]);

    // A question naming it, withdrawn before anyone answers.
    let q = env.question("manager", "person:owner", &[], &[&p]);
    env.ok(&[
        "decisions",
        "reverse",
        "manager",
        &q.id,
        "--in",
        "director",
        "--by",
        "session",
        "--reason",
        "asked the wrong person",
    ]);
    let v = env.attempts(&["--at", "director", "--awaiting-review"]);
    assert_eq!(ids(&v), vec![p.identity.clone()]);
    assert_eq!(list(&v)[0]["review"]["question"], Value::Null, "{v}");
}

#[test]
fn a_reversed_provisional_leaves_the_wait_naming_the_reversal() {
    let env = Env::new();
    tree(&env);
    let p2 = env.judgment("worker2", &["what=index by key", "result=provisional"], &[]);
    let r = env.ok(&[
        "decisions",
        "reverse",
        "worker2",
        &p2.id,
        "--in",
        "manager",
        "--by",
        "session",
        "--reason",
        "the index belongs to the schema review",
    ]);
    let rid = r.json()["id"].as_str().unwrap().to_string();
    assert!(awaiting_at(&env, "director").is_empty());
    let v = env.attempts(&["--at", "director", "--result", "provisional"]);
    let e = &list(&v)[0];
    assert_eq!(e["status"], "reversed");
    assert_eq!(e["review"]["state"], "reversed");
    assert_eq!(e["review"]["reversed_by"]["session"], "manager", "{e}");
    assert_eq!(e["review"]["reversed_by"]["id"], rid.as_str(), "{e}");
}

#[test]
fn escalating_and_passing_up_need_no_attempt() {
    let env = Env::new();
    tree(&env);
    let q = env.question("worker", "session:manager", &[], &[]);
    env.ok(&[
        "decisions",
        "readdress",
        "worker",
        &q.id,
        "--in",
        "manager",
        "--to",
        "session:director",
        "--by",
        "session",
    ]);
    assert!(list(&env.attempts(&["--at", "director"])).is_empty());
}

#[test]
fn a_raiser_ties_only_its_own_runs_attempts_to_its_question() {
    let env = Env::new();
    tree(&env);
    // manager's attempt is about something else; worker names it anyway.
    let m = env.judgment("manager", &["what=unrelated", "result=undecided"], &[]);
    let own = env.judgment("worker", &["what=own", "result=undecided"], &[]);
    let q = env.question("worker", "session:manager", &[], &[&m, &own]);

    assert!(list(&env.attempts(&["--in", "manager", "--question", &q.identity])).is_empty());
    let v = env.attempts(&["--in", "manager"]);
    assert_eq!(list(&v)[0]["question"], Value::Null, "{v}");
    assert_eq!(list(&v)[0]["questions"], serde_json::json!([]), "{v}");
    assert_eq!(
        ids(&env.attempts(&["--in", "worker", "--question", &q.identity])),
        vec![own.identity.clone()]
    );

    // An attempt in another session ties when it names the question itself.
    let cites = env.judgment("manager", &["what=tried", "result=undecided"], &[&q]);
    assert_eq!(
        ids(&env.attempts(&["--in", "manager", "--question", &q.identity])),
        vec![cites.identity.clone()]
    );
    assert_eq!(
        ids(&env.attempts(&["--at", "director", "--question", &q.identity])),
        vec![own.identity.clone(), cites.identity.clone()]
    );
}

#[test]
fn a_raiser_ties_the_attempts_of_the_sessions_its_run_continues() {
    let env = Env::new();
    env.init("first", None);
    let a = env.judgment("first", &["what=a", "result=undecided"], &[]);
    let template = env.root.join("work.md");
    let out = env.run(&[
        "init",
        "second",
        "--template",
        template.to_str().unwrap(),
        "--continues",
        "first",
        "--supersede",
    ]);
    assert_eq!(out.code, 0, "{}", out.all());
    let q = env.question("second", "person:owner", &[], &[&a]);
    let v = env.attempts(&["--in", "second", "--question", &q.identity]);
    assert_eq!(ids(&v), vec![a.identity.clone()]);
    assert_eq!(list(&v)[0]["questions"], serde_json::json!([q.identity]));
}

#[test]
fn every_tied_question_is_listed_and_the_filtered_one_is_named() {
    let env = Env::new();
    tree(&env);
    let a = env.judgment("worker", &["what=a", "result=undecided"], &[]);
    let q1 = env.question("worker", "session:manager", &[], &[&a]);
    let q2 = env.question("worker", "session:manager", &[], &[&a]);

    let v = env.attempts(&["--in", "worker"]);
    assert_eq!(list(&v)[0]["question"], q1.identity.as_str());
    assert_eq!(
        list(&v)[0]["questions"],
        serde_json::json!([q1.identity, q2.identity])
    );
    let v = env.attempts(&["--in", "worker", "--question", &q2.identity]);
    assert_eq!(ids(&v), vec![a.identity.clone()]);
    assert_eq!(list(&v)[0]["question"], q2.identity.as_str());
}

#[test]
fn an_answer_that_doesnt_read_verified_here_doesnt_end_the_wait() {
    let env = Env::new();
    tree(&env);
    let p = env.judgment("worker", &["what=p", "result=provisional"], &[]);
    let q = env.question("worker", "person:owner", &[], &[&p]);
    env.owner_answers(&q, "manager");
    assert!(awaiting_at(&env, "director").is_empty());

    // Without this host's note the answer reads remote.
    let note = env.sessions().join("manager").join("koto-verified-here");
    assert!(note.exists(), "the answer must have left a note");
    std::fs::remove_file(&note).unwrap();
    assert_eq!(awaiting_at(&env, "director"), vec![p.identity.clone()]);
    let v = env.attempts(&["--in", "worker", "--awaiting-review"]);
    assert_eq!(
        list(&v)[0]["review"],
        serde_json::json!({"state": "awaiting", "question": q.identity})
    );
    let shown = env.ok(&["decisions", "show", "worker", &p.id]).json();
    assert_eq!(shown["review"], list(&v)[0]["review"]);
}

#[test]
fn attempts_at_takes_an_anchor_as_the_position() {
    let env = Env::new();
    let opened = env.ok(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        "6c4b5d2f-3e0a-4b88-94e5-f60718293a64",
        "--source",
        "startup",
    ]);
    let lead = opened
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .unwrap()
        .to_string();
    env.init("w", Some(lead.as_str()));
    let template = env.root.join("work.md");
    let held = env
        .command(
            None,
            &["init", "h", "--template", template.to_str().unwrap()],
        )
        .env("KOTO_ANCHOR", &lead)
        .output()
        .unwrap();
    assert!(
        held.status.success(),
        "{}",
        String::from_utf8_lossy(&held.stderr)
    );
    env.init("x", None);
    let w = env.judgment("w", &["what=w", "result=undecided"], &[]);
    let h = env.judgment("h", &["what=h", "result=decided"], &[]);
    env.judgment("x", &["what=x", "result=decided"], &[]);

    let v = env.attempts(&["--at", &lead]);
    assert_eq!(v["at"], lead.as_str());
    assert_eq!(ids(&v), vec![w.identity.clone(), h.identity.clone()]);
    for e in list(&v) {
        assert_eq!(e["relation"], "below", "{e}");
    }
}

#[test]
fn a_hand_edited_field_reads_as_unreadable_not_as_an_attempt() {
    let env = Env::new();
    tree(&env);
    let good = env.judgment("worker", &["what=good", "result=decided"], &[]);
    let bad = env.judgment("worker", &["what=bad", "result=decided"], &[]);
    let staged = env.judgment("worker", &["what=s", "state=implement:1"], &[]);
    assert_eq!(list(&env.attempts(&["--in", "worker"])).len(), 2);
    assert_eq!(
        env.attempts(&["--in", "worker"])["unreadable"],
        serde_json::json!([])
    );

    let dir = env.sessions().join("worker/ctx/koto/decisions");
    for (d, from, to) in [
        (&bad, "\"result\":\"decided\"", "\"result\":\"maybe\""),
        (
            &staged,
            "\"state\":\"implement:1\"",
            "\"state\":\"implement\"",
        ),
    ] {
        let path = dir.join(&d.id);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{text}");
        std::fs::write(&path, text.replace(from, to)).unwrap();
    }

    let v = env.attempts(&["--in", "worker"]);
    assert_eq!(ids(&v), vec![good.identity.clone()]);
    let unreadable = v["unreadable"].as_array().unwrap();
    let got: Vec<&str> = unreadable
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(got, vec![bad.id.as_str(), staged.id.as_str()], "{v}");
    for e in unreadable {
        assert_eq!(e["status"], "unreadable");
        assert_eq!(e["session"], "worker");
        assert!(e["reason"]
            .as_str()
            .unwrap()
            .contains("stored subject field"));
    }
}

#[test]
fn a_hand_edited_question_ties_to_nothing_and_ends_no_wait() {
    let env = Env::new();
    tree(&env);
    let p1 = env.judgment(
        "worker2",
        &["what=name the table cache_entries", "result=provisional"],
        &[],
    );
    let q = env.question("director", "person:owner", &["state=review:1"], &[&p1]);
    let a = env.judgment("director", &["what=a", "result=undecided"], &[&q]);
    let v = env.attempts(&["--at", "director", "--result", "provisional"]);
    assert_eq!(list(&v)[0]["review"]["question"], q.identity.as_str());

    let path = env
        .sessions()
        .join("director/ctx/koto/decisions")
        .join(&q.id);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"state\":\"review:1\""), "{text}");
    std::fs::write(
        &path,
        text.replace("\"state\":\"review:1\"", "\"state\":\"review\""),
    )
    .unwrap();

    let v = env.attempts(&["--at", "director"]);
    let unreadable: Vec<&str> = v["unreadable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(unreadable, vec![q.id.as_str()], "{v}");
    let of = |identity: &str| {
        list(&v)
            .iter()
            .find(|e| e["identity"] == identity)
            .cloned()
            .unwrap()
    };
    assert_eq!(of(&a.identity)["questions"], serde_json::json!([]), "{v}");
    assert_eq!(of(&a.identity)["question"], Value::Null, "{v}");
    assert_eq!(of(&p1.identity)["review"]["question"], Value::Null, "{v}");

    // A person's answer on it doesn't end the wait.
    env.owner_answers(&q, "manager");
    assert_eq!(awaiting_at(&env, "director"), vec![p1.identity.clone()]);
    let shown = env.ok(&["decisions", "show", "worker2", &p1.id]).json();
    assert_eq!(shown["review"]["state"], "awaiting", "{shown}");
}

#[test]
fn a_judgment_without_the_fields_prints_what_main_printed() {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let p = e.unwrap().path();
            let dest = to.join(p.file_name().unwrap());
            if p.is_dir() {
                copy(&p, &dest);
            } else {
                std::fs::copy(&p, &dest).unwrap();
            }
        }
    }
    let env = Env::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/decisions-unchanged");
    copy(&fixture.join("sessions"), &env.sessions());
    let mut printed = String::new();
    for args in [
        &["decisions", "show", "plain", "000000000001"][..],
        &["decisions", "show", "plain", "000000000002"],
        &["decisions", "list", "plain"],
        &["decisions", "list", "lead", "--below"],
    ] {
        printed.push_str(&env.ok(args).stdout);
    }
    let want = std::fs::read_to_string(fixture.join("main-output.txt")).unwrap();
    assert_eq!(printed, want);
}
