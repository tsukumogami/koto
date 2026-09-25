//! `koto next` consulting an opted-in decider, against the `std::net` stub.
//!
//! docs/designs/current/DESIGN-jev-decision-offload.md, Decision 2. Every
//! opted-in command points `KOTO_DECIDER_ENDPOINT` at the stub and sets
//! `KOTO_DECIDER` and the key explicitly, with HOME in a temp dir, so no
//! test can reach a real provider or read a developer's config.

#[path = "support/decider_stub.rs"]
mod decider_stub;

#[path = "support/decider_session.rs"]
mod decider_session;

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use decider_session::*;
use decider_stub::{closed_url, Reply};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

const GO: (f64, f64, f64) = (0.95, 0.03, 0.02);

fn go() -> Reply {
    verdict(GO.0, GO.1, GO.2)
}

/// A harness on `tpl`, initialized and seeded, with `replies` queued.
fn ready(tpl: &str, replies: Vec<Reply>) -> Harness {
    let h = Harness::new(tpl);
    h.ready();
    for r in replies {
        h.stub.push(r);
    }
    h
}

/// What an opted-out user gets on the first tick of `tpl`.
fn opted_out_first_tick(tpl: &str) -> Value {
    let off = ready(tpl, vec![]);
    let out = off.next_mode("off");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(off.stub.request_count(), 0);
    json_out(&out)
}

fn transitions(h: &Harness) -> Vec<Value> {
    h.events_of("transitioned")
        .into_iter()
        .map(|e| e["payload"].clone())
        .collect()
}

fn decider_evidence(h: &Harness) -> Vec<Value> {
    h.events_of("evidence_submitted")
        .into_iter()
        .filter(|e| e["payload"]["source"] == "decider")
        .collect()
}

/// Assert the tick fell back: the response equals the opted-out one, one
/// consultation recorded with `outcome` (and `class`), and nothing applied.
fn assert_fell_back(
    h: &Harness,
    out_json: &Value,
    opted_out: &Value,
    outcome: &str,
    class: Option<&str>,
) {
    assert_eq!(
        out_json, opted_out,
        "response differs from the opted-out one"
    );
    let c = h.consultations();
    assert_eq!(c.len(), 1, "{:?}", c);
    assert_eq!(c[0]["outcome"], outcome, "{}", c[0]);
    match class {
        Some(cl) => assert_eq!(c[0]["error_class"], cl, "{}", c[0]),
        None => assert!(c[0].get("error_class").is_none(), "{}", c[0]),
    }
    assert!(decider_evidence(h).is_empty());
    assert!(!transitions(h).iter().any(|t| t["from"] == "review"));
}

/// `n` declared states `s1..sn` chained on `proceed`, each with `exit` in
/// `never` routing to `rethink`; `sn` routes `proceed` to `last`.
fn chain(n: usize, proceed_mode: &str) -> String {
    let mut s = String::from(
        "---\nname: chain\nversion: \"1.0\"\ninitial_state: s1\nvariables:\n  PLAN_DOC:\n    description: plan\n    default: docs/plan-7c1e.md\nstates:\n",
    );
    for i in 1..=n {
        let next = if i == n {
            "last".to_string()
        } else {
            format!("s{}", i + 1)
        };
        s.push_str(&format!(
            r#"  s{i}:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Step {i} clear?
        decider:
          answers:
            proceed: {{description: "Yes.", mode: {proceed_mode}}}
            exit: {{description: "No.", mode: never}}
          escape: {{value: unclear, description: "Unknown."}}
          inputs:
            - {{var: PLAN_DOC, label: plan_path}}
    transitions:
      - target: {next}
        when:
          verdict: proceed
      - target: rethink
        when:
          verdict: exit
"#
        ));
    }
    s.push_str(
        r#"  rethink:
    accepts:
      again: {type: boolean, required: true, description: again}
    transitions:
      - target: s1
        when: {again: true}
  last:
    accepts:
      done: {type: boolean, required: true, description: done}
    transitions:
      - target: finished
        when: {done: true}
  finished:
    terminal: true
---
"#,
    );
    for i in 1..=n {
        s.push_str(&format!("\n## s{}\n\nStep {}.\n", i, i));
    }
    s.push_str("\n## rethink\n\nr\n\n## last\n\nl\n\n## finished\n\nf\n");
    s
}

// ---------------------------------------------------------------------------
// Opt-in and warnings
// ---------------------------------------------------------------------------

#[test]
fn auto_without_a_key_builds_no_port() {
    let h = ready(&standard("auto", "never"), vec![go()]);
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER", "auto");
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    let out = h.next(cmd);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 0);
    assert!(h.consultations().is_empty());
    assert!(!h.lock_path().exists());
}

#[test]
fn user_mode_off_or_env_off_makes_no_request() {
    // User config off, with a user key and user endpoint.
    let h = ready(&standard("auto", "never"), vec![go()]);
    h.user_config(&format!(
        "[decider]\nmode = \"off\"\napi_key = \"{}\"\nendpoint = \"{}\"\n",
        KEY,
        h.stub.url()
    ));
    let out = h.next(h.koto());
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 0);
    assert!(!h.lock_path().exists());
    assert!(h.consultations().is_empty());

    // KOTO_DECIDER=off with an env key and endpoint.
    let h = ready(&standard("auto", "never"), vec![go()]);
    let out = h.next_mode("off");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 0);
    assert!(!h.lock_path().exists());
    assert!(h.consultations().is_empty());
}

#[test]
fn never_and_bogus_env_modes_warn_and_do_nothing() {
    let off = opted_out_first_tick(&standard("auto", "never"));
    for mode in ["never", "bogus"] {
        let h = ready(&standard("auto", "never"), vec![go()]);
        let out = h.next_mode(mode);
        assert!(out.status.success(), "{}", describe(&out));
        assert_eq!(h.stub.request_count(), 0, "{}", mode);
        assert!(h.consultations().is_empty());
        assert!(!h.lock_path().exists());
        let err = stderr(&out);
        assert!(err.contains("KOTO_DECIDER"), "{}: {}", mode, err);
        assert_eq!(json_out(&out), off, "{}", mode);
    }
}

