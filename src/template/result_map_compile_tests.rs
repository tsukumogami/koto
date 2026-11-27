//! The result-map rules of `template::result_map` end to end through the
//! compiler, from YAML source.

use std::io::Write;

use tempfile::NamedTempFile;

use crate::template::compile::compile;

/// A two-state template: `work` leads to `done`, and `extra` is spliced
/// into the front matter of the named state.
fn source(work_extra: &str, done_extra: &str) -> String {
    format!(
        r#"---
name: result-map
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: false
states:
  work:
    transitions:
      - target: done
{work_extra}  done:
    terminal: true
{done_extra}---

## work

Work.

## done

Done.
"#
    )
}

fn compile_src(src: &str) -> anyhow::Result<crate::template::types::CompiledTemplate> {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(src.as_bytes()).unwrap();
    compile(f.path(), true)
}

fn result_block(n: usize) -> String {
    let mut s = String::from("    result:\n");
    for i in 0..n {
        s.push_str(&format!("      k{i}: v\n"));
    }
    s
}

fn err_of(src: &str) -> String {
    format!(
        "{:#}",
        compile_src(src).expect_err("compilation should fail")
    )
}

#[test]
fn a_template_without_result_omits_the_key_from_compiled_json() {
    let compiled = compile_src(&source("", "")).unwrap();
    assert!(compiled.states["done"].result.is_none());
    let json = serde_json::to_string(&compiled).unwrap();
    assert!(!json.contains("\"result\""), "{json}");
}

#[test]
fn a_declared_map_compiles_into_the_terminal_state() {
    let compiled = compile_src(&source(
            "",
            "    result:\n      outcome: error\n      topic: \"{{TOPIC}}\"\n      step: \"${context.step}\"\n      pr: 12\n",
        ))
        .unwrap();
    let result = compiled.states["done"].result.as_ref().unwrap();
    assert_eq!(result["outcome"], "error");
    assert_eq!(result["topic"], "{{TOPIC}}");
    assert_eq!(result["step"], "${context.step}");
    assert_eq!(result["pr"], "12", "a scalar literal compiles as its text");
}

#[test]
fn thirty_two_keys_compile_and_thirty_three_fail() {
    compile_src(&source("", &result_block(32))).expect("32 keys is the limit, inclusive");
    let err = err_of(&source("", &result_block(33)));
    assert!(err.contains("\"done\""), "{err}");
    assert!(err.contains("33 keys"), "{err}");
    assert!(err.contains("the limit is 32"), "{err}");
}

#[test]
fn a_result_on_a_non_terminal_state_fails_naming_it() {
    let err = err_of(&source("    result:\n      a: b\n", ""));
    assert!(err.contains("\"work\""), "{err}");
    assert!(err.contains("non-terminal"), "{err}");
}

#[test]
fn a_bad_key_or_the_reserved_key_fails() {
    let err = err_of(&source("", "    result:\n      \"bad key\": x\n"));
    assert!(err.contains("\"bad key\""), "{err}");
    let err = err_of(&source("", "    result:\n      missing: x\n"));
    assert!(err.contains("reserved"), "{err}");
}

#[test]
fn a_mapping_or_sequence_value_fails() {
    let err = err_of(&source("", "    result:\n      nested:\n        a: b\n"));
    assert!(err.contains("a mapping"), "{err}");
    assert!(err.contains("must be a string"), "{err}");
    let err = err_of(&source("", "    result:\n      list: [a, b]\n"));
    assert!(err.contains("a sequence"), "{err}");
}

#[test]
fn an_undeclared_variable_fails_with_the_shared_wording() {
    let err = err_of(&source("", "    result:\n      t: \"{{NOPE}}\"\n"));
    assert!(
        err.contains("is not declared in the template's variables block"),
        "{err}"
    );
}

#[test]
fn a_bad_context_key_or_another_namespace_fails() {
    let err = err_of(&source("", "    result:\n      s: \"${context.a b}\"\n"));
    assert!(err.contains("not a valid context key"), "{err}");
    for v in ["${evidence.x}", "${gates.g.exit_code}"] {
        let err = err_of(&source("", &format!("    result:\n      s: \"{v}\"\n")));
        assert!(err.contains("cannot resolve"), "{v}: {err}");
    }
}
