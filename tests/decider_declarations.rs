//! Decider declarations on `accepts` fields: parsing, lowering, the
//! `E-DECIDER-*` compile rules, the floor, the declaration hash, and the
//! `expects` keys a declared field adds.
//!
//! docs/designs/DESIGN-jev-decision-offload.md, Decision 1.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use koto::cli::next_types::{derive_expects, ExpectsFieldSchema};
use koto::engine::evidence::validate_evidence;
use koto::template::compile::compile;
use koto::template::decider::{
    declaration_hash, DeciderInputSource, DeciderMode, FloorViolationKind, DEFAULT_MAX_BYTES,
    DEFAULT_THRESHOLD,
};
use koto::template::types::CompiledTemplate;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn declared_fixture() -> PathBuf {
    manifest_dir().join("tests/fixtures/decider/declared.md")
}

fn compile_src(src: &str) -> Result<CompiledTemplate, String> {
    compile_src_with(src, true)
}

fn compile_src_with(src: &str, strict: bool) -> Result<CompiledTemplate, String> {
    let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    f.write_all(src.as_bytes()).unwrap();
    compile(f.path(), strict).map_err(|e| format!("{:#}", e))
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
    match compile_src(src) {
        Ok(t) => t,
        Err(e) => panic!("expected the template to compile, got: {}\n{}", e, src),
    }
}

const DEFAULT_DECIDER: &str = r#"{answers: {proceed: {description: "Go ahead."}, exit: {description: "Stop here."}}, escape: {value: unclear, description: "Can't tell."}, inputs: [{context: outline.md, label: outline}, {var: PLAN_DOC, label: plan}]}"#;

const DEFAULT_TRANSITIONS: &str = "      - target: work
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
";

/// A template whose `review` state declares a decider on the `verdict` field.
/// Every piece can be swapped; the rest stays valid.
struct Tpl {
    field_type: &'static str,
    values: &'static str,
    question: &'static str,
    decider: String,
    extra_fields: String,
    transitions: String,
    review_gates: &'static str,
}

impl Default for Tpl {
    fn default() -> Self {
        Tpl {
            field_type: "enum",
            values: "        values: [proceed, exit]\n",
            question: "Is the item clear?",
            decider: DEFAULT_DECIDER.to_string(),
            extra_fields: String::new(),
            transitions: DEFAULT_TRANSITIONS.to_string(),
            review_gates: "",
        }
    }
}

impl Tpl {
    fn decider(mut self, d: &str) -> Self {
        self.decider = d.to_string();
        self
    }
    fn transitions(mut self, t: &str) -> Self {
        self.transitions = t.to_string();
        self
    }
    fn render(&self) -> String {
        format!(
            r#"---
name: t
version: "1.0"
initial_state: gather
variables:
  PLAN_DOC:
    description: plan
states:
  gather:
    default_action:
      command: "echo hi"
      capture_stdout_as: CAPTURED
    gates:
      outline:
        type: context-exists
        key: outline.md
    transitions:
      - target: review
        when:
          gates.outline.exists: true
  review:
{review_gates}    accepts:
      verdict:
        type: {field_type}
{values}        required: true
        description: "{question}"
        decider: {decider}
{extra_fields}    transitions:
{transitions}  work:
    transitions:
      - target: done
  guarded:
    default_action:
      command: "echo confirm"
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
  stopped:
    terminal: true
---

## gather

g

## review

r

## work

w

## guarded

c

## done

d

## stopped

s
"#,
            review_gates = self.review_gates,
            field_type = self.field_type,
            values = self.values,
            question = self.question,
            decider = self.decider,
            extra_fields = self.extra_fields,
            transitions = self.transitions,
        )
    }
}

/// A boolean-field variant: `ready` on `review`, true to `work`, false to
/// `stopped` unless overridden.
fn bool_tpl(decider: &str, transitions: &str) -> String {
    Tpl {
        field_type: "boolean",
        values: "",
        decider: decider.to_string(),
        transitions: transitions.to_string(),
        ..Tpl::default()
    }
    .render()
    .replace("      verdict:\n", "      ready:\n")
}

const BOOL_DECIDER: &str = r#"{answers: {true: {description: "Yes."}, false: {description: "No."}}, inputs: [{var: PLAN_DOC, label: plan}]}"#;

const BOOL_TRANSITIONS: &str = "      - target: work
        when:
          ready: true
      - target: stopped
        when:
          ready: false
";

