//! Integration tests for `koto decisions precedent`: the verified person
//! answers in effect in a session's chain, narrowed to a question label,
//! and the label koto copies onto every answer from its question.
//!
//! The sessions are the escalation ladder's: `worker` and `worker2`
//! dispatched by `manager`, `manager` by `director`, and an unrelated
//! `other-lead` with `other-worker` below it. A person's answer is recorded
//! from the accepted fixture under `tests/fixtures/prompt/`, never from a
//! live prompt.

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

use question_env::{Env, Q};
use serde_json::Value;

/// q1, raised in `worker` with `label=cache`, passed up to `director` and
/// then to `person:owner`, and answered by the owner in `answer_in`; the
/// question and the answer's id.
fn q1(env: &Env, answer_in: &str) -> (Q, String) {
    let q = env.raise("worker", "q1", "session:manager", &["label=cache"]);
    env.readdress(&q, "manager", "session:director");
    env.readdress(&q, "director", "person:owner");
    let a = env.person(&q, answer_in);
    (q, a)
}

fn read(env: &Env, person: &str, chain: &str, label: Option<&str>) -> Value {
    let mut args = vec![
        "decisions",
        "precedent",
        "--person",
        person,
        "--chain",
        chain,
    ];
    if let Some(l) = label {
        args.extend_from_slice(&["--label", l]);
    }
    env.ok(&args).json()
}

fn answers(env: &Env, person: &str, chain: &str, label: Option<&str>) -> Vec<Value> {
    read(env, person, chain, label)["answers"]
        .as_array()
        .unwrap()
        .clone()
}

