//! Integration tests for passing a question up under the local backend:
//! `koto decisions readdress`, the addressee history on the open reads,
//! `koto decisions open --at` and how a pass-up meets an answer and
//! `koto next`.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use serde_json::{json, Value};

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
    _records: prompt_fixture::Records,
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
        Env {
            _records: prompt_fixture::Records::new(),
            _tmp: tmp,
            root,
        }
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
        self.init_env(&[], name, extra);
    }

    /// `koto init` with `envs` set, so a `KOTO_ANCHOR` there holds the session.
    fn init_env(&self, envs: &[(&str, &str)], name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        let out = self.run_env(envs, &args);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
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

    /// Raise `question` in `name` to `recipient` with the content the
    /// fixture `lines` shows at position 1 stored for it; returns the
    /// escalation and the prompt header the content was given.
    fn escalate_shown(
        &self,
        name: &str,
        question: &str,
        recipient: &str,
        lines: &[Value],
    ) -> (Rec, String) {
        let (shown, labels) = prompt_fixture::shown(lines, 1);
        let content = prompt_fixture::content(&shown, &labels);
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let packet = format!("packet={}", prompt_fixture::packet(&content));
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
                "--subject",
                &packet,
                "--by",
                "session",
            ])
            .json();
        let rec = Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        };
        let file = self.root.join(format!("content-{}-{}", name, rec.id));
        std::fs::write(&file, &content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                name,
                &rec.id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        let header = written["prompt_header"].as_str().unwrap().to_string();
        (rec, header)
    }

    /// Record `person:owner`'s answer to `q` in `in_session` from the
    /// fixture `lines`, installed for this thread's harness session with
    /// `header` at position 1.
    fn person_answer(&self, q: &Rec, in_session: &str, header: &str, lines: &[Value]) -> Out {
        let harness = prompt_fixture::harness();
        prompt_fixture::install(
            &harness,
            &prompt_fixture::render(lines, &harness, &[header]),
        );
        let pointer = format!("{harness}/{}/1", prompt_fixture::CALL);
        self.run_env(
            &[("CLAUDE_CODE_SESSION_ID", harness.as_str())],
            &[
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
            ],
        )
    }

    fn readdress_run(&self, q: &Rec, in_session: &str, to: &str) -> Out {
        self.readdress_by(q, in_session, to, "session")
    }

    fn readdress_by(&self, q: &Rec, in_session: &str, to: &str, by: &str) -> Out {
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
            by,
        ])
    }

    /// Pass `q` up from `in_session` to `to`; returns the printed entry.
    fn readdress(&self, q: &Rec, in_session: &str, to: &str) -> Value {
        let out = self.readdress_run(q, in_session, to);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        out.json()
    }

    fn answer(&self, q: &Rec, in_session: &str, text: &str) -> String {
        let out = self.run(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--text",
            text,
            "--by",
            "session",
        ]);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn open(&self, person: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--person", person]).json();
        out["questions"].as_array().unwrap().clone()
    }

    fn at(&self, position: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--at", position]).json();
        assert_eq!(out["at"], position);
        out["questions"].as_array().unwrap().clone()
    }

    /// `koto next <name> --no-cleanup`, so a terminal session stays.
    fn next(&self, name: &str) -> Value {
        self.ok(&["next", name, "--no-cleanup"]).json()
    }

    /// Every file under the sessions base with its bytes, lock files aside.
    fn snapshot_all(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_dir(&self.sessions())
    }

    fn snapshot(&self, name: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_dir(&self.sessions().join(name))
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

    /// Open an anchor for `harness`, with `extra` flags, and return its name.
    fn open_anchor(&self, harness: &str, extra: &[&str]) -> String {
        let mut args = vec![
            "session",
            "anchor",
            "open",
            "--harness-session",
            harness,
            "--source",
            "startup",
        ];
        args.extend_from_slice(extra);
        let opened = self.run(&args);
        assert_eq!(opened.code, 0, "{}", opened.stderr);
        opened
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("anchor="))
            .unwrap()
            .to_string()
    }
}

fn snapshot_dir(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
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
    walk(dir, dir, &mut out);
    out
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

fn find<'a>(qs: &'a [Value], question: &str) -> &'a Value {
    let hits: Vec<&Value> = qs.iter().filter(|q| q["question"] == question).collect();
    assert_eq!(
        hits.len(),
        1,
        "{question} listed {} times: {qs:?}",
        hits.len()
    );
    hits[0]
}

