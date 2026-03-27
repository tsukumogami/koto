use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{anyhow, Context};
use serde::Deserialize;

use super::types::{
    ActionDecl, CompiledTemplate, FieldSchema, Gate, PollingConfig, TemplateState, Transition,
    VariableDecl, GATE_TYPE_COMMAND, GATE_TYPE_CONTEXT_EXISTS, GATE_TYPE_CONTEXT_MATCHES,
};

/// YAML front-matter structure of a template source file.
#[derive(Debug, Deserialize)]
struct SourceFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    initial_state: String,
    #[serde(default)]
    variables: HashMap<String, SourceVariable>,
    #[serde(default)]
    states: HashMap<String, SourceState>,
}

#[derive(Debug, Deserialize, Default)]
struct SourceVariable {
    #[serde(default)]
    description: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default: String,
}

#[derive(Debug, Deserialize, Default)]
struct SourceState {
    #[serde(default)]
    transitions: Vec<SourceTransition>,
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    gates: HashMap<String, SourceGate>,
    #[serde(default)]
    accepts: HashMap<String, SourceFieldSchema>,
    #[serde(default)]
    integration: Option<String>,
    #[serde(default)]
    default_action: Option<SourceActionDecl>,
}

/// Action declaration in source YAML.
#[derive(Debug, Deserialize)]
struct SourceActionDecl {
    #[serde(default)]
    command: String,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    requires_confirmation: bool,
    #[serde(default)]
    polling: Option<SourcePollingConfig>,
}

/// Polling configuration in source YAML.
#[derive(Debug, Deserialize)]
struct SourcePollingConfig {
    #[serde(default)]
    interval_secs: u32,
    #[serde(default)]
    timeout_secs: u32,
}

/// A transition in source YAML: either a bare string or a structured object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SourceTransition {
    /// Structured: `{target: "done", when: {field: value}}`
    Structured {
        target: String,
        #[serde(default)]
        when: Option<HashMap<String, serde_json::Value>>,
    },
}

/// Field schema in source YAML for an `accepts` block.
#[derive(Debug, Deserialize)]
struct SourceFieldSchema {
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    values: Vec<String>,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct SourceGate {
    #[serde(rename = "type")]
    gate_type: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    timeout: u32,
    #[serde(default)]
    key: String,
    #[serde(default)]
    pattern: String,
}

/// Compile a YAML/Markdown template source file to a FormatVersion=1 CompiledTemplate.
pub fn compile(source_path: &Path) -> anyhow::Result<CompiledTemplate> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read template source: {}", source_path.display()))?;

    let (frontmatter_str, body) = split_frontmatter(&content).ok_or_else(|| {
        anyhow!("invalid YAML: template must begin with YAML front-matter delimited by '---'")
    })?;

    let fm: SourceFrontmatter = serde_yml::from_str(frontmatter_str)
        .with_context(|| "invalid YAML: failed to parse front-matter")?;

    // Validate required front-matter fields.
    if fm.name.is_empty() {
        return Err(anyhow!("missing required field: name"));
    }
    if fm.version.is_empty() {
        return Err(anyhow!("missing required field: version"));
    }
    if fm.initial_state.is_empty() {
        return Err(anyhow!("missing required field: initial_state"));
    }
    if fm.states.is_empty() {
        return Err(anyhow!("template has no states"));
    }

    // Extract directives from the markdown body for each declared state.
    let directives = extract_directives(&fm.states, body);