/// Three declared states that route on a passing gate, so a single tick
/// passes through all three.
fn gate_routed_chain() -> String {
    let mut s = String::from(
        "---\nname: gated\nversion: \"1.0\"\ninitial_state: a\nvariables:\n  PLAN_DOC:\n    description: plan\n    default: docs/plan-7c1e.md\nstates:\n",
    );
    for (name, next) in [("a", "b"), ("b", "c"), ("c", "last")] {
        s.push_str(&format!(
            r#"  {name}:
    gates:
      ci:
        type: command
        command: "true"
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Clear?
        decider:
          answers:
            proceed: {{description: "Yes.", mode: shadow}}
            exit: {{description: "No.", mode: shadow}}
          escape: {{value: unclear, description: "Unknown."}}
          inputs:
            - {{var: PLAN_DOC, label: plan_path}}
    transitions:
      - target: {next}
        when:
          gates.ci.exit_code: 0
      - target: last
        when:
          verdict: exit
          gates.ci.exit_code: 1
"#
        ));
    }
    s.push_str(
        "  last:\n    accepts:\n      done: {type: boolean, required: true, description: done}\n    transitions:\n      - target: finished\n        when: {done: true}\n  finished:\n    terminal: true\n---\n\n## a\n\na\n\n## b\n\nb\n\n## c\n\nc\n\n## last\n\nl\n\n## finished\n\nf\n",
    );
    s
}

#[test]
fn the_warning_is_printed_once_per_invocation() {
    let h = Harness::new(&gate_routed_chain());
    h.init();
    let out = h.next_mode("bogus");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(json_out(&out)["state"], "last");
    assert_eq!(
        transitions(&h)
            .iter()
            .filter(|t| t["from"].is_string())
            .count(),
        3,
        "the tick passed through three declared states"
    );
    assert_eq!(
        stderr(&out).matches("KOTO_DECIDER").count(),
        1,
        "{}",
        stderr(&out)
    );

    // Two separate calls print it once each.
    let h = Harness::new(&gate_routed_chain());
    h.init();
    for _ in 0..2 {
        let out = h.next_mode("bogus");
        assert_eq!(
            stderr(&out).matches("KOTO_DECIDER").count(),
            1,
            "{}",
            stderr(&out)
        );
    }

    // A valid configuration prints no decider warning.
    let h = Harness::new(&gate_routed_chain());
    h.init();
    let out = h.next_mode("shadow");
    let err = stderr(&out);
    assert!(!err.contains("decider"), "{}", err);
    assert!(!err.contains("KOTO_DECIDER"), "{}", err);
}

#[test]
fn project_config_can_only_lower_the_mode() {
    // User auto, project shadow: consulted, not applied.
    let h = ready(&standard("auto", "never"), vec![go()]);
    h.project_config("[decider]\nmode = \"shadow\"\n");
    let out = h.next_mode("auto");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 1);
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "not_applied");
    assert_eq!(c[0]["fields"]["verdict"]["modes"]["proceed"], "shadow");

    // User shadow, project auto: the project can't raise it.
    let h = ready(&standard("auto", "never"), vec![go()]);
    h.project_config("[decider]\nmode = \"auto\"\n");
    let out = h.next_mode("shadow");
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations()[0]["outcome"], "not_applied");
}

#[test]
fn status_makes_no_request() {
    let h = ready(&standard("auto", "never"), vec![]);
    // Reach the declared state with the decider off.
    let out = h.next_mode("off");
    assert_eq!(json_out(&out)["state"], "review");
    let mut cmd = h.koto_mode("auto");
    cmd.args(["status", WF]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 0);
    assert!(h.consultations().is_empty());
}

#[test]
fn an_undeclared_state_makes_no_request() {
    // `work` declares nothing.
    let h = ready(&standard("shadow", "shadow"), vec![]);
    h.next_mode("off");
    let out = h.next_with("off", r#"{"verdict": "proceed"}"#);
    assert_eq!(json_out(&out)["state"], "work");
    let out = h.next_mode("auto");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(json_out(&out)["state"], "work");
    assert_eq!(h.stub.request_count(), 0);
    assert!(h.consultations().is_empty());
}

#[test]
fn all_values_off_makes_no_request() {
    let h = ready(&standard("off", "off"), vec![go()]);
    let out = h.next_mode("auto");
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 0);
    assert!(h.consultations().is_empty());
}

#[test]
fn a_failed_gate_defers_the_consultation_until_it_passes() {
    // `review` carries a legacy pass/block gate on ready.md.
    let tpl = standard("shadow", "shadow").replace(
        "  review:\n    accepts:",
        "  review:\n    gates:\n      ready:\n        type: context-exists\n        key: ready.md\n    accepts:",
    );
    let h = Harness::new(&tpl);
    h.init_with(&["--allow-legacy-gates"]);
    h.context_add("outline.md", OUTLINE);
    h.stub.push(go());
    let out = h.next_mode("shadow");
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 0);
    assert!(h.consultations().is_empty());

    h.context_add("ready.md", "ok");
    let out = h.next_mode("shadow");
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 1);
    let out = h.next_mode("shadow");
    assert!(out.status.success());
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);
}

// ---------------------------------------------------------------------------
// Stickiness and concurrency
// ---------------------------------------------------------------------------

#[test]
fn one_request_per_visit_across_calls() {
    // Answered.
    let h = ready(&standard("shadow", "shadow"), vec![go(), go(), go()]);
    for _ in 0..3 {
        let out = h.next_mode("shadow");
        assert_eq!(json_out(&out)["state"], "review");
    }
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);

    // Timed out.
    let h = ready(
        &standard("shadow", "shadow"),
        vec![go().delay(Duration::from_millis(1500)), go(), go()],
    );
    h.user_config("[decider]\ntimeout_ms = 200\n");
    for _ in 0..3 {
        h.next_mode("shadow");
    }
    assert_eq!(h.stub.request_count(), 1);
    let c = h.consultations();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["error_class"], "timeout");

    // Stopped at input_unavailable: nothing sent, recorded once.
    let tpl = standard("shadow", "shadow").replace(
        "- {context: outline.md, label: outline_item}",
        "- {context: outline.md, label: outline_item, max_bytes: 8}",
    );
    let h = ready(&tpl, vec![go()]);
    for _ in 0..3 {
        h.next_mode("shadow");
    }
    assert_eq!(h.stub.request_count(), 0);
    let c = h.consultations();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["outcome"], "input_unavailable");
}

