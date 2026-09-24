//! The grammar of a terminal state's declared `result:` map.
//!
//! A terminal state may declare `result:`, a flat map from key to string
//! value. koto resolves the map once, on the tick that lands the session in
//! that terminal, and writes it into the [`WorkflowResult`]'s `payload`, so
//! the outcome a workflow reports is something the template declares rather
//! than something an agent composes.
//!
//! A value is a string that may mix three forms:
//!
//! - literal text, copied as-is;
//! - `{{VAR}}`, resolved through the session's variables;
//! - `${context.<key>}`, resolved to the context content stored under `<key>`.
//!
//! No other `${...}` namespace is allowed here. `${evidence.x}` and
//! `${gates.g.x}` name things that only exist while a transition is being
//! taken, and a terminal result is read long after that.
//!
//! This module owns the grammar in one place: the compiler checks a template
//! against it, and the runtime resolver in
//! `crate::engine::terminal_result` scans values with the same token pattern,
//! so the two cannot disagree about what a reference is.
//!
//! [`WorkflowResult`]: crate::engine::types::WorkflowResult

use std::collections::BTreeMap;

use regex::Regex;

use super::types::VAR_REF_PATTERN;

/// Most keys a `result:` map may declare.
///
/// A result rides several logs and every `koto next` terminal response, so
/// it is bounded. The limit counts declared keys only; the reserved
/// [`RESULT_MISSING_KEY`] that the runtime may add is not one of them.
pub const RESULT_MAX_KEYS: usize = 32;

/// The payload key koto uses to list result keys that did not resolve.
///
/// Reserved: a template may not declare it, so a reader can always tell a
/// declared value from koto's report of unresolved ones.
pub const RESULT_MISSING_KEY: &str = "missing";

/// Pattern matching one reference in a result value: either `{{VAR}}`
/// (capture group 1) or `${context.<key>}` (capture group 2).
///
/// The context key capture is deliberately loose (`[^}]*`): the compiler
/// validates the captured key against the context-key grammar and reports
/// a precise reason, rather than letting a malformed key fall through as
/// literal text.
pub fn result_ref_regex() -> Regex {
    Regex::new(&format!(r"{}|\$\{{context\.([^}}]*)\}}", VAR_REF_PATTERN))
        .expect("result reference pattern is a valid regex")
}