    // Build compiled states.
    let mut compiled_states: BTreeMap<String, TemplateState> = BTreeMap::new();
    for (state_name, source_state) in &fm.states {
        let directive = directives.get(state_name).cloned().unwrap_or_default();
        if directive.is_empty() {
            return Err(anyhow!(
                "state {:?} has no directive section in markdown body",
                state_name
            ));
        }

        let mut compiled_gates: BTreeMap<String, Gate> = BTreeMap::new();
        for (gate_name, source_gate) in &source_state.gates {
            let gate = compile_gate(state_name, gate_name, source_gate)?;
            compiled_gates.insert(gate_name.clone(), gate);
        }

        // Transform source transitions to compiled transitions.
        let compiled_transitions: Vec<Transition> = source_state
            .transitions
            .iter()
            .map(|st| match st {
                SourceTransition::Structured { target, when } => Transition {
                    target: target.clone(),
                    when: when
                        .as_ref()
                        .map(|w| w.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                },
            })
            .collect();

        // Transform source accepts to compiled accepts.
        let compiled_accepts: Option<BTreeMap<String, FieldSchema>> =
            if source_state.accepts.is_empty() {
                None
            } else {
                Some(
                    source_state
                        .accepts
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                FieldSchema {
                                    field_type: v.field_type.clone(),
                                    required: v.required,
                                    values: v.values.clone(),
                                    description: v.description.clone(),
                                },
                            )
                        })
                        .collect(),
                )
            };

        // Transform source default_action to compiled ActionDecl.
        let compiled_action = source_state.default_action.as_ref().map(|sa| ActionDecl {
            command: sa.command.clone(),
            working_dir: sa.working_dir.clone(),
            requires_confirmation: sa.requires_confirmation,
            polling: sa.polling.as_ref().map(|sp| PollingConfig {
                interval_secs: sp.interval_secs,
                timeout_secs: sp.timeout_secs,
            }),
        });

        compiled_states.insert(
            state_name.clone(),
            TemplateState {
                directive,
                transitions: compiled_transitions,
                terminal: source_state.terminal,
                gates: compiled_gates,
                accepts: compiled_accepts,
                integration: source_state.integration.clone(),
                default_action: compiled_action,
            },
        );
    }

    // Validate transition targets exist.
    for (state_name, state) in &compiled_states {
        for transition in &state.transitions {
            if !compiled_states.contains_key(&transition.target) {
                return Err(anyhow!(
                    "state {:?} references undefined transition target {:?}",
                    state_name,
                    transition.target
                ));
            }
        }
    }

    // Validate initial_state is declared.
    if !compiled_states.contains_key(&fm.initial_state) {
        return Err(anyhow!(
            "initial_state {:?} is not a declared state",
            fm.initial_state
        ));
    }

    // Build compiled variables.
    let variables: BTreeMap<String, VariableDecl> = fm
        .variables
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                VariableDecl {
                    description: v.description,
                    required: v.required,
                    default: v.default,
                },
            )
        })
        .collect();

    let template = CompiledTemplate {
        format_version: 1,
        name: fm.name,
        version: fm.version,
        description: fm.description,
        initial_state: fm.initial_state,
        variables,
        states: compiled_states,
    };

    // Run validation rules (includes evidence routing validation).
    template
        .validate()
        .map_err(|e| anyhow!("validation error: {}", e))?;

    Ok(template)
}

fn compile_gate(state_name: &str, gate_name: &str, source: &SourceGate) -> anyhow::Result<Gate> {
    match source.gate_type.as_str() {
        GATE_TYPE_COMMAND => {
            if source.command.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: command must not be empty",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: source.command.clone(),
                timeout: source.timeout,
                key: String::new(),
                pattern: String::new(),
            })
        }
        GATE_TYPE_CONTEXT_EXISTS => {
            if source.key.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-exists gate must have a non-empty key",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: source.key.clone(),
                pattern: String::new(),
            })
        }
        GATE_TYPE_CONTEXT_MATCHES => {
            if source.key.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-matches gate must have a non-empty key",
                    state_name,
                    gate_name
                ));
            }
            if source.pattern.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-matches gate must have a non-empty pattern",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: source.key.clone(),
                pattern: source.pattern.clone(),
            })
        }
        other => Err(anyhow!(
            "state {:?} gate {:?}: unsupported gate type {:?}. \
             Field-based gates (field_not_empty, field_equals) have been replaced by accepts/when. \
             Use accepts blocks for evidence schema and when conditions for routing.",
            state_name,
            gate_name,
            other
        )),
    }
}

