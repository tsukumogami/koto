//! Value routing's compile rules: a `vars.NAME: <value>` condition in a
//! `when` clause or a `skip_if` map, and the `E-VAR-ROUTE-*` codes that
//! refuse a route naming an undeclared variable, a value the variable can't
//! hold, a capture, or a value another route out of the same state also
//! matches.
//!
//! docs/designs/DESIGN-koto-value-routing.md, Decisions 1, 2 and 4.

use std::io::Write as _;

use koto::template::compile::compile;
use koto::template::types::CompiledTemplate;

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

/// A template with `MODE` (`values: [auto, interactive]`), `TAG`
/// (`pattern: v[0-9]+`), a free `NOTE`, and a `route` state whose
/// transitions (YAML indented under `transitions:`) and extra state-level
/// YAML come from the caller. `capture` produces a capture named `SHA`.
fn template(transitions: &str, extra: &str) -> String {
    format!(
        r#"---
name: value-routing
version: "1.0"
initial_state: route
variables:
  MODE:
    values: [auto, interactive]
    default: interactive
  TAG:
    pattern: "v[0-9]+"
    default: v1
  NOTE:
    default: ""
states:
  route:
    accepts:
      verdict:
        type: enum
        values: [approve, reject]
{extra}
    transitions:
{transitions}
  capture:
    default_action:
      command: "echo abc"
      capture_stdout_as: SHA
    transitions:
      - target: fast
  fast:
    terminal: true
  slow:
    terminal: true
---

## route

Route.

## capture

Capture.

## fast

Fast.

## slow

Slow.
"#
    )
}

const TWO_ROUTES: &str = "      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          vars.MODE: interactive";

#[test]
fn value_routes_compile_to_a_string_where_is_set_would_stand() {
    let t = expect_ok(&template(TWO_ROUTES, ""));
    let route = &t.states["route"];
    assert_eq!(
        route.transitions[0].when.as_ref().unwrap()["vars.MODE"],
        serde_json::json!("auto")
    );
    assert_eq!(t.format_version, 1);
}

#[test]
fn value_and_is_set_false_and_different_values_compile() {
    expect_ok(&template(
        "      - target: fast
        when:
          vars.NOTE: hello
      - target: slow
        when:
          vars.NOTE:
            is_set: false",
        "",
    ));
    // Same value on both edges, told apart by another key.
    expect_ok(&template(
        "      - target: fast
        when:
          vars.MODE: auto
          verdict: approve
      - target: slow
        when:
          vars.MODE: auto
          verdict: reject",
        "",
    ));
    // Every allowlisted character a value can hold.
    expect_ok(&template(
        "      - target: fast
        when:
          vars.NOTE: \"a b.c_d/e:f@g+h-9\"",
        "",
    ));
}

#[test]
fn undeclared_variable_is_refused_in_when_and_skip_if() {
    let err = expect_err(
        &template(
            "      - target: fast
        when:
          vars.NOPE: x",
            "",
        ),
        "E-VAR-ROUTE-UNDECLARED",
    );
    assert!(err.contains("NOPE"), "got: {}", err);
    expect_err(
        &template(
            "      - target: fast
        when:
          vars.MODE: auto
      - target: slow",
            "    skip_if:
      vars.NOPE: x",
        ),
        "E-VAR-ROUTE-UNDECLARED",
    );
}

#[test]
fn a_value_the_variable_cannot_hold_is_refused() {
    let cases = [
        ("vars.MODE: atuo", "values:[auto,interactive]"),
        ("vars.TAG: release", "pattern:v[0-9]+"),
        ("vars.MODE: \"\"", "is empty"),
        ("vars.MODE: true", "must be a string"),
        ("vars.MODE: 3", "must be a string"),
        ("vars.NOTE: \"a$b\"", "koto init"),
    ];
    for (cond, needle) in cases {
        let when = format!(
            "      - target: fast
        when:
          {cond}"
        );
        let err = expect_err(&template(&when, ""), "E-VAR-ROUTE-VALUE");
        assert!(
            err.contains(needle),
            "{cond}: expected {needle:?} in: {err}"
        );
        let skip = format!(
            "    skip_if:
      {cond}"
        );
        expect_err(
            &template(
                "      - target: fast
        when:
          verdict: approve
      - target: slow",
                &skip,
            ),
            "E-VAR-ROUTE-VALUE",
        );
    }
}

