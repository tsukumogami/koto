//! Integration tests for `koto decisions history <session id>/<id>`: one
//! question as raised, where it stands, and everything this machine's
//! store holds about it in causal order, entries that don't count
//! included and marked.
//!
//! The sessions are the escalation ladder's: `worker` and `worker2`
//! dispatched by `manager`, `manager` by `director`, and an unrelated
//! `other-lead` with `other-worker` below it. A person's answer is recorded
//! from the accepted fixture under `tests/fixtures/prompt/`, never from a
//! live prompt. A stored entry koto's commands won't write (a pass-up with
//! a skewed clock, a file that doesn't parse) is placed by editing a
//! stored file.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;
#[path = "support/question_env.rs"]
mod question_env;

use question_env::{shift, Env, Out, Q};
use serde_json::{json, Value};

fn history_run(env: &Env, identity: &str) -> Out {
    env.run(&["decisions", "history", identity])
}

fn history(env: &Env, q: &Q) -> Value {
    env.ok(&["decisions", "history", &q.identity()]).json()
}

/// Each entry's `(entry, session, id)`.
fn order(doc: &Value) -> Vec<(String, String, String)> {
    doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["entry"].as_str().unwrap().to_string(),
                e["session"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn row(entry: &str, session: &str, id: &str) -> (String, String, String) {
    (entry.to_string(), session.to_string(), id.to_string())
}

fn entry<'d>(doc: &'d Value, session: &str, id: &str) -> &'d Value {
    doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["session"] == session && e["id"] == id)
        .unwrap_or_else(|| panic!("no {session}:{id} in {doc}"))
}

/// q1 as the walkthrough has it: raised in `worker` to `manager` with
/// `label=cache`, passed up to `director` and then to `person:owner`, and
/// answered by the owner in `director`. The question, both pass-ups' ids
/// and the answer's id.
struct Ladder {
    q: Q,
    up1: String,
    up2: String,
    answer: String,
}

fn ladder(env: &Env) -> Ladder {
    env.ladder();
    let q = env.raise("worker", "q1", "session:manager", &["label=cache"]);
    let up1 = env.readdress(&q, "manager", "session:director");
    let up2 = env.readdress(&q, "director", "person:owner");
    let answer = env.person(&q, "director");
    Ladder {
        q,
        up1,
        up2,
        answer,
    }
}

#[test]
fn the_document_carries_the_question_where_it_stands_and_its_answer() {
    let env = Env::new();
    let l = ladder(&env);
    let q5 = env.raise("worker2", "q5", "person:owner", &["label=cache"]);
    let settle = env.settle_ok(&q5, "worker2", &format!("director:{}", l.answer));

    // Run from a directory that isn't the workspace.
    let out = env
        .command(&[], &["decisions", "history", &l.q.identity()])
        .current_dir(&env.root)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let raised = env.show("worker", &l.q.id);
    let answer = env.show("director", &l.answer);

    assert_eq!(doc["identity"], l.q.identity().as_str());
    let q = &doc["question"];
    assert_eq!(q["id"], l.q.id.as_str());
    assert_eq!(q["question"], "q1");
    assert_eq!(q["text"], "what about q1?");
    assert_eq!(q["label"], "cache");
    assert_eq!(q["need"], "decision");
    assert_eq!(q["state"], Value::Null);
    assert_eq!(q["packet"], raised["decision"]["subject"]["packet"]);
    assert_eq!(q["content_stored"], true);
    assert_eq!(
        q["recipient"],
        json!({"typed": true, "kind": "session", "name": "manager"})
    );
    assert_eq!(
        doc["raiser"],
        json!({"name": "worker", "id": l.q.session_id.as_str()})
    );
    assert_eq!(doc["run_mode"], raised["decision"]["authority"]["run_mode"]);
    assert_eq!(doc["successor"], Value::Null);
    assert_eq!(doc["open"], false);
    assert_eq!(
        doc["addressee"],
        json!({"typed": true, "kind": "person", "handle": "owner"})
    );
    let addressees: Vec<Value> = doc["addressees"].as_array().unwrap().clone();
    assert_eq!(
        addressees,
        vec![
            json!({"typed": true, "kind": "session", "name": "manager"}),
            json!({"typed": true, "kind": "session", "name": "director"}),
            json!({"typed": true, "kind": "person", "handle": "owner"}),
        ]
    );
    let a = &doc["answer"];
    assert_eq!(a["session"], "director");
    assert_eq!(a["id"], l.answer.as_str());
    assert_eq!(a["outcome"], "answered");
    assert_eq!(a["text"], answer["decision"]["subject"]["text"]);
    assert_eq!(a["decider"], answer["decision"]["decider"]);
    assert_eq!(a["reading"], "verified");
    assert_eq!(a["run_mode"], answer["decision"]["authority"]["run_mode"]);

    assert_eq!(
        order(&doc),
        vec![
            row("raise", "worker", &l.q.id),
            row("pass-up", "manager", &l.up1),
            row("pass-up", "director", &l.up2),
            row("answer", "director", &l.answer),
            row("citing", "worker2", &settle),
        ]
    );
    let up = entry(&doc, "manager", &l.up1);
    assert_eq!(
        up["recipient"],
        json!({"typed": true, "kind": "session", "name": "director"})
    );
    assert_eq!(up["counted"], true);
    let owner = entry(&doc, "director", &l.answer);
    assert_eq!(owner["counted"], true);
    assert_eq!(owner["governs"], true);
    assert_eq!(owner["reading"], "verified");
    assert_eq!(owner["decider"], answer["decision"]["decider"]);
    let cite = entry(&doc, "worker2", &settle);
    assert_eq!(cite["kind"], "answer");
    assert_eq!(cite["outcome"], "settled");
    assert_eq!(
        cite["cites"],
        json!([{"session": "director", "id": l.answer.as_str()}])
    );
}