/// Split a markdown file into front-matter and body.
/// Returns (frontmatter_str, body_str) if the file starts with `---`.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim_start_matches('\n');
    if !content.starts_with("---") {
        return None;
    }
    // Find the closing `---` delimiter.
    let after_open = &content[3..];
    // Skip a newline immediately after the opening `---`.
    let after_open = after_open.trim_start_matches('\n');
    // Find the closing delimiter.
    let close_pos = find_frontmatter_close(after_open)?;
    let frontmatter = &after_open[..close_pos];
    let rest = &after_open[close_pos..];
    // Skip past the closing `---` line.
    let body_start = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
    Some((frontmatter, &rest[body_start..]))
}

fn find_frontmatter_close(s: &str) -> Option<usize> {
    let mut pos = 0;
    for line in s.lines() {
        if line.trim() == "---" {
            return Some(pos);
        }
        pos += line.len() + 1; // +1 for the newline
    }
    None
}

/// Extract directive content for each declared state from the markdown body.
///
/// States are identified by `## <state-name>` headings. Content between two
/// consecutive state headings belongs to the first. The declared state list
/// from the front-matter is the authority — headings that don't match a
/// declared state name are treated as directive content, not state boundaries.
fn extract_directives(
    states: &HashMap<String, SourceState>,
    body: &str,
) -> HashMap<String, String> {
    let state_names: std::collections::HashSet<&str> = states.keys().map(|s| s.as_str()).collect();

    let mut directives: HashMap<String, String> = HashMap::new();
    let mut current_state: Option<&str> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = parse_h2_heading(line) {
            if state_names.contains(heading) {
                // Save the previous state's directive.
                if let Some(state) = current_state {
                    directives.insert(
                        state.to_string(),
                        current_lines.join("\n").trim().to_string(),
                    );
                }
                current_state = Some(heading);
                current_lines.clear();
            } else {
                // Not a state boundary — treat as content.
                current_lines.push(line);
            }
        } else {
            current_lines.push(line);
        }
    }

    // Save the last state's directive.
    if let Some(state) = current_state {
        directives.insert(
            state.to_string(),
            current_lines.join("\n").trim().to_string(),
        );
    }

    directives
}

