//! The cost bound on questions: on a generated store of 1,000 sessions in
//! dispatcher chains four deep, holding 200 open questions (some waiting on
//! `person:owner`, some on a session, some passed up once or twice) beside
//! questions already answered, `koto decisions open --person`, `koto
//! decisions open --at <top coordinator>` and `koto next` on a session
//! holding answered, uncited questions each finish within five seconds and
//! return the count the generator intended. So do the terminal tick of a
//! session holding an open question, which keeps it, and the refused
//! `koto session cleanup` of it. The terminal tick of a lone session with
//! no question still removes it.
//!
//! The three reads then run again with the same store as the cloud
//! backend's local copy, against the in-process endpoint in
//! `support/fake_s3.rs`. The endpoint must record no request from the two
//! open reads. `koto next` under the cloud backend syncs the session it
//! ticks, question or not, so for it the test asserts that the requests
//! are exactly those the tick of a same-shaped session holding no question
//! makes: the questions read itself adds none.
//!
//! The store is generated through the binary, so every file is in the
//! format koto writes. The bounds hold for a release build, which CI runs
//! in its own job (`cargo test --release --test questions_cost`). A debug
//! build runs the same code on a store a tenth the size and checks only
//! the counts, the way `tests/retention_cost.rs` does.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fake_s3::FakeS3;
use serde_json::Value;

const RELEASE: bool = !cfg!(debug_assertions);

/// The bound on every timed operation, on a release build.
const BOUND: Duration = Duration::from_secs(5);

/// Ten sessions each: 100 hierarchies make the 1,000-session store.
const HIERARCHIES: usize = if RELEASE { 100 } else { 10 };

/// Threads generating hierarchies at once.
const THREADS: usize = 8;

/// Answered, uncited questions the `koto next` target holds.
const ANSWERED_AT_TARGET: usize = 3;

/// The `koto next` target: the session holding them.
const ANSWERED_HOLDER: &str = "w0-2";

/// A session of the same shape as [`ANSWERED_HOLDER`] (dispatched by a
/// middle session, dispatching one below it) that holds no question and is
/// addressed by none.
const PLAIN_PEER: &str = "w4-0";

const BUCKET: &str = "koto-questions-cost";

/// Variables koto reads an identity, a run mode, an authority, a store or
/// a session backend from.
const CLEARED_ENV: [&str; 11] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_HOME",
    "KOTO_SESSIONS_BASE",
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_ID",
    "KOTO_SESSION_NAME",
    "KOTO_RETENTION_KEEP",
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