/// Validate a terminal state's `result:` map.
///
/// `is_declared` answers whether a `{{VAR}}` name resolves at run time: a
/// declared variable, a capture name, or a runtime name. The wording of the
/// undeclared-variable error matches the other substitutable fields.
pub fn validate_result_map(
    state_name: &str,
    terminal: bool,
    result: &BTreeMap<String, String>,
    is_declared: impl Fn(&str) -> bool,
) -> Result<(), String> {
    if !terminal {
        return Err(format!(
            "state {:?}: result is declared on a non-terminal state; only a terminal \
             state reports a result\n  \
             remedy: move the result map to the terminal state this state leads to, \
             or mark the state terminal",
            state_name
        ));
    }
    if result.len() > RESULT_MAX_KEYS {
        return Err(format!(
            "state {:?}: result declares {} keys; the limit is {}\n  \
             remedy: drop keys a reader does not route on",
            state_name,
            result.len(),
            RESULT_MAX_KEYS
        ));
    }

    let re = result_ref_regex();
    for (key, value) in result {
        if key == RESULT_MISSING_KEY {
            return Err(format!(
                "state {:?}: result key {:?} is reserved; koto uses it to list result \
                 keys that did not resolve\n  \
                 remedy: rename the key",
                state_name, key
            ));
        }
        if let Some(reason) = crate::session::validate::unusable_context_key_reason(key) {
            return Err(format!(
                "state {:?}: result key {:?} is not a valid key: {}",
                state_name, key, reason
            ));
        }

        // Every reference in the value, and everything that looks like one.
        let mut covered: Vec<(usize, usize)> = Vec::new();
        for caps in re.captures_iter(value) {
            let whole = caps.get(0).expect("group 0 always matches");
            covered.push((whole.start(), whole.end()));
            if let Some(var) = caps.get(1) {
                let name = var.as_str();
                if !is_declared(name) {
                    return Err(format!(
                        "state '{}': variable reference '{{{{{}}}}}' in result key '{}' is not declared in the template's variables block",
                        state_name, name, key
                    ));
                }
            } else if let Some(ctx_key) = caps.get(2) {
                if let Some(reason) =
                    crate::session::validate::unusable_context_key_reason(ctx_key.as_str())
                {
                    return Err(format!(
                        "state {:?}: result key {:?} references ${{context.{}}}, which \
                         is not a valid context key: {}",
                        state_name,
                        key,
                        ctx_key.as_str(),
                        reason
                    ));
                }
            }
        }

        // Any `${` not consumed by a `${context.<key>}` match is another
        // namespace (`${evidence.x}`, `${gates.g.x}`) or an unterminated
        // reference. Neither may pass through as literal text: an author
        // who wrote one expected it to resolve.
        for (pos, _) in value.match_indices("${") {
            if covered.iter().any(|(s, e)| pos >= *s && pos < *e) {
                continue;
            }
            let rest = &value[pos..];
            let shown: String = match rest.find('}') {
                Some(end) => rest[..=end].to_string(),
                None => rest.to_string(),
            };
            return Err(format!(
                "state {:?}: result key {:?} holds {:?}, which a result cannot \
                 resolve; a result value may hold literal text, {{{{VAR}}}}, and \
                 ${{context.<key>}} only\n  \
                 remedy: record the value in the context store (a transition's \
                 context_assignments, or `koto context add`) and reference it as \
                 ${{context.<key>}}",
                state_name, key, shown
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn declared(name: &str) -> bool {
        name == "TOPIC"
    }

    #[test]
    fn accepts_all_three_forms() {
        let m = map(&[
            ("outcome", "error"),
            ("topic", "{{TOPIC}}"),
            ("step", "${context.step}"),
            ("mixed", "merge-state:${context.state}/{{TOPIC}}"),
        ]);
        validate_result_map("done", true, &m, declared).unwrap();
    }

    #[test]
    fn rejects_non_terminal() {
        let err = validate_result_map("work", false, &map(&[("a", "b")]), declared).unwrap_err();
        assert!(err.contains("\"work\""), "{err}");
        assert!(err.contains("non-terminal"), "{err}");
    }

    #[test]
    fn thirty_two_keys_pass_and_thirty_three_fail() {
        let mut m = BTreeMap::new();
        for i in 0..RESULT_MAX_KEYS {
            m.insert(format!("k{i}"), "v".to_string());
        }
        validate_result_map("done", true, &m, declared).unwrap();
        m.insert("k32".to_string(), "v".to_string());
        let err = validate_result_map("done", true, &m, declared).unwrap_err();
        assert!(err.contains("\"done\""), "{err}");
        assert!(err.contains("33"), "{err}");
        assert!(err.contains("32"), "{err}");
    }

    #[test]
    fn rejects_reserved_missing_key() {
        let err =
            validate_result_map("done", true, &map(&[("missing", "x")]), declared).unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn rejects_bad_key() {
        let err =
            validate_result_map("done", true, &map(&[("bad key", "x")]), declared).unwrap_err();
        assert!(err.contains("not usable"), "{err}");
    }

    #[test]
    fn rejects_undeclared_var() {
        let err =
            validate_result_map("done", true, &map(&[("t", "{{NOPE}}")]), declared).unwrap_err();
        assert!(
            err.contains("is not declared in the template's variables block"),
            "{err}"
        );
    }

    #[test]
    fn rejects_bad_context_key() {
        let err = validate_result_map("done", true, &map(&[("s", "${context.a b}")]), declared)
            .unwrap_err();
        assert!(err.contains("not a valid context key"), "{err}");
    }

    #[test]
    fn rejects_other_namespaces_and_unterminated() {
        for v in ["${evidence.x}", "${gates.g.x}", "pre-${context.x", "${}"] {
            let err = validate_result_map("done", true, &map(&[("s", v)]), declared).unwrap_err();
            assert!(err.contains("cannot resolve"), "{v}: {err}");
        }
    }

    #[test]
    fn plain_dollar_is_literal() {
        validate_result_map("done", true, &map(&[("s", "costs $5")]), declared).unwrap();
    }
}

/// The same rules end to end through the compiler, from YAML source.
#[cfg(test)]
mod compile_tests {
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
}
