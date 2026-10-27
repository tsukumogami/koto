//! `koto next` grading a unit-mode decider check against the `std::net`
//! stub (docs/designs/DESIGN-doc-rule-checks-m4.md, Decisions 1 to 3).
//!
//! A check that declares `units: jsonl` reads one JSON object per line from
//! its extraction and grades each unit's text on its own. These tests run a
//! ten-unit acceptance-criteria fixture through compile, the evaluator, the
//! gate's findings and output, and the response, with the stub failing two
//! of the ten.

#[path = "support/decider_stub.rs"]
mod decider_stub;
#[path = "support/host_session.rs"]
mod host_session;

#[path = "support/decider_session.rs"]
mod decider_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use decider_session::*;
use decider_stub::Reply;
use serde_json::{json, Value};

const GATE: &str = "ac_binary";
const RULE: &str = "ac_binary";

/// The ten units, in print order.
fn keys() -> Vec<String> {
    (1..=10).map(|i| format!("ac-{}", i)).collect()
}

fn unit_text(i: usize) -> String {
    format!(
        "- [ ] UNIT-TEXT-{:02}: the export of batch {} finishes.",
        i, i
    )
}

/// The fixture: one JSON line per unit, with its document and section.
fn fixture() -> String {
    keys()
        .iter()
        .enumerate()
        .map(|(i, k)| {
            json!({"key": k, "document": "docs/prds/PRD-x.md",
                "section": "acceptance-criteria", "text": unit_text(i + 1)})
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// The `review` state: a decider check over `units.jsonl`, `units` set to
/// the given line (empty for none), then `rest`.
fn review_state(units_line: &str, rest: &str) -> String {
    format!(
        r#"  review:
    gates:
      {GATE}:
        type: decider-check
        command: "cat units.jsonl"
        label: ac
{units_line}        criteria:
          {RULE}:
            rule_ref: "https://example.org/rules/ac-binary"
            question: "Can this acceptance criterion be checked pass or fail with no judgment?"
            pass: "It names one observable outcome a tester can check."
            fail: "It needs judgment, or names no observable outcome."
            escape: "It isn't an acceptance criterion."
            mode: veto
{rest}"#
    )
}

const UNITS_LINE: &str = "        units: jsonl\n";
const TO_DONE: &str = "    transitions:\n      - target: done";

fn template_with(states: &str, initial: &str, extra: &str) -> String {
    format!(
        r#"---
name: units
version: "1.0"
initial_state: {initial}
{extra}states:
{states}
  done:
    terminal: true
---

## review

Review the acceptance criteria.

## triage

Triage.

## work

Work.

## done

Done.
"#
    )
}

fn unit_template() -> String {
    template_with(&review_state(UNITS_LINE, TO_DONE), "review", "")
}

fn choice(pass: f64, fail: f64, unclear: f64) -> Reply {
    Reply::json(&json!({
        "model": "jev-test-1.2.3",
        "answers": {RULE: {"type": "choice", "choice": "fail",
            "probabilities": {"pass": pass, "fail": fail, "unclear": unclear}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    }))
}

fn pass() -> Reply {
    choice(0.95, 0.02, 0.03)
}

fn fail() -> Reply {
    choice(0.02, 0.95, 0.03)
}

/// The two units the stub fails.
const FAILING: [&str; 2] = ["ac-3", "ac-7"];

/// One reply per unit, in print order: fail for [`FAILING`], pass otherwise.
fn push_verdicts(h: &Harness) {
    for k in keys() {
        h.stub.push(if FAILING.contains(&k.as_str()) {
            fail()
        } else {
            pass()
        });
    }
}

fn harness(tpl: &str, units: &str) -> Harness {
    let h = Harness::new(tpl);
    std::fs::write(h.dir.join("units.jsonl"), units).unwrap();
    h.init();
    h
}

fn run(h: &Harness) -> Value {
    let mut cmd = h.koto_mode("auto");
    cmd.args(["next", WF, "--no-cleanup"]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    json_out(&out)
}

fn condition(out: &Value) -> Value {
    out["blocking_conditions"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["name"] == GATE))
        .cloned()
        .unwrap_or_else(|| panic!("no {} condition in {}", GATE, out))
}

fn findings(out: &Value) -> Vec<Value> {
    condition(out)["failure"]["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn checks(h: &Harness) -> Vec<Value> {
    h.events_of("decider_checked")
        .into_iter()
        .map(|e| e["payload"].clone())
        .collect()
}

/// The keys of the units findings name, in order.
fn finding_units(f: &[Value]) -> Vec<String> {
    f.iter()
        .filter_map(|f| f["unit"]["key"].as_str().map(String::from))
        .collect()
}

#[test]
fn the_failing_units_are_named_and_each_unit_is_graded_alone() {
    let h = harness(&unit_template(), &fixture());
    push_verdicts(&h);
    let out = run(&h);

    assert_eq!(out["action"], "gate_blocked", "{}", out);
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed", "{}", cond);

    // Two error findings, one per failing unit, and nothing for the rest.
    let f = findings(&out);
    assert_eq!(f.len(), 2, "{}", cond);
    for x in &f {
        assert_eq!(x["level"], "error");
        assert_eq!(x["rule_id"], RULE);
        assert_eq!(x["message_source"], "decider");
        assert_eq!(x["unit"]["document"], "docs/prds/PRD-x.md");
        assert_eq!(x["unit"]["section"], "acceptance-criteria");
    }
    assert_eq!(finding_units(&f), FAILING);
    assert!(f[0]["message"]
        .as_str()
        .unwrap()
        .starts_with("criterion ac_binary failed on unit ac-3: "));

    // output.units: every printed unit, in print order.
    let output = &cond["output"];
    assert_eq!(output["failed"], json!([RULE]));
    let units = output["units"].as_array().expect("units in unit mode");
    assert_eq!(units.len(), 10, "{}", output);
    for (u, k) in units.iter().zip(keys()) {
        assert_eq!(u["key"], k.as_str());
        let failed = if FAILING.contains(&k.as_str()) {
            json!([RULE])
        } else {
            json!([])
        };
        assert_eq!(u["failed"], failed, "{}", u);
        assert_eq!(u["awaiting"], json!([]));
    }

    // Ten requests, one per unit, each holding only its own unit's text.
    let requests = h.stub.requests();
    assert_eq!(requests.len(), 10);
    for (i, r) in requests.iter().enumerate() {
        let body = r.json();
        assert_eq!(body["state"], json!({"ac": unit_text(i + 1)}), "{}", body);
        let raw = r.body_str();
        for j in (1..=10).filter(|j| *j != i + 1) {
            assert!(!raw.contains(&format!("UNIT-TEXT-{:02}", j)), "{}", raw);
        }
    }

    // One record per (unit, criterion), each naming its unit.
    let c = checks(&h);
    assert_eq!(c.len(), 10);
    for (rec, k) in c.iter().zip(keys()) {
        assert_eq!(rec["unit"]["key"], k.as_str());
    }
    let hashes: std::collections::BTreeSet<&str> = c
        .iter()
        .map(|r| r["input_sha256"].as_str().unwrap())
        .collect();
    assert_eq!(hashes.len(), 10, "each unit's input hash is its own");
}

#[test]
fn a_second_next_in_the_visit_reuses_every_unit_verdict() {
    let h = harness(&unit_template(), &fixture());
    push_verdicts(&h);
    run(&h);
    assert_eq!(h.stub.request_count(), 10);
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 10, "no new request");
    assert_eq!(checks(&h).len(), 10, "no new record");
    assert_eq!(condition(&out)["status"], "failed");
    assert_eq!(finding_units(&findings(&out)), FAILING);
}

/// `review` grades the ten units, then moves to `triage`, which asks the
/// routing decider.
fn units_then_routing() -> String {
    let triage = r#"  triage:
    accepts:
      verdict:
        type: enum
        values: [go, stop]
        required: true
        description: Go?
        decider:
          answers:
            go: {description: "Yes.", mode: auto}
            stop: {description: "No.", mode: never}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when: {verdict: go}
      - target: done
        when: {verdict: stop}
  work:
    accepts:
      finished: {type: boolean, required: true, description: finished}
    transitions:
      - target: done
        when: {finished: true}
"#;
    let review = review_state(UNITS_LINE, "    transitions:\n      - target: triage\n");
    template_with(
        &format!("{}{}", review, triage),
        "review",
        "variables:\n  PLAN_DOC:\n    description: plan\n    default: docs/plan.md\n",
    )
}

#[test]
fn the_routing_decider_keeps_its_own_cap_after_ten_unit_requests() {
    let h = harness(&units_then_routing(), &fixture());
    for _ in 0..10 {
        h.stub.push(pass());
    }
    h.stub.push(Reply::json(&json!({
        "model": "m", "answers": {"verdict": {"type": "choice", "choice": "go",
            "probabilities": {"go": 0.97, "stop": 0.02, "unclear": 0.01}}}
    })));
    let out = run(&h);
    assert_eq!(out["state"], "work", "{}", out);
    assert_eq!(
        h.stub.request_count(),
        11,
        "ten units, then the routing call"
    );
    let last = h.stub.last_request().unwrap().json();
    assert!(last["questions"].get("verdict").is_some(), "{}", last);
    assert_eq!(h.consultations().len(), 1);
    assert!(checks(&h)
        .iter()
        .all(|c| c["reason"] != "cap_spent" && c["outcome"] == "pass"));
}

#[test]
fn a_check_without_units_grades_one_slice_as_before() {
    let tpl = template_with(&review_state("", TO_DONE), "review", "");
    let h = harness(&tpl, &fixture());
    h.stub.push(fail());
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 1, "one slice, one request");
    let body = h.stub.last_request().unwrap().json();
    assert_eq!(body["state"]["ac"], fixture());
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed");
    let f = findings(&out);
    assert_eq!(f.len(), 1);
    assert!(f[0].get("unit").is_none(), "{}", f[0]);
    assert!(f[0]["message"]
        .as_str()
        .unwrap()
        .starts_with("criterion ac_binary failed: the decider judged the ac to fail it"));
    let mut keys: Vec<&str> = cond["output"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent_answered",
            "awaiting",
            "error",
            "failed",
            "unanswered"
        ]
    );
    let c = checks(&h);
    assert_eq!(c.len(), 1);
    assert!(c[0].get("unit").is_none(), "{}", c[0]);
}

/// An extraction whose output isn't a set of usable units is an extraction
/// failure: nothing is asked and no unit is named.
fn assert_extraction_failed(units: &str) {
    let h = harness(&unit_template(), units);
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 0, "{}", units);
    let cond = condition(&out);
    assert_eq!(cond["status"], "pending", "{}", cond);
    assert_eq!(cond["output"]["unanswered"], json!([RULE]));
    assert_eq!(cond["output"]["units"], json!([]));
    let f = findings(&out);
    assert_eq!(f.len(), 1, "{}", cond);
    assert!(f[0].get("unit").is_none(), "{}", f[0]);
    assert!(f[0]["message"]
        .as_str()
        .unwrap()
        .contains("(extraction_failed)"));
    let c = checks(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["outcome"], "unanswered");
    assert_eq!(c[0]["reason"], "extraction_failed");
    assert!(c[0].get("unit").is_none());
    for fb in h.events_of("decider_fallback") {
        assert!(fb["payload"].get("unit").is_none(), "{}", fb);
    }
}

#[test]
fn a_key_starting_with_a_dash_fails_the_extraction() {
    let units = fixture().replace("\"ac-5\"", "\"-ac-5\"");
    assert_extraction_failed(&units);
}

#[test]
fn a_repeated_key_fails_the_extraction() {
    let units = fixture().replace("\"ac-5\"", "\"ac-4\"");
    assert_extraction_failed(&units);
}