/// A scratch home holding the store. koto's root is `home/.koto`, so the
/// local backend and the cloud backend's local copy are the same
/// `home/.koto/sessions`.
struct Store {
    tmp: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
    took: Duration,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

/// What the generator intends the reads to return.
#[derive(Default)]
struct Intended {
    open: usize,
    owner: usize,
    at_top0: usize,
    answered: usize,
    passed_once: usize,
    passed_twice: usize,
}

impl Store {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let ws = root.join("ws");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("work.md"), TEMPLATE).unwrap();
        Store { tmp, home, ws }
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(&self.ws)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .args(args);
        cmd
    }

    fn output(cmd: &mut std::process::Command) -> Out {
        let started = Instant::now();
        let o = cmd.output().unwrap();
        let took = started.elapsed();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            took,
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        Self::output(&mut self.command(args))
    }

    /// Run under the cloud backend, against `endpoint`.
    fn run_cloud(&self, endpoint: &str, args: &[&str]) -> Out {
        let mut cmd = self.command(args);
        cmd.env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        Self::output(&mut cmd)
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

    fn init(&self, name: &str, dispatched_by: Option<&str>) {
        let mut args = vec!["init", name, "--template", "work.md"];
        if let Some(parent) = dispatched_by {
            args.extend_from_slice(&["--dispatched-by", parent]);
        }
        self.ok(&args);
    }

    /// Record a question in `name`; returns its id.
    fn escalate(&self, name: &str, question: &str, recipient: &str) -> String {
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let out = self.ok(&[
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
        ]);
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn readdress(&self, raiser: &str, id: &str, in_session: &str, to: &str) {
        self.ok(&[
            "decisions",
            "readdress",
            raiser,
            id,
            "--in",
            in_session,
            "--to",
            to,
            "--by",
            "session",
        ]);
    }

    fn answer(&self, raiser: &str, id: &str, in_session: &str) {
        self.ok(&[
            "decisions",
            "answer",
            raiser,
            id,
            "--in",
            in_session,
            "--text",
            "settled",
            "--by",
            "session",
        ]);
    }

    /// One hierarchy of ten sessions: a top coordinator `t<h>` dispatching
    /// `m<h>-<c>`, each dispatching `w<h>-<c>`, each dispatching `l<h>-<c>`.
    /// Two open questions and at least one answered one.
    fn hierarchy(&self, h: usize) -> Intended {
        let top = format!("t{h}");
        self.init(&top, None);
        for c in 0..3 {
            let (m, w, l) = (
                format!("m{h}-{c}"),
                format!("w{h}-{c}"),
                format!("l{h}-{c}"),
            );
            self.init(&m, Some(&top));
            self.init(&w, Some(&m));
            self.init(&l, Some(&w));
        }
        let mut got = Intended::default();

        // Open at the bottom of the first chain: for the owner on even
        // hierarchies, for the session above on odd ones.
        let a = format!("l{h}-0");
        if h.is_multiple_of(2) {
            self.escalate(&a, "qa", "person:owner");
            got.owner += 1;
        } else {
            self.escalate(&a, "qa", &format!("session:w{h}-0"));
        }

        // Open at the bottom of the second chain, for the session above,
        // then passed up once to the next session, or twice and on to the
        // owner.
        let (l1, w1, m1) = (format!("l{h}-1"), format!("w{h}-1"), format!("m{h}-1"));
        let b = self.escalate(&l1, "qb", &format!("session:{w1}"));
        match h % 3 {
            1 => {
                self.readdress(&l1, &b, &w1, &format!("session:{m1}"));
                got.passed_once += 1;
            }
            2 => {
                self.readdress(&l1, &b, &w1, &format!("session:{m1}"));
                self.readdress(&l1, &b, &m1, "person:owner");
                got.owner += 1;
                got.passed_twice += 1;
            }
            _ => {}
        }
        got.open += 2;
        if h == 0 {
            got.at_top0 = 2;
        }

        // Answered and uncited in the third chain's middle.
        let (w2, m2) = (format!("w{h}-2"), format!("m{h}-2"));
        let answered = if h == 0 { ANSWERED_AT_TARGET } else { 1 };
        for i in 0..answered {
            let id = self.escalate(&w2, &format!("qc{i}"), "person:owner");
            self.answer(&w2, &id, &m2);
        }
        got.answered += answered;
        got
    }
}

fn build(store: &Store) -> Intended {
    let mut parts: Vec<Intended> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                s.spawn(move || {
                    (t..HIERARCHIES)
                        .step_by(THREADS)
                        .map(|h| store.hierarchy(h))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            parts.extend(handle.join().unwrap());
        }
    });
    let mut total = Intended::default();
    for p in parts {
        total.open += p.open;
        total.owner += p.owner;
        total.at_top0 += p.at_top0;
        total.answered += p.answered;
        total.passed_once += p.passed_once;
        total.passed_twice += p.passed_twice;
    }
    total
}

fn session_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        // koto's own bookkeeping (`.finished`) is no session.
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

fn session_count(dir: &Path) -> usize {
    session_names(dir).len()
}

fn count(v: &Value, key: &str) -> usize {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("no {key} array in {v}"))
        .len()
}

/// Print an operation's time, and hold it to the bound on a release build.
fn timed(what: &str, took: Duration) {
    println!("questions cost: {what}: {took:?}");
    if RELEASE {
        assert!(took < BOUND, "{what} took {took:?}");
    }
}