#[test]
fn a_rewind_and_a_self_loop_each_start_a_new_visit() {
    // Rewind: review -> work (agent) -> rewind -> review.
    let h = ready(&standard("shadow", "shadow"), vec![go(), go()]);
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 1);
    let out = h.next_with("shadow", r#"{"verdict": "proceed"}"#);
    assert_eq!(json_out(&out)["state"], "work");
    let out = h.koto().args(["rewind", WF]).output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let out = h.next_mode("shadow");
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 2);
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 2);
    let c = h.consultations();
    assert_ne!(c[0]["visit_seq"], c[1]["visit_seq"]);

    // Self-loop: `exit` routes review back to review.
    let tpl = standard("shadow", "shadow").replace(
        "      - target: rethink\n        when:\n          verdict: exit",
        "      - target: review\n        when:\n          verdict: exit",
    );
    let h = ready(&tpl, vec![go(), go()]);
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 1);
    let out = h.next_with("shadow", r#"{"verdict": "exit"}"#);
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 2);
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 2);
}

#[test]
fn the_lock_fails_fast_and_the_loser_records_nothing() {
    let h = ready(&standard("shadow", "shadow"), vec![]);
    // Reach review with the decider off, so the visit is open and
    // unconsulted.
    assert_eq!(json_out(&h.next_mode("off"))["state"], "review");
    h.user_config("[decider]\ntimeout_ms = 10000\n");
    h.stub.push(go().delay(Duration::from_secs(3)));

    let mut first = h.koto_mode("shadow");
    first
        .args(["next", WF])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let first = first.spawn().unwrap();
    assert!(
        h.stub.wait_for_requests(1, Duration::from_secs(10)),
        "the first tick never reached the stub"
    );
    let log_before = h.raw_log();

    let started = Instant::now();
    let second = h.next_mode("shadow");
    let elapsed = started.elapsed();
    let count_at_exit = h.stub.request_count();
    assert!(second.status.success(), "{}", describe(&second));
    assert!(
        elapsed < Duration::from_secs(1),
        "the loser waited {:?} for the lock",
        elapsed
    );
    assert_eq!(count_at_exit, 1, "the loser sent a request");
    let resp = json_out(&second);
    assert_eq!(resp["action"], "evidence_required");
    assert_eq!(resp["state"], "review");
    assert_eq!(h.raw_log(), log_before, "the loser appended to the log");

    let first = first.wait_with_output().unwrap();
    assert!(first.status.success(), "{}", describe(&first));
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);

    // A later tick on the same visit makes no request.
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);
}

#[test]
fn a_submission_during_the_call_wins_and_the_answer_is_not_applied() {
    let h = ready(&standard("auto", "auto"), vec![]);
    // Reach review with the decider off, so the visit is open and
    // unconsulted.
    assert_eq!(json_out(&h.next_mode("off"))["state"], "review");
    h.user_config("[decider]\ntimeout_ms = 10000\n");
    // An answer that would apply `proceed`, held until the agent is done.
    h.stub.push(go().hold());

    let mut first = h.koto_mode("auto");
    first
        .args(["next", WF])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let first = first.spawn().unwrap();
    assert!(
        h.stub.wait_for_requests(1, Duration::from_secs(10)),
        "the first tick never reached the stub"
    );

    // The agent submits while the call is in flight.
    let mut agent = h.koto();
    agent.args(["next", WF, "--with-data", r#"{"verdict": "exit"}"#]);
    let agent = agent.output().unwrap();
    assert!(agent.status.success(), "{}", describe(&agent));
    assert_eq!(json_out(&agent)["state"], "rethink");

    h.stub.release();
    let first = first.wait_with_output().unwrap();
    assert!(first.status.success(), "{}", describe(&first));
    assert_eq!(h.stub.request_count(), 1);

    // Counted once, as not_applied, and nothing stacked on the agent's move.
    let c = h.consultations();
    assert_eq!(c.len(), 1, "{:?}", c);
    assert_eq!(c[0]["outcome"], "not_applied");
    let evidence = h.events_of("evidence_submitted");
    assert!(
        evidence.iter().all(|e| e["payload"]["source"] != "decider"),
        "{:?}",
        evidence
    );
    let tail: Vec<(String, String)> = h
        .events()
        .iter()
        .skip_while(|e| e["type"] != "evidence_submitted")
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["payload"]["to"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        tail,
        vec![
            ("evidence_submitted".to_string(), String::new()),
            ("transitioned".to_string(), "rethink".to_string()),
            ("decider_consulted".to_string(), String::new()),
        ],
        "{}",
        h.raw_log()
    );

    // The session is where the agent put it.
    let out = h.next(h.koto());
    assert_eq!(json_out(&out)["state"], "rethink", "{}", describe(&out));
}

const BATCH_PARENT: &str = r#"---
name: batch-consult
version: "1.0"
initial_state: plan
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: [yes]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: review
        when:
          finalize: yes
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Clear?
        decider:
          answers:
            proceed: {description: "Yes.", mode: shadow}
            exit: {description: "No.", mode: shadow}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: done
        when:
          verdict: exit
  done:
    terminal: true
---

## plan

Plan.

## review

Review.

## done

Done.
"#;

const CHILD: &str = r#"---
name: child
version: "1.0"
initial_state: work
states:
  work:
    terminal: true
---

## work

w
"#;

#[test]
fn a_batch_scoped_tick_consults_once_without_a_lock_error() {
    let h = Harness::new(BATCH_PARENT);
    std::fs::write(h.dir.join("child.md"), CHILD).unwrap();
    h.init_with(&["--allow-legacy-gates"]);
    h.stub.push(go());
    let out = h.next_with(
        "shadow",
        r#"{"tasks": [{"name": "t1", "waits_on": []}], "finalize": "yes"}"#,
    );
    assert!(out.status.success(), "{}", describe(&out));
    let resp = json_out(&out);
    assert_eq!(resp["state"], "review", "{}", resp);
    assert!(!stderr(&out).contains("EWOULDBLOCK"));
    assert_ne!(resp["error"]["code"], "concurrent_tick");
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);

    // A declared state can't itself be batch-scoped: materialize_children
    // needs a required `tasks` field, and E-DECIDER-SIBLING-REQUIRED refuses a
    // required undeclared sibling. That the port's lock never collides with
    // the state-file lock is covered at the port level, in
    // `cli::decider_port::tests::consults_while_the_state_file_lock_is_held`.
    let tpl = BATCH_PARENT
        .replace("initial_state: plan", "initial_state: review")
        .replace(
            "  review:\n    accepts:\n",
            "  review:\n    gates:\n      done:\n        type: children-complete\n    materialize_children:\n      from_field: tasks\n      default_template: child.md\n    accepts:\n      tasks:\n        type: tasks\n        required: true\n",
        );
    let h = Harness::new(&tpl);
    let out = h
        .koto()
        .args([
            "init",
            WF,
            "--template",
            "template.md",
            "--allow-legacy-gates",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("E-DECIDER-SIBLING-REQUIRED"));
}

#[test]
fn a_crash_between_the_record_and_the_evidence_is_not_consulted_again() {
    let h = ready(&standard("shadow", "shadow"), vec![go(), go()]);
    h.next_mode("shadow");
    assert_eq!(h.stub.request_count(), 1);
    // Rewrite the record as `applied` with no evidence after it.
    let log = h
        .raw_log()
        .replace("\"outcome\":\"not_applied\"", "\"outcome\":\"applied\"");
    std::fs::write(h.state_path(), log).unwrap();
    assert_eq!(h.consultations()[0]["outcome"], "applied");

    let out = h.next_mode("auto");
    let resp = json_out(&out);
    assert_eq!(resp["action"], "evidence_required");
    assert_eq!(resp["state"], "review");
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations().len(), 1);
}

#[test]
fn the_lock_file_lives_in_the_session_dir_and_goes_with_it() {
    let h = ready(&standard("auto", "never"), vec![go()]);
    h.next_mode("auto");
    assert!(h.lock_path().exists());
    assert_eq!(std::fs::metadata(h.lock_path()).unwrap().len(), 0);
    // Finish the workflow; the terminal tick cleans the session up.
    let out = h.next_with("auto", r#"{"done": true}"#);
    assert_eq!(json_out(&out)["action"], "done");
    assert!(!h.session_dir().exists());
}

// ---------------------------------------------------------------------------
// Evaluation and application
// ---------------------------------------------------------------------------

#[test]
fn shadow_changes_nothing_the_agent_sees() {
    let tpl = standard("auto", "auto");
    for answer in [GO, (0.03, 0.95, 0.02), (0.02, 0.03, 0.95)] {
        let shadow = ready(&tpl, vec![verdict(answer.0, answer.1, answer.2)]);
        let off = ready(&tpl, vec![]);
        let script: [Option<&str>; 3] = [None, Some(r#"{"verdict": "exit"}"#), None];
        for step in script {
            let (a, b) = match step {
                None => (shadow.next_mode("shadow"), off.next_mode("off")),
                Some(d) => (shadow.next_with("shadow", d), off.next_with("off", d)),
            };
            assert_eq!(json_out(&a), json_out(&b), "answer {:?}", answer);
        }
        assert_eq!(transitions(&shadow), transitions(&off));
        assert_eq!(shadow.stub.request_count(), 1);
        assert_eq!(shadow.consultations().len(), 1);
    }
}

#[test]
fn auto_applies_and_advances_with_the_ordinary_response_shape() {
    let h = ready(&standard("auto", "never"), vec![go()]);
    let out = h.next_mode("auto");
    assert!(out.status.success(), "{}", describe(&out));
    let resp = json_out(&out);
    assert_eq!(resp["state"], "work");
    assert_eq!(resp["advanced"], true);
    assert_eq!(resp["action"], "evidence_required");

    // Same keys as the response an agent gets at `work` without a decider.
    let off = ready(&standard("auto", "never"), vec![]);
    off.next_mode("off");
    let off_resp = json_out(&off.next_with("off", r#"{"verdict": "proceed"}"#));
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&resp), keys(&off_resp));

    // The log: consulted (applied), decider evidence, transitioned.
    let events = h.events();
    let tail: Vec<&str> = events
        .iter()
        .skip_while(|e| e["type"] != "decider_consulted")
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        tail,
        vec!["decider_consulted", "evidence_submitted", "transitioned"]
    );
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "applied");
    // The declared state was reached by auto-advance within the tick.
    let before: Vec<Value> = transitions(&h);
    assert_eq!(before[1]["from"], "gather");
    assert_eq!(before[1]["to"], "review");
    let ev = decider_evidence(&h);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["payload"]["fields"], json!({"verdict": "proceed"}));
    let last = transitions(&h).pop().unwrap();
    assert_eq!(last["condition_type"], "auto");
    assert_eq!(last["from"], "review");
    assert_eq!(last["to"], "work");
}

#[test]
fn an_answer_exactly_at_threshold_applies() {
    let h = ready(&standard("auto", "never"), vec![verdict(0.9, 0.05, 0.05)]);
    let out = h.next_mode("auto");
    assert_eq!(json_out(&out)["state"], "work");
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "applied");
    assert_eq!(c[0]["fields"]["verdict"]["at_threshold"], true);
}

