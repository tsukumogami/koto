//! Transition-level `context_assignments` (koto#204): the reference grammar,
//! its compile-time checks, and the pure resolver the advance loop calls.
//!
//! A transition may carry a map of context key to value. Each value is a
//! string that may hold references, alone or embedded in literal text:
//!
//! | Form | Resolves to |
//! |------|-------------|
//! | `{{VAR}}` | the session's variable (including a capture delivered earlier in the same tick) |
//! | `${evidence.<field>}` | the value submitted for `<field>` in the evidence that drove the transition |
//! | `${gates.<gate>.<path>}` | a dot path into `<gate>`'s structured output for that tick |
//!
//! Anything else inside `${...}` is refused at compile time. An evidence field
//! that was not submitted, a gate path absent from the output, and a variable
//! with no binding all resolve to the empty string, and the transition still
//! fires. Resolution is a single pass: a resolved value that itself contains
//! `{{X}}` or `${context.y}` is written literally and never expanded again.
//!
//! The grammar is generic over gate output on purpose. A gate path is walked
//! through whatever JSON the gate produced, so a gate type added later needs no
//! assignment-side support for its fields to be readable.

use std::collections::{BTreeMap, HashMap};

use regex::Regex;

use super::types::{
    TemplateState, Transition, VariableDecl, EVIDENCE_NAMESPACE, GATES_EVIDENCE_NAMESPACE,
};

/// One token in an assignment value: a `{{VAR}}` reference or a `${...}`
/// reference. Everything outside a match is literal text.
const ASSIGNMENT_REF_PATTERN: &str = r"\{\{([A-Z][A-Z0-9_]*)\}\}|\$\{([^}]*)\}";

fn ref_regex() -> Regex {
    Regex::new(ASSIGNMENT_REF_PATTERN).expect("ASSIGNMENT_REF_PATTERN is a valid regex")
}

/// A reference parsed out of an assignment value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentRef {
    /// `{{NAME}}`
    Var(String),
    /// `${evidence.<field>}`
    Evidence(String),
    /// `${gates.<gate>.<path...>}`, with at least one path segment.
    Gate { gate: String, path: Vec<String> },
    /// Any other `${...}`; carries the text between the braces.
    Unsupported(String),
}

impl AssignmentRef {
    fn parse_dollar(inner: &str) -> AssignmentRef {
        let segments: Vec<&str> = inner.split('.').collect();
        let well_formed = segments.iter().all(|s| !s.is_empty());
        match segments.first().copied() {
            Some(ns) if ns == EVIDENCE_NAMESPACE && segments.len() == 2 && well_formed => {
                AssignmentRef::Evidence(segments[1].to_string())
            }
            Some(ns) if ns == GATES_EVIDENCE_NAMESPACE && segments.len() >= 3 && well_formed => {
                AssignmentRef::Gate {
                    gate: segments[1].to_string(),
                    path: segments[2..].iter().map(|s| s.to_string()).collect(),
                }
            }
            _ => AssignmentRef::Unsupported(inner.to_string()),
        }
    }
}

/// Every reference in `value`, in order of appearance.
pub fn parse_refs(value: &str) -> Vec<AssignmentRef> {
    ref_regex()
        .captures_iter(value)
        .map(|caps| match (caps.get(1), caps.get(2)) {
            (Some(var), _) => AssignmentRef::Var(var.as_str().to_string()),
            (None, Some(inner)) => AssignmentRef::parse_dollar(inner.as_str()),
            (None, None) => unreachable!("one alternative always matches"),
        })
        .collect()
}

