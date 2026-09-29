//! `decider-check` gates: parsing, lowering, defaults, the
//! `E-DECIDER-CHECK-*` compile rules, and the rule that nothing routes on a
//! check's output.
//!
//! docs/designs/DESIGN-koto-decider-checks.md, Decision 1.

use std::io::Write as _;

use koto::template::compile::compile;
use koto::template::decider_check::{CheckMode, DEFAULT_CHECK_LABEL, DEFAULT_CHECK_MAX_BYTES};
use koto::template::types::{CompiledTemplate, GATE_TYPE_DECIDER_CHECK};

fn compile_src(src: &str) -> Result<CompiledTemplate, String> {
    let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    f.write_all(src.as_bytes()).unwrap();
    compile(f.path(), true).map_err(|e| format!("{:#}", e))
}

fn expect_err(src: &str, code: &str) -> String {
    let err = match compile_src(src) {
        Ok(_) => panic!("expected {} but the template compiled:\n{}", code, src),
        Err(e) => e,
    };
    assert!(
        err.contains(&format!("{}:", code)),
        "expected {}, got: {}",
        code,
        err
    );
    err
}

fn expect_ok(src: &str) -> CompiledTemplate {
    compile_src(src).unwrap_or_else(|e| panic!("expected a compile, got: {}\n{}", e, src))
}

/// A template whose `review` state carries `gates` (YAML indented to sit
/// under `gates:`) and `extra` (YAML at state level, such as transitions).
fn template(gates: &str, transitions: &str) -> String {
    format!(
        r#"---
name: checks
version: "1.0"
initial_state: review
states:
  review:
    gates:
{gates}
    transitions:
{transitions}
  done:
    terminal: true
---

## review

Review the work.

## done

Done.
"#
    )
}

const PLAIN_TRANSITIONS: &str = "      - target: done";

fn criterion(id: &str) -> String {
    format!(
        r#"          {id}:
            rule_ref: "https://example.org/rules/{id}"
            question: "Does each comment give a reason?"
            pass: "Every comment says why."
            fail: "A comment restates the code."
            escape: "No comment, or can't tell.""#
    )
}

fn check(name: &str, criteria: &[String], extra: &str) -> String {
    format!(
        "      {name}:\n        type: decider-check\n        command: \"printf x\"\n{extra}        criteria:\n{}",
        criteria.join("\n")
    )
}

#[test]
fn two_checks_three_criteria_compile_with_defaults() {
    let gates = format!(
        "{}\n{}",
        check("comments", &[criterion("comment_reason")], ""),
        check(
            "criteria",
            &[criterion("ac_binary"), criterion("ac_scoped")],
            "        max_bytes: 4096\n        label: acs\n        timeout: 20\n"
        )
    );
    let t = expect_ok(&template(&gates, PLAIN_TRANSITIONS));
    let state = &t.states["review"];
    let g = &state.gates["comments"];
    assert_eq!(g.gate_type, GATE_TYPE_DECIDER_CHECK);
    let spec = g.decider_check.as_ref().unwrap();
    assert_eq!(spec.max_bytes, DEFAULT_CHECK_MAX_BYTES);
    assert_eq!(spec.max_bytes, 2560);
    assert_eq!(spec.label, DEFAULT_CHECK_LABEL);
    assert_eq!(spec.criteria[0].mode, CheckMode::Shadow);
    assert_eq!(spec.criteria[0].threshold, 0.9);

    let other = state.gates["criteria"].decider_check.as_ref().unwrap();
    assert_eq!(other.max_bytes, 4096);
    assert_eq!(other.label, "acs");
    assert_eq!(state.gates["criteria"].timeout, 20);
    // Declaration order is kept.
    let ids: Vec<&str> = other.criteria.iter().map(|c| c.rule_id.as_str()).collect();
    assert_eq!(ids, ["ac_binary", "ac_scoped"]);
}