fn assert_names(err: &str, parts: &[&str]) {
    for p in parts {
        assert!(err.contains(p), "expected {:?} in: {}", p, err);
    }
}

// ---------------------------------------------------------------------------
// declaration and lowering
// ---------------------------------------------------------------------------

#[test]
fn declared_fixture_compiles_with_enum_and_boolean_declarations() {
    let t = compile(&declared_fixture(), true).expect("fixture compiles");

    let review = t.states["review"].accepts.as_ref().unwrap();
    let verdict = &review["verdict"];
    let d = verdict.decider.as_ref().expect("verdict is declared");
    assert_eq!(verdict.values, vec!["proceed", "exit"]);
    assert_eq!(d.escape.as_ref().unwrap().value, "unclear");
    assert!(!verdict.values.contains(&"unclear".to_string()));
    assert_eq!(d.answers["proceed"].threshold, 0.92);
    assert_eq!(d.answers["proceed"].mode, DeciderMode::Shadow);
    assert_eq!(d.answers["exit"].mode, DeciderMode::Never);
    assert_eq!(d.answers["exit"].threshold, DEFAULT_THRESHOLD);
    assert_eq!(
        d.inputs[0].source,
        DeciderInputSource::Context("outline.md".into())
    );
    assert_eq!(d.inputs[0].max_bytes, 12000);
    assert_eq!(
        d.inputs[1].source,
        DeciderInputSource::Var("PLAN_DOC".into())
    );
    assert_eq!(d.inputs[1].max_bytes, DEFAULT_MAX_BYTES);
    assert!(review["rationale"].decider.is_none());

    let ready = &t.states["confirm"].accepts.as_ref().unwrap()["ready"];
    let d = ready.decider.as_ref().expect("ready is declared");
    assert!(d.escape.is_none());
    assert_eq!(
        d.answers.keys().collect::<Vec<_>>(),
        vec!["false", "true"],
        "bare true:/false: keys lower to strings"
    );
    assert_eq!(d.answers["false"].mode, DeciderMode::Auto);
    assert_eq!(
        d.inputs[0].source,
        DeciderInputSource::Var("CHANGED_FILES".into()),
        "a capture_stdout_as name is a valid var input"
    );
}

#[test]
fn defaults_resolve_at_compile_time() {
    let t = expect_ok(&Tpl::default().render());
    let d = t.states["review"].accepts.as_ref().unwrap()["verdict"]
        .decider
        .clone()
        .unwrap();
    for a in d.answers.values() {
        assert_eq!(a.mode, DeciderMode::Shadow);
        assert_eq!(a.threshold, 0.9);
    }
    for i in &d.inputs {
        assert_eq!(i.max_bytes, 8192);
    }
}

#[test]
fn explicit_default_budget_equals_omitted_budget() {
    let omitted = expect_ok(&Tpl::default().render());
    let explicit = expect_ok(
        &Tpl::default()
            .decider(&DEFAULT_DECIDER.replace(
                "{context: outline.md, label: outline}",
                "{context: outline.md, label: outline, max_bytes: 8192}",
            ))
            .render(),
    );
    let a = &omitted.states["review"].accepts.as_ref().unwrap()["verdict"];
    let b = &explicit.states["review"].accepts.as_ref().unwrap()["verdict"];
    assert_eq!(a.decider, b.decider);
    assert_eq!(
        declaration_hash(a.decider.as_ref().unwrap(), &a.description),
        declaration_hash(b.decider.as_ref().unwrap(), &b.description)
    );
    assert_eq!(
        serde_json::to_string_pretty(&omitted).unwrap(),
        serde_json::to_string_pretty(&explicit).unwrap()
    );
}

#[test]
fn misspelled_key_inside_the_block_is_named() {
    for (bad, key) in [
        (
            DEFAULT_DECIDER.replace(
                "{description: \"Go ahead.\"}",
                "{description: \"Go ahead.\", thresold: 0.9}",
            ),
            "thresold",
        ),
        (DEFAULT_DECIDER.replace("inputs:", "inptus:"), "inptus"),
        (
            DEFAULT_DECIDER.replace(
                "description: \"Can't tell.\"",
                "descripton: \"Can't tell.\"",
            ),
            "descripton",
        ),
        (
            DEFAULT_DECIDER.replace("label: plan}", "label: plan, max_byte: 10}"),
            "max_byte",
        ),
    ] {
        let err = compile_src(&Tpl::default().decider(&bad).render()).unwrap_err();
        assert!(err.contains(key), "expected {:?} named in: {}", key, err);
    }
}