const TWO_FIELDS: &str = r#"---
name: two
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Clear?
        decider:
          answers:
            proceed: {description: "Yes.", mode: auto}
            exit: {description: "No.", mode: auto}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
      scope:
        type: enum
        values: [small, large]
        required: true
        description: How big?
        decider:
          answers:
            small: {description: "One file.", mode: auto}
            large: {description: "Many files.", mode: auto}
          escape: {value: unknown, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when:
          verdict: proceed
          scope: small
      - target: rethink
        when:
          verdict: proceed
          scope: large
      - target: rethink
        when:
          verdict: exit
  work:
    accepts:
      done: {type: boolean, required: true, description: d}
    transitions:
      - target: finished
        when: {done: true}
  rethink:
    accepts:
      again: {type: boolean, required: true, description: a}
    transitions:
      - target: review
        when: {again: true}
  finished:
    terminal: true
---

## review

Review.

## work

w

## rethink

r

## finished

f
"#;

#[test]
fn one_field_below_threshold_applies_neither() {
    let reply = answers(json!({
        "verdict": {"type": "choice", "probabilities": {"proceed": 0.95, "exit": 0.03, "unclear": 0.02}},
        "scope": {"type": "choice", "probabilities": {"small": 0.6, "large": 0.3, "unknown": 0.1}},
    }));
    let h = Harness::new(TWO_FIELDS);
    h.init();
    h.stub.push(reply);
    let out = json_out(&h.next_mode("auto"));

    let off = Harness::new(TWO_FIELDS);
    off.init();
    let off_out = json_out(&off.next_mode("off"));
    assert_eq!(out, off_out);

    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "not_applied");
    assert_eq!(c[0]["fields"]["verdict"]["outcome"], "qualified");
    assert_eq!(c[0]["fields"]["scope"]["outcome"], "below_threshold");
    assert!(decider_evidence(&h).is_empty());
}

const BOOLEAN: &str = r#"---
name: bool
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  review:
    accepts:
      ready:
        type: boolean
        required: true
        description: The change is ready.
        decider:
          answers:
            true: {description: "Ready.", mode: auto, threshold: 0.9}
            false: {description: "Not ready.", mode: auto, threshold: 0.9}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when:
          ready: true
      - target: rethink
        when:
          ready: false
  work:
    accepts:
      done: {type: boolean, required: true, description: d}
    transitions:
      - target: finished
        when: {done: true}
  rethink:
    accepts:
      again: {type: boolean, required: true, description: a}
    transitions:
      - target: review
        when: {again: true}
  finished:
    terminal: true
