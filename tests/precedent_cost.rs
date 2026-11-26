//! What the attempts read costs on a large store.
//!
//! The store holds 1,000 sessions under one top coordinator, in units of
//! ten chained four deep: top, a lead, a manager under the lead and eight
//! workers under the manager. Each worker records an undecided attempt, a
//! question to its manager citing it, and a provisional judgment; each
//! manager records an attempt citing one worker's question and raises a
//! question to the owner naming one worker's provisional judgment, which
//! the owner answers in the lead through a recorded prompt fixture.
//!
//! `koto decisions attempts --at top` and `koto decisions attempts --in` a
//! worker each answer within five seconds on a release build, which CI
//! runs in its own job (`cargo test --release --test precedent_cost`). A
//! debug build runs the same code on a store a tenth the size and checks
//! only the counts, so the default suite stays fast.
//!
//! The first unit is recorded through the binary and the others are
//! copied from it with their names and session ids changed, which keeps
//! the fixture's build time low; the owner's questions and answers are
//! recorded through the binary in every unit. Everything sits in a
//! temporary directory the test removes.
//!
//! A cloud host's attempts read uses its local copy only: run against the
//! in-process endpoint in `tests/support/fake_s3.rs`, it makes no request.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prompt_fixture as fx;
use serde_json::Value;

const RELEASE: bool = !cfg!(debug_assertions);
const BOUND: Duration = Duration::from_secs(5);
const WORKERS: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
/// The workers whose question to their manager stays open.
const OPEN: [char; 2] = ['a', 'b'];

const CLEARED_ENV: [&str; 7] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
    "KOTO_SESSION_BACKEND",
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

struct Store {
    _records: fx::Records,
    _tmp: tempfile::TempDir,
    root: PathBuf,
    calls: std::cell::Cell<u32>,
    /// The fake bucket's endpoint and name, when the store is a cloud
    /// host's local copy.
    cloud: Option<(String, String)>,
}

fn unit(i: usize) -> String {
    format!("u{i:04}")
}

fn lead(i: usize) -> String {
    format!("l{}", unit(i))
}

fn manager(i: usize) -> String {
    format!("m{}", unit(i))
}

fn worker(i: usize, w: char) -> String {
    format!("w{}{w}", unit(i))
}