/// The `(identity, answer id)` pairs the read returns.
fn found(env: &Env, chain: &str, label: Option<&str>) -> Vec<(String, String)> {
    answers(env, "owner", chain, label)
        .iter()
        .map(|e| {
            (
                e["identity"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn one(q: &Q, a: &str) -> Vec<(String, String)> {
    vec![(q.identity(), a.to_string())]
}

#[test]
fn the_answer_is_found_from_the_raiser_its_sibling_and_every_coordinator() {
    let env = Env::new();
    env.ladder();
    let q = env.raise("worker", "q1", "session:manager", &["label=cache"]);
    env.readdress(&q, "manager", "session:director");
    env.readdress(&q, "director", "person:owner");
    assert!(answers(&env, "owner", "worker2", Some("cache")).is_empty());
    let a = env.person(&q, "director");
    let shown = env.show("director", &a);

    for chain in ["worker2", "worker", "manager", "director"] {
        let got = answers(&env, "owner", chain, Some("cache"));
        assert_eq!(got.len(), 1, "--chain {chain}: {got:?}");
        let e = &got[0];
        assert_eq!(e["identity"], q.identity().as_str());
        assert_eq!(e["label"], "cache");
        assert_eq!(e["session"], "director");
        assert_eq!(e["id"], a.as_str());
        assert_eq!(e["text"], shown["decision"]["subject"]["text"]);
        assert_eq!(e["time"], shown["decision"]["time"]);
        assert_eq!(e["decider"]["kind"], "person");
        assert_eq!(e["decider"]["handle"]["value"], "owner");
        assert_eq!(e["reading"], "verified");
    }
    assert!(answers(&env, "someone-else", "worker2", None).is_empty());
    assert!(answers(&env, "owner", "other-worker", Some("cache")).is_empty());
    let doc = read(&env, "owner", "worker2", Some("cache"));
    assert_eq!(doc["person"], "owner");
    assert_eq!(doc["chain"], "worker2");
    assert_eq!(doc["label"], "cache");
}

#[test]
fn the_label_is_compared_byte_for_byte() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    for label in ["Cache", "cach", "cache ", " cache"] {
        assert!(
            answers(&env, "owner", "worker2", Some(label)).is_empty(),
            "--label {label:?}"
        );
    }
    // Composed and decomposed forms of the same text are different labels.
    let composed = "caf\u{e9}";
    let decomposed = "cafe\u{301}";
    let qc = env.raise(
        "worker",
        "q2",
        "person:owner",
        &[&format!("label={composed}")],
    );
    let ac = env.person(&qc, "director");
    assert_eq!(found(&env, "worker2", Some(composed)), one(&qc, &ac));
    assert!(found(&env, "worker2", Some(decomposed)).is_empty());
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &a));

    for bad in ["", &"x".repeat(129)] {
        let out = env.run(&[
            "decisions",
            "precedent",
            "--person",
            "owner",
            "--chain",
            "worker2",
            "--label",
            bad,
        ]);
        assert_eq!(out.code, 2, "--label of {} bytes: {}", bad.len(), out.all());
        assert_eq!(out.json()["code"], "invalid_decision", "{}", out.all());
    }
}

#[test]
fn with_no_label_every_answer_comes_back_with_its_label() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    let q2 = env.raise("worker2", "q2", "person:owner", &[]);
    let a2 = env.person(&q2, "manager");
    let all = answers(&env, "owner", "worker", None);
    assert_eq!(all.len(), 2, "{all:?}");
    let of = |q: &Q| {
        all.iter()
            .find(|e| e["identity"] == q.identity().as_str())
            .unwrap()
            .clone()
    };
    assert_eq!(of(&q)["label"], "cache");
    assert_eq!(of(&q)["id"], a.as_str());
    assert_eq!(of(&q2)["label"], Value::Null);
    assert_eq!(of(&q2)["id"], a2.as_str());
    for label in ["cache", "null", "none"] {
        assert!(!found(&env, "worker", Some(label)).contains(&(q2.identity(), a2.clone())));
    }
}

#[test]
fn only_a_verified_person_answer_in_effect_is_precedent() {
    let env = Env::new();
    env.ladder();
    // Closed by its addressee session's answer.
    let qs = env.raise("worker", "qs", "session:manager", &["label=cache"]);
    env.say_ok(&qs, "manager", "Postgres, per the owner");
    // Withdrawn by its raiser.
    let qw = env.raise("worker", "qw", "person:owner", &["label=cache"]);
    env.withdraw(&qw, "worker", "not needed");
    // An answer by a session that names the owner.
    let qn = env.raise("worker", "qn", "session:manager", &["label=cache"]);
    let out = env.answer_run(
        &qn,
        "manager",
        &[
            "--text",
            "owner says Postgres",
            "--reason",
            "the owner said so: person:owner",
            "--by",
            "session",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.all());
    assert!(found(&env, "worker", None).is_empty());

    // A settle in effect is not precedent in its own right.
    let (q, first) = q1(&env, "director");
    let q5 = env.raise("worker2", "q5", "person:owner", &["label=cache"]);
    env.settle_ok(&q5, "worker2", &format!("director:{first}"));
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &first));

    // A second person answer governs and replaces the first; a settle
    // accepts the one returned and refuses the one left out.
    let second = env.person(&q, "manager");
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &second));
    let q6 = env.raise("worker2", "q6", "person:owner", &["label=cache"]);
    let refused = env.settle(&q6, "worker2", &["--basis", &format!("director:{first}")]);
    assert_eq!(refused.code, 2, "{}", refused.all());
    env.settle_ok(&q6, "worker2", &format!("manager:{second}"));
}

#[test]
fn every_answer_returned_is_a_basis_a_settle_accepts() {
    let env = Env::new();
    env.ladder();
    q1(&env, "director");
    let q2 = env.raise("worker2", "q2", "person:owner", &["label=queue"]);
    env.person(&q2, "other-lead");
    for e in answers(&env, "owner", "manager", None) {
        let label = e["label"].as_str().unwrap();
        let q = env.raise(
            "worker2",
            &format!("for-{label}"),
            "person:owner",
            &[&format!("label={label}")],
        );
        let basis = format!(
            "{}:{}",
            e["session"].as_str().unwrap(),
            e["id"].as_str().unwrap()
        );
        env.settle_ok(&q, "worker2", &basis);
    }
}