---

## review

Review.

## work

w

## rethink

r

## finished

f
"#;

#[test]
fn boolean_answers_apply_by_side_or_escape() {
    for (p, state, outcome, applied) in [
        (0.95, "work", "qualified", Some(true)),
        (0.05, "rethink", "qualified", Some(false)),
        (0.5, "review", "escape", None),
    ] {
        let h = Harness::new(BOOLEAN);
        h.init();
        h.stub
            .push(answers(json!({"ready": {"type": "noul", "noul": p}})));
        let out = json_out(&h.next_mode("auto"));
        assert_eq!(out["state"], state, "P(true) {}", p);
        let c = h.consultations();
        assert_eq!(c[0]["fields"]["ready"]["outcome"], outcome);
        match applied {
            Some(v) => {
                assert_eq!(c[0]["outcome"], "applied");
                assert_eq!(
                    decider_evidence(&h)[0]["payload"]["fields"]["ready"],
                    json!(v)
                );
            }
            None => {
                assert_eq!(c[0]["outcome"], "not_applied");
                assert!(decider_evidence(&h).is_empty());
            }
        }
    }
}

#[test]
fn a_tie_is_the_escape() {
    let h = ready(&standard("auto", "auto"), vec![verdict(0.45, 0.45, 0.1)]);
    let out = json_out(&h.next_mode("auto"));
    assert_eq!(out["state"], "review");
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "not_applied");
    assert_eq!(c[0]["fields"]["verdict"]["outcome"], "escape");
    assert!(c[0]["fields"]["verdict"].get("winning").is_none());
}

#[test]
fn an_answer_matching_no_transition_is_not_applied() {
    // `proceed` also needs FLAG set, and FLAG is empty.
    let tpl = standard("auto", "never")
        .replace(
            "variables:\n  PLAN_DOC:",
            "variables:\n  FLAG:\n    description: flag\n    default: \"\"\n  PLAN_DOC:",
        )
        .replace(
            "      - target: work\n        when:\n          verdict: proceed\n",
            "      - target: work\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: true}\n",
        );
    let opted_out = opted_out_first_tick(&tpl);
    let h = ready(&tpl, vec![go()]);
    let out = json_out(&h.next_mode("auto"));
    assert_fell_back(&h, &out, &opted_out, "not_applied", None);
}

// -- the runtime floor ------------------------------------------------------

const FLOOR_BASE: &str = r#"---
name: floor
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  review:
GATES    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Is the item clear?
        decider:
          answers:
            proceed: {description: "Clear.", mode: auto}
            exit: {description: "Vague.", mode: never}
          escape: {value: unclear, description: "Cannot tell."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
ROUTES
  work:
    accepts:
      done: {type: boolean, required: true, description: d}
    transitions:
      - target: finished
        when: {done: true}
  rethink:
    accepts:
      again: {type: boolean, required: true, description: a}
    transitions:
      - target: review
        when: {again: true}
  guarded:
    default_action:
      command: "echo ok"
      requires_confirmation: true
    transitions:
      - target: finished
  finished:
    terminal: true
---

## review

Review.

## work

w

## rethink

r

## guarded

g

## finished

f
"#;

/// The gate-conditioned fixture: `proceed` reaches `work` only through
/// `evidence.verdict: present` plus a `gates.*` key.
fn floor_gate() -> String {
    FLOOR_BASE
        .replace(
            "GATES",
            "    gates:\n      ci:\n        type: command\n        command: \"true\"\n",
        )
        .replace(
            "ROUTES",
            "      - target: work\n        when: {evidence.verdict: present, gates.ci.exit_code: 0}\n      - target: rethink\n        when: {verdict: exit, gates.ci.exit_code: 1}",
        )
}

/// The same `evidence.verdict: present` shape, without the gate key, to
/// `target`.
fn floor_via_vars(target: &str) -> String {
    FLOOR_BASE.replace("GATES", "").replace(
        "ROUTES",
        &format!(
            "      - target: {}\n        when: {{evidence.verdict: present, vars.PLAN_DOC: {{is_set: true}}}}\n      - target: rethink\n        when: {{verdict: exit, vars.PLAN_DOC: {{is_set: false}}}}",
            target
        ),
    )
}

fn compiles(h: &Harness) {
    let out = h
        .koto()
        .args(["template", "compile", "template.md"])
        .output()
        .unwrap();
    assert!(out.status.success(), "compile: {}", describe(&out));
    assert!(!stderr(&out).contains("E-DECIDER-FLOOR"));
}

fn check_floor_blocks(tpl: &str, gate_output: Option<Value>, target: &str) {
    let h = Harness::new(tpl);
    compiles(&h);

    // Exactly one conditional transition matches the answer.
    let compiled = compile_lib(tpl);
    let mut ev = json!({"verdict": "proceed"});
    if let Some(g) = gate_output {
        ev["gates"] = g;
    }
    let vars: HashMap<String, String> = [("PLAN_DOC".to_string(), PLAN_DOC.to_string())].into();
    assert_eq!(
        koto::engine::advance::conditional_matches(&compiled.states["review"], &ev, &vars),
        vec![target.to_string()]
    );

    h.init();
    h.stub.push(verdict(0.99, 0.005, 0.005));
    let out = json_out(&h.next_mode("auto"));

    let off = Harness::new(tpl);
    off.init();
    let off_out = json_out(&off.next_mode("off"));
    assert_eq!(out, off_out);

    let c = h.consultations();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["outcome"], "not_applied");
    assert_eq!(c[0]["fields"]["verdict"]["outcome"], "qualified");
    assert!(decider_evidence(&h).is_empty());
    assert!(!transitions(&h).iter().any(|t| t["from"] == "review"));
}

fn compile_lib(src: &str) -> koto::template::types::CompiledTemplate {
    let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    std::io::Write::write_all(&mut f, src.as_bytes()).unwrap();
    koto::template::compile::compile(f.path(), true).unwrap()
}

#[test]
fn the_runtime_floor_blocks_a_gate_conditioned_route() {
    check_floor_blocks(
        &floor_gate(),
        Some(json!({"ci": {"exit_code": 0, "error": ""}})),
        "work",
    );
}

#[test]
fn the_runtime_floor_blocks_terminal_and_confirmation_targets() {
    check_floor_blocks(&floor_via_vars("finished"), None, "finished");
    check_floor_blocks(&floor_via_vars("guarded"), None, "guarded");
}