impl Store {
    fn new(cloud: Option<&fake_s3::FakeS3>) -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "cache", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("ws/work.md"), TEMPLATE).unwrap();
        Store {
            _records: fx::Records::new(),
            _tmp: tmp,
            root,
            calls: std::cell::Cell::new(0),
            cloud: cloud.map(|s3| (s3.endpoint(), s3.bucket().to_string())),
        }
    }

    fn sessions(&self) -> PathBuf {
        match self.cloud {
            Some(_) => self.root.join("home/.koto/sessions"),
            None => self.root.join("sessions"),
        }
    }

    fn run(&self, harness: Option<&str>, args: &[&str]) -> (i32, String, String) {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        if let Some(h) = harness {
            cmd.env("CLAUDE_CODE_SESSION_ID", h);
        }
        cmd.current_dir(self.root.join("ws"))
            .env("HOME", self.root.join("home"))
            .args(args);
        match &self.cloud {
            Some((endpoint, bucket)) => {
                for name in ["KOTO_HOME", "KOTO_SESSIONS_BASE"] {
                    cmd.env_remove(name);
                }
                cmd.env("XDG_CACHE_HOME", self.root.join("cache"))
                    .env("KOTO_SESSION_BACKEND", "cloud")
                    .env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint)
                    .env("KOTO_SESSION_CLOUD_BUCKET", bucket)
                    .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                    .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                    .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                    .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
                    .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0");
            }
            None => {
                cmd.env("KOTO_HOME", self.root.join("home"))
                    .env("KOTO_SESSIONS_BASE", self.sessions());
            }
        }
        let o = cmd.output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_h(None, args)
    }

    fn ok_h(&self, harness: Option<&str>, args: &[&str]) -> Value {
        let (code, out, err) = self.run(harness, args);
        assert_eq!(code, 0, "{args:?}\n{out}\n{err}");
        serde_json::from_str(out.trim()).unwrap_or(Value::Null)
    }

    fn init(&self, name: &str, parent: Option<&str>) {
        let mut args = vec!["init", name, "--template", "work.md"];
        if let Some(p) = parent {
            args.extend_from_slice(&["--dispatched-by", p]);
        }
        self.ok(&args);
    }

    fn record(&self, session: &str, kind: &str, subjects: &[&str], basis: Option<&str>) -> String {
        let outcome = if kind == "escalate" {
            "escalated"
        } else {
            "settled"
        };
        let mut args = vec![
            "decisions",
            "record",
            session,
            "--kind",
            kind,
            "--outcome",
            outcome,
        ];
        for s in subjects {
            args.extend_from_slice(&["--subject", s]);
        }
        if let Some(b) = basis {
            args.extend_from_slice(&["--basis", b]);
        }
        args.extend_from_slice(&["--by", "session"]);
        self.ok(&args)["id"].as_str().unwrap().to_string()
    }

    /// Record unit `i` through the binary.
    fn build_unit(&self, i: usize) {
        let (l, m) = (lead(i), manager(i));
        self.init(&l, Some("top"));
        self.init(&m, Some(&l));
        let recipient = format!("recipient=session:{m}");
        let mut first_question = None;
        for w in WORKERS {
            let name = worker(i, w);
            self.init(&name, Some(&m));
            let a = self.record(
                &name,
                "judgment",
                &[
                    "what=pick the cache",
                    "state=implement:1",
                    "result=undecided",
                ],
                None,
            );
            let q = self.record(
                &name,
                "escalate",
                &[
                    "question=cache-store",
                    "text=which store?",
                    &recipient,
                    "state=implement:1",
                ],
                Some(&format!("{name}:{a}")),
            );
            first_question.get_or_insert(format!("{name}:{q}"));
            self.record(
                &name,
                "judgment",
                &["what=name the table", "result=provisional"],
                None,
            );
            // The manager answers every worker's question but a's and b's.
            if !OPEN.contains(&w) {
                self.ok(&[
                    "decisions",
                    "answer",
                    &name,
                    &q,
                    "--in",
                    &m,
                    "--by",
                    "session",
                    "--text",
                    "SQLite",
                ]);
            }
        }
        self.record(
            &m,
            "judgment",
            &["what=cannot choose", "result=undecided"],
            first_question.as_deref(),
        );
    }

    /// Copy unit 0 to unit `i`: every file of its sessions, with the
    /// sessions' names and ids changed, and each context manifest's hashes
    /// recomputed.
    fn copy_unit(&self, i: usize) {
        let mut from: Vec<String> = vec![lead(0), manager(0)];
        from.extend(WORKERS.iter().map(|w| worker(0, *w)));
        let mut swaps: Vec<(String, String)> = vec![(unit(0), unit(i))];
        for (j, name) in from.iter().enumerate() {
            let header = std::fs::read_to_string(
                self.sessions()
                    .join(name)
                    .join(format!("koto-{name}.state.jsonl")),
            )
            .unwrap();
            let first: Value = serde_json::from_str(header.lines().next().unwrap()).unwrap();
            let id = first["session_id"].as_str().unwrap().to_string();
            swaps.push((id, format!("{i:08x}-{j:04x}-4000-8000-000000000000")));
        }
        let swap = |s: &str| {
            let mut s = s.to_string();
            for (a, b) in &swaps {
                s = s.replace(a.as_str(), b.as_str());
            }
            s
        };
        for name in &from {
            let src = self.sessions().join(name);
            let dst = self.sessions().join(swap(name));
            copy_tree(&src, &src, &dst, &swap);
            rehash(&dst.join("ctx"));
        }
    }

    /// The manager of unit `i` asks the owner about worker a's
    /// provisional judgment, and the owner answers in the lead.
    fn review(&self, i: usize) {
        let (l, m, w) = (lead(i), manager(i), worker(i, 'a'));
        let (question, labels) = fx::shown(&fx::lines("accepted"), 1);
        let content = fx::content(&question, &labels);
        let packet = format!("packet={}", fx::packet(&content));
        let q = self.record(
            &m,
            "escalate",
            &[
                "question=table-name",
                "text=keep the name?",
                "recipient=person:owner",
                &packet,
            ],
            Some(&format!("{w}:000000000003")),
        );
        let file = self.root.join(format!("content-{m}"));
        std::fs::write(&file, &content).unwrap();
        let written = self.ok(&[
            "decisions",
            "content",
            "write",
            &m,
            &q,
            "--file",
            file.to_str().unwrap(),
        ]);
        let header = written["prompt_header"].as_str().unwrap().to_string();
        let n = self.calls.get() + 1;
        self.calls.set(n);
        let call = format!("toolu_ZQXMARKERcall{n:015}");
        let lines = fx::with_call(&fx::lines("accepted"), &call);
        fx::install(
            &fx::harness(),
            &fx::render(&lines, &fx::harness(), &[&header]),
        );
        let pointer = format!("{}/{call}/1", fx::harness());
        let out = self.ok_h(
            Some(&fx::harness()),
            &[
                "decisions",
                "answer",
                &m,
                &q,
                "--in",
                &l,
                "--by",
                "person:owner",
                "--prompt",
                &pointer,
            ],
        );
        assert_eq!(out["decision"]["prompt"]["mark"]["value"], "verified");
    }

    fn timed(&self, args: &[&str]) -> (Value, Duration) {
        let start = Instant::now();
        let v = self.ok(args);
        (v, start.elapsed())
    }
}

