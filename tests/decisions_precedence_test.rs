//! Integration tests for which entries aimed at a question count and
//! which one is in effect: a person's answer outranks a session's, a
//! session's answer counts only from the question's addressee, a raiser
//! closes its own question only by withdrawing it or settling it from
//! evidence, a need its raiser can't settle, where a pass-up may go, and
//! the limit of twenty open questions waiting on a person.
//!
//! Every test runs the real binary with `HOME`, `KOTO_HOME` and
//! `KOTO_SESSIONS_BASE` in its own temporary directory. A person's answer
//! is recorded from the accepted fixture under `tests/fixtures/prompt/`,
//! installed as a harness session's record the way
//! `tests/decisions_person_answer_test.rs` does, never from a live prompt.
//! A stored entry that koto's commands won't write is placed by editing a
//! stored file, the way `tests/decisions_readdress_test.rs` does; where
//! that edits a person's answer, the test notes the edited body as checked
//! on this host with koto's own `note_verified_here`, as the host that
//! wrote it would have.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use prompt_fixture as fx;
use serde_json::{json, Value};

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

    fn error(&self) -> String {
        self.json()["error"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// A raised question: its session, id, the session's id and the prompt
/// header its content was given.
#[derive(Clone)]
struct Q {
    session: String,
    id: String,
    session_id: String,
    header: String,
}

impl Q {
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

    fn run(&self, args: &[&str]) -> Out {
        self.run_env(&[], args)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(out.code, 0, "expected success from {args:?}\n{}", out.all());
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn escalate_run(&self, name: &str, question: &str, recipient: &str, extra: &[&str]) -> Out {
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let mut args = vec![
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
        ];
        for e in extra {
            args.extend_from_slice(&["--subject", e]);
        }
        args.extend_from_slice(&["--by", "session"]);
        self.run(&args)
    }

    /// Raise `question` in `name` to `recipient`, with `extra` subject
    /// fields and the content the accepted fixture shows stored for it, so
    /// a person's answer can be recorded to it.
    fn raise(&self, name: &str, question: &str, recipient: &str, extra: &[&str]) -> Q {
        let lines = fx::lines("accepted");
        let (shown, labels) = fx::shown(&lines, 1);
        let content = fx::content(&shown, &labels);
        let packet = format!("packet={}", fx::packet(&content));
        let mut all: Vec<&str> = extra.to_vec();
        all.push(&packet);
        let out = self.escalate_run(name, question, recipient, &all);
        assert_eq!(out.code, 0, "{}", out.all());
        let out = out.json();
        let id = out["id"].as_str().unwrap().to_string();
        let file = self.root.join(format!("content-{name}-{id}"));
        std::fs::write(&file, &content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                name,
                &id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        Q {
            session: name.to_string(),
            id,
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
            header: written["prompt_header"].as_str().unwrap().to_string(),
        }
    }

    /// `koto decisions answer <q> --in <in_session>` with `extra`.
    fn answer_run(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        let mut args = vec!["decisions", "answer", &q.session, &q.id, "--in", in_session];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// A session's `answered` answer to `q` from `in_session`.
    fn say(&self, q: &Q, in_session: &str, text: &str) -> Out {
        self.answer_run(q, in_session, &["--text", text, "--by", "session"])
    }

    /// A session's `answered` answer that must be accepted; its id.
    fn say_ok(&self, q: &Q, in_session: &str, text: &str) -> String {
        let out = self.say(q, in_session, text);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// Record `person:owner`'s answer to `q` in `in_session` from a fresh
    /// copy of the accepted fixture (its own call id), with `extra` flags.
    fn person_run(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        let call = format!("toolu_ZQXMARKERcall{n:012}");
        let lines = fx::with_call(&fx::lines("accepted"), &call);
        let harness = fx::harness();
        fx::install(&harness, &fx::render(&lines, &harness, &[&q.header]));
        let pointer = format!("{harness}/{call}/1");
        let mut args = vec![
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--by",
            "person:owner",
            "--prompt",
            &pointer,
        ];
        args.extend_from_slice(extra);
        self.run_env(&[("CLAUDE_CODE_SESSION_ID", harness.as_str())], &args)
    }

    /// A person's answer that must be accepted; its id.
    fn person(&self, q: &Q, in_session: &str) -> String {
        self.person_with(q, in_session, &[])
    }

    fn person_with(&self, q: &Q, in_session: &str, extra: &[&str]) -> String {
        let out = self.person_run(q, in_session, extra);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn withdraw(&self, q: &Q, in_session: &str, reason: &str) -> Out {
        self.answer_run(
            q,
            in_session,
            &[
                "--outcome",
                "withdrawn",
                "--reason",
                reason,
                "--by",
                "session",
            ],
        )
    }

    fn settle(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        let mut args = vec![
            "--outcome",
            "settled",
            "--text",
            "settled",
            "--by",
            "session",
        ];
        args.extend_from_slice(extra);
        self.answer_run(q, in_session, &args)
    }

    fn readdress_run(&self, q: &Q, in_session: &str, to: &str) -> Out {
        self.run(&[
            "decisions",
            "readdress",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--to",
            to,
            "--by",
            "session",
        ])
    }

    fn readdress(&self, q: &Q, in_session: &str, to: &str) -> String {
        let out = self.readdress_run(q, in_session, to);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn open(&self, person: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--person", person]).json();
        out["questions"].as_array().unwrap().clone()
    }

    fn at(&self, position: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--at", position]).json();
        out["questions"].as_array().unwrap().clone()
    }

    fn next(&self, name: &str) -> Value {
        self.ok(&["next", name, "--no-cleanup"]).json()
    }

    /// `koto decisions show <holder> <id>`.
    fn show(&self, holder: &str, id: &str) -> Value {
        self.ok(&["decisions", "show", holder, id]).json()
    }

    /// The entry for `id` in `koto decisions list <holder> --format v1`.
    fn listed(&self, holder: &str, id: &str) -> Value {
        let out = self
            .ok(&["decisions", "list", holder, "--format", "v1"])
            .json();
        out["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("{holder} lists no {id}: {out}"))
            .clone()
    }

    /// Every file under the sessions directory with its bytes, lock files
    /// aside.
    fn snapshot_all(&self) -> BTreeMap<PathBuf, Vec<u8>> {
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
        let dir = self.sessions();
        walk(&dir, &dir, &mut out);
        out
    }

    fn aimed(&self, holder: &str, q: &Q, id: &str) -> PathBuf {
        self.sessions()
            .join(holder)
            .join("ctx/koto/aimed")
            .join(&q.session_id)
            .join(id)
    }

    fn stored(&self, holder: &str, q: &Q, id: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.aimed(holder, q, id)).unwrap()).unwrap()
    }

    /// Store `body` as entry `id` in `holder`'s aimed area for `q`.
    fn plant(&self, holder: &str, q: &Q, id: &str, body: &Value) {
        let path = self.aimed(holder, q, id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(body).unwrap()).unwrap();
    }

    /// Note on this host that `holder` checked the prompt of the stored
    /// person answer `body`, as the host that wrote it would have.
    fn note(&self, holder: &str, body: &Value) {
        let parsed =
            koto::decisions::body::DecisionBody::parse(&serde_json::to_vec(body).unwrap()).unwrap();
        koto::decisions::prompt::note_verified_here(
            &self.root.join("home"),
            &self.sessions().join(holder),
            holder,
            &parsed,
        )
        .unwrap();
    }

    /// Record a judgment in `name` citing `basis`.
    fn cite(&self, name: &str, basis: &str) {
        self.ok(&[
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

fn holds(qs: &[Value], question: &str) -> bool {
    names(qs).contains(&question)
}

/// The session ids `with` gives a time `years` years from `time`.
fn shift(time: &str, years: i32) -> String {
    let year: i32 = time[..4].parse().unwrap();
    format!("{:04}{}", year + years, &time[4..])
}

fn chain(env: &Env) {
    env.init("director", &[]);
    env.init("manager", &["--dispatched-by", "director"]);
    env.init("worker", &["--dispatched-by", "manager"]);
    env.init("other-lead", &[]);
    env.init("other-worker", &["--dispatched-by", "other-lead"]);
}

const POSITIONS: [&str; 5] = [
    "worker",
    "manager",
    "director",
    "other-lead",
    "other-worker",
];

fn in_no_open_read(env: &Env, question: &str) {
    assert!(!holds(&env.open("owner"), question), "--person owner");
    for p in POSITIONS {
        assert!(!holds(&env.at(p), question), "--at {p}");
    }
}

// Which entries count.

#[test]
fn a_person_question_is_listed_closed_by_an_unrelated_session_and_reported() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "person:owner", &[]);
    let open = env.open("owner");
    assert_eq!(names(&open), vec!["q1"]);
    assert_eq!(open[0]["identity"], q.identity().as_str());

    let answer = env.person(&q, "other-lead");
    in_no_open_read(&env, "q1");
    let reported = questions(&env.next("worker"));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0]["identity"], q.identity().as_str());
    assert_eq!(reported[0]["answer"]["id"], answer.as_str());
    assert_eq!(reported[0]["answer"]["session"], "other-lead");
    assert_eq!(reported[0]["answer"]["decider"]["kind"], "person");
    assert_eq!(reported[0]["answer"]["reading"], "verified");
    assert_eq!(
        reported[0]["answer"]["text"],
        fx::answer_of(&fx::lines("accepted"), 1).as_str()
    );
    let shown = env.show("other-lead", &answer);
    assert_eq!(shown["counted"], true);
    assert_eq!(shown["governs"], true);
}

#[test]
fn a_sessions_answer_counts_only_from_the_addressee() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    for s in ["director", "other-lead"] {
        let out = env.say(&q1, s, "no");
        assert_eq!(out.code, 2, "{}", out.all());
        assert_eq!(out.json()["field"], "in");
        assert!(out.error().contains("addressee"), "{}", out.error());
    }
    env.readdress(&q1, "manager", "session:director");
    let out = env.say(&q1, "manager", "no");
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(out.error().contains("addressee"), "{}", out.error());
    let answer = env.say_ok(&q1, "director", "yes");
    assert_eq!(env.show("director", &answer)["counted"], true);

    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    env.say_ok(&q2, "manager", "yes");
    in_no_open_read(&env, "q2");
}

#[test]
fn the_addressees_successor_answers_for_it() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    env.init("succ", &["--continues", "manager", "--supersede"]);
    let id = env.say_ok(&q, "succ", "yes");
    assert_eq!(env.show("succ", &id)["governs"], true);
    in_no_open_read(&env, "q1");
}

#[test]
fn a_session_never_answers_a_question_waiting_on_a_person() {
    let env = Env::new();
    chain(&env);
    let raised = env.raise("worker", "q1", "person:owner", &[]);
    let passed = env.raise("worker", "q2", "session:manager", &[]);
    env.readdress(&passed, "manager", "person:owner");
    let all0 = env.snapshot_all();
    for q in [&raised, &passed] {
        for s in ["worker", "manager", "director", "other-lead"] {
            let out = env.say(q, s, "no");
            assert_eq!(out.code, 2, "{s}: {}", out.all());
        }
    }
    assert_eq!(env.snapshot_all(), all0, "a refusal wrote");
    // `manager` was q2's addressee; the message says who it waits on.
    let out = env.say(&passed, "manager", "no");
    assert!(out.error().contains("person:owner"), "{}", out.error());

    // A person's answer from a session that never raised nor held it.
    env.person(&raised, "other-worker");
    env.person(&passed, "other-worker");
    in_no_open_read(&env, "q1");
    in_no_open_read(&env, "q2");
}

#[test]
fn a_persons_answer_closes_a_question_waiting_on_a_session() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    env.person(&q, "other-lead");
    in_no_open_read(&env, "q1");
}

#[test]
fn every_refusal_writes_nothing_and_earlier_checks_answer_first() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let all0 = env.snapshot_all();

    let missing = Q {
        id: "000000000099".to_string(),
        ..q.clone()
    };
    let out = env.say(&missing, "manager", "x");
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["code"], "decision_not_found");
    let out = env.withdraw(&missing, "worker", "x");
    assert_eq!(out.json()["code"], "decision_not_found");

    // An answer in the asking session is still refused as such.
    let out = env.say(&q, "worker", "x");
    assert_eq!(out.code, 2);
    assert!(out.error().contains("asker"), "{}", out.error());

    for out in [
        env.say(&q, "other-lead", "x"),
        env.withdraw(&q, "manager", "x"),
        env.answer_run(&q, "worker", &["--outcome", "withdrawn", "--by", "session"]),
        env.settle(&q, "worker", &[]),
        env.readdress_run(&q, "manager", "session:other-lead"),
    ] {
        assert_eq!(out.code, 2, "{}", out.all());
    }
    assert_eq!(env.snapshot_all(), all0);

    // A reversed question is still refused as reversed.
    env.ok(&[
        "decisions",
        "reverse",
        "worker",
        &q.id,
        "--in",
        "manager",
        "--by",
        "session",
    ]);
    let all1 = env.snapshot_all();
    let out = env.withdraw(&q, "worker", "x");
    assert_eq!(out.code, 2);
    assert!(out.error().contains("reversed"), "{}", out.error());
    assert_eq!(env.snapshot_all(), all1);
}

#[test]
fn a_stored_answer_from_a_session_that_isnt_the_addressee_counts_for_nothing() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    // An answer by `manager` to q2, moved into `other-lead` as an answer
    // to q1 recorded there.
    let id = env.say_ok(&q2, "manager", "planted");
    let mut body = env.stored("manager", &q2, &id);
    let lead = env
        .escalate_run("other-lead", "x", "session:director", &[])
        .json();
    body["session"] = lead["decision"]["session"].clone();
    body["id"] = json!("000000000050");
    body["target"]["decision"] = json!(q1.id);
    body["subject"]["id"] = json!(q1.id);
    env.plant("other-lead", &q1, "000000000050", &body);

    for e in [
        env.listed("other-lead", "000000000050"),
        env.show("other-lead", "000000000050"),
    ] {
        assert_eq!(e["counted"], false, "{e}");
        assert_eq!(e["reason"], "not_addressee");
        assert_eq!(e["governs"], false);
        assert_eq!(e["status"], "in-effect");
    }
    assert!(holds(&env.at("manager"), "q1"));
    assert!(holds(&env.at("director"), "q1"));
    // q2's own answer is reported; the planted one isn't.
    assert_eq!(names(&questions(&env.next("worker"))), vec!["q2"]);

    // A question holding only entries that don't count takes what an
    // open question takes.
    env.readdress(&q1, "manager", "session:director");
    env.say_ok(&q1, "director", "yes");
}

#[test]
fn a_stored_answer_is_judged_against_the_addressee_the_question_has_now() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let answer = env.say_ok(&q1, "manager", "yes");
    assert_eq!(env.show("manager", &answer)["governs"], true);
    // A pass-up by `manager` to `director`, stored earlier than the answer.
    let pass = env.readdress(&q2, "manager", "session:director");
    let mut body = env.stored("manager", &q2, &pass);
    body["id"] = json!("000000000051");
    body["target"]["decision"] = json!(q1.id);
    body["subject"]["id"] = json!(q1.id);
    let earlier = shift(
        env.stored("manager", &q1, &answer)["time"]
            .as_str()
            .unwrap(),
        -1,
    );
    body["time"] = json!(earlier);
    env.plant("manager", &q1, "000000000051", &body);

    let e = env.show("manager", &answer);
    assert_eq!(e["counted"], false, "{e}");
    assert_eq!(e["reason"], "not_addressee");
    let at = env.at("director");
    assert!(holds(&at, "q1"), "{at:?}");
}

