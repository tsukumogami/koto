//! The cost bound of the question reads this crate added: on a generated
//! store of 1,001 sessions in dispatcher chains four deep, holding 201 open
//! questions, 50 questions closed by a person's verified answer over five
//! labels and 100 closed by a session's answer, `koto decisions precedent`
//! with and without `--label` and `koto decisions history` of a question
//! passed up twice each answer within five seconds and return the count the
//! generator intended.
//!
//! The same store is then read as the cloud backend's local copy against
//! the in-process endpoint in `support/fake_s3.rs`: the reads return the
//! same counts and the endpoint records no request.
//!
//! The bound holds for a release build, which CI runs in its own job
//! (`cargo test --release --test precedent_cost`). A debug build runs the
//! same code on a store a tenth the size and checks only the counts. The
//! sessions holding no decision are written straight to the store, the way
//! `tests/retention_cost.rs` writes its store; every decision is recorded
//! through the binary, a person's answer from the accepted fixture under
//! `tests/fixtures/prompt/`. Everything lives under a temporary directory
//! removed when the test ends.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;
#[path = "support/question_env.rs"]
mod question_env;

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use fake_s3::FakeS3;
use koto::session::context::ContextStore;
use koto::session::local::LocalBackend;
use question_env::{Env, Q};
use serde_json::{json, Value};

const RELEASE: bool = !cfg!(debug_assertions);
const LABELS: usize = 5;
const BOUND: Duration = Duration::from_secs(5);

fn iso(t: SystemTime) -> String {
    koto::engine::types::iso8601_at(t)
}

/// What the generator put in the store.
struct Store {
    chains: usize,
    person: usize,
    passed_up: Q,
}

/// Write `name`'s log straight to the store, at the template's first
/// state, with a dispatched-by link to `parent`.
fn session(backend: &LocalBackend, base: &Path, template: &str, name: &str, parent: Option<&str>) {
    let t = SystemTime::now() - Duration::from_secs(3600);
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let header = json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "abc",
        "created_at": iso(t),
        "session_id": format!("id-{name}"),
    });
    let events = [
        json!({"seq": 1, "timestamp": iso(t), "type": "workflow_initialized",
               "payload": {"template_path": template, "variables": {}}}),
        json!({"seq": 2, "timestamp": iso(t), "type": "transitioned",
               "payload": {"from": null, "to": "work", "condition_type": "auto"}}),
    ];
    let mut text = header.to_string();
    for e in events {
        text.push('\n');
        text.push_str(&e.to_string());
    }
    text.push('\n');
    std::fs::write(dir.join(format!("koto-{name}.state.jsonl")), text).unwrap();
    if let Some(p) = parent {
        let body = json!({"v": 1, "session": p, "session_id": format!("id-{p}"),
                          "workspace": null, "at": iso(t)});
        backend
            .add_with_writer(
                name,
                &format!("koto/links/dispatched-by/{p}"),
                body.to_string().as_bytes(),
                "koto",
            )
            .unwrap();
    }
}

/// `root` dispatches each chain's `top`; each `top` dispatches three
/// managers, each manager two workers. In every chain two questions stay
/// open and one is closed by its manager's answer; in the first half of
/// the chains one more is closed by the owner's answer, labelled `l0` to
/// `l4` in turn. Chain 0 also holds a question passed up twice and still
/// open.
fn build(env: &Env) -> Store {
    let chains = if RELEASE { 100 } else { 10 };
    let base = env.sessions();
    let tdir = env.root.join("templates");
    std::fs::create_dir_all(&tdir).unwrap();
    let (template, _) =
        koto::cache::compile_cached_into(&env.root.join("work.md"), &tdir, true).unwrap();
    let template = template.to_string_lossy().into_owned();
    let backend = LocalBackend::with_base_dir(base.clone());
    session(&backend, &base, &template, "root", None);
    for c in 0..chains {
        let top = format!("c{c}-top");
        session(&backend, &base, &template, &top, Some("root"));
        for m in 0..3 {
            let mid = format!("c{c}-m{m}");
            session(&backend, &base, &template, &mid, Some(&top));
            for w in 0..2 {
                session(
                    &backend,
                    &base,
                    &template,
                    &format!("c{c}-w{m}{w}"),
                    Some(&mid),
                );
            }
        }
    }

    let open = |c: usize, worker: &str, mid: &str| {
        let out = env.escalate_run(
            &format!("c{c}-{worker}"),
            "open",
            &format!("session:c{c}-{mid}"),
            &["label=open"],
        );
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()
    };
    let mut person = 0;
    for c in 0..chains {
        open(c, "w00", "m0");
        open(c, "w11", "m1");
        let raised = open(c, "w10", "m1");
        let id = raised["id"].as_str().unwrap();
        env.ok(&[
            "decisions",
            "answer",
            &format!("c{c}-w10"),
            id,
            "--in",
            &format!("c{c}-m1"),
            "--text",
            "yes",
            "--by",
            "session",
        ]);
        if c < chains / 2 {
            let label = format!("label=l{}", c % LABELS);
            let q = env.raise(&format!("c{c}-w01"), "asked", "person:owner", &[&label]);
            env.person(&q, &format!("c{c}-top"));
            person += 1;
        }
    }
    let passed_up = env.raise("c0-w21", "passed", "session:c0-m2", &["label=up"]);
    env.readdress(&passed_up, "c0-m2", "session:c0-top");
    env.readdress(&passed_up, "c0-top", "person:owner");
    Store {
        chains,
        person,
        passed_up,
    }
}