fn copy_tree(root: &Path, at: &Path, dst_root: &Path, swap: &dyn Fn(&str) -> String) {
    for e in std::fs::read_dir(at).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_tree(root, &p, dst_root, swap);
            continue;
        }
        let rel = swap(p.strip_prefix(root).unwrap().to_str().unwrap());
        let to = dst_root.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&to, swap(&text)).unwrap();
    }
}

fn rehash(ctx: &Path) {
    use sha2::{Digest, Sha256};
    let path = ctx.join("manifest.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut manifest: Value = serde_json::from_str(&text).unwrap();
    if let Some(keys) = manifest["keys"].as_object_mut() {
        for (key, entry) in keys.iter_mut() {
            let bytes = std::fs::read(ctx.join(key)).unwrap();
            entry["hash"] = Value::from(hex::encode(Sha256::digest(&bytes)));
            entry["size"] = Value::from(bytes.len());
        }
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

fn count(v: &Value) -> usize {
    v["attempts"].as_array().unwrap().len()
}

/// The times of the timed reads.
struct Times {
    at: Duration,
    awaiting: Duration,
    within: Duration,
}

/// Run the reads over a store of `units` units and check every count the
/// generator intends.
fn check(store: &Store, units: usize) -> Times {
    let sessions = std::fs::read_dir(store.sessions())
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .count();
    assert_eq!(sessions, units * 10 + 1);
    let open = store.ok(&["decisions", "open", "--at", "top"]);
    assert_eq!(
        open["questions"].as_array().unwrap().len(),
        units * OPEN.len(),
        "{open}"
    );

    let (all, at) = store.timed(&["decisions", "attempts", "--at", "top"]);
    // Two attempts per worker and one per manager.
    assert_eq!(count(&all), units * (WORKERS.len() * 2 + 1));
    let tied = all["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| !e["question"].is_null())
        .count();
    // Each worker's undecided attempt and the manager's attempt. The
    // question the manager puts to the owner names worker a's provisional,
    // which it reviews but does not tie: the raiser isn't in a's run.
    assert_eq!(tied, units * (WORKERS.len() + 1));

    let (waiting, awaiting) = store.timed(&[
        "decisions",
        "attempts",
        "--at",
        "top",
        "--result",
        "provisional",
        "--awaiting-review",
    ]);
    assert_eq!(count(&waiting), units * (WORKERS.len() - 1));

    let probe = worker(units / 2, 'c');
    let (mine, within) = store.timed(&["decisions", "attempts", "--in", &probe]);
    assert_eq!(count(&mine), 2);
    assert!(mine["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["session"] == probe.as_str()));
    eprintln!(
        "precedent_cost: {sessions} sessions; attempts --at top {at:?}; \
         --awaiting-review {awaiting:?}; --in {within:?} (release: {RELEASE})"
    );
    Times {
        at,
        awaiting,
        within,
    }
}

#[test]
fn the_attempts_read_answers_within_its_bound_on_a_large_store() {
    let units = if RELEASE { 100 } else { 10 };
    let store = Store::new(None);
    store.init("top", None);
    store.build_unit(0);
    for i in 1..units {
        store.copy_unit(i);
    }
    for i in 0..units {
        store.review(i);
    }
    let t = check(&store, units);
    if RELEASE {
        for (what, took) in [
            ("attempts --at top", t.at),
            ("attempts --at top --awaiting-review", t.awaiting),
            ("attempts --in a worker", t.within),
        ] {
            assert!(took < BOUND, "{what} took {took:?}, over {BOUND:?}");
        }
    }
}

/// The same reads over a cloud host's local copy make no request. Its
/// store is recorded through the binary, unit by unit, so it is smaller:
/// four units on a release build, two on a debug one.
#[test]
fn a_cloud_hosts_attempts_read_makes_no_request() {
    let units = if RELEASE { 4 } else { 2 };
    let s3 = fake_s3::FakeS3::start("koto-attempts");
    let store = Store::new(Some(&s3));
    store.init("top", None);
    for i in 0..units {
        store.build_unit(i);
        store.review(i);
    }
    s3.clear_requests();
    check(&store, units);
    let requests: Vec<String> = s3
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.key))
        .collect();
    assert!(requests.is_empty(), "the reads made requests: {requests:?}");
}