#[test]
fn a_person_answer_read_remotely_counts_as_its_holders_answer() {
    let env = Env::new();
    chain(&env);
    // q1 waits on `other-lead`, q2 on `manager`.
    let q1 = env.raise("worker", "q1", "session:other-lead", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let a1 = env.person(&q1, "other-lead");
    let a2 = env.person(&q2, "other-lead");
    // `other-lead` arrives on this host as an imported session: its
    // entries are here, the host-local note of checked prompts isn't.
    std::fs::remove_file(env.sessions().join("other-lead").join("koto-verified-here")).unwrap();

    let e1 = env.show("other-lead", &a1);
    assert_eq!(e1["reading"], "a remote session's answer", "{e1}");
    assert_eq!(e1["counted"], true, "{e1}");
    assert_eq!(e1["governs"], true, "{e1}");
    assert!(!holds(&env.at("other-lead"), "q1"));

    let e2 = env.show("other-lead", &a2);
    assert_eq!(e2["counted"], false, "{e2}");
    assert_eq!(e2["reason"], "not_addressee");
    assert!(holds(&env.at("manager"), "q2"));
    assert!(holds(&env.at("worker"), "q2"));
}

// Which answer is in effect.

#[test]
fn a_persons_later_answer_governs_over_a_sessions_earlier_one() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    env.readdress(&q, "manager", "session:director");
    let session = env.say_ok(&q, "director", "Postgres");
    let owner = env.person(&q, "other-lead");
    let s = env.listed("director", &session);
    let o = env.listed("other-lead", &owner);
    assert_eq!(
        (s["counted"].clone(), s["governs"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(
        (o["counted"].clone(), o["governs"].clone()),
        (json!(true), json!(true))
    );
    in_no_open_read(&env, "q1");
    let reported = questions(&env.next("worker"));
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0]["answer"]["id"], owner.as_str());
}

#[test]
fn a_session_answer_after_the_owners_is_refused_and_a_stored_one_doesnt_govern() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let owner = env.person(&q, "other-lead");
    let all0 = env.snapshot_all();
    let out = env.say(&q, "manager", "again");
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(out.error().contains("already closed"), "{}", out.error());
    assert!(
        out.error().contains(&format!("other-lead:{owner}")),
        "{}",
        out.error()
    );
    assert_eq!(env.snapshot_all(), all0);

    // A session's answer stored later than the owner's.
    let id = env.say_ok(&q2, "manager", "later");
    let mut body = env.stored("manager", &q2, &id);
    body["id"] = json!("000000000052");
    body["target"]["decision"] = json!(q.id);
    body["subject"]["id"] = json!(q.id);
    body["time"] = json!(shift(body["time"].as_str().unwrap(), 1));
    env.plant("manager", &q, "000000000052", &body);
    let late = env.show("manager", "000000000052");
    assert_eq!(late["counted"], true, "{late}");
    assert_eq!(late["governs"], false);
    assert_eq!(env.show("other-lead", &owner)["governs"], true);

    // Cited, the owner's answer ends the report; the stored entry changes
    // nothing.
    env.cite("worker", &format!("other-lead:{owner}"));
    assert_eq!(names(&questions(&env.next("worker"))), vec!["q2"]);
}