#[test]
fn an_open_question_has_no_answer_and_a_superseded_raiser_names_its_successor() {
    let env = Env::new();
    env.ladder();
    let q = env.raise("worker", "q1", "person:owner", &["need=decision"]);
    let doc = history(&env, &q);
    assert_eq!(doc["open"], true);
    assert_eq!(doc["answer"], Value::Null);
    assert_eq!(doc["question"]["need"], "decision");
    assert_eq!(doc["successor"], Value::Null);
    env.init("worker-next", &["--continues", "worker", "--supersede"]);
    assert_eq!(history(&env, &q)["successor"], "worker-next");
}

#[test]
fn every_answer_outcome_is_listed_with_what_show_gives() {
    let env = Env::new();
    env.ladder();
    let q = env.raise("worker", "q1", "session:manager", &[]);
    let other = env.raise("worker", "q0", "session:manager", &[]);
    // From a session that isn't the addressee, which koto won't record:
    // `manager`'s answer to another question, moved into `worker2` as an
    // answer to q1 recorded there. Listed, not counted.
    let moved = env.say_ok(&other, "manager", "stray");
    let mut body = env.stored("manager", &other, &moved);
    let w2 = env
        .escalate_run("worker2", "x", "session:manager", &[])
        .json();
    let stray = "000000000050".to_string();
    body["session"] = w2["decision"]["session"].clone();
    body["id"] = json!(stray);
    body["target"]["decision"] = json!(q.id);
    body["subject"]["id"] = json!(q.id);
    env.plant("worker2", &q, &stray, &body);
    let doc = history(&env, &q);
    assert_eq!(doc["open"], true);
    let e = entry(&doc, "worker2", &stray);
    assert_eq!(e["counted"], false);
    assert_eq!(e["reason"], "not_addressee");
    assert_eq!(e["governs"], false);

    let said = env.say_ok(&q, "manager", "yes");
    let withdrawn = {
        let q2 = env.raise("worker", "q2", "session:manager", &[]);
        let w = env.withdraw(&q2, "worker", "moot");
        (q2, w)
    };
    let doc = history(&env, &q);
    assert_eq!(doc["open"], false);
    for (holder, id) in [("worker2", &stray), ("manager", &said)] {
        let e = entry(&doc, holder, id);
        let shown = env.show(holder, id);
        assert_eq!(e["counted"], shown["counted"], "{holder}:{id}");
        assert_eq!(e["governs"], shown["governs"], "{holder}:{id}");
        assert_eq!(e.get("reason"), shown.get("reason"), "{holder}:{id}");
    }
    let doc2 = history(&env, &withdrawn.0);
    let w = entry(&doc2, "worker", &withdrawn.1);
    assert_eq!(w["outcome"], "withdrawn");
    assert_eq!(w["governs"], true);
    assert_eq!(doc2["answer"]["outcome"], "withdrawn");
}

