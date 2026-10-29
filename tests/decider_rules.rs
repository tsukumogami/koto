//! A failed decider-check criterion hands its rule back in `koto next`
//! (docs/designs/DESIGN-doc-rule-checks-m4.md, Decision 4).
//!
//! A criterion may declare a `summary` and a `rule_text`. When a veto
//! criterion fails, its blocking condition carries a `rules` entry: the
//! summary on the first failing evaluation of that check and rule in the
//! visit, the full text on every one after it, and a `rule_delivered` event
//! is appended once the response has printed. These tests drive the stub
//! decider through compile, evaluation, response assembly and the log, and
//! check that neither text reaches the log.

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

/// Texts carrying a sentinel, so a test can say they reach no record.
const SUMMARY: &str = "SENTINEL-SUM-5b1d: acceptance criteria must be binary.";
const FULL: &str =
    "SENTINEL-FULL-8e2a: an acceptance criterion states one observable result a tester can check.";

/// One criterion's YAML under `criteria`, with the texts it declares.
fn criterion(id: &str, mode: &str, summary: Option<&str>, rule_text: Option<&str>) -> String {
    let mut out = format!(
        r#"          {id}:
            rule_ref: "https://example.org/rules/{id}"
            question: "Can this acceptance criterion be checked pass or fail with no judgment ({id})?"
            pass: "It names one observable outcome a tester can check."
            fail: "It needs judgment, or names no observable outcome."
            escape: "It isn't an acceptance criterion."
            mode: {mode}
"#
    );
    if let Some(s) = summary {
        out.push_str(&format!("            summary: {:?}\n", s));
    }
    if let Some(t) = rule_text {
        out.push_str(&format!("            rule_text: {:?}\n", t));
    }
    out
}

fn both(id: &str) -> String {
    criterion(id, "veto", Some(SUMMARY), Some(FULL))
}

/// `review` checks `units.jsonl` (as units when `units` is set) and moves
/// to `work`; `work` goes back to `review` or on to `done` on evidence.
fn template(units: bool, criteria: &str) -> String {
    let units_line = if units { "        units: jsonl\n" } else { "" };
    format!(
        r#"---
name: rules
version: "1.0"
initial_state: review
states:
  review:
    gates:
      {GATE}:
        type: decider-check
        command: "cat units.jsonl"
        label: ac
{units_line}        criteria:
{criteria}    transitions:
      - target: work
  work:
    accepts:
      back: {{type: boolean, required: true, description: back to review}}
    transitions:
      - target: review
        when: {{back: true}}
      - target: done
        when: {{back: false}}
  done:
    terminal: true
---

## review

Review the acceptance criteria.

## work

Work.

## done

Done.
"#
    )
}

/// `n` units, keyed `ac-1` to `ac-n`, one JSON line each.
fn fixture(n: usize) -> String {
    (1..=n)
        .map(|i| {
            json!({"key": format!("ac-{}", i), "text": format!("- [ ] Batch {} exports.", i)})
                .to_string()
                + "\n"
        })
        .collect()
}