#[test]
fn an_answer_read_as_a_remote_sessions_is_not_precedent() {
    let env = Env::new();
    env.ladder();
    let (_, a) = q1(&env, "director");
    env.forget_verified("director");
    assert_eq!(
        env.show("director", &a)["reading"],
        "a remote session's answer"
    );
    assert!(found(&env, "worker", Some("cache")).is_empty());
    let q2 = env.raise("worker", "q2", "person:owner", &["label=cache"]);
    let a2 = env.person(&q2, "manager");
    assert_eq!(found(&env, "worker", Some("cache")), one(&q2, &a2));
}

#[test]
fn the_read_stays_inside_the_chain() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    let q9 = env.raise("other-worker", "q9", "person:owner", &["label=cache"]);
    let a9 = env.person(&q9, "other-lead");
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &a));
    assert_eq!(found(&env, "other-worker", Some("cache")), one(&q9, &a9));
    assert_eq!(found(&env, "other-lead", Some("cache")), one(&q9, &a9));

    // A session linked to no other: its own questions and its own answers.
    env.init("solo", &[]);
    assert!(found(&env, "solo", None).is_empty());
    let qs = env.raise("solo", "qs", "person:owner", &["label=cache"]);
    let a_s = env.person(&qs, "other-lead");
    let q3 = env.raise("worker", "q3", "person:owner", &["label=cache"]);
    let a3 = env.person(&q3, "solo");
    let mut want = vec![(qs.identity(), a_s), (q3.identity(), a3)];
    want.sort();
    let mut got = found(&env, "solo", Some("cache"));
    got.sort();
    assert_eq!(got, want);
    assert!(!found(&env, "solo", Some("cache")).contains(&(q.identity(), a.clone())));

    // An anchor is a position like a session.
    let lead = env.open_anchor("6c4d5e2f-3a0b-4b88-94e5-f60718293a64", &[]);
    env.init("held", &["--dispatched-by", &lead]);
    let qh = env.raise("held", "qh", "person:owner", &["label=cache"]);
    let ah = env.person(&qh, "other-lead");
    assert_eq!(found(&env, &lead, Some("cache")), one(&qh, &ah));
    assert_eq!(found(&env, "held", Some("cache")), one(&qh, &ah));
}

#[test]
fn an_answer_given_outside_the_raisers_chain_is_found_from_it() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "other-lead");
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &a));
    assert_eq!(found(&env, "other-worker", Some("cache")), one(&q, &a));
}

#[test]
fn a_successor_of_the_raiser_or_of_a_coordinator_finds_the_answer() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    env.init("worker-next", &["--continues", "worker", "--supersede"]);
    env.init("manager-next", &["--continues", "manager", "--supersede"]);
    assert_eq!(found(&env, "worker-next", Some("cache")), one(&q, &a));
    assert_eq!(found(&env, "manager-next", Some("cache")), one(&q, &a));
}

#[test]
fn the_answer_carries_its_questions_label_and_outlives_the_raiser() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    assert_eq!(
        env.show("director", &a)["decision"]["subject"]["label"],
        "cache"
    );
    // Every outcome carries it; an answer to a question with no label
    // carries no label field at all.
    let qw = env.raise("worker", "qw", "person:owner", &["label=queue"]);
    let w = env.withdraw(&qw, "worker", "moot");
    assert_eq!(env.stored("worker", &qw, &w)["subject"]["label"], "queue");
    let qn = env.raise("worker", "qn", "session:manager", &[]);
    let n = env.say_ok(&qn, "manager", "yes");
    let stored = env.stored("manager", &qn, &n);
    assert!(stored["subject"].get("label").is_none(), "{stored}");

    env.cleanup("worker");
    let got = answers(&env, "owner", "manager", Some("cache"));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["identity"], q.identity().as_str());
    assert_eq!(got[0]["label"], "cache");
    assert_eq!(got[0]["id"], a.as_str());
}