#[test]
fn misspelled_key_is_named_by_koto_template_compile() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("t.md");
    std::fs::write(
        &src,
        Tpl::default()
            .decider(&DEFAULT_DECIDER.replace(
                "{description: \"Go ahead.\"}",
                "{description: \"Go ahead.\", thresold: 0.95}",
            ))
            .render(),
    )
    .unwrap();
    let out = assert_cmd::Command::cargo_bin("koto")
        .unwrap()
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("HOME", dir.path())
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("thresold"), "output: {}", all);
}

#[test]
fn unknown_keys_outside_the_block_on_a_field_stay_lenient() {
    // SourceFieldSchema is deliberately not strict: that is what lets an
    // older koto drop the whole decider block.
    let src = Tpl::default().render().replace(
        "        required: true\n        description: \"Is",
        "        required: true\n        future_key: 1\n        description: \"Is",
    );
    expect_ok(&src);
}

#[test]
fn var_inputs_accept_variables_and_captures_but_not_runtime_names() {
    expect_ok(
        &Tpl::default()
            .decider(&DEFAULT_DECIDER.replace("var: PLAN_DOC", "var: CAPTURED"))
            .render(),
    );
    for runtime in ["SESSION_NAME", "SESSION_DIR"] {
        let err = expect_err(
            &Tpl::default()
                .decider(&DEFAULT_DECIDER.replace("var: PLAN_DOC", &format!("var: {}", runtime)))
                .render(),
            "E-DECIDER-INPUT",
        );
        assert_names(&err, &[runtime, "runtime name"]);
    }
}

#[test]
fn unknown_mode_is_e_decider_mode() {
    let err = expect_err(
        &Tpl::default()
            .decider(&DEFAULT_DECIDER.replace(
                "{description: \"Go ahead.\"}",
                "{description: \"Go ahead.\", mode: automatic}",
            ))
            .render(),
        "E-DECIDER-MODE",
    );
    assert_names(
        &err,
        &[
            "state \"review\"",
            "field \"verdict\"",
            "value \"proceed\"",
            "automatic",
        ],
    );
}

#[test]
fn input_with_both_or_neither_source_is_e_decider_input() {
    let both = DEFAULT_DECIDER.replace(
        "{var: PLAN_DOC, label: plan}",
        "{var: PLAN_DOC, context: outline.md, label: plan}",
    );
    let err = expect_err(&Tpl::default().decider(&both).render(), "E-DECIDER-INPUT");
    assert_names(&err, &["state \"review\"", "field \"verdict\"", "both"]);

    let neither = DEFAULT_DECIDER.replace("{var: PLAN_DOC, label: plan}", "{label: plan}");
    let err = expect_err(
        &Tpl::default().decider(&neither).render(),
        "E-DECIDER-INPUT",
    );
    assert_names(&err, &["neither"]);
}

// ---------------------------------------------------------------------------
// compile rules
// ---------------------------------------------------------------------------

#[test]
fn decider_on_string_number_or_tasks_is_e_decider_field_type() {
    for ft in ["string", "number", "tasks"] {
        let err = expect_err(
            &Tpl {
                field_type: ft,
                values: "",
                transitions: "      - target: work\n".to_string(),
                ..Tpl::default()
            }
            .render(),
            "E-DECIDER-FIELD-TYPE",
        );
        assert_names(&err, &["state \"review\"", "field \"verdict\"", ft]);
    }
}

#[test]
fn empty_question_is_e_decider_question() {
    let err = expect_err(
        &Tpl {
            question: "",
            ..Tpl::default()
        }
        .render(),
        "E-DECIDER-QUESTION",
    );
    assert_names(&err, &["state \"review\"", "field \"verdict\""]);
}

