use std::collections::BTreeSet;

use crate::template::types::CompiledTemplate;

/// Generate stateDiagram-v2 Mermaid text from a compiled template.
///
/// Output uses LF line endings unconditionally. BTreeMap iteration on
/// `states` ensures deterministic output across runs.
pub fn to_mermaid(template: &CompiledTemplate) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("stateDiagram-v2".to_string());
    lines.push("    direction LR".to_string());

    // Initial state marker.
    lines.push(format!("    [*] --> {}", template.initial_state));

    // Collect terminal states for [*] markers at the end.
    let mut terminal_states: BTreeSet<&str> = BTreeSet::new();

    // States iterate in BTreeMap order (deterministic).
    for (state_name, state) in &template.states {
        if state.terminal {
            terminal_states.insert(state_name);
        }

        for transition in &state.transitions {
            let label = transition_label(transition);
            if let Some(label) = label {
                lines.push(format!(
                    "    {} --> {} : {}",
                    state_name, transition.target, label
                ));
            } else {
                lines.push(format!("    {} --> {}", state_name, transition.target));
            }
        }
    }

    // Terminal state markers.
    for state_name in &terminal_states {
        lines.push(format!("    {} --> [*]", state_name));
    }

    // Gate annotations.
    for (state_name, state) in &template.states {
        for gate_name in state.gates.keys() {
            lines.push(format!(
                "    note left of {} : gate: {}",
                state_name, gate_name
            ));
        }
    }

    // Join with LF, ensure trailing newline.
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