#[test]
fn everything_is_matched_by_identity() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    env.cleanup("worker");
    env.init("worker", &["--dispatched-by", "manager"]);
    let fresh = env.raise("worker", "q1", "session:manager", &["label=queue"]);
    assert_eq!(fresh.id, q.id, "the ids must collide for this test");
    assert_ne!(fresh.session_id, q.session_id);
    assert!(found(&env, "worker", Some("queue")).is_empty());
    let got = answers(&env, "owner", "worker", Some("cache"));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["identity"], q.identity().as_str());
    assert_eq!(got[0]["label"], "cache");
    assert_eq!(got[0]["id"], a.as_str());
}

#[test]
fn cited_and_below_are_read_from_the_named_session() {
    let env = Env::new();
    env.ladder();
    let (_, a) = q1(&env, "director");
    env.cite("manager", &format!("director:{a}"));
    let flags = |chain: &str| {
        let e = answers(&env, "owner", chain, Some("cache"))[0].clone();
        (e["cited"].as_bool().unwrap(), e["below"].as_bool().unwrap())
    };
    assert_eq!(flags("manager"), (true, false));
    assert_eq!(flags("director"), (false, true));
    assert_eq!(flags("worker2"), (false, false));
    // A successor's run holds its predecessor's decisions.
    env.init("manager-next", &["--continues", "manager", "--supersede"]);
    assert_eq!(flags("manager-next"), (true, false));
}

#[test]
fn several_answers_come_back_in_one_order_and_runs_repeat_byte_for_byte() {
    let env = Env::new();
    env.ladder();
    let mut want = Vec::new();
    for (n, holder) in ["director", "manager", "worker2", "other-lead"]
        .iter()
        .enumerate()
    {
        let q = env.raise("worker", &format!("q{n}"), "person:owner", &["label=cache"]);
        let a = env.person(&q, holder);
        want.push((q.identity(), a));
    }
    // Recorded one after another, so their times rise in that order.
    assert_eq!(found(&env, "worker", Some("cache")), want);
    let args = [
        "decisions",
        "precedent",
        "--person",
        "owner",
        "--chain",
        "worker",
    ];
    let first = env.ok(&args).stdout;
    assert_eq!(env.ok(&args).stdout, first);
}

#[test]
fn bad_arguments_exit_2_and_the_read_writes_nothing() {
    let env = Env::new();
    env.ladder();
    q1(&env, "director");
    let before = env.snapshot_all();
    for args in [
        vec!["--person", "owner", "--chain", "nosuch"],
        vec!["--person", "owner", "--chain", "../worker"],
        vec!["--chain", "worker"],
        vec!["--person", "owner"],
        vec!["--person", "", "--chain", "worker"],
    ] {
        let mut all = vec!["decisions", "precedent"];
        all.extend_from_slice(&args);
        let out = env.run(&all);
        assert_eq!(out.code, 2, "{args:?}: {}", out.all());
    }
    for chain in ["worker", "worker2", "director", "other-lead"] {
        answers(&env, "owner", chain, None);
        answers(&env, "owner", chain, Some("cache"));
    }
    assert_eq!(env.snapshot_all(), before);
}

// A person answer read in an imported copy, or in one a pull brought
// down, reads as a remote session's answer and is not precedent.

mod remote {
    use super::fake_s3::FakeS3;
    use super::koto_command;
    use super::prompt_fixture as fx;
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    const BUCKET: &str = "koto-precedent";
    const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

    fn mk(p: PathBuf) -> PathBuf {
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(p).unwrap()
    }

    fn workspace(dir: PathBuf) -> PathBuf {
        let ws = mk(dir);
        std::fs::write(ws.join("work.md"), super::question_env::TEMPLATE).unwrap();
        ws
    }