#[test]
fn the_earliest_counted_session_entry_governs() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let first = env.say_ok(&q, "manager", "first");
    let mut body = env.stored("manager", &q, &first);
    body["id"] = json!("000000000053");
    body["time"] = json!(shift(body["time"].as_str().unwrap(), 1));
    env.plant("manager", &q, "000000000053", &body);
    assert_eq!(env.show("manager", &first)["governs"], true);
    let second = env.show("manager", "000000000053");
    assert_eq!(second["counted"], true);
    assert_eq!(second["governs"], false);

    let out = env.say(&q, "manager", "third");
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(
        out.error().contains(&format!("manager:{first}")),
        "{}",
        out.error()
    );
}

#[test]
fn a_person_answer_citing_another_governs_whatever_its_time() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "person:owner", &[]);
    let first = env.person(&q, "other-lead");
    let basis = format!("other-lead:{first}");
    let second = env.person_with(&q, "other-worker", &["--basis", &basis]);
    let check = |env: &Env| {
        let a = env.show("other-lead", &first);
        let b = env.show("other-worker", &second);
        assert_eq!(
            (a["counted"].clone(), a["governs"].clone()),
            (json!(true), json!(false))
        );
        assert_eq!(
            (b["counted"].clone(), b["governs"].clone()),
            (json!(true), json!(true))
        );
    };
    check(&env);

    // The second host's clock was behind.
    let mut body = env.stored("other-worker", &q, &second);
    let first_time = env.stored("other-lead", &q, &first)["time"].clone();
    body["time"] = json!(shift(first_time.as_str().unwrap(), -1));
    env.plant("other-worker", &q, &second, &body);
    env.note("other-worker", &body);
    assert_eq!(env.show("other-worker", &second)["reading"], "verified");
    check(&env);
}