/// If the line is a `## heading`, return the heading text.
fn parse_h2_heading(line: &str) -> Option<&str> {
    let line = line.trim_end();
    line.strip_prefix("## ").map(|s| s.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn valid_template_compiles_correctly() {
        let src = r#"---
name: quick-task
version: "1.0"
description: A focused task workflow
initial_state: assess

variables:
  TASK:
    description: What to build
    required: true

states:
  assess:
    transitions:
      - target: done
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

## done

Work is complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path()).unwrap();

        assert_eq!(result.format_version, 1);
        assert_eq!(result.name, "quick-task");
        assert_eq!(result.version, "1.0");
        assert_eq!(result.initial_state, "assess");
        assert!(result.states.contains_key("assess"));
        assert!(result.states.contains_key("done"));

        let assess = &result.states["assess"];
        assert_eq!(assess.directive, "Analyze the task: {{TASK}}");
        assert_eq!(assess.transitions.len(), 1);
        assert_eq!(assess.transitions[0].target, "done");
        assert!(assess.transitions[0].when.is_none());

        let done = &result.states["done"];
        assert!(done.terminal);
        assert_eq!(done.directive, "Work is complete.");

        let var = &result.variables["TASK"];
        assert!(var.required);
        assert_eq!(var.description, "What to build");
    }

    #[test]
    fn missing_name_returns_error() {
        let src = r#"---
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("missing required field: name"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn missing_initial_state_returns_error() {
        let src = r#"---
name: test
version: "1.0"
states:
  start:
    terminal: true
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("missing required field: initial_state"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let src = r#"---
name: [broken yaml
version: "1.0"
---
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("invalid YAML") || err.to_string().contains("YAML"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn no_frontmatter_returns_error() {
        let src = "This is not a valid template.\n";
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("front-matter"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn unknown_gate_type_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: unknown_type
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("unsupported gate type"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn field_not_empty_gate_rejected() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: field_not_empty
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported gate type"), "got: {}", msg);
        assert!(msg.contains("accepts/when"), "got: {}", msg);
    }

    #[test]
    fn field_equals_gate_rejected() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: field_equals
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported gate type"), "got: {}", msg);
        assert!(msg.contains("accepts/when"), "got: {}", msg);
    }

    #[test]
    fn command_gate_empty_command_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: command
        command: ""
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("command must not be empty"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn compiled_json_round_trips() {
        let src = r#"---
name: round-trip
version: "2.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

The one and only state.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path()).unwrap();
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);
    }

    #[test]
    fn undefined_transition_target_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: nonexistent
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("undefined transition target"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn state_missing_directive_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
  orphan:
    terminal: true
---

## start

Hello.
"#;
        // orphan has no ## orphan heading in body
        let f = write_temp(src);
        let err = compile(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("orphan") && err.to_string().contains("directive"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn accepts_when_integration_compiles() {
        let src = r#"---
name: review
version: "1.0"
initial_state: analyze
states:
  analyze:
    integration: delegate_review
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: review
        when:
          decision: escalate
  deploy:
    transitions:
      - target: done
  review:
    transitions:
      - target: done
  done:
    terminal: true
---

## analyze

Review the results.

## deploy

Deploy to production.

## review

Escalate to senior review.

## done

Complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path()).unwrap();

        let analyze = &result.states["analyze"];
        assert_eq!(analyze.integration, Some("delegate_review".to_string()));
        assert!(analyze.accepts.is_some());
        let accepts = analyze.accepts.as_ref().unwrap();
        assert!(accepts.contains_key("decision"));
        let schema = &accepts["decision"];
        assert_eq!(schema.field_type, "enum");
        assert!(schema.required);
        assert_eq!(schema.values, vec!["proceed", "escalate"]);

        assert_eq!(analyze.transitions.len(), 2);
        assert_eq!(analyze.transitions[0].target, "deploy");
        assert!(analyze.transitions[0].when.is_some());
        let when = analyze.transitions[0].when.as_ref().unwrap();
        assert_eq!(when["decision"], serde_json::json!("proceed"));
    }

    #[test]
    fn command_gate_alongside_accepts_when() {
        let src = r#"---
name: mixed
version: "1.0"
initial_state: check
states:
  check:
    accepts:
      decision:
        type: enum
        values: [go, stop]
        required: true
    transitions:
      - target: done
        when:
          decision: go
      - target: halt
        when:
          decision: stop
    gates:
      ci:
        type: command
        command: ./check-ci.sh
  done:
    terminal: true
  halt:
    terminal: true
---

## check

Check the environment and decide.

## done

Proceed.

## halt

Stop.
"#;
        let f = write_temp(src);
        compile(f.path()).unwrap();
    }

    #[test]
    fn compiled_json_round_trips_with_evidence_routing() {
        let src = r#"---
name: evidence-rt
version: "1.0"
initial_state: decide
states:
  decide:
    accepts:
      choice:
        type: enum
        values: [a, b]
        required: true
    transitions:
      - target: path_a
        when:
          choice: a
      - target: path_b
        when:
          choice: b
    integration: my_tool
  path_a:
    transitions:
      - target: done
  path_b:
    transitions:
      - target: done
  done:
    terminal: true
---

## decide

Pick a path.

## path_a

Path A.

## path_b

Path B.

## done

Complete.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path()).unwrap();
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);
    }
}