    struct Host {
        _root: tempfile::TempDir,
        home: PathBuf,
        cache: PathBuf,
        endpoint: String,
    }

    impl Host {
        fn new(s3: &FakeS3) -> Self {
            let root = tempfile::TempDir::new().unwrap();
            let home = mk(root.path().join("home"));
            let cache = mk(root.path().join("cache"));
            Host {
                _root: root,
                home,
                cache,
                endpoint: s3.endpoint(),
            }
        }

        fn run(&self, ws: &Path, harness: Option<&str>, args: &[&str]) -> (i32, String, String) {
            let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
            let mut cmd = koto_command::koto_std();
            cmd.env_clear()
                .current_dir(ws)
                .env("PATH", format!("{}:{SYSTEM_PATH}", koto_dir.display()))
                .env("HOME", &self.home)
                .env("XDG_CACHE_HOME", &self.cache)
                .env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
                .args(args);
            if let Some(h) = harness {
                cmd.env("CLAUDE_CODE_SESSION_ID", h);
            }
            let o = cmd.output().unwrap();
            (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        }

        fn ok(&self, ws: &Path, harness: Option<&str>, args: &[&str]) -> Value {
            let (code, stdout, stderr) = self.run(ws, harness, args);
            assert_eq!(code, 0, "{args:?}\n{stdout}\n{stderr}");
            serde_json::from_str(stdout.trim()).unwrap_or(Value::Null)
        }

        fn precedent(&self, ws: &Path, chain: &str) -> Vec<Value> {
            self.ok(
                ws,
                None,
                &[
                    "decisions",
                    "precedent",
                    "--person",
                    "owner",
                    "--chain",
                    chain,
                ],
            )["answers"]
                .as_array()
                .unwrap()
                .clone()
        }
    }

    /// Raise a labelled question in `raiser` and record the owner's answer
    /// to it in `answerer`, dispatched by the raiser; the answer's id.
    fn record(host: &Host, ws: &Path) -> String {
        host.ok(ws, None, &["init", "raiser", "--template", "work.md"]);
        host.ok(
            ws,
            None,
            &[
                "init",
                "answerer",
                "--template",
                "work.md",
                "--dispatched-by",
                "raiser",
            ],
        );
        let lines = fx::lines("accepted");
        let (question, labels) = fx::shown(&lines, 1);
        let content = fx::content(&question, &labels);
        let packet = format!("packet={}", fx::packet(&content));
        let q = host.ok(
            ws,
            None,
            &[
                "decisions",
                "record",
                "raiser",
                "--kind",
                "escalate",
                "--outcome",
                "escalated",
                "--subject",
                "question=which-way",
                "--subject",
                "text=which way?",
                "--subject",
                "recipient=person:owner",
                "--subject",
                "label=cache",
                "--subject",
                &packet,
                "--by",
                "session",
            ],
        );
        let qid = q["id"].as_str().unwrap().to_string();
        let file = ws.join("content.json");
        std::fs::write(&file, &content).unwrap();
        let written = host.ok(
            ws,
            None,
            &[
                "decisions",
                "content",
                "write",
                "raiser",
                &qid,
                "--file",
                file.to_str().unwrap(),
            ],
        );
        let header = written["prompt_header"].as_str().unwrap();
        fx::install(
            &fx::harness(),
            &fx::render(&lines, &fx::harness(), &[header]),
        );
        let pointer = format!("{}/{}/1", fx::harness(), fx::CALL);
        let answer = host.ok(
            ws,
            Some(fx::harness().as_str()),
            &[
                "decisions",
                "answer",
                "raiser",
                &qid,
                "--in",
                "answerer",
                "--by",
                "person:owner",
                "--prompt",
                &pointer,
            ],
        );
        answer["id"].as_str().unwrap().to_string()
    }

