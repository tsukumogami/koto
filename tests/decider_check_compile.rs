//! `decider-check` gates: parsing, lowering, defaults, the
//! `E-DECIDER-CHECK-*` compile rules, and the rule that nothing routes on a
//! check's output.
//!
//! docs/designs/DESIGN-koto-decider-checks.md, Decision 1.

use std::io::Write as _;

use koto::template::compile::compile;
use koto::template::decider_check::{CheckMode, DEFAULT_CHECK_LABEL, DEFAULT_CHECK_MAX_BYTES};
use koto::template::types::{CompiledTemplate, GATE_TYPE_DECIDER_CHECK};

#[test]
fn schema_and_built_in_default_cover_the_type() {
    use koto::template::types::{gate_type_builtin_default, gate_type_schema, GateSchemaFieldType};
    let schema = gate_type_schema(GATE_TYPE_DECIDER_CHECK).expect("schema");
    assert_eq!(
        schema,
        &[
            ("failed", GateSchemaFieldType::Array),
            ("unanswered", GateSchemaFieldType::Array),
            ("error", GateSchemaFieldType::Str),
        ]
    );
    let compile_time = gate_type_builtin_default(GATE_TYPE_DECIDER_CHECK).expect("default");
    let runtime = koto::gate::built_in_default(GATE_TYPE_DECIDER_CHECK).expect("default");
    assert_eq!(compile_time, runtime);
    assert_eq!(
        compile_time,
        serde_json::json!({"failed": [], "unanswered": [], "error": ""})
    );
    assert!(koto::template::types::SUPPORTED_GATE_TYPES.contains(&GATE_TYPE_DECIDER_CHECK));
}

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

#[test]
fn a_compiled_check_with_no_spec_is_refused_on_load() {
    let t = expect_ok(&template(
        &check("g", &[criterion("r")], ""),
        PLAIN_TRANSITIONS,
    ));
    let mut json = serde_json::to_value(&t).unwrap();
    // The control: the same JSON with the spec loads.
    serde_json::from_value::<CompiledTemplate>(json.clone()).unwrap();

    let gate = json["states"]["review"]["gates"]["g"]
        .as_object_mut()
        .unwrap();
    assert!(gate.remove("decider_check").is_some());
    let text = serde_json::to_string_pretty(&json).unwrap();
    let e = serde_json::from_str::<CompiledTemplate>(&text)
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("E-DECIDER-CHECK-SPEC: state \"review\" check \"g\": the compiled template"),
        "{}",
        e
    );

    // `koto template validate` on the file refuses it with the same code.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.json");
    std::fs::write(&path, &text).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_koto"))
        .args(["template", "validate"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("E-DECIDER-CHECK-SPEC"), "{}", stdout);

    // Any other gate type without a decider_check key is untouched.
    let mut plain = serde_json::to_value(&t).unwrap();
    let g = plain["states"]["review"]["gates"]["g"]
        .as_object_mut()
        .unwrap();
    g.remove("decider_check");
    g.insert("type".into(), serde_json::json!("command"));
    serde_json::from_value::<CompiledTemplate>(plain).unwrap();
}

/// Loading never changes a template: every compiled snapshot, and a
/// template with a decider check, read back and written again is the same
/// bytes, so the same `template_hash`.
#[test]
fn a_template_that_compiles_today_loads_back_to_the_same_bytes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seen = 0;
    let mut dirs = vec![root.join("tests/fixtures/compiled-snapshots")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let t: CompiledTemplate =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
            assert_eq!(
                serde_json::to_string_pretty(&t).unwrap(),
                text,
                "{}",
                path.display()
            );
            seen += 1;
        }
    }
    assert!(seen >= 10, "only {} snapshots read", seen);

    let t = expect_ok(&template(
        &check("g", &[criterion("r"), criterion("s")], ""),
        PLAIN_TRANSITIONS,
    ));
    let text = serde_json::to_string_pretty(&t).unwrap();
    let back: CompiledTemplate = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
}

#[test]
fn every_decider_check_code_the_compiler_emits_is_documented() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut codes = std::collections::BTreeSet::new();
    for file in [
        "src/template/compile.rs",
        "src/template/types.rs",
        "src/template/decider_check.rs",
    ] {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("E-DECIDER-CHECK-") {
            let tail = &rest[i..];
            let end = tail
                .find(|c: char| !(c.is_ascii_uppercase() || c == '-'))
                .unwrap_or(tail.len());
            let code = tail[..end].trim_end_matches('-');
            if code.len() > "E-DECIDER-CHECK-".len() {
                codes.insert(code.to_string());
            }
            rest = &tail[end..];
        }
    }
    assert!(codes.len() >= 10, "{:?}", codes);
    let doc = std::fs::read_to_string(root.join("docs/reference/error-codes.md")).unwrap();
    for code in &codes {
        assert!(
            doc.contains(&format!("**{} (error)**", code)),
            "{} is not documented in docs/reference/error-codes.md",
            code
        );
    }
}