#[test]
fn of_two_person_answers_citing_neither_the_later_governs() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "person:owner", &[]);
    let first = env.person(&q, "other-lead");
    let second = env.person(&q, "other-worker");
    assert_eq!(env.show("other-lead", &first)["governs"], false);
    assert_eq!(env.show("other-worker", &second)["governs"], true);
    assert_eq!(env.show("other-lead", &first)["counted"], true);
    // Exactly one closing entry governs.
    let governing: usize = ["other-lead", "other-worker"]
        .iter()
        .map(|s| {
            env.ok(&["decisions", "list", s, "--format", "v1"]).json()["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["governs"] == true)
                .count()
        })
        .sum();
    assert_eq!(governing, 1);
}

#[test]
fn nothing_aimed_at_a_removed_raisers_question_counts_for_a_new_one() {
    let env = Env::new();
    chain(&env);
    let old = env.raise("worker", "q1", "session:manager", &[]);
    let old_person = env.raise("worker", "q2", "person:owner", &[]);
    env.readdress(&old, "manager", "session:director");
    env.say_ok(&old, "director", "old");
    env.person(&old_person, "other-lead");
    std::fs::remove_dir_all(env.sessions().join("worker")).unwrap();
    env.init("worker", &["--dispatched-by", "manager"]);
    let fresh = env.raise("worker", "q1", "session:manager", &[]);
    env.raise("worker", "q2", "person:owner", &[]);
    assert_ne!(fresh.session_id, old.session_id);
    let at = env.at("manager");
    let e = at.iter().find(|e| e["question"] == "q1").unwrap();
    assert_eq!(e["identity"], fresh.identity().as_str());
    assert_eq!(e["addressees"].as_array().unwrap().len(), 1);
    assert!(holds(&env.open("owner"), "q2"));
    assert!(env.next("worker").get("questions").is_none());
}

// What the asker's tick reports.

#[test]
fn the_report_follows_the_entry_in_effect() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    env.readdress(&q, "manager", "session:director");
    let session = env.say_ok(&q, "director", "Postgres");
    env.cite("worker", &format!("director:{session}"));
    assert!(env.next("worker").get("questions").is_none());

    let owner = env.person(&q, "other-lead");
    for _ in 0..2 {
        let reported = questions(&env.next("worker"));
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(reported[0]["answer"]["id"], owner.as_str());
        assert_eq!(reported[0]["answer"]["decider"]["kind"], "person");
    }
    env.cite("worker", &format!("director:{session}"));
    assert_eq!(questions(&env.next("worker")).len(), 1);
    env.cite("worker", &format!("other-lead:{owner}"));
    assert!(env.next("worker").get("questions").is_none());
}