    fn remote_reading(host: &Host, ws: &Path, holder: &str, id: &str) {
        let shown = host.ok(ws, None, &["decisions", "show", holder, id]);
        assert_eq!(shown["reading"], "a remote session's answer", "{shown}");
    }

    fn compile(host: &Host, ws: &Path) {
        let (code, _, err) = host.run(ws, None, &["template", "compile", "work.md"]);
        assert_eq!(code, 0, "{err}");
    }

    #[test]
    fn a_pulled_copy_or_one_imported_here_under_a_new_name_is_not_precedent() {
        let _records = fx::Records::new();
        let s3 = FakeS3::start(BUCKET);
        let shared = tempfile::TempDir::new().unwrap();
        let ws_a = workspace(shared.path().join("ws-a"));
        let first = Host::new(&s3);
        let id = record(&first, &ws_a);
        assert_eq!(first.precedent(&ws_a, "raiser").len(), 1);
        assert_eq!(first.precedent(&ws_a, "answerer").len(), 1);

        // Brought down by a pull on another host.
        let other = Host::new(&s3);
        for name in ["raiser", "answerer"] {
            let (code, out, err) = other.run(
                &ws_a,
                None,
                &["session", "resolve", name, "--keep", "remote"],
            );
            assert_eq!(code, 0, "{out}\n{err}");
        }
        remote_reading(&other, &ws_a, "answerer", &id);
        assert!(other.precedent(&ws_a, "raiser").is_empty());
        assert!(other.precedent(&ws_a, "answerer").is_empty());

        // Imported on the recording host under a new name: once the copy
        // is the only one holding the answer, the answer is not precedent.
        let ws_b = workspace(shared.path().join("ws-b"));
        compile(&first, &ws_b);
        first.ok(
            &ws_b,
            None,
            &[
                "session",
                "import",
                "answerer",
                "--from",
                ws_a.to_str().unwrap(),
                "--as",
                "copy",
            ],
        );
        std::fs::remove_dir_all(first.home.join(".koto/sessions/answerer")).unwrap();
        remote_reading(&first, &ws_b, "copy", &id);
        assert!(first.precedent(&ws_a, "raiser").is_empty());
        assert!(first.precedent(&ws_b, "copy").is_empty());
    }

    #[test]
    fn a_copy_imported_on_another_host_is_not_precedent() {
        let _records = fx::Records::new();
        let s3 = FakeS3::start(BUCKET);
        let shared = tempfile::TempDir::new().unwrap();
        let ws_a = workspace(shared.path().join("ws-a"));
        let first = Host::new(&s3);
        let id = record(&first, &ws_a);

        let second = Host::new(&s3);
        let ws_b = workspace(shared.path().join("ws-b"));
        compile(&second, &ws_b);
        for name in ["raiser", "answerer"] {
            second.ok(
                &ws_b,
                None,
                &["session", "import", name, "--from", ws_a.to_str().unwrap()],
            );
        }
        remote_reading(&second, &ws_b, "answerer", &id);
        assert!(second.precedent(&ws_b, "raiser").is_empty());
        assert!(second.precedent(&ws_b, "answerer").is_empty());
    }
}

/// Reverse `q` in `in_session`, as an operator does.
fn reverse(env: &Env, q: &Q, in_session: &str) {
    env.ok(&[
        "decisions",
        "reverse",
        &q.session,
        &q.id,
        "--in",
        in_session,
        "--by",
        "session",
    ]);
}

#[test]
fn a_reversed_question_stays_reversed_once_its_raiser_is_removed() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    reverse(&env, &q, "director");
    assert!(found(&env, "worker2", Some("cache")).is_empty());