#[test]
fn explicit_mode_and_threshold_are_kept() {
    let c = format!(
        "{}\n            threshold: 0.95\n            mode: veto",
        criterion("comment_reason")
    );
    let t = expect_ok(&template(&check("comments", &[c], ""), PLAIN_TRANSITIONS));
    let c = &t.states["review"].gates["comments"]
        .decider_check
        .as_ref()
        .unwrap()
        .criteria[0];
    assert_eq!(c.mode, CheckMode::Veto);
    assert_eq!(c.threshold, 0.95);
}

#[test]
fn threshold_bounds() {
    for (t, ok) in [
        ("0.5", true),
        ("1.0", true),
        ("0.49", false),
        ("1.01", false),
    ] {
        let c = format!("{}\n            threshold: {}", criterion("r"), t);
        let src = template(&check("g", &[c], ""), PLAIN_TRANSITIONS);
        if ok {
            expect_ok(&src);
        } else {
            expect_err(&src, "E-DECIDER-CHECK-THRESHOLD");
        }
    }
    let c = format!("{}\n            threshold: high", criterion("r"));
    expect_err(
        &template(&check("g", &[c], ""), PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-THRESHOLD",
    );
}

#[test]
fn missing_text_or_ids_are_refused() {
    for drop in ["rule_ref", "question", "pass", "fail", "escape"] {
        let c: String = criterion("r")
            .lines()
            .filter(|l| !l.trim_start().starts_with(&format!("{}:", drop)))
            .collect::<Vec<_>>()
            .join("\n");
        let err = expect_err(
            &template(&check("g", &[c], ""), PLAIN_TRANSITIONS),
            "E-DECIDER-CHECK-FIELD",
        );
        assert!(err.contains("state \"review\""), "{}", err);
        assert!(err.contains("\"g\""), "{}", err);
        assert!(err.contains("\"r\""), "{}", err);
    }
    let long = "x".repeat(129);
    expect_err(
        &template(&check("g", &[criterion(&long)], ""), PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-FIELD",
    );
}

#[test]
fn unknown_criterion_key_and_empty_criteria_are_refused() {
    let c = format!("{}\n            severity: high", criterion("r"));
    expect_err(
        &template(&check("g", &[c], ""), PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-FIELD",
    );
    let gates = "      g:\n        type: decider-check\n        command: \"printf x\"\n        criteria: {}";
    expect_err(&template(gates, PLAIN_TRANSITIONS), "E-DECIDER-CHECK-FIELD");
    let gates = "      g:\n        type: decider-check\n        command: \"printf x\"";
    expect_err(&template(gates, PLAIN_TRANSITIONS), "E-DECIDER-CHECK-FIELD");
}

#[test]
fn unknown_mode_is_refused() {
    let c = format!("{}\n            mode: auto", criterion("r"));
    expect_err(
        &template(&check("g", &[c], ""), PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-MODE",
    );
}

#[test]
fn duplicate_rule_id_on_a_state_is_refused() {
    let gates = format!(
        "{}\n{}",
        check("a", &[criterion("same")], ""),
        check("b", &[criterion("same")], "")
    );
    expect_err(
        &template(&gates, PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-DUPLICATE",
    );
}

#[test]
fn a_fifth_criterion_on_a_state_is_refused() {
    let four: Vec<String> = (1..=4).map(|i| criterion(&format!("r{}", i))).collect();
    expect_ok(&template(&check("a", &four, ""), PLAIN_TRANSITIONS));
    let gates = format!(
        "{}\n{}",
        check("a", &four, ""),
        check("b", &[criterion("r5")], "")
    );
    expect_err(
        &template(&gates, PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-LIMIT",
    );
}

#[test]
fn budget_bounds() {
    for (b, ok) in [
        ("1", true),
        ("8192", true),
        ("0", false),
        ("8193", false),
        ("big", false),
    ] {
        let src = template(
            &check(
                "g",
                &[criterion("r")],
                &format!("        max_bytes: {}\n", b),
            ),
            PLAIN_TRANSITIONS,
        );
        if ok {
            expect_ok(&src);
        } else {
            expect_err(&src, "E-DECIDER-CHECK-BUDGET");
        }
    }
}

#[test]
fn bad_labels_are_refused() {
    for l in ["\"has space\"", "\"\"", "42"] {
        expect_err(
            &template(
                &check("g", &[criterion("r")], &format!("        label: {}\n", l)),
                PLAIN_TRANSITIONS,
            ),
            "E-DECIDER-CHECK-LABEL",
        );
    }
}

#[test]
fn overridable_false_and_poll_are_refused() {
    expect_err(
        &template(
            &check("g", &[criterion("r")], "        overridable: false\n"),
            PLAIN_TRANSITIONS,
        ),
        "E-DECIDER-CHECK-OVERRIDABLE",
    );
    expect_err(
        &template(
            &check(
                "g",
                &[criterion("r")],
                "        poll:\n          interval_secs: 5\n          timeout_secs: 60\n",
            ),
            PLAIN_TRANSITIONS,
        ),
        "E-DECIDER-CHECK-POLL",
    );
}

#[test]
fn empty_command_is_refused() {
    let gates = format!(
        "      g:\n        type: decider-check\n        command: \"\"\n        criteria:\n{}",
        criterion("r")
    );
    expect_err(
        &template(&gates, PLAIN_TRANSITIONS),
        "E-DECIDER-CHECK-FIELD",
    );
}

#[test]
fn decider_check_keys_on_another_gate_type_are_refused() {
    for key in ["max_bytes: 10", "label: x", "criteria:\n          r: {}"] {
        let gates = format!(
            "      ci:\n        type: command\n        command: \"true\"\n        {}",
            key
        );
        let transitions = "      - target: done\n        when:\n          gates.ci.exit_code: 0";
        expect_err(&template(&gates, transitions), "E-DECIDER-CHECK-FIELD");
    }
}

#[test]
fn routing_on_a_check_is_refused() {
    let gates = check("comments", &[criterion("r")], "");
    let when = "      - target: done\n        when:\n          gates.comments.error: \"\"";
    let err = expect_err(&template(&gates, when), "E-DECIDER-CHECK-ROUTE");
    assert!(err.contains("\"comments\""), "{}", err);

    let assign = "      - target: done\n        context_assignments:\n          note.md: \"${gates.comments.error}\"";
    expect_err(&template(&gates, assign), "E-DECIDER-CHECK-ROUTE");

    let src = format!(
        r#"---
name: checks
version: "1.0"
initial_state: review
states:
  review:
    gates:
{gates}
    skip_if:
      gates.comments.error: ""
    transitions:
      - target: done
  done:
    terminal: true
---

## review

Review.

## done

Done.
"#
    );
    expect_err(&src, "E-DECIDER-CHECK-ROUTE");
}

#[test]
fn strict_mode_does_not_ask_a_check_to_route() {
    // compile_src is strict: a state whose only gate is a decider check and
    // whose transition doesn't route on gates.* still compiles.
    expect_ok(&template(
        &check("g", &[criterion("r")], ""),
        PLAIN_TRANSITIONS,
    ));
}

#[test]
fn compiled_json_round_trips_and_validates_from_cache() {
    let t = expect_ok(&template(
        &check("g", &[criterion("r")], ""),
        PLAIN_TRANSITIONS,
    ));
    let json = serde_json::to_string(&t).unwrap();
    let back: CompiledTemplate = serde_json::from_str(&json).unwrap();
    back.validate(true).unwrap();
    assert!(json.contains("\"decider_check\""));

    // A cached template edited to break a rule is refused on load.
    let mut bad = back.clone();
    bad.states
        .get_mut("review")
        .unwrap()
        .gates
        .get_mut("g")
        .unwrap()
        .overridable = false;
    let e = bad.validate(true).unwrap_err();
    assert!(e.starts_with("E-DECIDER-CHECK-OVERRIDABLE"), "{}", e);
}