#[test]
fn answers_must_match_the_value_set() {
    let missing = DEFAULT_DECIDER.replace(", exit: {description: \"Stop here.\"}", "");
    let err = expect_err(
        &Tpl::default().decider(&missing).render(),
        "E-DECIDER-ANSWERS",
    );
    assert_names(
        &err,
        &[
            "state \"review\"",
            "field \"verdict\"",
            "missing",
            "\"exit\"",
        ],
    );

    let extra = DEFAULT_DECIDER.replace(
        "exit: {description: \"Stop here.\"}",
        "exit: {description: \"Stop here.\"}, later: {description: \"Later.\"}",
    );
    let err = expect_err(
        &Tpl::default().decider(&extra).render(),
        "E-DECIDER-ANSWERS",
    );
    assert_names(&err, &["\"later\""]);

    let bool_extra = BOOL_DECIDER.replace(
        "false: {description: \"No.\"}",
        "false: {description: \"No.\"}, maybe: {description: \"Maybe.\"}",
    );
    let err = expect_err(
        &bool_tpl(&bool_extra, BOOL_TRANSITIONS),
        "E-DECIDER-ANSWERS",
    );
    assert_names(&err, &["field \"ready\"", "\"maybe\""]);

    let bool_missing = BOOL_DECIDER.replace(", false: {description: \"No.\"}", "");
    let err = expect_err(
        &bool_tpl(&bool_missing, BOOL_TRANSITIONS),
        "E-DECIDER-ANSWERS",
    );
    assert_names(&err, &["\"false\""]);

    let dup = DEFAULT_DECIDER.replace(
        "exit: {description: \"Stop here.\"}",
        "exit: {description: \"Stop here.\"}, \"exit\": {description: \"Again.\"}",
    );
    let err = compile_src(&Tpl::default().decider(&dup).render()).unwrap_err();
    assert!(
        err.contains("E-DECIDER-ANSWERS") || err.contains("duplicate"),
        "got: {}",
        err
    );
}

#[test]
fn answer_without_description_is_e_decider_value_description() {
    for bad in [
        DEFAULT_DECIDER.replace("{description: \"Stop here.\"}", "{description: \"\"}"),
        DEFAULT_DECIDER.replace("{description: \"Stop here.\"}", "{mode: shadow}"),
    ] {
        let err = expect_err(
            &Tpl::default().decider(&bad).render(),
            "E-DECIDER-VALUE-DESCRIPTION",
        );
        assert_names(
            &err,
            &["state \"review\"", "field \"verdict\"", "value \"exit\""],
        );
    }
}

#[test]
fn enum_escape_rules() {
    let no_escape = DEFAULT_DECIDER.replace(
        " escape: {value: unclear, description: \"Can't tell.\"},",
        "",
    );
    let err = expect_err(
        &Tpl::default().decider(&no_escape).render(),
        "E-DECIDER-ESCAPE",
    );
    assert_names(&err, &["state \"review\"", "field \"verdict\""]);

    let empty_value = DEFAULT_DECIDER.replace("value: unclear", "value: \"\"");
    expect_err(
        &Tpl::default().decider(&empty_value).render(),
        "E-DECIDER-ESCAPE",
    );

    let no_desc = DEFAULT_DECIDER.replace(", description: \"Can't tell.\"", "");
    let err = expect_err(
        &Tpl::default().decider(&no_desc).render(),
        "E-DECIDER-ESCAPE",
    );
    assert_names(&err, &["value \"unclear\""]);

    let in_values = DEFAULT_DECIDER.replace("value: unclear", "value: exit");
    let err = expect_err(
        &Tpl::default().decider(&in_values).render(),
        "E-DECIDER-ESCAPE",
    );
    assert_names(&err, &["value \"exit\"", "also in values"]);
}

#[test]
fn boolean_refuses_any_escape_key() {
    for escape in [
        "escape: {value: unclear, description: \"Can't tell.\"}, ",
        "escape: {}, ",
        "escape: ~, ",
    ] {
        let d = BOOL_DECIDER.replace("inputs:", &format!("{}inputs:", escape));
        let err = expect_err(&bool_tpl(&d, BOOL_TRANSITIONS), "E-DECIDER-ESCAPE");
        assert_names(&err, &["field \"ready\"", "boolean"]);
    }
    // And a boolean with no escape compiles.
    expect_ok(&bool_tpl(BOOL_DECIDER, BOOL_TRANSITIONS));
}

#[test]
fn routing_on_the_escape_is_e_decider_escape_routed() {
    let t = Tpl::default().transitions(
        "      - target: work
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
      - target: guarded
        when:
          verdict: unclear
",
    );
    let err = expect_err(&t.render(), "E-DECIDER-ESCAPE-ROUTED");
    assert_names(
        &err,
        &[
            "state \"review\"",
            "field \"verdict\"",
            "value \"unclear\"",
            "\"guarded\"",
        ],
    );
    assert!(
        !err.contains("is not in allowed values"),
        "the generic routing error must not be reported instead: {}",
        err
    );
}