#[test]
fn a_question_the_run_closed_itself_isnt_reported() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let w = env.withdraw(&q1, "worker", "not needed");
    assert_eq!(w.code, 0, "{}", w.all());
    let s = env.settle(&q2, "worker", &["--evidence", "event:worker:1"]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(env.next("worker").get("questions").is_none());
}

// Where a pass-up may go.

#[test]
fn a_pass_up_goes_to_a_person_or_the_recorders_dispatcher() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let all0 = env.snapshot_all();
    for (in_s, to) in [
        ("manager", "session:other-lead"),
        ("manager", "session:worker"),
        ("worker", "session:director"),
    ] {
        let out = env.readdress_run(&q, in_s, to);
        assert_eq!(out.code, 2, "{in_s} {to}: {}", out.all());
        assert_eq!(out.json()["field"], "to");
        assert!(out.error().contains("dispatcher"), "{}", out.error());
    }
    let out = env.readdress_run(&q, "manager", "session:other-lead");
    assert!(out.error().contains("session:director"), "{}", out.error());
    let out = env.readdress_run(&q, "worker", "session:director");
    assert!(out.error().contains("session:manager"), "{}", out.error());
    assert_eq!(env.snapshot_all(), all0);

    let a = env.raise("worker", "a", "session:manager", &[]);
    let b = env.raise("worker", "b", "session:manager", &[]);
    let c = env.raise("worker", "c", "session:manager", &[]);
    let pass = env.readdress(&a, "manager", "session:director");
    assert_eq!(
        env.stored("manager", &a, &pass)["subject"]["dispatcher"],
        "director"
    );
    env.readdress(&b, "manager", "person:owner");
    env.readdress(&c, "worker", "person:owner");

    // No recorded dispatcher: only a person.
    let d = env.raise("worker", "d", "session:director", &[]);
    let out = env.readdress_run(&d, "director", "session:manager");
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(
        out.error().contains("no recorded dispatcher"),
        "{}",
        out.error()
    );
    env.readdress(&d, "director", "person:owner");
}

#[test]
fn a_stored_pass_up_breaking_the_destination_rule_is_left_out() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let pass = env.readdress(&q2, "manager", "session:director");
    let mut body = env.stored("manager", &q2, &pass);
    body["id"] = json!("000000000054");
    body["target"]["decision"] = json!(q1.id);
    body["subject"]["id"] = json!(q1.id);
    body["subject"]["recipient"] = json!("session:other-lead");
    env.plant("manager", &q1, "000000000054", &body);

    let e = env.show("manager", "000000000054");
    assert_eq!(e["counted"], false, "{e}");
    assert_eq!(e["reason"], "destination");
    assert!(e.get("governs").is_none());
    assert!(!holds(&env.at("other-lead"), "q1"));
    let at = env.at("manager");
    let q = at.iter().find(|e| e["question"] == "q1").unwrap();
    assert_eq!(
        q["addressees"],
        json!([{"typed": true, "kind": "session", "name": "manager"}])
    );
    assert_eq!(env.show("manager", &pass)["counted"], true);

    // Judged from the dispatcher it recorded: one naming `other-lead`
    // keeps it, whatever `manager`'s links say now.
    body["subject"]["dispatcher"] = json!("other-lead");
    env.plant("manager", &q1, "000000000054", &body);
    assert_eq!(env.show("manager", "000000000054")["counted"], true);
    assert!(holds(&env.at("other-lead"), "q1"));
}

#[test]
fn a_pass_up_the_ordering_skips_reads_not_counted() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let applied = env.readdress(&q, "manager", "session:director");
    let template = env.stored("manager", &q, &applied);
    let time = template["time"].as_str().unwrap().to_string();

    // A pass-up by `other-lead` (neither raiser nor addressee), one naming
    // the addressee it already had, and a second written against
    // `manager` at the same time as the applied one.
    let lead = env
        .escalate_run("other-lead", "x", "session:director", &[])
        .json();
    let mut by_lead = template.clone();
    by_lead["session"] = lead["decision"]["session"].clone();
    by_lead["id"] = json!("000000000055");
    by_lead["subject"]["recipient"] = json!("person:owner");
    env.plant("other-lead", &q, "000000000055", &by_lead);

    let mut same = template.clone();
    same["id"] = json!("000000000056");
    same["subject"]["recipient"] = json!("session:director");
    env.plant("manager", &q, "000000000056", &same);

    let mut twin = template.clone();
    twin["id"] = json!("000000000057");
    twin["subject"]["recipient"] = json!("person:owner");
    twin["time"] = json!(time);
    env.plant("manager", &q, "000000000057", &twin);

    for (holder, id) in [
        ("other-lead", "000000000055"),
        ("manager", "000000000056"),
        ("manager", "000000000057"),
    ] {
        let e = env.show(holder, id);
        assert_eq!(e["counted"], false, "{holder} {id}: {e}");
        assert_eq!(e["reason"], "skipped");
    }
    assert_eq!(env.show("manager", &applied)["counted"], true);
}

// Withdrawing and settling.

#[test]
fn the_raiser_withdraws_its_question_with_a_reason() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "person:owner", &[]);
    let out = env.withdraw(&q, "worker", "not needed");
    assert_eq!(out.code, 0, "{}", out.all());
    let entry = out.json();
    assert_eq!(entry["decision"]["kind"], "answer");
    assert_eq!(entry["decision"]["outcome"], "withdrawn");
    let id = entry["id"].as_str().unwrap();
    assert!(env.aimed("worker", &q, id).exists());
    in_no_open_read(&env, "q1");
    assert!(env.next("worker").get("questions").is_none());
    let listed = env.listed("worker", id);
    assert_eq!(listed["decision"]["outcome"], "withdrawn");
    assert_eq!(listed["governs"], true);
}