fn session(name: &str) -> Value {
    json!({"typed": true, "kind": "session", "name": name})
}

fn anchor(name: &str) -> Value {
    json!({"typed": true, "kind": "anchor", "name": name})
}

fn person(handle: &str) -> Value {
    json!({"typed": true, "kind": "person", "handle": handle})
}

/// `director` above `manager` above `worker`, and an unrelated pair.
fn chain(env: &Env) {
    env.init("director", &[]);
    env.init("manager", &["--dispatched-by", "director"]);
    env.init("worker", &["--dispatched-by", "manager"]);
    env.init("other-lead", &[]);
    env.init("other-worker", &["--dispatched-by", "other-lead"]);
}

#[test]
fn the_walkthrough_passes_a_question_up_twice_and_answers_it() {
    let env = Env::new();
    chain(&env);
    let lines = prompt_fixture::lines("accepted");
    let (q1, header) = env.escalate_shown("worker", "q1", "session:manager", &lines);
    let q2 = env.escalate("worker", "q2", "session:manager");
    env.escalate("other-worker", "q3", "session:other-lead");

    // Before any pass-up.
    let at_manager = env.at("manager");
    assert_eq!(names(&at_manager), vec!["q1", "q2"]);
    for q in &at_manager {
        assert_eq!(q["addressee"], session("manager"));
        assert_eq!(q["addressees"], json!([session("manager")]));
        assert_eq!(q["recipient"], session("manager"));
        assert_eq!(q["relation"], "addressee");
    }
    let at_director = env.at("director");
    assert_eq!(names(&at_director), vec!["q1", "q2"]);
    assert!(at_director.iter().all(|q| q["relation"] == "below"));
    assert_eq!(names(&env.at("worker")), vec!["q1", "q2"]);
    assert!(env.at("worker").iter().all(|q| q["relation"] == "raised"));
    assert!(env.open("owner").is_empty());
    let next_before = env.next("worker");

    // manager passes q1 to director.
    let (w0, d0) = (env.snapshot("worker"), env.snapshot("director"));
    let show0 = env.ok(&["decisions", "show", "worker", &q1.id]).stdout;
    let out = env.readdress(&q1, "manager", "session:director");
    let d = &out["decision"];
    assert_eq!(d["kind"], "readdress");
    assert_eq!(d["outcome"], "passed");
    assert_eq!(d["target"]["id"], q1.session_id.as_str());
    assert_eq!(d["target"]["decision"], q1.id.as_str());
    assert_eq!(out["question"], q1.identity().as_str());
    assert_eq!(out["addressee"], session("director"));
    assert_eq!(
        out["addressees"],
        json!([session("manager"), session("director")])
    );
    assert_eq!(env.aimed_files("manager", &q1.session_id).len(), 1);
    assert_eq!(env.snapshot("worker"), w0);
    assert_eq!(env.snapshot("director"), d0);
    assert_eq!(
        env.ok(&["decisions", "show", "worker", &q1.id]).stdout,
        show0
    );

    for position in ["worker", "manager", "director"] {
        let qs = env.at(position);
        let a = find(&qs, "q1");
        assert_eq!(a["addressee"], session("director"), "at {position}");
        assert_eq!(
            a["addressees"],
            json!([session("manager"), session("director")])
        );
        assert_eq!(a["recipient"], session("manager"));
        let b = find(&qs, "q2");
        assert_eq!(b["addressee"], session("manager"));
        assert_eq!(b["addressees"].as_array().unwrap().len(), 1);
    }
    assert_eq!(find(&env.at("director"), "q1")["relation"], "addressee");
    assert_eq!(find(&env.at("manager"), "q1")["relation"], "below");
    assert_eq!(find(&env.at("worker"), "q1")["relation"], "raised");

    // manager is no longer the addressee.
    let (all0, manager_files) = (
        env.snapshot_all(),
        env.aimed_files("manager", &q1.session_id),
    );
    let out = env.readdress_run(&q1, "manager", "session:director");
    assert_eq!(out.code, 2, "{}", out.stdout);
    let out = env.readdress_run(&q1, "manager", "person:owner");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["field"], "in");
    let out = env.readdress_run(&q1, "other-lead", "person:owner");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(env.aimed_files("manager", &q1.session_id), manager_files);
    assert_eq!(env.snapshot_all(), all0);

    // A pass-up alone reports nothing on the asker's tick.
    assert_eq!(env.next("worker"), next_before);

    // director passes it to a person.
    assert!(env.open("owner").is_empty());
    env.readdress(&q1, "director", "person:owner");
    let owner = env.open("owner");
    assert_eq!(names(&owner), vec!["q1"]);
    assert_eq!(owner[0]["addressee"], person("owner"));
    assert_eq!(
        owner[0]["addressees"],
        json!([session("manager"), session("director"), person("owner")])
    );
    for position in ["manager", "director"] {
        let qs = env.at(position);
        let a = find(&qs, "q1");
        assert_eq!(a["addressee"], person("owner"));
        assert_eq!(a["relation"], "below");
    }
    assert_eq!(names(&env.at("other-lead")), vec!["q3"]);
    assert!(env.next("worker").get("questions").is_none());

    // The owner's answer closes it everywhere.
    let out = env.person_answer(&q1, "director", &header, &lines);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(env.open("owner").is_empty());
    for position in ["worker", "manager", "director"] {
        assert_eq!(names(&env.at(position)), vec!["q2"], "at {position}");
    }
    let reported = questions(&env.next("worker"));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0]["identity"], q1.identity().as_str());
    assert_eq!(reported[0]["state"], "answered");
    assert_eq!(
        reported[0]["answer"]["text"],
        prompt_fixture::answer_of(&lines, 1).as_str()
    );
    assert_eq!(reported[0]["answer"]["session"], "director");

    // An answered question isn't passed up.
    let w0 = env.snapshot("worker");
    let out = env.readdress_run(&q1, "worker", "person:owner");
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert!(
        out.json()["error"].as_str().unwrap().contains("answered"),
        "{}",
        out.stdout
    );
    assert_eq!(env.snapshot("worker"), w0);
    let _ = q2;
}