#[test]
fn threshold_bounds() {
    for (t, text) in [("0.4", "0.4"), ("1.1", "1.1"), (".nan", "NaN")] {
        let d = DEFAULT_DECIDER.replace(
            "{description: \"Go ahead.\"}",
            &format!("{{description: \"Go ahead.\", threshold: {}}}", t),
        );
        let err = expect_err(&Tpl::default().decider(&d).render(), "E-DECIDER-THRESHOLD");
        assert_names(
            &err,
            &[
                "state \"review\"",
                "field \"verdict\"",
                "value \"proceed\"",
                text,
            ],
        );
    }
    for t in ["0.5", "1.0", "1"] {
        let d = DEFAULT_DECIDER.replace(
            "{description: \"Go ahead.\"}",
            &format!("{{description: \"Go ahead.\", threshold: {}}}", t),
        );
        expect_ok(&Tpl::default().decider(&d).render());
    }
}

#[test]
fn input_rules() {
    let cases: Vec<(String, &str)> = vec![
        (
            DEFAULT_DECIDER.replace(
                "inputs: [{context: outline.md, label: outline}, {var: PLAN_DOC, label: plan}]",
                "inputs: []",
            ),
            "at least one input",
        ),
        (
            DEFAULT_DECIDER.replace("label: plan", "label: \"\""),
            "label",
        ),
        (
            DEFAULT_DECIDER.replace("label: plan", "label: outline"),
            "label \"outline\" is used by more than one input",
        ),
        (
            DEFAULT_DECIDER.replace("label: plan}", "label: plan, max_bytes: 0}"),
            "max_bytes 0",
        ),
        (
            DEFAULT_DECIDER.replace("var: PLAN_DOC", "var: NOT_DECLARED"),
            "NOT_DECLARED",
        ),
        (
            DEFAULT_DECIDER.replace("context: outline.md", "context: ../outline.md"),
            "not usable",
        ),
        (
            DEFAULT_DECIDER.replace("context: outline.md", "context: \"{{NOPE}}/outline.md\""),
            "NOPE",
        ),
        (
            DEFAULT_DECIDER.replace("context: outline.md", "context: other.md"),
            "no context-exists or context-matches gate",
        ),
    ];
    for (d, needle) in cases {
        let err = expect_err(&Tpl::default().decider(&d).render(), "E-DECIDER-INPUT");
        assert_names(&err, &["state \"review\"", "field \"verdict\"", needle]);
    }
}

#[test]
fn context_input_may_not_reference_a_runtime_name() {
    for runtime in ["SESSION_NAME", "SESSION_DIR"] {
        let key = format!("\"{{{{{}}}}}-outline.md\"", runtime);
        let src = Tpl::default()
            .decider(&DEFAULT_DECIDER.replace("context: outline.md", &format!("context: {}", key)))
            .render()
            .replace("key: outline.md", &format!("key: {}", key));
        let err = expect_err(&src, "E-DECIDER-INPUT");
        assert_names(&err, &[runtime, "runtime name"]);
    }
}

#[test]
fn context_input_with_a_declared_reference_compiles_when_gated() {
    let src = Tpl::default()
        .decider(&DEFAULT_DECIDER.replace("context: outline.md", "context: \"{{PLAN_DOC}}.md\""))
        .render()
        .replace("key: outline.md", "key: \"{{PLAN_DOC}}.md\"");
    expect_ok(&src);
}

#[test]
fn context_input_gated_by_context_matches_compiles() {
    let src = Tpl::default().render().replace(
        "      outline:\n        type: context-exists\n        key: outline.md\n    transitions:\n      - target: review\n        when:\n          gates.outline.exists: true",
        "      outline:\n        type: context-matches\n        key: outline.md\n        pattern: \".+\"\n    transitions:\n      - target: review\n        when:\n          gates.outline.matches: true",
    );
    assert!(src.contains("context-matches"));
    expect_ok(&src);
}

