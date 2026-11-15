//! The decider's records -- consultations, the agent's answers and directed
//! exits -- as `koto decider report` reads them back from the session logs
//! real `koto next` runs write against the `std::net` stub.
//!
//! docs/designs/current/DESIGN-jev-decision-offload.md, Decision 4. Every opted-in
//! command points `KOTO_DECIDER_ENDPOINT` at the stub and sets
//! `KOTO_DECIDER` and the key explicitly, and every harness sets `HOME` to
//! a temp directory, so no test writes to a developer's real `~/.koto`.

#[path = "support/decider_stub.rs"]
mod decider_stub;

#[path = "support/decider_session.rs"]
mod decider_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::Path;
use std::process::Output;

use decider_session::*;
use decider_stub::Reply;
use serde_json::{json, Value};

fn go() -> Reply {
    verdict(0.95, 0.03, 0.02)
}

fn ready(tpl: &str, replies: Vec<Reply>) -> Harness {
    let h = Harness::new(tpl);
    h.ready();
    for r in replies {
        h.stub.push(r);
    }
    h
}

/// `koto decider report --json` over the harness's store, with `extra`.
fn report_with(h: &Harness, extra: &[&str]) -> Value {
    let out = h
        .koto()
        .args(["decider", "report", "--json"])
        .args(extra)
        .output()
        .unwrap();
    ok(&out);
    json_out(&out)
}

fn report(h: &Harness) -> Value {
    report_with(h, &[])
}

/// What the report's header counts.
fn counts(h: &Harness) -> Value {
    report(h)["header"].clone()
}

/// The report's one question.
fn only_question(r: &Value) -> Value {
    let qs = r["questions"].as_array().unwrap();
    assert_eq!(qs.len(), 1, "{}", r);
    qs[0].clone()
}

/// The session log's header line.
fn header(h: &Harness) -> Value {
    let body = h.raw_log();
    serde_json::from_str(body.lines().next().unwrap()).unwrap()
}

fn ok(out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
}