#[test]
fn a_pass_up_is_refused_and_writes_nothing_when_its_target_is_wrong() {
    let env = Env::new();
    chain(&env);
    let q = env.escalate("worker", "q1", "session:manager");
    let untyped = env.escalate("worker", "q2", "manager");
    let judgment = env
        .ok(&[
            "decisions",
            "record",
            "worker",
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=use postgres",
            "--by",
            "session",
        ])
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reversed = env.escalate("worker", "q3", "session:manager");
    env.ok(&[
        "decisions",
        "reverse",
        "worker",
        &reversed.id,
        "--in",
        "manager",
        "--by",
        "session",
    ]);
    let answered = env.escalate("worker", "q4", "session:manager");
    env.answer(&answered, "manager", "yes");
    // A decision id that worker doesn't hold: other-worker records until
    // its id is past every one of worker's.
    let worker_ids = [&q.id, &untyped.id, &judgment, &reversed.id, &answered.id];
    let elsewhere = loop {
        let r = env.escalate("other-worker", "q5", "session:other-lead");
        if !worker_ids.contains(&&r.id) {
            break r;
        }
    };

    let all0 = env.snapshot_all();
    let wrong = vec![
        Rec {
            id: "000000009999".to_string(),
            ..q.clone()
        },
        Rec {
            id: judgment,
            ..q.clone()
        },
        untyped,
        reversed,
        answered,
        // An id that exists only in another session.
        Rec {
            id: elsewhere.id.clone(),
            ..q.clone()
        },
    ];
    for target in &wrong {
        let out = env.readdress_run(target, "manager", "session:director");
        assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
        assert!(out.json()["code"].is_string(), "{}", out.stdout);
    }
    for to in [
        "director",
        "session:not a name",
        "person:",
        "session:manager",
    ] {
        let out = env.readdress_run(&q, "manager", to);
        assert_eq!(out.code, 2, "{to}: {}", out.stdout);
        assert_eq!(out.json()["field"], "to", "{to}");
    }
    let out = env.readdress_by(&q, "manager", "session:director", "person:owner");
    assert_eq!(out.code, 2, "{}", out.stdout);
    let out = env.run(&[
        "decisions",
        "record",
        "manager",
        "--kind",
        "readdress",
        "--outcome",
        "passed",
        "--subject",
        "session=worker",
        "--subject",
        &format!("id={}", q.id),
        "--subject",
        "recipient=session:director",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["field"], "kind");
    assert_eq!(env.snapshot_all(), all0);
}

#[test]
fn a_pass_up_lists_reads_back_and_is_not_reversed() {
    let env = Env::new();
    chain(&env);
    let q = env.escalate("worker", "q1", "session:manager");
    let entry = env.readdress(&q, "manager", "session:director");
    let id = entry["id"].as_str().unwrap();

    let listing = env
        .ok(&["decisions", "list", "manager", "--format", "v1"])
        .json();
    let listed = listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("not listed: {listing}"))
        .clone();
    assert_eq!(listed["decision"]["kind"], "readdress");
    assert_eq!(listed["recipient"], session("director"));

    let out = env.run(&[
        "decisions",
        "reverse",
        "manager",
        id,
        "--in",
        "director",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["reason"], "reverses_readdress");
    assert_eq!(
        find(&env.at("director"), "q1")["addressee"],
        session("director")
    );
}

#[test]
fn the_raiser_passes_its_own_question_between_people() {
    let env = Env::new();
    env.init("a", &[]);
    let q = env.escalate("a", "q1", "person:someone-else");
    let other = env.escalate("a", "q2", "session:coord");
    assert_eq!(names(&env.open("someone-else")), vec!["q1"]);

    let a0 = env.aimed_files("a", &q.session_id);
    assert!(a0.is_empty());
    env.readdress(&q, "a", "person:owner");
    assert_eq!(env.aimed_files("a", &q.session_id).len(), 1);
    assert!(env.open("someone-else").is_empty());
    let owner = env.open("owner");
    assert_eq!(names(&owner), vec!["q1"]);
    assert_eq!(
        owner[0]["addressees"],
        json!([person("someone-else"), person("owner")])
    );
    assert_eq!(owner[0]["recipient"], person("someone-else"));

    // q2 is untouched.
    let qs = env.at("a");
    let b = find(&qs, "q2");
    assert_eq!(b["addressees"], json!([session("coord")]));
    assert_eq!(b["relation"], "raised");
    let _ = other;
}

#[test]
fn an_unlinked_raiser_is_found_by_its_addressee_only() {
    let env = Env::new();
    chain(&env);
    env.init("loose", &[]);
    let q = env.escalate("loose", "q1", "session:director");

    let at = env.at("director");
    assert_eq!(find(&at, "q1")["relation"], "addressee");
    assert!(env.at("manager").is_empty());

    env.readdress(&q, "director", "person:owner");
    assert!(env.at("director").is_empty());
    assert_eq!(names(&env.open("owner")), vec!["q1"]);
    assert_eq!(find(&env.at("loose"), "q1")["relation"], "raised");
}

#[test]
fn a_coordinators_successor_holds_its_position() {
    let env = Env::new();
    chain(&env);
    env.init("loose", &[]);
    env.escalate("worker", "q1", "session:director");
    env.escalate("loose", "q2", "session:manager");
    env.init("succ", &["--continues", "manager", "--supersede"]);

    let qs = env.at("succ");
    assert_eq!(names(&qs), vec!["q1", "q2"]);
    assert_eq!(find(&qs, "q1")["relation"], "below");
    assert_eq!(find(&qs, "q2")["relation"], "addressee");
}

#[test]
fn an_anchor_is_a_position_like_a_session() {
    let env = Env::new();
    let top = env.open_anchor("3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60731", &[]);
    let lead = env.open_anchor(
        "4a2f3b0d-1c8e-4f66-b2c3-d4e5f6071842",
        &[&format!("--dispatched-by={top}")],
    );
    env.init("w", &["--dispatched-by", &lead]);
    let held = [("KOTO_ANCHOR", lead.as_str())];
    env.init_env(&held, "h", &[]);
    env.init("x", &[]);
    env.escalate("w", "q1", "session:x");
    env.escalate("h", "q2", "session:x");
    env.escalate("x", "q3", &format!("anchor:{lead}"));

    let at_lead = env.at(&lead);
    assert_eq!(names(&at_lead), vec!["q1", "q2", "q3"]);
    assert_eq!(find(&at_lead, "q1")["relation"], "below");
    assert_eq!(find(&at_lead, "q2")["relation"], "below");
    assert_eq!(find(&at_lead, "q3")["relation"], "addressee");
    assert_eq!(find(&at_lead, "q3")["addressee"], anchor(&lead));
    let at_top = env.at(&top);
    assert_eq!(names(&at_top), vec!["q1", "q2"]);
}

#[test]
fn a_recreated_raiser_doesnt_inherit_an_earlier_pass_up() {
    let env = Env::new();
    chain(&env);
    let old = env.escalate("worker", "q1", "session:manager");
    env.readdress(&old, "manager", "session:director");
    env.readdress(&old, "worker", "person:owner");

    std::fs::remove_dir_all(env.sessions().join("worker")).unwrap();
    env.init("worker", &["--dispatched-by", "manager"]);
    let fresh = env.escalate("worker", "q1", "session:manager");
    assert_ne!(fresh.session_id, old.session_id);

    for qs in [env.at("worker"), env.at("manager"), env.at("director")] {
        let q = find(&qs, "q1");
        assert_eq!(q["identity"], fresh.identity().as_str());
        assert_eq!(q["addressees"], json!([session("manager")]));
    }
    assert!(env.open("owner").is_empty());
}

#[test]
fn open_takes_exactly_one_of_person_and_at_and_writes_nothing() {
    let env = Env::new();
    chain(&env);
    env.escalate("worker", "q1", "session:manager");
    let all0 = env.snapshot_all();

    let out = env.run(&["decisions", "open"]);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let out = env.run(&["decisions", "open", "--person", "owner", "--at", "manager"]);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let out = env.run(&["decisions", "open", "--at", "nobody"]);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);

    env.at("director");
    env.open("owner");
    assert_eq!(env.snapshot_all(), all0);
}