#[test]
fn the_floor_control_applies() {
    let tpl = floor_via_vars("work");
    let h = Harness::new(&tpl);
    compiles(&h);
    h.init();
    h.stub.push(verdict(0.99, 0.005, 0.005));
    let out = json_out(&h.next_mode("auto"));
    assert_eq!(out["state"], "work");
    assert_eq!(h.consultations()[0]["outcome"], "applied");
    assert_eq!(decider_evidence(&h).len(), 1);
}

// -- failures ---------------------------------------------------------------

#[test]
fn every_failure_kind_falls_back_to_the_opted_out_response() {
    let tpl = standard("auto", "never");
    let opted_out = opted_out_first_tick(&tpl);

    struct Case {
        name: &'static str,
        reply: Option<Reply>,
        endpoint: Option<String>,
        timeout_ms: Option<u64>,
        outcome: &'static str,
        class: Option<&'static str>,
    }
    let cases = vec![
        Case {
            name: "timeout",
            reply: Some(go().delay(Duration::from_millis(1500))),
            endpoint: None,
            timeout_ms: Some(200),
            outcome: "error",
            class: Some("timeout"),
        },
        Case {
            name: "refused",
            reply: None,
            endpoint: Some(closed_url()),
            timeout_ms: None,
            outcome: "error",
            class: Some("connect"),
        },
        Case {
            name: "http 500",
            reply: Some(Reply::status(500)),
            endpoint: None,
            timeout_ms: None,
            outcome: "error",
            class: Some("http_status"),
        },
        Case {
            name: "http 401",
            reply: Some(Reply::status(401)),
            endpoint: None,
            timeout_ms: None,
            outcome: "error",
            class: Some("http_status"),
        },
        Case {
            name: "malformed",
            reply: Some(Reply::raw(200, "not json at all")),
            endpoint: None,
            timeout_ms: None,
            outcome: "error",
            class: Some("malformed"),
        },
        Case {
            name: "undeclared value",
            reply: Some(answers(json!({"verdict": {"type": "choice",
                "probabilities": {"proceed": 0.9, "exit": 0.05, "merge": 0.05}}}))),
            endpoint: None,
            timeout_ms: None,
            outcome: "error",
            class: Some("mismatched"),
        },
        Case {
            name: "below threshold",
            reply: Some(verdict(0.6, 0.3, 0.1)),
            endpoint: None,
            timeout_ms: None,
            outcome: "not_applied",
            class: None,
        },
        Case {
            name: "escape",
            reply: Some(verdict(0.02, 0.03, 0.95)),
            endpoint: None,
            timeout_ms: None,
            outcome: "not_applied",
            class: None,
        },
        Case {
            name: "never",
            reply: Some(verdict(0.03, 0.95, 0.02)),
            endpoint: None,
            timeout_ms: None,
            outcome: "not_applied",
            class: None,
        },
    ];
    for case in cases {
        let h = ready(&tpl, case.reply.into_iter().collect());
        if let Some(ms) = case.timeout_ms {
            h.user_config(&format!("[decider]\ntimeout_ms = {}\n", ms));
        }
        let mut cmd = h.koto_mode("auto");
        if let Some(e) = &case.endpoint {
            cmd.env("KOTO_DECIDER_ENDPOINT", e);
        }
        let out = h.next(cmd);
        assert!(out.status.success(), "{}: {}", case.name, describe(&out));
        let resp = json_out(&out);
        assert_eq!(resp, opted_out, "{}", case.name);
        let c = h.consultations();
        assert_eq!(c.len(), 1, "{}", case.name);
        assert_eq!(c[0]["outcome"], case.outcome, "{}: {}", case.name, c[0]);
        match case.class {
            Some(cl) => assert_eq!(c[0]["error_class"], cl, "{}", case.name),
            None => assert!(c[0].get("error_class").is_none(), "{}", case.name),
        }
        assert!(decider_evidence(&h).is_empty(), "{}", case.name);
    }
}

#[test]
fn input_problems_fall_back_without_a_request() {
    // An unset context key: `notes.md` is gated only in an unreached state.
    let unset = standard("auto", "never")
        .replace(
            "- {var: PLAN_DOC, label: plan_path}",
            "- {var: PLAN_DOC, label: plan_path}\n            - {context: notes.md, label: notes}",
        )
        .replace(
            "  rethink:\n    accepts:",
            "  rethink:\n    gates:\n      notes:\n        type: context-exists\n        key: notes.md\n    accepts:",
        )
        .replace(
            "      - target: review\n        when:\n          again: true\n",
            "      - target: review\n        when:\n          again: true\n          gates.notes.exists: true\n",
        )
        .replace(
            "      - target: finished\n        when:\n          again: false\n",
            "      - target: finished\n        when:\n          again: false\n          gates.notes.exists: true\n",
        );
    // An input over its byte budget.
    let over = standard("auto", "never").replace(
        "- {context: outline.md, label: outline_item}",
        "- {context: outline.md, label: outline_item, max_bytes: 16}",
    );
    for (name, tpl) in [("unset", unset), ("over budget", over)] {
        let opted_out = opted_out_first_tick(&tpl);
        let h = ready(&tpl, vec![go()]);
        let out = json_out(&h.next_mode("auto"));
        assert_eq!(h.stub.request_count(), 0, "{}", name);
        assert_fell_back(&h, &out, &opted_out, "input_unavailable", None);
        assert!(h.consultations()[0].get("input_sha256").is_none());
    }
}

#[test]
fn the_default_timeout_is_two_seconds() {
    let h = ready(
        &standard("auto", "never"),
        vec![go().delay(Duration::from_millis(2500))],
    );
    let out = h.next_mode("auto");
    assert_eq!(json_out(&out)["state"], "review");
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "error");
    assert_eq!(c[0]["error_class"], "timeout");
}

#[test]
fn a_short_timeout_bounds_the_whole_tick() {
    let h = ready(
        &standard("auto", "never"),
        vec![go().delay(Duration::from_secs(10))],
    );
    h.user_config("[decider]\ntimeout_ms = 200\n");
    let started = Instant::now();
    let out = h.next_mode("auto");
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(2), "took {:?}", elapsed);
    assert_eq!(json_out(&out)["state"], "review");
    let c = h.consultations();
    assert_eq!(c[0]["outcome"], "error");
    assert_eq!(c[0]["error_class"], "timeout");
    let latency = c[0]["latency_ms"].as_u64().unwrap();
    assert!(latency <= 450, "latency_ms {}", latency);
}