#[test]
fn a_capture_can_be_tested_for_presence_but_not_routed_on() {
    expect_err(
        &template(
            "      - target: fast
        when:
          vars.SHA: abc",
            "",
        ),
        "E-VAR-ROUTE-CAPTURE",
    );
    expect_err(
        &template(
            "      - target: fast
        when:
          verdict: approve
      - target: slow",
            "    skip_if:
      vars.SHA: abc",
        ),
        "E-VAR-ROUTE-CAPTURE",
    );
    expect_ok(&template(
        "      - target: fast
        when:
          vars.SHA:
            is_set: true",
        "",
    ));
}

#[test]
fn overlapping_value_routes_are_refused() {
    let err = expect_err(
        &template(
            "      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          vars.MODE: auto",
            "",
        ),
        "E-VAR-ROUTE-OVERLAP",
    );
    assert!(err.contains("fast") && err.contains("slow"), "got: {}", err);
    expect_err(
        &template(
            "      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          vars.MODE:
            is_set: true",
            "",
        ),
        "E-VAR-ROUTE-OVERLAP",
    );
}

#[test]
fn a_pair_sharing_no_vars_key_keeps_the_old_message() {
    let err = compile_src(&template(
        "      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          verdict: approve",
        "",
    ))
    .unwrap_err();
    assert!(!err.contains("E-VAR-ROUTE-OVERLAP"), "got: {}", err);
    assert!(err.contains("not mutually exclusive"), "got: {}", err);
}

#[test]
fn a_skip_if_value_selects_the_route_with_the_same_value() {
    expect_ok(&template(TWO_ROUTES, "    skip_if:\n      vars.MODE: auto"));
}

/// The first line of a compile error, without anything the caller wraps
/// around the validator's message.
fn first_line_from(err: &str, start: &str) -> String {
    let at = err
        .find(start)
        .unwrap_or_else(|| panic!("{:?} not in: {}", start, err));
    err[at..].lines().next().unwrap().to_string()
}

#[test]
fn both_vars_error_families_name_the_variable_the_same_way() {
    // An `{is_set: ...}` condition on an undeclared name: the older family.
    let err = compile_src(&template(
        "      - target: fast
        when:
          vars.NOPE:
            is_set: true",
        "",
    ))
    .unwrap_err();
    assert_eq!(
        first_line_from(&err, "state "),
        "state \"route\" transition to \"fast\": when clause references undeclared variable \
         \"NOPE\"; add it to the template's variables block"
    );

    // A value route: the E-VAR-ROUTE-* family, in a `when` clause and in a
    // `skip_if` map.
    let err = expect_err(
        &template(
            "      - target: fast
        when:
          vars.MODE: \"\"",
            "",
        ),
        "E-VAR-ROUTE-VALUE",
    );
    assert_eq!(
        first_line_from(&err, "E-VAR-ROUTE-VALUE"),
        "E-VAR-ROUTE-VALUE: state \"route\" transition to \"fast\": when clause value for \
         variable \"MODE\" is empty; an empty variable counts as not set and matches no value"
    );
    let err = expect_err(
        &template(
            "      - target: fast
        when:
          verdict: approve
      - target: slow",
            "    skip_if:
      vars.MODE: atuo",
        ),
        "E-VAR-ROUTE-VALUE",
    );
    assert_eq!(
        first_line_from(&err, "E-VAR-ROUTE-VALUE"),
        "E-VAR-ROUTE-VALUE: state \"route\": skip_if routes on variable \"MODE\" = \"atuo\", \
         which koto init would refuse (values:[auto,interactive]); the route could never fire"
    );

    // The overlap refusal names the variable the same way, not its key.
    let err = expect_err(
        &template(
            "      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          vars.MODE: auto",
            "",
        ),
        "E-VAR-ROUTE-OVERLAP",
    );
    assert_eq!(
        first_line_from(&err, "E-VAR-ROUTE-OVERLAP"),
        "E-VAR-ROUTE-OVERLAP: state \"route\": transitions to \"fast\" and \"slow\" can both \
         match one value of variable \"MODE\""
    );
}