fn choice(rule_id: &str, pass: f64, fail: f64) -> Reply {
    Reply::json(&json!({
        "model": "jev-test-1.2.3",
        "answers": {rule_id: {"type": "choice", "choice": "fail",
            "probabilities": {"pass": pass, "fail": fail, "unclear": 0.03}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    }))
}

fn fail(rule_id: &str) -> Reply {
    choice(rule_id, 0.02, 0.95)
}

fn pass(rule_id: &str) -> Reply {
    choice(rule_id, 0.95, 0.02)
}

/// The two of ten units the stub fails.
const FAILING: [&str; 2] = ["ac-3", "ac-7"];

/// One reply per unit of the ten-unit fixture, in print order.
fn push_ten(h: &Harness) {
    for i in 1..=10 {
        let key = format!("ac-{}", i);
        h.stub.push(if FAILING.contains(&key.as_str()) {
            fail(RULE)
        } else {
            pass(RULE)
        });
    }
}

fn harness(tpl: &str, units: &str) -> Harness {
    let h = Harness::new(tpl);
    std::fs::write(h.dir.join("units.jsonl"), units).unwrap();
    h.init();
    h
}

fn next(h: &Harness, extra: &[&str]) -> Value {
    let mut cmd = h.koto_mode("auto");
    cmd.args(["next", WF, "--no-cleanup"]);
    cmd.args(extra);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    json_out(&out)
}

fn run(h: &Harness) -> Value {
    next(h, &[])
}

fn condition(out: &Value) -> Value {
    out["blocking_conditions"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["name"] == GATE))
        .cloned()
        .unwrap_or_else(|| panic!("no {} condition in {}", GATE, out))
}

fn rules(out: &Value) -> Value {
    condition(out)["rules"].clone()
}

fn deliveries(h: &Harness) -> Vec<Value> {
    h.events_of("rule_delivered")
        .into_iter()
        .map(|e| e["payload"].clone())
        .collect()
}

fn assert_no_text_logged(h: &Harness) {
    let log = h.raw_log();
    assert!(!log.contains("SENTINEL"), "a rule text reached the log");
}

#[test]
fn the_rule_comes_back_as_summary_then_full_and_again_as_summary_on_a_new_visit() {
    let h = harness(&template(true, &both(RULE)), &fixture(10));
    push_ten(&h);
    let out = run(&h);
    assert_eq!(condition(&out)["status"], "failed");
    assert_eq!(
        rules(&out),
        json!([{"rule_id": RULE, "level": "summary", "text": SUMMARY,
            "units": FAILING}])
    );

    // The same visit, nothing changed: the full text.
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 10, "the verdicts were reused");
    assert_eq!(
        rules(&out),
        json!([{"rule_id": RULE, "level": "full", "text": FULL, "units": FAILING}])
    );

    // Leave for work and come back: a new visit starts at the summary.
    next(&h, &["--to", "work"]);
    push_ten(&h);
    let out = next(&h, &["--with-data", r#"{"back": true}"#]);
    assert_eq!(out["state"], "review", "{}", out);
    assert_eq!(rules(&out)[0]["level"], "summary", "{}", out);
    assert_eq!(rules(&out)[0]["text"], SUMMARY);

    let d = deliveries(&h);
    let levels: Vec<&str> = d.iter().map(|p| p["level"].as_str().unwrap()).collect();
    assert_eq!(levels, ["summary", "full", "summary"]);
    for p in &d {
        assert_eq!(
            *p,
            json!({"state": "review", "gate": GATE, "rule_id": RULE,
                "level": p["level"], "unit_count": 2})
        );
    }
    assert_no_text_logged(&h);
}

#[test]
fn a_summary_only_rule_stays_at_the_summary() {
    let tpl = template(false, &criterion(RULE, "veto", Some(SUMMARY), None));
    let h = harness(&tpl, "anything\n");
    h.stub.push(fail(RULE));
    for _ in 0..2 {
        let out = run(&h);
        assert_eq!(
            rules(&out),
            json!([{"rule_id": RULE, "level": "summary", "text": SUMMARY}])
        );
    }
}

#[test]
fn a_rule_text_only_rule_is_full_from_its_first_failure() {
    let tpl = template(false, &criterion(RULE, "veto", None, Some(FULL)));
    let h = harness(&tpl, "anything\n");
    h.stub.push(fail(RULE));
    let out = run(&h);
    assert_eq!(
        rules(&out),
        json!([{"rule_id": RULE, "level": "full", "text": FULL}])
    );
    assert_eq!(deliveries(&h)[0]["level"], "full");
}

#[test]
fn a_criterion_with_no_rule_texts_adds_no_rules() {
    let tpl = template(false, &criterion(RULE, "veto", None, None));
    let h = harness(&tpl, "anything\n");
    h.stub.push(fail(RULE));
    let out = run(&h);
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed");
    assert!(cond.get("rules").is_none(), "{}", cond);
    assert!(deliveries(&h).is_empty());
}

#[test]
fn a_single_slice_check_climbs_the_same_ladder_without_units() {
    let h = harness(&template(false, &both(RULE)), "anything\n");
    h.stub.push(fail(RULE));
    let out = run(&h);
    assert_eq!(
        rules(&out),
        json!([{"rule_id": RULE, "level": "summary", "text": SUMMARY}])
    );
    let out = run(&h);
    assert_eq!(
        rules(&out),
        json!([{"rule_id": RULE, "level": "full", "text": FULL}])
    );
    for p in deliveries(&h) {
        assert!(p.get("unit_count").is_none(), "{}", p);
    }
    assert_no_text_logged(&h);
}

#[test]
fn a_failing_shadow_criterion_delivers_nothing() {
    let criteria =
        both("veto_rule") + &criterion("shadow_rule", "shadow", Some(SUMMARY), Some(FULL));
    let h = harness(&template(false, &criteria), "anything\n");
    h.stub.push(fail("veto_rule"));
    h.stub.push(fail("shadow_rule"));
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 2);
    let r = rules(&out);
    assert_eq!(r.as_array().unwrap().len(), 1, "{}", r);
    assert_eq!(r[0]["rule_id"], "veto_rule");
    let d = deliveries(&h);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["rule_id"], "veto_rule");
}

#[test]
fn a_second_rule_starts_at_the_summary_while_the_first_is_at_full() {
    let h = harness(&template(false, &(both("first") + &both("second"))), "v1\n");
    h.stub.push(fail("first"));
    h.stub.push(pass("second"));
    let out = run(&h);
    assert_eq!(
        rules(&out),
        json!([{"rule_id": "first", "level": "summary", "text": SUMMARY}])
    );

    // A changed slice is asked again in the same visit; now both fail.
    std::fs::write(h.dir.join("units.jsonl"), "v2\n").unwrap();
    h.stub.push(fail("first"));
    h.stub.push(fail("second"));
    let out = run(&h);
    assert_eq!(
        rules(&out),
        json!([
            {"rule_id": "first", "level": "full", "text": FULL},
            {"rule_id": "second", "level": "summary", "text": SUMMARY},
        ])
    );
}

#[test]
fn twenty_failing_units_make_one_entry_and_one_record() {
    let h = harness(&template(true, &both(RULE)), &fixture(20));
    for _ in 0..20 {
        h.stub.push(fail(RULE));
    }
    let out = run(&h);
    let r = rules(&out);
    assert_eq!(r.as_array().unwrap().len(), 1, "{}", r);
    let keys: Vec<String> = (1..=20).map(|i| format!("ac-{}", i)).collect();
    assert_eq!(r[0]["units"], json!(keys));
    let d = deliveries(&h);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["unit_count"], 20);
}

#[test]
fn rule_texts_outside_their_bounds_are_refused_at_compile() {
    let long_summary = "s".repeat(301);
    let long_rule = "r".repeat(4097);
    let cases = [
        criterion(RULE, "veto", Some(""), None),
        criterion(RULE, "veto", Some(&long_summary), None),
        criterion(RULE, "veto", None, Some(&long_rule)),
    ];
    for c in cases {
        let h = Harness::new(&template(false, &c));
        let out = h
            .koto()
            .args(["init", WF, "--template", "template.md"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{}", describe(&out));
        assert!(
            describe(&out).contains("E-DECIDER-CHECK-RULE"),
            "{}",
            describe(&out)
        );
    }
}