#[test]
fn a_withdrawal_needs_a_reason_the_raiser_and_a_session() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let all0 = env.snapshot_all();
    let out = env.answer_run(&q, "worker", &["--outcome", "withdrawn", "--by", "session"]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "reason");
    for s in ["manager", "director", "other-lead"] {
        let out = env.withdraw(&q, s, "x");
        assert_eq!(out.code, 2, "{s}: {}", out.all());
        assert_eq!(out.json()["field"], "in");
    }
    let out = env.answer_run(
        &q,
        "worker",
        &[
            "--outcome",
            "withdrawn",
            "--reason",
            "x",
            "--by",
            "person:owner",
        ],
    );
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "by");
    assert_eq!(env.snapshot_all(), all0);

    env.init(
        "worker2",
        &[
            "--continues",
            "worker",
            "--supersede",
            "--dispatched-by",
            "manager",
        ],
    );
    let out = env.withdraw(&q, "worker2", "the successor withdraws it");
    assert_eq!(out.code, 0, "{}", out.all());
    in_no_open_read(&env, "q1");
}

#[test]
fn a_stored_withdrawal_from_another_session_counts_for_nothing() {
    let env = Env::new();
    chain(&env);
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let donor = env.raise("worker", "q2", "session:manager", &[]);
    let w = env.withdraw(&donor, "worker", "x").json();
    let mut body = w["decision"].clone();
    let mgr = env.escalate_run("manager", "x", "person:owner", &[]).json();
    body["session"] = mgr["decision"]["session"].clone();
    body["id"] = json!("000000000058");
    body["target"]["decision"] = json!(q.id);
    body["subject"]["id"] = json!(q.id);
    env.plant("manager", &q, "000000000058", &body);
    let e = env.show("manager", "000000000058");
    assert_eq!(e["counted"], false);
    assert_eq!(e["reason"], "not_raiser");
    assert!(holds(&env.at("manager"), "q1"));
}

#[test]
fn the_raiser_settles_from_the_owners_answer_to_a_question_with_its_label() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &["label=cache"]);
    env.readdress(&q1, "manager", "session:director");
    let session = env.say_ok(&q1, "director", "Postgres");
    let owner = env.person(&q1, "other-lead");
    let q5 = env.raise("worker", "q5", "person:owner", &["label=cache"]);
    let q6 = env.raise("worker", "q6", "person:owner", &["label=queue"]);
    let q7 = env.raise("worker", "q7", "person:owner", &[]);
    let q8 = env.raise("worker", "q8", "person:dana", &["label=cache"]);
    let all0 = env.snapshot_all();

    let refused = |q: &Q, basis: &str, says: &str| {
        let extra: Vec<&str> = if basis.is_empty() {
            vec![]
        } else {
            vec!["--basis", basis]
        };
        let out = env.settle(q, "worker", &extra);
        assert_eq!(out.code, 2, "{}", out.all());
        assert!(
            out.error().contains(says),
            "wanted '{says}': {}",
            out.error()
        );
    };
    let owner_basis = format!("other-lead:{owner}");
    refused(&q5, "", "--basis");
    refused(
        &q5,
        &format!("director:{session}"),
        "isn't a person's answer",
    );
    refused(&q6, &owner_basis, "labelled 'cache', not 'queue'");
    refused(&q7, &owner_basis, "no label");
    refused(&q8, &owner_basis, "not person:dana");
    assert_eq!(env.snapshot_all(), all0);

    let out = env.settle(&q5, "worker", &["--basis", &owner_basis]);
    assert_eq!(out.code, 0, "{}", out.all());
    let settled = out.json()["id"].as_str().unwrap().to_string();
    in_no_open_read(&env, "q5");
    assert_eq!(names(&env.open("owner")), vec!["q6", "q7"]);

    // Settled once, it isn't settled or withdrawn again.
    let out = env.withdraw(&q5, "worker", "x");
    assert_eq!(out.code, 2);
    assert!(
        out.error().contains(&format!("worker:{settled}")),
        "{}",
        out.error()
    );

    // The owner answers q1 again; the settle's basis no longer governs.
    env.person(&q1, "other-worker");
    let e = env.show("worker", &settled);
    assert_eq!(e["counted"], false, "{e}");
    assert_eq!(e["reason"], "settle_basis");
    assert!(holds(&env.open("owner"), "q5"));
    refused(
        &q5,
        &owner_basis,
        "isn't the answer in effect on its own question",
    );
}