#[test]
fn a_pass_up_stored_with_an_earlier_time_than_the_one_it_follows_still_applies() {
    let env = Env::new();
    chain(&env);
    let q = env.escalate("worker", "q1", "session:manager");
    let first = env.readdress(&q, "manager", "session:director");
    let second = env.readdress(&q, "director", "person:owner");
    assert_eq!(second["addressee"], person("owner"));

    // The director's clock was behind: its entry reads earlier than the
    // manager's, though it could only have been written after it.
    let path = env
        .sessions()
        .join("director/ctx/koto/aimed")
        .join(&q.session_id)
        .join(second["id"].as_str().unwrap());
    let mut body: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let earlier = first["decision"]["time"]
        .as_str()
        .unwrap()
        .replace("2026", "2025");
    body["time"] = json!(earlier);
    std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();

    let qs = env.open("owner");
    assert_eq!(names(&qs), vec!["q1"]);
    assert_eq!(
        qs[0]["addressees"],
        json!([session("manager"), session("director"), person("owner")])
    );
    assert!(env.at("director").iter().all(|e| e["relation"] == "below"));
}

#[test]
fn a_held_session_with_its_own_dispatcher_is_below_both() {
    let env = Env::new();
    let lead = env.open_anchor("5b3a4c1e-2d9f-4a77-83d4-e5f607182953", &[]);
    env.init("boss", &[]);
    env.init("x", &[]);
    let held = [("KOTO_ANCHOR", lead.as_str())];
    env.init_env(&held, "hd", &["--dispatched-by", "boss"]);
    env.escalate("hd", "q1", "session:x");

    for position in [lead.as_str(), "boss"] {
        let qs = env.at(position);
        assert_eq!(names(&qs), vec!["q1"], "{position}");
        assert_eq!(qs[0]["relation"], "below", "{position}");
    }
}