/// stderr lines that mention a ledger. koto writes none any more.
fn warnings(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter(|l| l.contains("ledger"))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Pairing in one session
// ---------------------------------------------------------------------------

#[test]
fn a_shadow_consultation_and_the_agent_answer_pair_on_session_id_and_visit_seq() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    let schema_before = header(&h)["schema_version"].clone();
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));

    let r = report(&h);
    assert_eq!(r["header"]["consulted"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["answered"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["orphaned_answered"], 0);
    assert_eq!(r["header"]["null_session_id"], 0);
    // The stub is an env endpoint.
    assert_eq!(r["header"]["custom_endpoint_consultations"], 1);

    let event = &h.consultations()[0];
    let q = only_question(&r);
    assert_eq!(q["state"], "review");
    assert_eq!(q["field"], "verdict");
    assert_eq!(
        q["declaration_hash"],
        event["fields"]["verdict"]["declaration_hash"]
    );
    assert_eq!(q["consultations"], 1);
    assert_eq!(q["paired"], 1);

    // The decider chose proceed at threshold and the agent answered exit:
    // the pair joins on the header's session_id and the visit's seq.
    let hdr = header(&h);
    let sid = hdr["session_id"].as_str().unwrap();
    assert!(!sid.is_empty());
    let d = &q["disagreements"][0];
    assert_eq!(d["session"], WF);
    assert_eq!(d["session_id"], sid);
    assert_eq!(d["visit_seq"], event["visit_seq"]);
    assert_eq!(d["agent"], "exit");
    assert_eq!(d["decider"], "proceed");
    assert_eq!(d["endpoint_origin"], "env");

    // The header's schema_version is untouched.
    assert_eq!(hdr["schema_version"], schema_before);
}

#[test]
fn every_outcome_is_read_as_exactly_one_consultation() {
    let unset = standard("auto", "never")
        .replace(
            "- {var: PLAN_DOC, label: plan_path}",
            "- {var: PLAN_DOC, label: plan_path}\n            - {context: notes.md, label: notes}",
        )
        .replace(
            "  rethink:\n    accepts:",
            "  rethink:\n    gates:\n      notes:\n        type: context-exists\n        key: notes.md\n    accepts:",
        );
    let cases: Vec<(&str, String, Option<Reply>)> = vec![
        ("applied", standard("auto", "never"), Some(go())),
        (
            "not_applied",
            standard("auto", "never"),
            Some(verdict(0.6, 0.3, 0.1)),
        ),
        ("input_unavailable", unset, None),
        ("error", standard("auto", "never"), Some(Reply::status(500))),
    ];
    for (outcome, tpl, reply) in cases {
        let h = ready(&tpl, reply.into_iter().collect());
        ok(&h.next_mode("auto"));
        assert_eq!(h.consultations()[0]["outcome"], outcome);
        let r = report(&h);
        assert_eq!(r["header"]["lines"], 1, "{}: {}", outcome, r["header"]);
        assert_eq!(r["header"]["consulted"], 1, "{}", outcome);
        let q = only_question(&r);
        assert_eq!(q["outcomes"][outcome], 1, "{}: {}", outcome, q["outcomes"]);
    }
}

#[test]
fn no_answer_is_read_without_an_unapplied_consultation_or_a_declared_field() {
    // No consultation: the user isn't opted in.
    let h = ready(&standard("shadow", "shadow"), vec![]);
    ok(&h.next_mode("off"));
    ok(&h.next_with("off", r#"{"verdict": "exit"}"#));
    assert_eq!(counts(&h)["lines"], 0);

    // Applied: the decider's evidence moved the session on; the agent's
    // next submission is on another state. It finishes the session, so the
    // log is kept to be read.
    let h = ready(&standard("auto", "never"), vec![go()]);
    ok(&h.next_mode("auto"));
    ok(&h
        .koto_mode("auto")
        .args([
            "next",
            WF,
            "--with-data",
            r#"{"done": true}"#,
            "--no-cleanup",
        ])
        .output()
        .unwrap());
    let c = counts(&h);
    assert_eq!(c["consulted"], 1, "{}", c);
    assert_eq!(c["answered"], 0, "{}", c);

    // Not applied, but the submission holds no declared field.
    let tpl = standard("shadow", "shadow")
        .replace(
            "        required: true\n        description: Is the outline item clear",
            "        required: false\n        description: Is the outline item clear",
        )
        .replace(
            "    transitions:\n      - target: work\n",
            "      note:\n        type: string\n        required: false\n        description: A note\n    transitions:\n      - target: work\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"note": "looked at it"}"#));
    let c = counts(&h);
    assert_eq!(c["answered"], 0, "{}", c);
    assert_eq!(c["consulted"], 1, "{}", c);

    // And with the declared field alongside, only the declared one counts.
    ok(&h.next_with(
        "shadow",
        r#"{"note": "free text zq-sentinel", "verdict": "proceed"}"#,
    ));
    let r = report(&h);
    assert_eq!(r["header"]["answered"], 1, "{}", r["header"]);
    let q = only_question(&r);
    assert_eq!(q["confusion"]["rows"]["proceed"]["proceed"], 1, "{}", q);
    // The sentinel holds non-hex letters, so a session id or a hash can't
    // match it by chance.
    for args in [&["--json"][..], &[][..]] {
        let out = h
            .koto()
            .args(["decider", "report"])
            .args(args)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains("zq-sentinel"));
    }
}

#[test]
fn two_submissions_in_one_visit_are_each_read_as_an_answer() {
    // `proceed` also needs FLAG set, and FLAG is empty, so an agent's
    // `proceed` matches no transition and the session stays on `review`.
    let tpl = standard("shadow", "shadow")
        .replace(
            "variables:\n  PLAN_DOC:",
            "variables:\n  FLAG:\n    description: flag\n    default: \"\"\n  PLAN_DOC:",
        )
        .replace(
            "      - target: work\n        when:\n          verdict: proceed\n",
            "      - target: work\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: true}\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "proceed"}"#));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    assert_eq!(h.stub.request_count(), 1);

    let r = report(&h);
    assert_eq!(r["header"]["consulted"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["answered"], 2, "{}", r["header"]);
    assert_eq!(r["header"]["orphaned_answered"], 0);
    // Both pair with the one consultation, and the later answer wins.
    let q = only_question(&r);
    assert_eq!(q["paired"], 1);
    assert_eq!(q["confusion"]["rows"]["exit"]["proceed"], 1, "{}", q);
    assert_eq!(q["confusion"]["rows"]["proceed"]["proceed"], 0, "{}", q);
}

// ---------------------------------------------------------------------------
// Directed exits (koto#254)
// ---------------------------------------------------------------------------

/// `koto next wf --to <target>`, with no decider environment at all: the
/// record doesn't depend on the decider still being on.
fn directed(h: &Harness, target: &str) -> Output {
    let mut cmd = h.koto();
    cmd.args(["next", WF, "--to", target, "--no-cleanup"]);
    cmd.output().unwrap()
}

/// The directed exits the report lists for its one question.
fn listed_exits(r: &Value) -> Vec<Value> {
    only_question(r)["disagreements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["agent"] == "directed_exit")
        .cloned()
        .collect()
}

#[test]
fn a_directed_exit_from_a_consulted_visit_is_read_against_its_visit_seq() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&directed(&h, "rethink"));

    let r = report(&h);
    assert_eq!(r["header"]["consulted"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["directed_exits"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["orphaned_directed_exits"], 0);
    assert_eq!(r["header"]["answered"], 0);
    let exits = listed_exits(&r);
    assert_eq!(exits.len(), 1, "{:?}", exits);
    let d = &exits[0];
    let sid = header(&h)["session_id"].as_str().unwrap().to_string();
    assert_eq!(d["session_id"], sid.as_str());
    assert_eq!(d["session"], WF);
    assert_eq!(d["visit_seq"], h.consultations()[0]["visit_seq"]);
    assert_eq!(d["directed_to"], "rethink");
    assert_eq!(d["decider"], "proceed");

    // The session log is what it always was: one directed_transition.
    assert_eq!(h.events_of("directed_transition").len(), 1);
}

#[test]
fn a_directed_exit_from_an_unconsulted_visit_reads_as_none() {
    // Not opted in: no consultation, so nothing to pair with.
    let h = ready(&standard("shadow", "shadow"), vec![]);
    ok(&h.next_mode("off"));
    ok(&directed(&h, "rethink"));
    assert_eq!(counts(&h)["lines"], 0);

    // A consultation on `review` doesn't reach an exit from a later state.
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    ok(&directed(&h, "review"));
    let c = counts(&h);
    assert_eq!(c["directed_exits"], 0, "{}", c);
    assert_eq!(c["consulted"], 1, "{}", c);
    assert_eq!(c["answered"], 1, "{}", c);
}

#[test]
fn a_directed_exit_after_an_answer_in_the_same_visit_reads_as_none() {
    // `proceed` also needs FLAG set, and FLAG is empty, so the answer
    // matches no transition and the agent leaves with `--to` instead.
    let tpl = standard("shadow", "shadow")
        .replace(
            "variables:\n  PLAN_DOC:",
            "variables:\n  FLAG:\n    description: flag\n    default: \"\"\n  PLAN_DOC:",
        )
        .replace(
            "      - target: work\n        when:\n          verdict: proceed\n",
            "      - target: work\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: true}\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "proceed"}"#));
    ok(&directed(&h, "work"));
    let c = counts(&h);
    assert_eq!(c["answered"], 1, "{}", c);
    assert_eq!(c["directed_exits"], 0, "{}", c);
}

#[test]
fn only_the_consulted_visit_reads_as_an_exit_not_a_later_one() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&directed(&h, "rethink"));
    // Back into `review` by `--to`, which consults nothing: the new visit
    // has no consultation, so leaving it again has nothing to pair with.
    ok(&directed(&h, "review"));
    ok(&directed(&h, "rethink"));
    let r = report(&h);
    assert_eq!(r["header"]["directed_exits"], 1, "{}", r["header"]);
    let exits = listed_exits(&r);
    assert_eq!(exits.len(), 1, "{:?}", exits);
    assert_eq!(exits[0]["directed_to"], "rethink");
    assert_eq!(h.stub.request_count(), 1);
}

#[test]
fn a_self_loop_directed_transition_is_a_lap_not_an_exit() {
    // `review` gains an edge to itself, making `--to review` from `review` a
    // valid directed transition: it begins a new visit without choosing a
    // route over the decider's.
    let tpl = standard("shadow", "shadow")
        .replace(
            "variables:\n  PLAN_DOC:",
            "variables:\n  FLAG:\n    description: flag\n    default: \"\"\n  PLAN_DOC:",
        )
        .replace(
            "      - target: work\n        when:\n          verdict: proceed\n",
            "      - target: work\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: true}\n      - target: review\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: false}\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    assert_eq!(counts(&h)["consulted"], 1);
    ok(&directed(&h, "review"));
    let c = counts(&h);
    assert_eq!(c["directed_exits"], 0, "{}", c);
    assert_eq!(c["consulted"], 1, "{}", c);
    assert_eq!(h.events_of("directed_transition").len(), 1);
}

#[test]
fn an_answer_with_data_reads_as_no_directed_exit() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    let c = counts(&h);
    assert_eq!(c["consulted"], 1, "{}", c);
    assert_eq!(c["answered"], 1, "{}", c);
    assert_eq!(c["directed_exits"], 0, "{}", c);
    assert_eq!(c["lines"], 2, "{}", c);
}

#[test]
fn the_answer_is_read_after_the_decider_is_switched_off() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    // No KOTO_DECIDER at all on the answering command.
    let mut cmd = h.koto();
    cmd.args(["next", WF, "--with-data", r#"{"verdict": "exit"}"#]);
    ok(&cmd.output().unwrap());
    let c = counts(&h);
    assert_eq!(c["answered"], 1, "{}", c);
    assert_eq!(c["orphaned_answered"], 0, "{}", c);
}

#[test]
fn an_empty_header_session_id_reads_as_none_and_changes_nothing_else() {
    let control = ready(&standard("shadow", "shadow"), vec![go()]);
    let control_first = json_out(&control.next_mode("shadow"));
    let control_second = json_out(&control.next_with("shadow", r#"{"verdict": "exit"}"#));

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    // Blank the header's session_id, as an older log would have it.
    let body = h.raw_log();
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let mut hdr: Value = serde_json::from_str(&lines[0]).unwrap();
    hdr["session_id"] = json!("");
    lines[0] = hdr.to_string();
    std::fs::write(h.state_path(), lines.join("\n") + "\n").unwrap();

    let first = json_out(&h.next_mode("shadow"));
    let second = json_out(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    assert_eq!(first, control_first);
    assert_eq!(second, control_second);

    let mine = h.consultations();
    let theirs = control.consultations();
    assert_eq!(mine.len(), 1);
    for key in ["state", "visit_seq", "outcome", "fields", "input_sha256"] {
        assert_eq!(mine[0][key], theirs[0][key], "{}", key);
    }

    // Both records are read, with no session id, so they never pair.
    let r = report(&h);
    assert_eq!(r["header"]["lines"], 2, "{}", r["header"]);
    assert_eq!(r["header"]["null_session_id"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["orphaned_answered"], 1, "{}", r["header"]);
    assert_eq!(only_question(&r)["paired"], 0);
}

// ---------------------------------------------------------------------------
// Size and content
// ---------------------------------------------------------------------------

/// A template whose one declared field has `n` values, each named with a
/// 60-character prefix, so even a trimmed `consulted` line is over 4 KiB.
fn wide_template(n: usize) -> (String, Vec<String>) {
    let names: Vec<String> = (0..n)
        .map(|i| format!("{}{:03}", "v".repeat(60), i))
        .collect();
    let answers: String = names
        .iter()
        .map(|v| {
            format!(
                "            {}: {{description: \"Value {}.\", mode: shadow}}\n",
                v, v
            )
        })
        .collect();
    let tpl = format!(
        r#"---
name: wide
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
        values: [{values}]
        required: true
        description: Which one?
        decider:
          answers:
{answers}          escape: {{value: unclear, description: "Unknown."}}
          inputs:
            - {{var: PLAN_DOC, label: plan_path}}
    transitions:
      - target: done
        when:
          verdict: {first}
  done:
    terminal: true
---

## review

Pick one.

## done

d
"#,
        values = names.join(", "),
        answers = answers,
        first = names[0],
    );
    (tpl, names)
}

#[test]
fn a_wide_consultation_is_read_whole_from_the_session_with_no_warning() {
    let (tpl, names) = wide_template(80);
    let h = Harness::new(&tpl);
    h.init();
    let mut probs = serde_json::Map::new();
    for (i, n) in names.iter().enumerate() {
        probs.insert(n.clone(), json!(if i == 0 { 0.5 } else { 0.005 }));
    }
    probs.insert("unclear".to_string(), json!(0.105));
    h.stub.push(answers(
        json!({"verdict": {"type": "choice", "probabilities": probs}}),
    ));
    let out = h.next_mode("shadow");
    ok(&out);
    assert_eq!(h.stub.request_count(), 1);
    assert!(warnings(&out).is_empty(), "{}", stderr(&out));

    // The session log carries every probability, and the report reads the
    // consultation with every declared value.
    let c = h.consultations();
    assert_eq!(c.len(), 1);
    let p = c[0]["fields"]["verdict"]["probabilities"]
        .as_object()
        .unwrap();
    assert_eq!(p.len(), names.len() + 1);
    let r = report(&h);
    assert_eq!(r["header"]["consulted"], 1);
    let q = only_question(&r);
    assert_eq!(q["values"].as_array().unwrap().len(), names.len(), "{}", q);
}

#[test]
fn no_input_string_or_key_reaches_the_log_or_the_report() {
    // Context inputs only, so the log's own variables block holds no input.
    let tpl =
        standard("auto", "never").replace("            - {var: PLAN_DOC, label: plan_path}\n", "");
    for reply in [
        verdict(0.6, 0.3, 0.1),
        go(),
        Reply::status(401),
        Reply::status(500),
    ] {
        let h = ready(&tpl, vec![reply]);
        let first = h.next_mode("auto");
        ok(&first);
        // Answer if the decider didn't.
        if json_out(&first)["state"] == "review" {
            ok(&h.next_with("auto", r#"{"verdict": "exit"}"#));
        }
        let req = h.stub.last_request().unwrap();
        let inputs: Vec<String> = req.json()["state"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(inputs.iter().any(|i| i == OUTLINE));

        let log = h.raw_log();
        let report_json = report(&h).to_string();
        let table = h.koto().args(["decider", "report"]).output().unwrap();
        let table = String::from_utf8_lossy(&table.stdout).into_owned();
        for text in inputs.iter().map(String::as_str).chain([KEY]) {
            assert!(!log.contains(text), "session log carries {:?}", text);
            assert!(!report_json.contains(text), "report carries {:?}", text);
            assert!(!table.contains(text), "report table carries {:?}", text);
        }
        assert_eq!(report(&h)["header"]["consulted"], 1);
    }
}

// ---------------------------------------------------------------------------
// Failure tolerance
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// A consulting run with no koto root it can create: nothing is written
/// there, so nothing warns and nothing else changes.
#[cfg(unix)]
#[test]
fn an_uncreatable_koto_dir_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    if running_as_root() {
        return;
    }
    let control = ready(&standard("shadow", "shadow"), vec![go()]);
    let control_out = control.next_mode("shadow");
    ok(&control_out);

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    let home = h.home();
    assert!(!home.join(".koto").exists());
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = h.next_mode("shadow");
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(json_out(&out), json_out(&control_out));
    assert!(warnings(&out).is_empty(), "{}", stderr(&out));
    assert!(!home.join(".koto").exists());
    assert_eq!(h.consultations().len(), 1);
}

// ---------------------------------------------------------------------------
// Children read from below their parent
// ---------------------------------------------------------------------------

const PARENT: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: plan
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
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize results.
"#;

const CHILD: &str = r#"---
name: batch-child
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
        description: Is the task clear?
        decider:
          answers:
            proceed: {description: "Clear.", mode: auto}
            exit: {description: "Vague.", mode: never}
          escape: {value: unclear, description: "Cannot tell."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when:
          verdict: proceed
      - target: work
        when:
          verdict: exit
  work:
    accepts:
      finished:
        type: boolean
        required: true
        description: Is the work finished?
    transitions:
      - target: done
        when:
          finished: true
  done:
    terminal: true
---

## review

Review the task.

## work

Do the task.

## done

Done.
"#;

#[test]
fn child_records_are_read_from_the_child_sessions() {
    let h = Harness::new(PARENT);
    std::fs::write(h.dir.join("child.md"), CHILD).unwrap();
    let run = |mode: &str, args: &[&str]| {
        let mut cmd = h.koto_mode(mode);
        cmd.args(args);
        let out = cmd.output().unwrap();
        ok(&out);
        out
    };
    run("off", &["init", "parent", "--template", "template.md"]);
    let tasks = json!({"tasks": [
        {"name": "A", "waits_on": [], "vars": {}},
        {"name": "B", "waits_on": [], "vars": {}},
    ]});
    run(
        "off",
        &["next", "parent", "--with-data", &tasks.to_string()],
    );
    let sessions = h.dir.join("sessions");
    assert!(sessions.join("parent.A").exists());
    assert!(sessions.join("parent.B").exists());

    // A: shadow, so the agent answers the consulted visit.
    h.stub.push(go());
    run("shadow", &["next", "parent.A"]);
    run(
        "shadow",
        &["next", "parent.A", "--with-data", r#"{"verdict": "exit"}"#],
    );
    // B: auto applies, so the agent never answers the consulted visit.
    h.stub.push(go());
    run("auto", &["next", "parent.B"]);
    assert_eq!(h.stub.request_count(), 2);
    for child in ["parent.A", "parent.B"] {
        run(
            "auto",
            &["next", child, "--with-data", r#"{"finished": true}"#],
        );
    }

    let mut finalize = tasks.clone();
    finalize["finalize"] = json!("yes");
    let out = run(
        "off",
        &["next", "parent", "--with-data", &finalize.to_string()],
    );
    assert_eq!(json_out(&out)["state"], "summarize", "{}", describe(&out));
    // The children have lineage to the parent, so their terminal ticks kept
    // them with its hierarchy, and their records with them.
    for child in ["parent.A", "parent.B"] {
        assert!(sessions.join(child).exists());
    }

    let r = report(&h);
    assert_eq!(r["header"]["consulted"], 2, "{}", r["header"]);
    assert_eq!(r["header"]["answered"], 1, "{}", r["header"]);
    assert_eq!(r["header"]["orphaned_answered"], 0, "{}", r["header"]);
    let q = only_question(&r);
    assert_eq!(q["outcomes"]["applied"], 1, "{}", q);
    assert_eq!(q["outcomes"]["not_applied"], 1, "{}", q);
    // A's answer pairs with A's consultation, under A's own session id.
    assert_eq!(q["paired"], 1);
    let d = &q["disagreements"][0];
    assert_eq!(d["session"], "parent.A");
    assert_eq!(d["agent"], "exit");
    let a_log =
        std::fs::read_to_string(sessions.join("parent.A").join("koto-parent.A.state.jsonl"))
            .unwrap();
    let a_header: Value = serde_json::from_str(a_log.lines().next().unwrap()).unwrap();
    assert_eq!(d["session_id"], a_header["session_id"]);
}

// ---------------------------------------------------------------------------
// Session-feed contract
// ---------------------------------------------------------------------------

#[test]
fn validate_feed_accepts_logs_from_consulting_runs() {
    let spec = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/session-feed.md");
    for (mode, reply) in [
        ("shadow", go()),
        ("auto", go()),
        ("auto", Reply::status(500)),
    ] {
        let h = ready(&standard("auto", "never"), vec![reply]);
        ok(&h.next_mode(mode));
        assert_eq!(h.consultations().len(), 1);
        let out = h
            .koto()
            .args(["template", "validate-feed"])
            .arg(h.state_path())
            .env("KOTO_FEED_SPEC", &spec)
            .output()
            .unwrap();
        assert!(out.status.success(), "{} {}", mode, describe(&out));
    }
}