#[test]
fn pass_ups_with_a_reversed_clock_keep_the_order_of_addressees() {
    let env = Env::new();
    let l = ladder(&env);
    let first = env.stored("manager", &l.q, &l.up1);
    let mut second = env.stored("director", &l.q, &l.up2);
    second["time"] = json!(shift(first["time"].as_str().unwrap(), -1));
    env.plant("director", &l.q, &l.up2, &second);
    let doc = history(&env, &l.q);
    assert_eq!(doc["addressees"].as_array().unwrap().len(), 3, "{doc}");
    assert_eq!(
        order(&doc),
        vec![
            row("raise", "worker", &l.q.id),
            row("pass-up", "manager", &l.up1),
            row("pass-up", "director", &l.up2),
            row("answer", "director", &l.answer),
        ]
    );
}

#[test]
fn a_citing_decision_never_comes_before_the_answer_it_cites() {
    let env = Env::new();
    let l = ladder(&env);
    env.init("alpha", &[]);
    let cite = env.cite("alpha", &format!("director:{}", l.answer));
    // The same stored time as the answer, from a session whose name sorts
    // first.
    let time = env.show("director", &l.answer)["decision"]["time"].clone();
    let path = env.sessions().join("alpha/ctx/koto/decisions").join(&cite);
    let mut body: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    body["time"] = time;
    std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    let doc = history(&env, &l.q);
    let rows = order(&doc);
    assert_eq!(
        rows[rows.len() - 2..].to_vec(),
        vec![
            row("answer", "director", &l.answer),
            row("citing", "alpha", &cite),
        ]
    );
}

#[test]
fn an_entry_that_does_not_parse_is_listed_and_counts_for_nothing() {
    let env = Env::new();
    let l = ladder(&env);
    let before = history(&env, &l.q);
    let path = env.aimed("manager", &l.q, "000000000077");
    std::fs::write(&path, b"{not json").unwrap();
    let doc = history(&env, &l.q);
    let entries = doc["entries"].as_array().unwrap();
    let bad = entries.last().unwrap();
    assert_eq!(bad["entry"], "unreadable");
    assert_eq!(bad["session"], "manager");
    assert_eq!(bad["id"], "000000000077");
    assert_eq!(bad["counted"], false);
    assert_eq!(
        entries[..entries.len() - 1].to_vec(),
        before["entries"].as_array().unwrap().clone()
    );
    assert_eq!(doc["answer"], before["answer"]);
    assert_eq!(doc["open"], false);
}

#[test]
fn the_history_is_by_identity() {
    let env = Env::new();
    let l = ladder(&env);
    env.cleanup("worker");
    env.init("worker", &["--dispatched-by", "manager"]);
    let fresh = env.raise("worker", "q1", "session:manager", &["label=queue"]);
    assert_eq!(fresh.id, l.q.id, "the ids must collide for this test");
    let doc = history(&env, &fresh);
    assert_eq!(order(&doc), vec![row("raise", "worker", &fresh.id)]);
    assert_eq!(doc["open"], true);
    assert_eq!(doc["question"]["label"], "queue");
    // The removed raiser's question is no longer held.
    let out = history_run(&env, &l.q.identity());
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["code"], "decision_not_found");
}

#[test]
fn identities_that_name_no_question_exit_2() {
    let env = Env::new();
    let l = ladder(&env);
    let missing = format!("{}/000000000099", l.q.session_id);
    let out = history_run(&env, &missing);
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["code"], "decision_not_found");
    assert!(out.error().contains(&missing), "{}", out.error());
    let unknown = format!("20200101T000000Z-000000000000/{}", l.q.id);
    let out = history_run(&env, &unknown);
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["code"], "decision_not_found", "{}", out.all());

    // A pass-up isn't a question; neither is an untyped escalate.
    let manager_id = env.show("manager", &l.up1)["decision"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out2 = history_run(&env, &format!("{manager_id}/{}", l.up1));
    assert_eq!(out2.code, 2, "{}", out2.all());
    assert_eq!(out2.json()["code"], "invalid_decision", "{}", out2.all());
    let untyped = env.escalate_run("worker", "q4", "coord", &[]).json();
    let out = history_run(
        &env,
        &format!("{}/{}", l.q.session_id, untyped["id"].as_str().unwrap()),
    );
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(out.json()["code"], "invalid_decision");

    for bad in [
        l.q.id.clone(),
        format!("worker/{}", l.q.id),
        format!("{}/", l.q.session_id),
        format!("{}/q1", l.q.session_id),
        format!("{}/{}/x", l.q.session_id, l.q.id),
    ] {
        let out = history_run(&env, &bad);
        assert_eq!(out.code, 2, "{bad}: {}", out.all());
    }
    let out = env.run(&["decisions", "history"]);
    assert_eq!(out.code, 2);
}