/// Compile-time checks for one transition's assignments.
///
/// `variables` is the template's variables block and `captures` its capture
/// names; either makes a `{{NAME}}` legal. The runtime names
/// (`SESSION_DIR`, `SESSION_NAME`) are refused: they are substituted by the
/// CLI outside the advance loop, which is where assignments resolve.
pub fn validate_transition_assignments(
    state_name: &str,
    state: &TemplateState,
    transition: &Transition,
    variables: &BTreeMap<String, VariableDecl>,
    captures: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in &transition.context_assignments {
        if let Some(reason) = crate::session::validate::unusable_context_key_reason(key) {
            return Err(format!(
                "state {:?} transition to {:?}: context_assignments key {:?} is not a usable context key: {}",
                state_name, transition.target, key, reason
            ));
        }
        for reference in parse_refs(value) {
            match reference {
                AssignmentRef::Var(name) => {
                    if !variables.contains_key(&name) && !captures.contains_key(&name) {
                        return Err(format!(
                            "state '{}': variable reference '{{{{{}}}}}' in context_assignments {:?} of the transition to '{}' is not declared in the template's variables block",
                            state_name, name, key, transition.target
                        ));
                    }
                }
                AssignmentRef::Evidence(field) => {
                    let declared = state
                        .accepts
                        .as_ref()
                        .is_some_and(|a| a.contains_key(&field));
                    if !declared {
                        return Err(format!(
                            "state {:?} transition to {:?}: context_assignments {:?} references evidence field {:?}, which is not declared in accepts",
                            state_name, transition.target, key, field
                        ));
                    }
                }
                AssignmentRef::Gate { gate, .. } => {
                    if !state.gates.contains_key(&gate) {
                        return Err(format!(
                            "state {:?}: context_assignments {:?} on the transition to {:?} references gate {:?} which is not declared in this state",
                            state_name, key, transition.target, gate
                        ));
                    }
                }
                AssignmentRef::Unsupported(inner) => {
                    return Err(format!(
                        "state {:?} transition to {:?}: context_assignments {:?} holds unsupported reference \"${{{}}}\"; \
                         an assignment may reference only ${{evidence.<field>}}, ${{gates.<gate>.<path>}} or {{{{VAR}}}}",
                        state_name, transition.target, key, inner
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Render a JSON value as the string an assignment stores.
fn render(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => value.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => value.to_string(),
    }
}

/// Walk `path` into `value`. Object members are looked up by name; array
/// elements by a decimal index.
fn walk<'a>(value: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// What an assignment value can read when its transition fires.
pub struct AssignmentInputs<'a> {
    /// Variables: the log's bindings with this tick's overlay layered over.
    pub variables: &'a HashMap<String, String>,
    /// The agent evidence that drove the transition (flat field map).
    pub evidence: &'a BTreeMap<String, serde_json::Value>,
    /// Structured gate output for this tick, keyed by gate name.
    pub gates: &'a serde_json::Map<String, serde_json::Value>,
}

/// Resolve one assignment value in a single pass.
pub fn resolve_value(value: &str, inputs: &AssignmentInputs<'_>) -> String {
    ref_regex()
        .replace_all(value, |caps: &regex::Captures<'_>| {
            let reference = match (caps.get(1), caps.get(2)) {
                (Some(var), _) => AssignmentRef::Var(var.as_str().to_string()),
                (None, Some(inner)) => AssignmentRef::parse_dollar(inner.as_str()),
                (None, None) => unreachable!("one alternative always matches"),
            };
            match reference {
                AssignmentRef::Var(name) => {
                    inputs.variables.get(&name).cloned().unwrap_or_default()
                }
                AssignmentRef::Evidence(field) => {
                    inputs.evidence.get(&field).map(render).unwrap_or_default()
                }
                AssignmentRef::Gate { gate, path } => inputs
                    .gates
                    .get(&gate)
                    .and_then(|output| walk(output, &path))
                    .map(render)
                    .unwrap_or_default(),
                // Refused at compile time; a template that bypassed the
                // compiler keeps the text as written rather than guessing.
                AssignmentRef::Unsupported(_) => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// Resolve every assignment on `transition`. `None` when it declares none, so
/// the `Transitioned` event omits the field.
pub fn resolve_assignments(
    transition: &Transition,
    inputs: &AssignmentInputs<'_>,
) -> Option<BTreeMap<String, String>> {
    if transition.context_assignments.is_empty() {
        return None;
    }
    Some(
        transition
            .context_assignments
            .iter()
            .map(|(key, value)| (key.clone(), resolve_value(value, inputs)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inputs_with<'a>(
        variables: &'a HashMap<String, String>,
        evidence: &'a BTreeMap<String, serde_json::Value>,
        gates: &'a serde_json::Map<String, serde_json::Value>,
    ) -> AssignmentInputs<'a> {
        AssignmentInputs {
            variables,
            evidence,
            gates,
        }
    }

    #[test]
    fn parses_each_reference_form() {
        let refs = parse_refs("a {{TOPIC}} ${evidence.detail} ${gates.ci.exit_code} ${context.x}");
        assert_eq!(
            refs,
            vec![
                AssignmentRef::Var("TOPIC".into()),
                AssignmentRef::Evidence("detail".into()),
                AssignmentRef::Gate {
                    gate: "ci".into(),
                    path: vec!["exit_code".into()]
                },
                AssignmentRef::Unsupported("context.x".into()),
            ]
        );
    }

    #[test]
    fn malformed_namespaces_are_unsupported() {
        for inner in [
            "evidence",
            "evidence.a.b",
            "gates.g",
            "gates..x",
            "foo.bar",
            "",
        ] {
            assert_eq!(
                parse_refs(&format!("${{{}}}", inner)),
                vec![AssignmentRef::Unsupported(inner.to_string())],
                "{inner}"
            );
        }
    }

    #[test]
    fn literal_is_written_as_is() {
        let (v, e, g) = (HashMap::new(), BTreeMap::new(), serde_json::Map::new());
        assert_eq!(resolve_value("done", &inputs_with(&v, &e, &g)), "done");
    }

    #[test]
    fn gate_path_walks_nested_output() {
        let (v, e) = (HashMap::new(), BTreeMap::new());
        let mut g = serde_json::Map::new();
        g.insert(
            "g".into(),
            json!({"payload": {"pr": "x", "n": 3, "list": ["a", "b"]}}),
        );
        let inputs = inputs_with(&v, &e, &g);
        assert_eq!(resolve_value("${gates.g.payload.pr}", &inputs), "x");
        assert_eq!(resolve_value("${gates.g.payload.n}", &inputs), "3");
        assert_eq!(resolve_value("${gates.g.payload.list.1}", &inputs), "b");
        assert_eq!(resolve_value("${gates.g.payload.missing}", &inputs), "");
        assert_eq!(resolve_value("${gates.other.payload.pr}", &inputs), "");
    }

    #[test]
    fn evidence_resolves_inside_a_literal_and_absent_is_empty() {
        let (v, g) = (HashMap::new(), serde_json::Map::new());
        let mut e = BTreeMap::new();
        e.insert("detail".to_string(), json!("disk full"));
        e.insert("count".to_string(), json!(2));
        let inputs = inputs_with(&v, &e, &g);
        assert_eq!(
            resolve_value("blocked: ${evidence.detail} (${evidence.count})", &inputs),
            "blocked: disk full (2)"
        );
        assert_eq!(resolve_value("x${evidence.optional}y", &inputs), "xy");
    }

    #[test]
    fn resolved_values_are_not_expanded_again() {
        let mut v = HashMap::new();
        v.insert("TOPIC".to_string(), "real".to_string());
        let g = serde_json::Map::new();
        let mut e = BTreeMap::new();
        e.insert("detail".to_string(), json!("{{TOPIC}} ${context.y}"));
        let inputs = inputs_with(&v, &e, &g);
        assert_eq!(
            resolve_value("${evidence.detail}", &inputs),
            "{{TOPIC}} ${context.y}"
        );
        assert_eq!(resolve_value("{{TOPIC}}", &inputs), "real");
    }
}