// ---------------------------------------------------------------------------
// The cap
// ---------------------------------------------------------------------------

#[test]
fn five_chained_states_make_at_most_four_requests() {
    let h = Harness::new(&chain(5, "auto"));
    h.init();
    for _ in 0..5 {
        h.stub.push(go());
    }
    let out = json_out(&h.next_mode("auto"));
    assert_eq!(h.stub.request_count(), 4);
    assert_eq!(out["action"], "evidence_required");
    assert_eq!(out["state"], "s5");
    assert_eq!(h.consultations().len(), 4);
}

#[test]
fn an_answer_routing_to_a_state_visited_this_call_is_not_applied() {
    // s1 -> s2 -> s3, and s3's `proceed` leads back to s2.
    let tpl = chain(3, "auto").replacen("      - target: last\n", "      - target: s2\n", 1);
    let h = Harness::new(&tpl);
    h.init();
    for _ in 0..3 {
        h.stub.push(go());
    }
    let out = json_out(&h.next_mode("auto"));
    assert_eq!(out["state"], "s3");
    assert_eq!(out["action"], "evidence_required");
    let outcomes: Vec<Value> = h
        .consultations()
        .iter()
        .map(|c| c["outcome"].clone())
        .collect();
    assert_eq!(
        outcomes,
        vec![json!("applied"), json!("applied"), json!("not_applied")]
    );
}

// ---------------------------------------------------------------------------
// Input assembly and the payload
// ---------------------------------------------------------------------------

const ACTION_INPUTS: &str = r#"---
name: inputs
version: "1.0"
initial_state: gather
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  gather:
    default_action:
      command: "printf 'ACTION-OUTLINE-91b2' | koto context add {{SESSION_NAME}} outline.md"
    gates:
      outline:
        type: context-exists
        key: outline.md
    transitions:
      - target: measure
        when:
          gates.outline.exists: true
  measure:
    default_action:
      command: "echo CAPTURED-5e21"
      capture_stdout_as: CHANGED
    transitions:
      - target: review
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Clear?
        decider:
          answers:
            proceed: {description: "Yes.", mode: shadow}
            exit: {description: "No.", mode: shadow}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {context: outline.md, label: outline_item}
            - {var: PLAN_DOC, label: plan_path}
            - {var: CHANGED, label: changed}
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: done
        when:
          verdict: exit
  done:
    terminal: true
---

## gather

g

## measure

m

## review

r

## done

d
"#;

#[test]
fn context_variables_and_captures_reach_the_payload() {
    let h = Harness::new(ACTION_INPUTS);
    h.init();
    h.stub.push(go());
    let out = h.next_mode("shadow");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(json_out(&out)["state"], "review");
    assert_eq!(h.stub.request_count(), 1);
    let body = h.stub.last_request().unwrap().json();
    assert_eq!(
        body["state"],
        json!({
            "outline_item": "ACTION-OUTLINE-91b2",
            "plan_path": PLAN_DOC,
            "changed": "CAPTURED-5e21",
        })
    );
}

#[test]
fn the_default_budget_is_8192_bytes() {
    for (size, requests, outcome) in [(8192, 1, "not_applied"), (8193, 0, "input_unavailable")] {
        let h = Harness::new(&standard("shadow", "shadow"));
        h.init();
        h.context_add("outline.md", &"x".repeat(size));
        h.stub.push(go());
        h.next_mode("shadow");
        assert_eq!(h.stub.request_count(), requests, "{} bytes", size);
        assert_eq!(h.consultations()[0]["outcome"], outcome, "{} bytes", size);
    }
}

#[test]
fn the_payload_holds_only_the_question_and_its_inputs() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    // Another context key and a template variable that aren't declared as
    // inputs must not travel.
    h.context_add("secret-notes.md", "UNDECLARED-CONTEXT-33aa");
    h.next_mode("shadow");
    let req = h.stub.last_request().unwrap();
    assert_eq!(
        req.json(),
        json!({
            "model": "jev-latest",
            "state": {"outline_item": OUTLINE, "plan_path": PLAN_DOC},
            "questions": {"verdict": {
                "type": "choice",
                "instructions": "Is the outline item clear enough to implement?",
                "criteria": {
                    "proceed": "Names a concrete change with checkable criteria.",
                    "exit": "Vague, contradictory, or needs design first.",
                    "unclear": "Missing, truncated, or unjudgeable."
                }
            }}
        })
    );
    let body = req.body_str();
    for absent in [
        "UNDECLARED-CONTEXT-33aa",
        "\"wf\"",
        "Review the outline item for",
        KEY,
    ] {
        assert!(!body.contains(absent), "payload carries {:?}", absent);
    }
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {}", KEY).as_str())
    );
}

#[test]
fn identical_inputs_hash_identically() {
    let a = ready(&standard("shadow", "shadow"), vec![go()]);
    a.next_mode("shadow");
    let b = ready(&standard("shadow", "shadow"), vec![go()]);
    b.next_mode("shadow");
    let ha = a.consultations()[0]["input_sha256"].clone();
    let hb = b.consultations()[0]["input_sha256"].clone();
    assert!(ha.as_str().unwrap().len() == 64);
    assert_eq!(ha, hb);

    let c = Harness::new(&standard("shadow", "shadow"));
    c.init();
    c.context_add("outline.md", "a different outline");
    c.stub.push(go());
    c.next_mode("shadow");
    assert_ne!(c.consultations()[0]["input_sha256"], ha);
}

// ---------------------------------------------------------------------------
// Recorded metadata
// ---------------------------------------------------------------------------

#[test]
fn directive_bytes_match_what_the_agent_would_have_received() {
    let tpl = standard("shadow", "shadow").replace(
        "Review the outline item for {{PLAN_DOC}} and decide whether it is clear.\n",
        "Review the outline item for {{PLAN_DOC}} and decide whether it is clear.\n\n<!-- details -->\n\nRead {{PLAN_DOC}} first, then compare it with the outline.\n",
    );
    let off = ready(&tpl, vec![]);
    let resp = json_out(&off.next_mode("off"));
    let details = resp["details"]
        .as_str()
        .expect("first delivery carries details");
    let expected = resp["directive"].as_str().unwrap().len() + details.len();

    let h = ready(&tpl, vec![go()]);
    let out = json_out(&h.next_mode("shadow"));
    assert_eq!(out, resp);
    assert_eq!(
        h.consultations()[0]["directive_bytes"].as_u64().unwrap() as usize,
        expected
    );
}