#[test]
fn the_read_writes_nothing_and_repeats_byte_for_byte() {
    let env = Env::new();
    let l = ladder(&env);
    let q5 = env.raise("worker2", "q5", "person:owner", &["label=cache"]);
    env.settle_ok(&q5, "worker2", &format!("director:{}", l.answer));
    let before = env.snapshot_all();
    let first = env.ok(&["decisions", "history", &l.q.identity()]).stdout;
    assert_eq!(
        env.ok(&["decisions", "history", &l.q.identity()]).stdout,
        first
    );
    history(&env, &q5);
    assert_eq!(env.snapshot_all(), before);
}

/// Record an attempt in `session` citing `basis`; its id.
fn attempt(env: &Env, session: &str, what: &str, basis: &[String]) -> String {
    let w = format!("what={what}");
    let mut args = vec![
        "decisions",
        "record",
        session,
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        &w,
        "--subject",
        "result=undecided",
    ];
    for b in basis {
        args.extend_from_slice(&["--basis", b]);
    }
    args.extend_from_slice(&["--by", "session"]);
    env.ok(&args).json()["id"].as_str().unwrap().to_string()
}

/// Raise `question` in `name` to `recipient`, its escalate citing `basis`.
fn raise_naming(env: &Env, name: &str, question: &str, recipient: &str, basis: &[String]) -> Q {
    let qf = format!("question={question}");
    let tf = format!("text=what about {question}?");
    let rf = format!("recipient={recipient}");
    let mut args = vec![
        "decisions",
        "record",
        name,
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        &qf,
        "--subject",
        &tf,
        "--subject",
        &rf,
    ];
    for b in basis {
        args.extend_from_slice(&["--basis", b]);
    }
    args.extend_from_slice(&["--by", "session"]);
    let v = env.ok(&args).json();
    Q {
        session: name.to_string(),
        id: v["id"].as_str().unwrap().to_string(),
        session_id: v["decision"]["session"]["id"].as_str().unwrap().to_string(),
        header: String::new(),
    }
}

#[test]
fn the_attempts_tied_to_the_question_are_listed_and_each_pass_up_names_them() {
    let env = Env::new();
    env.ladder();
    // Made before the raise, named by its escalate: listed ahead of it.
    let a1 = attempt(&env, "worker", "a1", &[]);
    let q = raise_naming(
        &env,
        "worker",
        "q1",
        "session:manager",
        &[format!("worker:{a1}")],
    );
    let cite_q = format!("worker:{}", q.id);
    // Citing the question from another session ties it.
    let a2 = attempt(&env, "manager", "a2", std::slice::from_ref(&cite_q));
    // Neither names the other: not tied.
    let loose = attempt(&env, "worker2", "loose", &[]);
    let up = env.readdress(&q, "manager", "session:director");
    let a3 = attempt(&env, "director", "a3", std::slice::from_ref(&cite_q));
    let doc = history(&env, &q);
    assert_eq!(
        order(&doc),
        vec![
            row("attempt", "worker", &a1),
            row("raise", "worker", &q.id),
            row("attempt", "manager", &a2),
            row("pass-up", "manager", &up),
            row("attempt", "director", &a3),
        ],
        "{doc}"
    );
    assert_eq!(
        entry(&doc, "manager", &up)["attempts"],
        json!([
            {"session": "worker", "id": a1},
            {"session": "manager", "id": a2},
        ])
    );
    let shown = entry(&doc, "worker", &a1);
    assert_eq!(shown["result"], "undecided");
    assert_eq!(shown["what"], "a1");
    assert_eq!(shown["status"], "in-effect");
    assert!(
        !order(&doc).iter().any(|r| r.1 == "worker2" && r.2 == loose),
        "{doc}"
    );

    // An escalate naming another session's attempt doesn't tie it.
    let theirs = attempt(&env, "manager", "theirs", &[]);
    let q2 = raise_naming(
        &env,
        "worker",
        "q2",
        "session:manager",
        &[format!("manager:{theirs}")],
    );
    let doc2 = history(&env, &q2);
    assert_eq!(order(&doc2), vec![row("raise", "worker", &q2.id)], "{doc2}");
}