const SECOND_FIELD: &str = "      scope:
        type: enum
        values: [small, large]
        required: true
        description: \"How big is it?\"
        decider: {answers: {small: {description: \"Small.\"}, large: {description: \"Large.\"}}, escape: {value: unsure, description: \"Unsure.\"}, inputs: [INPUT]}
";

#[test]
fn a_shared_label_must_mean_the_same_input() {
    let same = SECOND_FIELD.replace("INPUT", "{context: outline.md, label: outline}");
    expect_ok(
        &Tpl {
            extra_fields: same,
            ..Tpl::default()
        }
        .render(),
    );

    for input in [
        "{var: PLAN_DOC, label: outline}",
        "{context: outline.md, label: outline, max_bytes: 100}",
    ] {
        let other = SECOND_FIELD.replace("INPUT", input);
        let err = expect_err(
            &Tpl {
                extra_fields: other,
                ..Tpl::default()
            }
            .render(),
            "E-DECIDER-INPUT",
        );
        assert_names(
            &err,
            &["state \"review\"", "label \"outline\"", "different source"],
        );
    }
}

#[test]
fn a_required_sibling_without_a_decider_is_refused() {
    let err = expect_err(
        &Tpl {
            extra_fields: "      notes:\n        type: string\n        required: true\n"
                .to_string(),
            ..Tpl::default()
        }
        .render(),
        "E-DECIDER-SIBLING-REQUIRED",
    );
    assert_names(
        &err,
        &["state \"review\"", "field \"notes\"", "\"verdict\""],
    );

    // An optional sibling without a decider is fine.
    expect_ok(
        &Tpl {
            extra_fields:
                "      rationale:\n        type: string\n        required: false\n        description: why\n"
                    .to_string(),
            ..Tpl::default()
        }
        .render(),
    );
}

// ---------------------------------------------------------------------------
// the floor
// ---------------------------------------------------------------------------

fn with_proceed_mode(mode: &str) -> String {
    DEFAULT_DECIDER.replace(
        "{description: \"Go ahead.\"}",
        &format!("{{description: \"Go ahead.\", mode: {}}}", mode),
    )
}

const TO_TERMINAL: &str = "      - target: done
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
";

const TO_GUARDED: &str = "      - target: guarded
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
";

const GATE_CONDITIONED: &str = "      - target: work
        when:
          verdict: proceed
          gates.ci.exit_code: 0
      - target: stopped
        when:
          verdict: exit
";

const CI_GATE: &str = "    gates:\n      ci:\n        type: command\n        command: \"true\"\n";

fn floor_case(mode: &str, transitions: &str, gates: &'static str) -> String {
    Tpl {
        review_gates: gates,
        ..Tpl::default()
    }
    .decider(&with_proceed_mode(mode))
    .transitions(transitions)
    .render()
}

#[test]
fn auto_on_a_terminal_route_is_refused() {
    let err = expect_err(&floor_case("auto", TO_TERMINAL, ""), "E-DECIDER-FLOOR");
    assert_names(
        &err,
        &[
            "state \"review\"",
            "field \"verdict\"",
            "value \"proceed\"",
            "\"done\"",
            "terminal",
        ],
    );
}

#[test]
fn auto_on_a_confirmation_guarded_route_is_refused() {
    let err = expect_err(&floor_case("auto", TO_GUARDED, ""), "E-DECIDER-FLOOR");
    assert_names(&err, &["\"guarded\"", "requires_confirmation"]);
}

#[test]
fn auto_on_a_gate_conditioned_route_is_refused() {
    let err = expect_err(
        &floor_case("auto", GATE_CONDITIONED, CI_GATE),
        "E-DECIDER-FLOOR",
    );
    assert_names(&err, &["\"work\"", "gate", "gates.ci.exit_code"]);
}

#[test]
fn only_auto_answers_are_floor_checked() {
    for mode in ["shadow", "never", "off"] {
        expect_ok(&floor_case(mode, TO_TERMINAL, ""));
        expect_ok(&floor_case(mode, TO_GUARDED, ""));
        expect_ok(&floor_case(mode, GATE_CONDITIONED, CI_GATE));
    }
    // And auto on a route that breaks no rule compiles.
    expect_ok(&floor_case("auto", DEFAULT_TRANSITIONS, ""));
}

#[test]
fn the_floor_holds_without_strict_gate_checking() {
    // `--allow-legacy-gates` (strict = false) relaxes gate routing only.
    for (t, g) in [
        (TO_TERMINAL, ""),
        (TO_GUARDED, ""),
        (GATE_CONDITIONED, CI_GATE),
    ] {
        let err = compile_src_with(&floor_case("auto", t, g), false).unwrap_err();
        assert!(err.contains("E-DECIDER-FLOOR"), "got: {}", err);
    }
}

#[test]
fn auto_boolean_on_a_terminal_route_is_refused_however_the_value_is_spelled() {
    let auto_true = BOOL_DECIDER.replace(
        "true: {description: \"Yes.\"}",
        "true: {description: \"Yes.\", mode: auto}",
    );
    let terminal = "      - target: done
        when:
          ready: true
      - target: stopped
        when:
          ready: false
";
    let err = expect_err(&bool_tpl(&auto_true, terminal), "E-DECIDER-FLOOR");
    assert_names(
        &err,
        &["field \"ready\"", "value \"true\"", "\"done\"", "terminal"],
    );

    let quoted = terminal
        .replace("ready: true", "ready: \"true\"")
        .replace("ready: false", "ready: \"false\"");
    let err = expect_err(&bool_tpl(&auto_true, &quoted), "E-DECIDER-FLOOR");
    assert_names(&err, &["value \"true\"", "\"done\""]);

    let auto_false = BOOL_DECIDER.replace(
        "false: {description: \"No.\"}",
        "false: {description: \"No.\", mode: auto}",
    );
    let err = expect_err(&bool_tpl(&auto_false, &quoted), "E-DECIDER-FLOOR");
    assert_names(&err, &["value \"false\"", "\"stopped\""]);
}

#[test]
fn floor_helper_reports_every_violation_without_compiling() {
    // Compile a template the floor accepts (shadow), then ask the helper what
    // auto would break: it must not depend on the declared mode.
    let t = expect_ok(&floor_case("shadow", TO_TERMINAL, ""));
    let v = t.floor_violations("review", "verdict", "proceed");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].target, "done");
    assert_eq!(v[0].transition_index, 0);
    assert_eq!(v[0].kind, FloorViolationKind::TerminalTarget);
    assert!(t
        .floor_violations("review", "verdict", "unclear")
        .is_empty());
    assert!(t.floor_violations("nope", "verdict", "proceed").is_empty());

    let t = expect_ok(&floor_case("shadow", GATE_CONDITIONED, CI_GATE));
    let v = t.floor_violations("review", "verdict", "proceed");
    assert_eq!(
        v.iter().map(|x| x.kind.clone()).collect::<Vec<_>>(),
        vec![FloorViolationKind::GateConditioned {
            gate_key: "gates.ci.exit_code".into()
        }]
    );
    // exit routes to a terminal state.
    let v = t.floor_violations("review", "verdict", "exit");
    assert_eq!(v[0].kind, FloorViolationKind::TerminalTarget);
    assert_eq!(v[0].target, "stopped");

    // The per-transition half works on any transition, however it reaches
    // the field, which is what the runtime recheck needs.
    let t = expect_ok(&floor_case("shadow", TO_GUARDED, ""));
    let tr = &t.states["review"].transitions[0];
    assert_eq!(
        t.transition_floor_violations(tr),
        vec![FloorViolationKind::ConfirmationRequired]
    );
    let mut presence = tr.clone();
    presence.when = Some(
        [
            ("evidence.verdict".to_string(), serde_json::json!("present")),
            ("gates.ci.exit_code".to_string(), serde_json::json!(0)),
        ]
        .into_iter()
        .collect(),
    );
    presence.target = "done".into();
    assert_eq!(
        t.transition_floor_violations(&presence),
        vec![
            FloorViolationKind::TerminalTarget,
            FloorViolationKind::GateConditioned {
                gate_key: "gates.ci.exit_code".into()
            }
        ]
    );
}

// ---------------------------------------------------------------------------
// agent-facing contract
// ---------------------------------------------------------------------------

#[test]
fn declared_fields_carry_question_and_value_descriptions() {
    let t = compile(&declared_fixture(), true).unwrap();

    let expects = derive_expects(&t.states["review"]).unwrap();
    let verdict = serde_json::to_value(&expects.fields["verdict"]).unwrap();
    assert_eq!(
        verdict["description"],
        "Is the plan outline item clear and scoped enough to implement?"
    );
    assert_eq!(
        verdict["value_descriptions"],
        serde_json::json!({
            "proceed": "Names a concrete change with checkable criteria.",
            "exit": "Vague, contradictory, or needs design first."
        })
    );
    assert_eq!(verdict["values"], serde_json::json!(["proceed", "exit"]));
    let text = serde_json::to_string(&expects).unwrap();
    assert!(!text.contains("unclear"), "escape leaked: {}", text);
    assert!(!text.contains("decider"), "declaration leaked: {}", text);

    // An undeclared sibling with a description gets neither key.
    let rationale = serde_json::to_value(&expects.fields["rationale"]).unwrap();
    assert!(rationale.get("description").is_none());
    assert!(rationale.get("value_descriptions").is_none());

    let expects = derive_expects(&t.states["confirm"]).unwrap();
    let ready = serde_json::to_value(&expects.fields["ready"]).unwrap();
    assert_eq!(
        ready["value_descriptions"],
        serde_json::json!({
            "true": "Only the named files changed.",
            "false": "Other files changed too."
        })
    );
}

#[test]
fn undeclared_fields_serialize_as_before() {
    // The exact bytes a field produced before decider blocks existed.
    let schema = ExpectsFieldSchema {
        field_type: "enum".into(),
        required: true,
        values: vec!["a".into(), "b".into()],
        item_schema: None,
        description: None,
        value_descriptions: None,
    };
    assert_eq!(
        serde_json::to_string(&schema).unwrap(),
        r#"{"type":"enum","required":true,"values":["a","b"]}"#
    );

    let t = compile(
        &manifest_dir().join("tests/fixtures/template-hash/no-decider.md"),
        true,
    )
    .unwrap();
    let expects = derive_expects(&t.states["review"]).unwrap();
    assert_eq!(
        serde_json::to_string(&expects.fields).unwrap(),
        r#"{"rationale":{"type":"string","required":false},"ready":{"type":"boolean","required":false},"verdict":{"type":"enum","required":true,"values":["proceed","exit"]}}"#
    );
}

#[test]
fn submitting_the_escape_is_rejected_like_any_outside_value() {
    let t = compile(&declared_fixture(), true).unwrap();
    let accepts = t.states["review"].accepts.as_ref().unwrap();
    let escape = validate_evidence(&serde_json::json!({"verdict": "unclear"}), accepts)
        .unwrap_err()
        .to_string();
    let other = validate_evidence(&serde_json::json!({"verdict": "bogus"}), accepts)
        .unwrap_err()
        .to_string();
    assert_eq!(escape.replace("unclear", "X"), other.replace("bogus", "X"));
    assert!(validate_evidence(&serde_json::json!({"verdict": "proceed"}), accepts).is_ok());
}

#[cfg(unix)]
#[test]
fn koto_next_and_status_carry_the_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let src = d.join("t.md");
    // Start directly at a declared state so no default_action runs.
    std::fs::write(
        &src,
        std::fs::read_to_string(declared_fixture())
            .unwrap()
            .replace("initial_state: gather", "initial_state: review"),
    )
    .unwrap();
    let koto = |args: &[&str]| {
        let out = assert_cmd::Command::cargo_bin("koto")
            .unwrap()
            .current_dir(d)
            .env("HOME", d)
            .env("XDG_CACHE_HOME", d.join("cache"))
            .env("KOTO_SESSIONS_BASE", d.join("sessions"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    koto(&["init", "s1", "--template", src.to_str().unwrap()]);
    for out in [koto(&["next", "s1"]), koto(&["status", "s1"])] {
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let verdict = &v["expects"]["fields"]["verdict"];
        assert_eq!(
            verdict["description"],
            "Is the plan outline item clear and scoped enough to implement?",
            "{}",
            out
        );
        assert_eq!(
            verdict["value_descriptions"]["proceed"],
            "Names a concrete change with checkable criteria."
        );
        assert!(!out.contains("unclear"), "escape leaked: {}", out);
        assert!(v["expects"]["fields"]["rationale"]
            .get("description")
            .is_none());
    }
}

// ---------------------------------------------------------------------------
// shipped templates declare nothing and round-trip
// ---------------------------------------------------------------------------

fn skill_templates() -> Vec<PathBuf> {
    fn walk(dir: &Path, in_templates: bool, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                let here = in_templates || p.file_name().is_some_and(|n| n == "koto-templates");
                walk(&p, here, out);
            } else if in_templates && p.extension().is_some_and(|e| e == "md") {
                // Only files that are templates: a template opens with YAML
                // front-matter. Rendered diagrams next to them don't.
                let text = std::fs::read_to_string(&p).unwrap();
                if text.starts_with("---") {
                    out.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&manifest_dir().join("plugins/koto-skills"), false, &mut out);
    out.sort();
    out
}

#[test]
fn skill_templates_have_no_decider_and_round_trip() {
    let templates = skill_templates();
    assert!(!templates.is_empty(), "no koto-templates found");
    for path in templates {
        let compiled = compile(&path, true)
            .unwrap_or_else(|e| panic!("{} failed to compile: {:#}", path.display(), e));
        let json = serde_json::to_string_pretty(&compiled).unwrap();
        assert!(
            !json.contains("\"decider\""),
            "{} compiled with a decider key",
            path.display()
        );
        let back: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_string_pretty(&back).unwrap(),
            json,
            "{} does not round-trip byte for byte",
            path.display()
        );
    }
}