#[test]
fn a_settle_refuses_a_remote_answer_and_a_removed_sessions_reused_name() {
    let env = Env::new();
    chain(&env);
    env.init("gone", &[]);
    let q1 = env.raise("worker", "q1", "person:owner", &["label=cache"]);
    let owner = env.person(&q1, "gone");
    let q5 = env.raise("worker", "q5", "person:owner", &["label=cache"]);

    // `gone` reads as imported: this host holds no note that it checked
    // the prompt.
    std::fs::remove_file(env.sessions().join("gone").join("koto-verified-here")).unwrap();
    let basis = format!("gone:{owner}");
    let out = env.settle(&q5, "worker", &["--basis", &basis]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(out.error().contains("remote"), "{}", out.error());

    // A new `gone` writes a judgment under the removed answer's id; the
    // basis isn't taken for the owner's answer.
    std::fs::remove_dir_all(env.sessions().join("gone")).unwrap();
    env.init("gone", &[]);
    let j = env
        .ok(&[
            "decisions",
            "record",
            "gone",
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
    assert_eq!(
        j["id"],
        owner.as_str(),
        "the ids must collide for this test"
    );
    let all0 = env.snapshot_all();
    let out = env.settle(&q5, "worker", &["--basis", &basis]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(
        out.error().contains("isn't a person's answer"),
        "{}",
        out.error()
    );
    assert_eq!(env.snapshot_all(), all0);
    assert!(holds(&env.open("owner"), "q5"));
}

#[test]
fn a_question_waiting_on_a_session_is_settled_only_with_evidence() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let q3 = env.raise("worker", "q3", "session:manager", &[]);
    let out = env.settle(&q1, "worker", &[]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "basis");
    let out = env.settle(&q1, "worker", &["--basis", "manager:000000000077"]);
    assert_eq!(out.code, 2);
    assert_eq!(
        env.settle(&q1, "worker", &["--evidence", "event:worker:1"])
            .code,
        0
    );
    let j = env
        .ok(&[
            "decisions",
            "record",
            "manager",
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
    let basis = format!("manager:{}", j["id"].as_str().unwrap());
    assert_eq!(env.settle(&q2, "worker", &["--basis", &basis]).code, 0);
    // Not the raiser or its successor.
    let out = env.settle(&q3, "manager", &["--basis", &basis]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "in");
    // A closed question isn't settled.
    let answer = env.say_ok(&q3, "manager", "yes");
    let out = env.settle(&q3, "worker", &["--basis", &basis]);
    assert_eq!(out.code, 2);
    assert!(
        out.error().contains(&format!("manager:{answer}")),
        "{}",
        out.error()
    );
}

#[test]
fn record_refuses_every_answer_outcome_and_reverse_refuses_a_settle_or_withdrawal() {
    let env = Env::new();
    chain(&env);
    for outcome in ["answered", "settled", "withdrawn"] {
        let out = env.run(&[
            "decisions",
            "record",
            "worker",
            "--kind",
            "answer",
            "--outcome",
            outcome,
            "--subject",
            "session=worker",
            "--subject",
            "id=000000000001",
            "--subject",
            "text=x",
            "--by",
            "session",
        ]);
        assert_eq!(out.code, 2, "{outcome}: {}", out.all());
    }
    let q1 = env.raise("worker", "q1", "session:manager", &[]);
    let q2 = env.raise("worker", "q2", "session:manager", &[]);
    let w = env.withdraw(&q1, "worker", "x").json();
    let s = env
        .settle(&q2, "worker", &["--evidence", "event:worker:1"])
        .json();
    for e in [w, s] {
        let out = env.run(&[
            "decisions",
            "reverse",
            "worker",
            e["id"].as_str().unwrap(),
            "--in",
            "worker",
            "--by",
            "session",
        ]);
        assert_eq!(out.code, 2, "{}", out.all());
        assert_eq!(out.json()["reason"], "reverses_answer");
    }
}

// Needs.

#[test]
fn a_need_is_raised_listed_and_closed_like_a_question() {
    let env = Env::new();
    chain(&env);
    for need in ["credential", "reserved-step", "decision"] {
        let out = env.escalate_run(
            "worker",
            &format!("x-{need}"),
            "person:owner",
            &[&format!("need={need}")],
        );
        assert_eq!(out.code, 0, "{need}: {}", out.all());
    }
    let out = env.escalate_run("worker", "bad", "person:owner", &["need=money"]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "subject.need");
    let long = format!("label={}", "x".repeat(129));
    let out = env.escalate_run("worker", "bad", "person:owner", &[&long]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "subject.label");
    let fits = format!("label={}", "x".repeat(128));
    assert_eq!(
        env.escalate_run("worker", "fits", "person:owner", &[&fits])
            .code,
        0
    );

    let open = env.open("owner");
    let need_of = |q: &str| {
        open.iter().find(|e| e["question"] == q).unwrap()["need"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(need_of("x-credential"), "credential");
    assert_eq!(need_of("x-reserved-step"), "reserved-step");
    assert_eq!(need_of("x-decision"), "decision");
    assert_eq!(need_of("fits"), "decision");
    for p in ["manager", "director"] {
        let at = env.at(p);
        let e = at.iter().find(|e| e["question"] == "x-credential").unwrap();
        assert_eq!(e["need"], "credential");
        assert_eq!(e["relation"], "below");
    }

    let n1 = env.raise("worker", "n1", "person:owner", &["need=credential"]);
    env.person(&n1, "other-lead");
    in_no_open_read(&env, "n1");
    let n2 = env.raise("worker", "n2", "session:manager", &["need=reserved-step"]);
    env.say_ok(&n2, "manager", "done");
    in_no_open_read(&env, "n2");
}

#[test]
fn the_raiser_cant_settle_a_need_but_can_withdraw_it() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "person:owner", &["label=cache"]);
    let owner = env.person(&q1, "other-lead");
    let n1 = env.raise(
        "worker",
        "n1",
        "person:owner",
        &["need=credential", "label=cache"],
    );
    let basis = format!("other-lead:{owner}");
    let out = env.settle(&n1, "worker", &["--basis", &basis]);
    assert_eq!(out.code, 2);
    assert!(out.error().contains("credential need"), "{}", out.error());

    // A stored settle of the need counts for nothing.
    let donor = env.raise("worker", "q2", "person:owner", &["label=cache"]);
    let s = env.settle(&donor, "worker", &["--basis", &basis]).json();
    let mut body = s["decision"].clone();
    body["id"] = json!("000000000059");
    body["target"]["decision"] = json!(n1.id);
    body["subject"]["id"] = json!(n1.id);
    env.plant("worker", &n1, "000000000059", &body);
    let e = env.show("worker", "000000000059");
    assert_eq!(e["counted"], false);
    assert_eq!(e["reason"], "need_not_settleable");
    assert!(holds(&env.open("owner"), "n1"));

    assert_eq!(env.withdraw(&n1, "worker", "no longer needed").code, 0);
    in_no_open_read(&env, "n1");
}

// The limit of twenty.

fn raise_many(env: &Env, name: &str, n: usize, to: &str) -> Vec<String> {
    (0..n)
        .map(|i| {
            let out = env.escalate_run(name, &format!("{to}-{i}"), to, &[]);
            assert_eq!(out.code, 0, "{i}: {}", out.all());
            out.json()["id"].as_str().unwrap().to_string()
        })
        .collect()
}

fn refused_at_limit(env: &Env, name: &str, to: &str) {
    let all0 = env.snapshot_all();
    let out = env.escalate_run(name, "one-more", to, &[]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(out.error().contains("twenty"), "{}", out.error());
    assert_eq!(env.snapshot_all(), all0);
}

#[test]
fn twenty_open_questions_to_people_refuse_the_next() {
    let env = Env::new();
    chain(&env);
    env.init("busy", &[]);
    raise_many(&env, "busy", 15, "person:owner");
    let five = raise_many(&env, "busy", 5, "person:someone-else");
    refused_at_limit(&env, "busy", "person:owner");
    refused_at_limit(&env, "busy", "person:someone-else");
    assert_eq!(
        env.escalate_run("busy", "s", "session:manager", &[]).code,
        0
    );
    assert_eq!(env.escalate_run("busy", "a", "anchor:boss", &[]).code, 0);
    raise_many(&env, "other-lead", 20, "person:owner");

    // A withdrawal frees one place.
    let q = Q {
        session: "busy".to_string(),
        id: five[0].clone(),
        session_id: String::new(),
        header: String::new(),
    };
    assert_eq!(env.withdraw(&q, "busy", "dropped").code, 0);
    assert_eq!(
        env.escalate_run("busy", "next", "person:owner", &[]).code,
        0
    );
    refused_at_limit(&env, "busy", "person:owner");
}

#[test]
fn an_answer_or_a_settle_frees_a_place_and_an_uncounted_entry_doesnt() {
    let env = Env::new();
    chain(&env);
    let q1 = env.raise("worker", "q1", "person:owner", &["label=cache"]);
    let owner = env.person(&q1, "other-lead");
    let a = env.raise("worker", "a", "person:owner", &["label=cache"]);
    let b = env.raise("worker", "b", "person:owner", &["label=cache"]);
    raise_many(&env, "worker", 18, "person:owner");
    refused_at_limit(&env, "worker", "person:owner");

    // A stored session answer that doesn't count frees nothing.
    let donor = env.raise("manager", "d", "session:director", &[]);
    let id = env.say_ok(&donor, "director", "x");
    let w = env
        .escalate_run("other-lead", "y", "session:other-lead", &[])
        .json();
    let mut body = env.stored("director", &donor, &id);
    body["session"] = w["decision"]["session"].clone();
    body["id"] = json!("000000000060");
    body["target"] = json!({"name": "worker", "id": a.session_id, "decision": a.id});
    body["subject"]["session"] = json!("worker");
    body["subject"]["id"] = json!(a.id);
    env.plant("other-lead", &a, "000000000060", &body);
    assert_eq!(env.show("other-lead", "000000000060")["counted"], false);
    refused_at_limit(&env, "worker", "person:owner");

    env.person(&a, "other-lead");
    assert_eq!(
        env.escalate_run("worker", "x1", "person:owner", &[]).code,
        0
    );
    refused_at_limit(&env, "worker", "person:owner");
    let out = env.settle(&b, "worker", &["--basis", &format!("other-lead:{owner}")]);
    assert_eq!(out.code, 0, "{}", out.all());
    assert_eq!(
        env.escalate_run("worker", "x2", "person:owner", &[]).code,
        0
    );
    refused_at_limit(&env, "worker", "person:owner");
}

#[test]
fn the_count_follows_the_addressee_the_raiser_and_its_run() {
    let env = Env::new();
    chain(&env);
    // Passed up to a person: counts toward `worker`, not `manager`.
    let up = env.raise("worker", "up", "session:manager", &[]);
    raise_many(&env, "worker", 19, "person:owner");
    assert_eq!(
        env.escalate_run("worker", "twentieth", "person:owner", &[])
            .code,
        0
    );
    refused_at_limit(&env, "worker", "person:owner");
    // Pass `up` to a person: never refused by the limit, and `worker` now
    // holds twenty-one.
    env.readdress(&up, "manager", "person:owner");
    raise_many(&env, "manager", 20, "person:owner");

    // A successor is refused while `worker` holds twenty.
    env.init(
        "worker2",
        &[
            "--continues",
            "worker",
            "--supersede",
            "--dispatched-by",
            "manager",
        ],
    );
    refused_at_limit(&env, "worker2", "person:owner");

    // One raised to a person and passed to a session no longer counts.
    env.init("solo", &["--dispatched-by", "manager"]);
    let ids = raise_many(&env, "solo", 20, "person:owner");
    refused_at_limit(&env, "solo", "person:owner");
    let moved = Q {
        session: "solo".to_string(),
        id: ids[0].clone(),
        session_id: String::new(),
        header: String::new(),
    };
    env.readdress(&moved, "solo", "session:manager");
    assert_eq!(
        env.escalate_run("solo", "after", "person:owner", &[]).code,
        0
    );

    // A reversed escalation doesn't count.
    refused_at_limit(&env, "solo", "person:owner");
    env.ok(&[
        "decisions",
        "reverse",
        "solo",
        &ids[1],
        "--in",
        "manager",
        "--by",
        "session",
    ]);
    assert_eq!(
        env.escalate_run("solo", "after2", "person:owner", &[]).code,
        0
    );

    // A new session under a removed session's name starts at zero.
    std::fs::remove_dir_all(env.sessions().join("solo")).unwrap();
    env.init("solo", &["--dispatched-by", "manager"]);
    assert_eq!(
        env.escalate_run("solo", "fresh", "person:owner", &[]).code,
        0
    );
}