/// The two open reads, under whichever backend `run` uses.
fn open_reads(label: &str, intended: &Intended, run: &dyn Fn(&[&str]) -> Out) {
    let out = run(&["decisions", "open", "--person", "owner"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(count(&out.json(), "questions"), intended.owner);
    timed(&format!("{label} decisions open --person owner"), out.took);

    let out = run(&["decisions", "open", "--at", "t0"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(count(&out.json(), "questions"), intended.at_top0);
    timed(&format!("{label} decisions open --at t0"), out.took);
}

/// `koto next` on the session holding answered, uncited questions.
fn next_read(label: &str, run: &dyn Fn(&[&str]) -> Out) {
    let out = run(&["next", ANSWERED_HOLDER, "--no-cleanup"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(count(&v, "questions"), ANSWERED_AT_TARGET);
    assert!(v["questions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|q| q["state"] == "answered"));
    timed(
        &format!("{label} next {ANSWERED_HOLDER} (answered, uncited)"),
        out.took,
    );
}

/// The requests `s3` recorded, sorted, with `session`'s name replaced so
/// the requests two sessions' ticks make can be compared.
fn requests_of(s3: &FakeS3, session: &str) -> Vec<String> {
    let mut out: Vec<String> = s3
        .requests()
        .iter()
        .map(|r| {
            let list = r.list_prefix.as_deref().unwrap_or("");
            format!(
                "{} {} {}",
                r.method,
                r.key.replace(session, "<session>"),
                list.replace(session, "<session>")
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_questions_reads_answer_within_their_bounds_on_a_large_store() {
    let store = Store::new();

    let started = Instant::now();
    let intended = build(&store);
    let generation = started.elapsed();
    let sessions = session_count(&store.sessions());
    println!(
        "questions cost: generated {sessions} sessions, {} open questions ({} for the owner, \
         {} passed up once, {} twice), {} answered, in {generation:?}",
        intended.open,
        intended.owner,
        intended.passed_once,
        intended.passed_twice,
        intended.answered,
    );
    assert_eq!(sessions, HIERARCHIES * 10);
    assert_eq!(intended.open, HIERARCHIES * 2);
    assert!(intended.passed_once > 0 && intended.passed_twice > 0);

    // A first run of a freshly built binary can pay for the OS checking
    // it, which isn't the operation's cost.
    store.ok(&["version"]);

    // Every open question, read whole, once.
    let all: usize = (0..HIERARCHIES)
        .map(|h| {
            let v = store
                .ok(&["decisions", "open", "--at", &format!("t{h}")])
                .json();
            count(&v, "questions")
        })
        .sum();
    assert_eq!(all, intended.open);

    open_reads("local", &intended, &|args| store.run(args));
    next_read("local", &|args| store.run(args));

    // The same reads with the store as the cloud backend's local copy. The
    // open reads read only the local store, so the endpoint sees nothing
    // from them.
    let s3 = FakeS3::start(BUCKET);
    let endpoint = s3.endpoint();
    let cloud = |args: &[&str]| store.run_cloud(&endpoint, args);
    s3.clear_requests();
    open_reads("cloud", &intended, &cloud);
    let requests = requests_of(&s3, ANSWERED_HOLDER);
    assert!(
        requests.is_empty(),
        "an open read reached the bucket: {requests:?}"
    );

    // `koto next` under the cloud backend syncs the session it ticks
    // whether or not it holds a question, so it is never silent. The
    // questions read adds nothing to that: the tick of the session holding
    // answered questions makes exactly the requests the tick of a session
    // of the same shape holding none makes. The first tick of each pushes
    // a session the bucket hasn't seen; the comparison is of the second.
    for name in [ANSWERED_HOLDER, PLAIN_PEER] {
        let out = cloud(&["next", name, "--no-cleanup"]);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        println!(
            "questions cost: cloud first next {name} (pushes the session): {:?}",
            out.took
        );
    }
    s3.clear_requests();
    next_read("cloud", &cloud);
    let with_questions = requests_of(&s3, ANSWERED_HOLDER);
    s3.clear_requests();
    let out = cloud(&["next", PLAIN_PEER, "--no-cleanup"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(out.json().get("questions").is_none());
    let without = requests_of(&s3, PLAIN_PEER);
    println!(
        "questions cost: cloud next requests: {} holding questions {with_questions:?}, \
         {} holding none",
        with_questions.len(),
        without.len()
    );
    assert_eq!(
        with_questions, without,
        "the questions read reached the bucket"
    );
    drop(s3);

    // The terminal tick of a session holding an open question keeps it.
    let holder = "l1-0";
    let out = store.ok(&["next", holder, "--with-data", r#"{"go":"go"}"#]);
    let v = out.json();
    assert_eq!(v["state"], "done", "{v}");
    assert_eq!(v["retention"]["retained"], true, "{v}");
    assert_eq!(v["retention"]["reason"], "open_question", "{v}");
    assert_eq!(count(&v["retention"], "open_questions"), 1);
    assert!(store.sessions().join(holder).is_dir());
    timed(
        "terminal tick of a session holding an open question",
        out.took,
    );

    // Its explicit removal is refused.
    let out = store.run(&["session", "cleanup", holder]);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let v: Value = serde_json::from_str(out.stdout.trim())
        .or_else(|_| serde_json::from_str(out.stderr.trim()))
        .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", out.stdout, out.stderr));
    assert_eq!(v["error"]["code"], "holds_question", "{v}");
    assert!(store.sessions().join(holder).is_dir());
    timed("refused session cleanup of it", out.took);

    // A lone session with no question pays nothing new: its terminal tick
    // removes it.
    store.init("lone", None);
    let out = store.ok(&["next", "lone", "--with-data", r#"{"go":"go"}"#]);
    let v = out.json();
    assert_eq!(v["state"], "done", "{v}");
    assert_eq!(v["retention"]["retained"], false, "{v}");
    assert!(!store.sessions().join("lone").exists());
    timed("terminal tick of a lone session with no question", out.took);
    let left = session_names(&store.sessions());
    assert_eq!(left.len(), HIERARCHIES * 10, "{left:?}");

    store.tmp.close().unwrap();
}