#[test]
fn endpoint_origin_names_where_the_endpoint_came_from() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    h.next_mode("shadow");
    assert_eq!(h.consultations()[0]["endpoint_origin"], "env");

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    h.user_config(&format!(
        "[decider]\nmode = \"shadow\"\napi_key = \"{}\"\nendpoint = \"{}\"\n",
        KEY,
        h.stub.url()
    ));
    let out = h.next(h.koto());
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 1);
    assert_eq!(h.consultations()[0]["endpoint_origin"], "user");
}

#[test]
fn provider_and_model_are_recorded() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    h.next_mode("shadow");
    let c = &h.consultations()[0];
    assert_eq!(c["provider"], "jev");
    assert_eq!(c["model"], "jev-test-1.2.3");

    let h = ready(
        &standard("shadow", "shadow"),
        vec![Reply::json(
            &json!({"answers": {"verdict": {"type": "choice",
            "probabilities": {"proceed": 0.95, "exit": 0.03, "unclear": 0.02}}}}),
        )],
    );
    h.next_mode("shadow");
    assert_eq!(h.consultations()[0]["model"], "unknown");
}

#[test]
fn the_record_holds_no_input_key_or_response_text() {
    const RESPONSE_MARKER: &str = "RESPONSE-TEXT-0b9d";
    for reply in [
        Reply::json(
            &json!({"model": "jev-test-1.2.3", "answers": {"verdict": {"type": "choice",
            "probabilities": {"proceed": 0.95, "exit": 0.03, "unclear": 0.02},
            "rationale": RESPONSE_MARKER}}, "note": RESPONSE_MARKER}),
        ),
        Reply::raw(500, RESPONSE_MARKER),
        Reply::raw(200, RESPONSE_MARKER),
    ] {
        let h = ready(&standard("auto", "never"), vec![reply]);
        h.next_mode("auto");
        let c = h.consultations();
        assert_eq!(c.len(), 1);
        let text = c[0].to_string();
        for secret in [
            OUTLINE,
            PLAN_DOC,
            KEY,
            RESPONSE_MARKER,
            "HTTP 500",
            "not JSON",
        ] {
            assert!(
                !text.contains(secret),
                "record carries {:?}: {}",
                secret,
                text
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Evidence marking
// ---------------------------------------------------------------------------

#[test]
fn with_data_cannot_set_source() {
    // An undeclared `source` key is refused by accepts validation.
    let h = ready(&standard("shadow", "shadow"), vec![]);
    h.next_mode("off");
    let out = h.next_with("off", r#"{"source": "decider", "verdict": "proceed"}"#);
    assert!(!out.status.success());
    assert!(h
        .events_of("evidence_submitted")
        .iter()
        .all(|e| e["payload"].get("source").is_none()));

    // A declared `source` field lands in `fields`, never on the event.
    let tpl = standard("shadow", "shadow").replace(
        "    transitions:\n      - target: work\n",
        "      source:\n        type: string\n        required: false\n        description: where\n    transitions:\n      - target: work\n",
    );
    let h = ready(&tpl, vec![]);
    h.next_mode("off");
    let out = h.next_with("off", r#"{"source": "decider", "verdict": "proceed"}"#);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = h.events_of("evidence_submitted");
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["payload"]["fields"]["source"], "decider");
    assert!(ev[0]["payload"].get("source").is_none());
}

#[test]
fn validate_feed_accepts_a_log_with_decider_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let log = produce_applied_session_log(tmp.path());
    let spec =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/session-feed.md");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_koto"))
        .args(["template", "validate-feed"])
        .arg(&log)
        .env("KOTO_FEED_SPEC", &spec)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
}

/// The fixture a v0.12.2 compatibility check reads. When
/// `KOTO_DECIDER_COMPAT_LOG` is set, the produced log is copied there.
#[test]
fn produces_the_compatibility_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    let log = produce_applied_session_log(tmp.path());
    let body = std::fs::read_to_string(&log).unwrap();
    let types: Vec<String> = body
        .lines()
        .skip(1)
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let i = types.iter().position(|t| t == "decider_consulted").unwrap();
    assert_eq!(types[i + 1], "evidence_submitted");
    assert_eq!(types[i + 2], "transitioned");
    if let Ok(dest) = std::env::var("KOTO_DECIDER_COMPAT_LOG") {
        std::fs::copy(&log, dest).unwrap();
    }
}

// ---------------------------------------------------------------------------
// A key that can't be an HTTP header
// ---------------------------------------------------------------------------

#[test]
fn a_key_with_a_control_character_does_not_opt_in_or_panic() {
    const SECRET: &str = "sk-CTRL-SECRET-5d1e";
    let bad = [
        format!("{SECRET}\nX"),
        format!("{SECRET}\rX"),
        format!("{SECRET}\u{1}X"),
    ];
    for raw in &bad {
        // From the environment.
        let h = ready(&standard("auto", "auto"), vec![go()]);
        let mut cmd = h.koto_mode("auto");
        cmd.env("KOTO_DECIDER_API_KEY", raw);
        let out = h.next(cmd);
        let err = stderr(&out);
        assert!(out.status.success(), "{:?}: {}", raw, describe(&out));
        assert!(!err.contains("panicked"), "{:?}: {}", raw, err);
        assert!(!err.contains(SECRET), "{:?}: {}", raw, err);
        assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
        assert!(
            err.contains("KOTO_DECIDER_API_KEY: the decider API key contains a control"),
            "{:?}: {}",
            raw,
            err
        );
        assert_eq!(json_out(&out)["state"], "review", "{:?}", raw);
        assert_eq!(h.stub.request_count(), 0, "{:?}", raw);
        assert!(h.consultations().is_empty(), "{:?}", raw);
        assert!(!h.raw_log().contains(SECRET));

        // From user config, with the endpoint from the same layer.
        let h = ready(&standard("auto", "auto"), vec![go()]);
        let escaped = raw
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\u{1}', "\\u0001");
        h.user_config(&format!(
            "[decider]\nmode = \"auto\"\napi_key = \"{}\"\nendpoint = \"{}\"\n",
            escaped,
            h.stub.url()
        ));
        let out = h.next(h.koto());
        let err = stderr(&out);
        assert!(out.status.success(), "{:?}: {}", raw, describe(&out));
        assert!(
            !err.contains("panicked") && !err.contains(SECRET),
            "{}",
            err
        );
        assert!(err.contains("user config: the decider API key"), "{}", err);
        assert_eq!(h.stub.request_count(), 0, "{:?}", raw);
        assert!(h.consultations().is_empty(), "{:?}", raw);
    }
}