    // The raiser goes, and with it the question; the reversal recorded in
    // director still withdraws the answer, for the read and for a settle.
    env.cleanup("worker");
    assert!(found(&env, "worker2", Some("cache")).is_empty());
    assert!(found(&env, "manager", None).is_empty());
    let q7 = env.raise("worker2", "q7", "person:owner", &["label=cache"]);
    let out = env.settle(&q7, "worker2", &["--basis", &format!("director:{a}")]);
    assert_eq!(out.code, 2, "{}", out.all());
    assert!(
        out.error().contains("isn't the answer in effect"),
        "{}",
        out.error()
    );
    let shown = env.show("director", &a);
    assert_eq!(shown["counted"], false, "{shown}");
    assert_eq!(shown["governs"], false, "{shown}");
    assert_eq!(shown["reason"], "no_question", "{shown}");
}

#[test]
fn a_replaced_answer_stays_replaced_while_its_replacement_is_readable() {
    let env = Env::new();
    env.ladder();
    let (q, first) = q1(&env, "director");
    // The second person answer cites the one it replaces.
    let second = env.person(&q, "manager");
    let basis = env.stored("manager", &q, &second)["basis"].clone();
    assert_eq!(
        basis,
        serde_json::json!([{"session": "director", "id": first}]),
        "{basis}"
    );
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &second));

    // With the raiser removed, the replacement governs; once it no longer
    // reads as verified here it isn't precedent, and it still keeps the
    // answer it cites from coming back.
    env.cleanup("worker");
    assert_eq!(found(&env, "worker2", Some("cache")), one(&q, &second));
    env.forget_verified("manager");
    assert!(found(&env, "worker2", Some("cache")).is_empty());
    let q7 = env.raise("worker2", "q7", "person:owner", &["label=cache"]);
    let out = env.settle(&q7, "worker2", &["--basis", &format!("director:{first}")]);
    assert_eq!(out.code, 2, "{}", out.all());
}

#[test]
fn removing_the_replacing_session_is_the_stated_limit() {
    let env = Env::new();
    env.ladder();
    let (q, first) = q1(&env, "director");
    let second = env.person(&q, "worker2");
    assert_eq!(found(&env, "manager", Some("cache")), one(&q, &second));
    // Nothing readable says the first answer was replaced once both the
    // raiser and the session holding the replacement are gone.
    env.cleanup("worker");
    env.cleanup("worker2");
    assert_eq!(found(&env, "manager", Some("cache")), one(&q, &first));
}

#[test]
fn a_succession_joins_the_chain_in_both_directions() {
    let env = Env::new();
    env.ladder();
    let (q, a) = q1(&env, "director");
    // manager-next continues manager and has no dispatcher of its own; it
    // dispatches worker3.
    env.init("manager-next", &["--continues", "manager", "--supersede"]);
    env.init("worker3", &["--dispatched-by", "manager-next"]);
    let q3 = env.raise("worker3", "q3", "person:owner", &["label=queue"]);
    let a3 = env.person(&q3, "manager-next");
    for at in [
        "worker3",
        "manager-next",
        "manager",
        "worker",
        "worker2",
        "director",
    ] {
        assert_eq!(
            found(&env, at, Some("queue")),
            one(&q3, &a3),
            "--chain {at}"
        );
        assert_eq!(found(&env, at, Some("cache")), one(&q, &a), "--chain {at}");
    }
    assert!(found(&env, "other-worker", Some("queue")).is_empty());
}

#[test]
fn show_reads_a_removed_raisers_answer_the_way_precedent_does() {
    let env = Env::new();
    env.ladder();
    let (_, a) = q1(&env, "director");
    let before = env.show("director", &a);
    assert_eq!(before["counted"], true, "{before}");
    assert_eq!(before["governs"], true, "{before}");
    assert!(before.get("reason").is_none(), "{before}");
    env.cleanup("worker");
    assert_eq!(found(&env, "manager", Some("cache")).len(), 1);
    let after = env.show("director", &a);
    assert_eq!(after["counted"], true, "{after}");
    assert_eq!(after["governs"], true, "{after}");
    assert_eq!(after["reason"], "question_gone", "{after}");
}