/// Build a transition label from the `when` conditions.
///
/// Returns `None` for unconditional transitions. For conditional transitions,
/// formats each condition as `key: value` and joins them with `, `.
fn transition_label(transition: &crate::template::types::Transition) -> Option<String> {
    let when = transition.when.as_ref()?;
    if when.is_empty() {
        return None;
    }

    // BTreeMap iterates in sorted key order.
    let parts: Vec<String> = when
        .iter()
        .map(|(key, value)| {
            let val_str = match value.as_str() {
                Some(s) => s.to_string(),
                None => value.to_string(),
            };
            format!("{}: {}", key, val_str)
        })
        .collect();

    Some(parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::types::{
        CompiledTemplate, Gate, TemplateState, Transition, GATE_TYPE_COMMAND,
    };
    use std::collections::BTreeMap;

    fn minimal_template() -> CompiledTemplate {
        let mut states = BTreeMap::new();
        states.insert(
            "start".to_string(),
            TemplateState {
                directive: "Begin.".to_string(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "done".to_string(),
            TemplateState {
                directive: "Done.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        CompiledTemplate {
            format_version: 1,
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "start".to_string(),
            variables: BTreeMap::new(),
            states,
        }
    }

    #[test]
    fn minimal_template_produces_valid_mermaid() {
        let output = to_mermaid(&minimal_template());
        assert!(output.starts_with("stateDiagram-v2\n"));
        assert!(output.contains("direction LR"));
        assert!(output.contains("[*] --> start"));
        assert!(output.contains("start --> done"));
        assert!(output.contains("done --> [*]"));
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn lf_line_endings_only() {
        let output = to_mermaid(&minimal_template());
        assert!(!output.contains("\r\n"), "output must use LF, not CRLF");
        assert!(!output.contains('\r'), "output must not contain CR");
    }

    #[test]
    fn deterministic_output() {
        let t = minimal_template();
        let first = to_mermaid(&t);
        let second = to_mermaid(&t);
        assert_eq!(first, second, "output must be byte-identical across runs");
    }

    #[test]
    fn single_terminal_state() {
        let mut states = BTreeMap::new();
        states.insert(
            "only".to_string(),
            TemplateState {
                directive: "The only state.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let t = CompiledTemplate {
            format_version: 1,
            name: "single".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "only".to_string(),
            variables: BTreeMap::new(),
            states,
        };
        let output = to_mermaid(&t);
        assert!(output.contains("[*] --> only"));
        assert!(output.contains("only --> [*]"));
        // No transition lines between states.
        let lines: Vec<&str> = output.lines().collect();
        let transition_lines: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("-->") && !l.contains("[*]"))
            .collect();
        assert!(
            transition_lines.is_empty(),
            "single-state template should have no inter-state transitions"
        );
    }

    #[test]
    fn conditional_transitions_show_labels() {
        let mut states = BTreeMap::new();
        let mut when_build = BTreeMap::new();
        when_build.insert("route".to_string(), serde_json::json!("build"));
        let mut when_investigate = BTreeMap::new();
        when_investigate.insert("route".to_string(), serde_json::json!("investigate"));

        states.insert(
            "evaluate".to_string(),
            TemplateState {
                directive: "Evaluate.".to_string(),
                transitions: vec![
                    Transition {
                        target: "implement".to_string(),
                        when: Some(when_build),
                    },
                    Transition {
                        target: "research".to_string(),
                        when: Some(when_investigate),
                    },
                ],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "implement".to_string(),
            TemplateState {
                directive: "Implement.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "research".to_string(),
            TemplateState {
                directive: "Research.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );

        let t = CompiledTemplate {
            format_version: 1,
            name: "branching".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "evaluate".to_string(),
            variables: BTreeMap::new(),
            states,
        };
        let output = to_mermaid(&t);
        assert!(
            output.contains("evaluate --> implement : route: build"),
            "got:\n{}",
            output
        );
        assert!(
            output.contains("evaluate --> research : route: investigate"),
            "got:\n{}",
            output
        );
    }

    #[test]
    fn gate_annotations() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "check-repo".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "test -d .git".to_string(),
                timeout: 0,
            },
        );
        let output = to_mermaid(&t);
        assert!(
            output.contains("note left of start : gate: check-repo"),
            "got:\n{}",
            output
        );
    }

    #[test]
    fn matches_design_example() {
        // Reproduce the 5-state example from the design doc.
        let mut states = BTreeMap::new();
        states.insert(
            "explore".to_string(),
            TemplateState {
                directive: "Explore.".to_string(),
                transitions: vec![Transition {
                    target: "evaluate".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: {
                    let mut g = BTreeMap::new();
                    g.insert(
                        "check-repo".to_string(),
                        Gate {
                            gate_type: GATE_TYPE_COMMAND.to_string(),
                            command: "test -d .git".to_string(),
                            timeout: 0,
                        },
                    );
                    g
                },
                accepts: None,
                integration: None,
                default_action: None,
            },
        );

        let mut when_build = BTreeMap::new();
        when_build.insert("route".to_string(), serde_json::json!("build"));
        let mut when_investigate = BTreeMap::new();
        when_investigate.insert("route".to_string(), serde_json::json!("investigate"));

        states.insert(
            "evaluate".to_string(),
            TemplateState {
                directive: "Evaluate.".to_string(),
                transitions: vec![
                    Transition {
                        target: "implement".to_string(),
                        when: Some(when_build),
                    },
                    Transition {
                        target: "research".to_string(),
                        when: Some(when_investigate),
                    },
                ],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "implement".to_string(),
            TemplateState {
                directive: "Implement.".to_string(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "research".to_string(),
            TemplateState {
                directive: "Research.".to_string(),
                transitions: vec![Transition {
                    target: "evaluate".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        states.insert(
            "done".to_string(),
            TemplateState {
                directive: "Done.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );

        let t = CompiledTemplate {
            format_version: 1,
            name: "example".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "explore".to_string(),
            variables: BTreeMap::new(),
            states,
        };

        let output = to_mermaid(&t);
        // Verify all expected lines from the design doc example.
        assert!(output.contains("[*] --> explore"));
        assert!(output.contains("explore --> evaluate"));
        assert!(output.contains("evaluate --> implement : route: build"));
        assert!(output.contains("evaluate --> research : route: investigate"));
        assert!(output.contains("implement --> done"));
        assert!(output.contains("research --> evaluate"));
        assert!(output.contains("done --> [*]"));
        assert!(output.contains("note left of explore : gate: check-repo"));
    }
}