/// Run `args` through `command`, timed; its JSON.
fn timed(what: &str, mut command: std::process::Command) -> (Value, Duration) {
    let started = Instant::now();
    let out = command.output().unwrap();
    let took = started.elapsed();
    assert!(
        out.status.success(),
        "{what}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!(
        "{what}: {} ms ({} build)",
        took.as_millis(),
        if RELEASE { "release" } else { "debug" }
    );
    (serde_json::from_slice(&out.stdout).unwrap(), took)
}

/// Each read with its intended count, through `command`.
fn reads(store: &Store, backend: &str, command: &dyn Fn(&[&str]) -> std::process::Command) {
    let identity = store.passed_up.identity();
    let cases: Vec<(String, Vec<&str>, usize)> = vec![
        (
            "precedent".to_string(),
            vec![
                "decisions",
                "precedent",
                "--person",
                "owner",
                "--chain",
                "root",
            ],
            store.person,
        ),
        // Every chain's top hangs from `root`, so a leaf's chain is the
        // whole store and holds every answer labelled `l0`.
        (
            "precedent --label, from a leaf".to_string(),
            vec![
                "decisions",
                "precedent",
                "--person",
                "owner",
                "--chain",
                "c0-w00",
                "--label",
                "l0",
            ],
            store.person / LABELS,
        ),
        (
            "precedent --label, from the top".to_string(),
            vec![
                "decisions",
                "precedent",
                "--person",
                "owner",
                "--chain",
                "root",
                "--label",
                "l0",
            ],
            store.person / LABELS,
        ),
        (
            "history".to_string(),
            vec!["decisions", "history", &identity],
            3,
        ),
    ];
    // A first run of a freshly built binary can pay for the OS checking it,
    // which isn't the read's cost.
    command(&["version"]).output().unwrap();
    for (what, args, want) in cases {
        let what = format!("{backend}: {what}");
        let (v, took) = timed(&what, command(&args));
        let got = if args[1] == "history" {
            assert_eq!(v["addressees"].as_array().unwrap().len(), 3, "{v}");
            assert_eq!(v["open"], true);
            v["entries"].as_array().unwrap().len()
        } else {
            v["answers"].as_array().unwrap().len()
        };
        assert_eq!(got, want, "{what}");
        if RELEASE {
            assert!(took < BOUND, "{what} took {took:?}");
        }
    }
}

#[test]
fn the_question_reads_answer_within_their_bound_and_ask_nothing_of_the_bucket() {
    let env = Env::cloud_copy();
    let store = build(&env);
    let sessions = std::fs::read_dir(env.sessions()).unwrap().count();
    assert_eq!(sessions, 1 + store.chains * 10);
    assert_eq!(store.person, store.chains / 2);

    reads(&store, "local", &|args| env.command(&[], args));

    let s3 = FakeS3::start("koto-precedent-cost");
    let endpoint = s3.endpoint();
    let cloud = |args: &[&str]| {
        let mut cmd = env.command(
            &[
                ("KOTO_SESSION_BACKEND", "cloud"),
                ("KOTO_SESSION_CLOUD_ENDPOINT", &endpoint),
                ("KOTO_SESSION_CLOUD_BUCKET", "koto-precedent-cost"),
                ("KOTO_SESSION_CLOUD_REGION", "us-east-1"),
                ("KOTO_SESSION_CLOUD_PATH_STYLE", "true"),
                ("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key"),
                ("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key"),
            ],
            args,
        );
        cmd.env_remove("KOTO_SESSIONS_BASE").env_remove("KOTO_HOME");
        cmd
    };
    s3.clear_requests();
    reads(&store, "cloud", &cloud);
    assert_eq!(s3.requests().len(), 0, "{:?}", s3.requests());
}

/// A question whose history is long: answered by its addressee, then 500
/// more answers aimed at it (50 on a debug build), each a copy of the first
/// stored straight into the addressee's aimed area under its own id. Each
/// entry is judged against the one resolution of the question, so the read
/// stays within the bound however many entries the question has.
#[test]
fn a_long_history_reads_within_the_bound() {
    let env = Env::new();
    env.ladder();
    let copies = if RELEASE { 500 } else { 50 };
    let q = env.raise("worker", "long", "session:manager", &["label=long"]);
    let first = env.say_ok(&q, "manager", "settled");
    let body = env.stored("manager", &q, &first);
    for i in 0..copies {
        let id = format!("{:012}", 900_000 + i);
        let mut copy = body.clone();
        copy["id"] = json!(id);
        env.plant("manager", &q, &id, &copy);
    }
    env.ok(&["version"]);
    let identity = q.identity();
    let (v, took) = timed(
        "history, long",
        env.command(&[], &["decisions", "history", &identity]),
    );
    // The raise and every answer, the first of them the one in effect.
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), copies + 2, "{v}");
    assert_eq!(v["answer"]["id"], first.as_str());
    let governing = entries.iter().filter(|e| e["governs"] == true).count();
    assert_eq!(governing, 1, "{v}");
    if RELEASE {
        assert!(took < BOUND, "history, long took {took:?}");
    }
}