#[test]
fn the_successor_of_the_addressee_passes_the_question_up() {
    let env = Env::new();
    chain(&env);
    let q = env.escalate("worker", "q1", "session:manager");
    env.init("succ", &["--continues", "manager", "--supersede"]);
    assert_eq!(find(&env.at("succ"), "q1")["relation"], "addressee");

    let out = env.readdress_run(&q, "manager", "session:director");
    assert_eq!(out.code, 2, "{}", out.stdout);
    let entry = env.readdress(&q, "succ", "session:director");
    assert_eq!(
        entry["addressees"],
        json!([session("manager"), session("director")])
    );
    assert_eq!(find(&env.at("director"), "q1")["relation"], "addressee");
    assert_eq!(find(&env.at("succ"), "q1")["relation"], "below");
}

#[test]
fn the_successor_of_the_raiser_passes_its_question_up() {
    let env = Env::new();
    chain(&env);
    let q = env.escalate("worker", "q1", "session:manager");
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

    let entry = env.readdress(&q, "worker2", "person:owner");
    assert_eq!(
        entry["addressees"],
        json!([session("manager"), person("owner")])
    );
    let qs = env.open("owner");
    assert_eq!(names(&qs), vec!["q1"]);
    assert_eq!(qs[0]["identity"], q.identity().as_str());
}
