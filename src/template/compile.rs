use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{anyhow, Context};
use serde::Deserialize;

use super::types::{
    default_failure_policy, ActionDecl, CompiledTemplate, FailurePolicy, FieldSchema, Gate,
    MaterializeChildrenSpec, PollingConfig, TemplateState, Transition, VariableDecl,
    GATE_TYPE_CHILDREN_COMPLETE, GATE_TYPE_COMMAND, GATE_TYPE_CONTEXT_EXISTS,
    GATE_TYPE_CONTEXT_MATCHES,
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

/// YAML front-matter view of a single state.
///
/// `#[serde(deny_unknown_fields)]` is applied here (but NOT on
/// `CompiledTemplate::TemplateState`) so that typos or unknown keys in
/// template source are caught at compile time, while the compile cache
/// remains forward-compatible with newer binaries that may add fields.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    materialize_children: Option<SourceMaterializeChildrenSpec>,
    #[serde(default)]
    failure: bool,
    #[serde(default)]
    skipped_marker: bool,
}

/// YAML front-matter view of a `materialize_children` hook.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMaterializeChildrenSpec {
    #[serde(default)]
    from_field: String,
    #[serde(default)]
    default_template: String,
    #[serde(default = "default_failure_policy")]
    failure_policy: FailurePolicy,
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
    #[serde(default)]
    override_default: Option<serde_json::Value>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    name_filter: Option<String>,
}

/// Compile a YAML/Markdown template source file to a FormatVersion=1 CompiledTemplate.
///
/// `strict` is passed through to `validate()`. When `true`, a state with gates
/// but no `gates.*` when-clause references is a hard error. When `false`, the
/// same condition emits a warning to stderr and compilation continues.
pub fn compile(source_path: &Path, strict: bool) -> anyhow::Result<CompiledTemplate> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read template source: {}", source_path.display()))?;

    let (frontmatter_str, body) = split_frontmatter(&content).ok_or_else(|| {
        anyhow!("invalid YAML: template must begin with YAML front-matter delimited by '---'")
    })?;

    let fm: SourceFrontmatter = serde_yaml_ng::from_str(frontmatter_str)
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
        let (directive, details) = directives.get(state_name).cloned().unwrap_or_default();
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

        let compiled_materialize_children =
            source_state
                .materialize_children
                .as_ref()
                .map(|sm| MaterializeChildrenSpec {
                    from_field: sm.from_field.clone(),
                    default_template: sm.default_template.clone(),
                    failure_policy: sm.failure_policy,
                });

        compiled_states.insert(
            state_name.clone(),
            TemplateState {
                directive,
                details,
                transitions: compiled_transitions,
                terminal: source_state.terminal,
                gates: compiled_gates,
                accepts: compiled_accepts,
                integration: source_state.integration.clone(),
                default_action: compiled_action,
                materialize_children: compiled_materialize_children,
                failure: source_state.failure,
                skipped_marker: source_state.skipped_marker,
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
        .validate(strict)
        .map_err(|e| anyhow!("validation error: {}", e))?;

    // Issue 8: E9 resolution check and F5 (skipped_marker reachability)
    // on batch-eligible child templates. Both need the source path to
    // resolve relative `default_template` references against the parent
    // template's directory, so they live here rather than in `validate`.
    validate_default_template_references(&template, source_path)?;

    Ok(template)
}

/// E9 resolution + F5 warning.
///
/// For every state with a `materialize_children` hook, resolve the hook's
/// `default_template` against the compiling template's directory. The path
/// must point to a file that itself compiles without error; any failure is
/// surfaced as an E9 error naming the declaring state.
///
/// When the child template compiles, fire warning F5 on stderr if the
/// child lacks a terminal state with `skipped_marker: true` that is
/// reachable from its initial state. The check is intentionally permissive
/// about "scheduler-writable transitions" (Decision 9): for now any
/// transition counts as reachable. F5 is a warning, not an error, because
/// batch-eligibility is not knowable when a child template is compiled in
/// isolation.
///
/// Warnings are printed to stderr via `eprintln!` in the same style as
/// D4/D5 and W1-W5.
fn validate_default_template_references(
    template: &CompiledTemplate,
    source_path: &Path,
) -> anyhow::Result<()> {
    // Resolve the parent template's directory. Relative default_template
    // paths anchor here. `canonicalize` may fail if the source path is a
    // temporary with a stripped parent; fall back to the raw parent.
    let source_dir = source_path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .or_else(|| source_path.parent().map(|p| p.to_path_buf()));

    for (state_name, state) in &template.states {
        let hook = match &state.materialize_children {
            Some(h) => h,
            None => continue,
        };
        // E1 already caught empty from_field; E9's non-emptiness is
        // checked in validate(). Skip empty here defensively.
        if hook.default_template.is_empty() {
            continue;
        }

        // Resolve default_template against source_dir. Absolute paths
        // pass through unchanged.
        let candidate = std::path::Path::new(&hook.default_template);
        let resolved: std::path::PathBuf = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else if let Some(dir) = &source_dir {
            dir.join(candidate)
        } else {
            candidate.to_path_buf()
        };

        // Guard against infinite recursion: a template that names itself
        // as its own default_template would loop. Compare canonical paths
        // when possible, raw otherwise.
        let same_as_source = match (resolved.canonicalize(), source_path.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => resolved == source_path,
        };
        if same_as_source {
            return Err(anyhow!(
                "E9: state {:?}: default_template {:?} resolves to the parent template itself; \
                 child templates must be distinct files\n  \
                 remedy: point default_template at a separate child template file",
                state_name,
                hook.default_template
            ));
        }

        // Compile the child. Any error is wrapped as E9.
        let child = compile(&resolved, true).map_err(|e| {
            anyhow!(
                "E9: state {:?}: default_template {:?} (resolved to {}) did not compile: {}\n  \
                 remedy: fix the child template so it compiles, or point default_template at a valid template file",
                state_name,
                hook.default_template,
                resolved.display(),
                e
            )
        })?;

        // F5 warning: child lacks a reachable skipped_marker terminal.
        if !child_has_reachable_skipped_marker(&child) {
            eprintln!(
                "warning: F5: child template {:?} (referenced by state {:?} default_template {:?}) \
                 has no reachable terminal state with `skipped_marker: true`; \
                 the batch scheduler will not be able to materialize skip markers for this template\n  \
                 remedy: add a terminal state with `skipped_marker: true` reachable from the initial state",
                child.name,
                state_name,
                hook.default_template
            );
        }
    }
    Ok(())
}

/// F5 helper: walk transitions from `initial_state` and return true if any
/// reachable state is terminal with `skipped_marker: true`.
///
/// Decision 9 distinguishes scheduler-writable transitions from
/// agent-submitted ones, but we conservatively treat every transition as
/// reachable here. Narrowing to scheduler-writable transitions lands in a
/// later phase once the transition metadata is finalized.
// TODO(issue-8/F5): narrow reachability to scheduler-writable transitions
// once Decision 9's metadata is exposed on `Transition`.
fn child_has_reachable_skipped_marker(child: &CompiledTemplate) -> bool {
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut frontier: Vec<&str> = Vec::new();
    if child.states.contains_key(child.initial_state.as_str()) {
        frontier.push(child.initial_state.as_str());
    }
    while let Some(name) = frontier.pop() {
        if !visited.insert(name) {
            continue;
        }
        let state = match child.states.get(name) {
            Some(s) => s,
            None => continue,
        };
        if state.terminal && state.skipped_marker {
            return true;
        }
        for transition in &state.transitions {
            if !visited.contains(transition.target.as_str()) {
                frontier.push(transition.target.as_str());
            }
        }
    }
    false
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
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
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
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
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
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
            })
        }
        GATE_TYPE_CHILDREN_COMPLETE => {
            // Validate completion prefix.
            if let Some(ref completion) = source.completion {
                if completion != "terminal"
                    && !completion.starts_with("state:")
                    && !completion.starts_with("context:")
                {
                    return Err(anyhow!(
                        "state {:?} gate {:?}: unknown completion prefix {:?}; \
                         only \"terminal\" is supported (\"state:*\" and \"context:*\" are reserved)",
                        state_name,
                        gate_name,
                        completion
                    ));
                }
                if completion.starts_with("state:") || completion.starts_with("context:") {
                    return Err(anyhow!(
                        "state {:?} gate {:?}: completion mode {:?} is reserved but not yet implemented",
                        state_name,
                        gate_name,
                        completion
                    ));
                }
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: source.override_default.clone(),
                completion: source.completion.clone(),
                name_filter: source.name_filter.clone(),
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

/// The `<!-- details -->` marker used to split directive from details content.
const DETAILS_MARKER: &str = "<!-- details -->";

/// Extract directive and details content for each declared state from the
/// markdown body.
///
/// States are identified by `## <state-name>` headings. Content between two
/// consecutive state headings belongs to the first. The declared state list
/// from the front-matter is the authority — headings that don't match a
/// declared state name are treated as directive content, not state boundaries.
///
/// Within each state's content, if a `<!-- details -->` line is found, the
/// content before it becomes the directive and the content after becomes the
/// details. Only the first occurrence of the marker is used. If no marker is
/// present, the entire content is the directive and details is empty.
fn extract_directives(
    states: &HashMap<String, SourceState>,
    body: &str,
) -> HashMap<String, (String, String)> {
    let state_names: std::collections::HashSet<&str> = states.keys().map(|s| s.as_str()).collect();

    let mut directives: HashMap<String, (String, String)> = HashMap::new();
    let mut current_state: Option<&str> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = parse_h2_heading(line) {
            if state_names.contains(heading) {
                // Save the previous state's directive.
                if let Some(state) = current_state {
                    directives.insert(state.to_string(), split_directive_details(&current_lines));
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
        directives.insert(state.to_string(), split_directive_details(&current_lines));
    }

    directives
}

/// Split collected lines into (directive, details) at the first `<!-- details -->` marker.
fn split_directive_details(lines: &[&str]) -> (String, String) {
    let marker_pos = lines.iter().position(|line| line.trim() == DETAILS_MARKER);

    match marker_pos {
        Some(pos) => {
            let directive = lines[..pos].join("\n").trim().to_string();
            let details = lines[pos + 1..].join("\n").trim().to_string();
            (directive, details)
        }
        None => {
            let directive = lines.join("\n").trim().to_string();
            (directive, String::new())
        }
    }
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
    use tempfile::{NamedTempFile, TempDir};

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    /// Write a parent template source to `<dir>/<parent_file>` alongside a
    /// default child template at `<dir>/<child_file>`. Returns the
    /// `TempDir` (to keep it alive) and the parent template path.
    ///
    /// The child is a minimal valid template; by default it includes a
    /// terminal state with `skipped_marker: true` so F5 does not fire.
    /// Pass `child_src` to override with a template that should trigger F5
    /// or exercise other child-compile behavior.
    fn write_parent_with_child(
        parent_src: &str,
        parent_file: &str,
        child_file: &str,
        child_src: Option<&str>,
    ) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let child = child_src.unwrap_or(
            r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
      - target: skipped
  done:
    terminal: true
  skipped:
    terminal: true
    skipped_marker: true
---

## start

Do work.

## done

Complete.

## skipped

Skipped.
"#,
        );
        std::fs::write(dir.path().join(child_file), child).unwrap();
        let parent_path = dir.path().join(parent_file);
        std::fs::write(&parent_path, parent_src).unwrap();
        (dir, parent_path)
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
        let result = compile(f.path(), true).unwrap();

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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let compiled = compile(f.path(), true).unwrap();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let err = compile(f.path(), true).unwrap_err();
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
        let result = compile(f.path(), true).unwrap();

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
        // A gate with gates.* routing alongside agent accepts/when compiles in strict mode.
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
          gates.ci.exit_code: 0
      - target: halt
        when:
          gates.ci.exit_code: 1
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
        compile(f.path(), true).unwrap();
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
        let compiled = compile(f.path(), true).unwrap();
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);
    }

    #[test]
    fn details_marker_splits_directive_and_details() {
        let src = r#"---
name: details-test
version: "1.0"
initial_state: work
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do the main task.

<!-- details -->

Here are some extra guidelines:
- Step 1
- Step 2

## done

Work is complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let work = &result.states["work"];
        assert_eq!(work.directive, "Do the main task.");
        assert_eq!(
            work.details,
            "Here are some extra guidelines:\n- Step 1\n- Step 2"
        );

        // State without marker should have empty details.
        let done = &result.states["done"];
        assert_eq!(done.directive, "Work is complete.");
        assert!(done.details.is_empty());
    }

    #[test]
    fn no_details_marker_produces_empty_details() {
        let src = r#"---
name: no-details
version: "1.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

Just a directive, no details marker here.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let only = &result.states["only"];
        assert_eq!(only.directive, "Just a directive, no details marker here.");
        assert!(only.details.is_empty());
    }

    #[test]
    fn multiple_details_markers_only_first_splits() {
        let src = r#"---
name: multi-details
version: "1.0"
initial_state: work
states:
  work:
    terminal: true
---

## work

The directive part.

<!-- details -->

First details section.

<!-- details -->

This stays in details, not a second split.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let work = &result.states["work"];
        assert_eq!(work.directive, "The directive part.");
        assert_eq!(
            work.details,
            "First details section.\n\n<!-- details -->\n\nThis stays in details, not a second split."
        );
    }

    #[test]
    fn compiled_json_round_trips_with_details() {
        let src = r#"---
name: round-trip-details
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Main directive.

<!-- details -->

Extra context for first visit.

## done

Complete.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();

        // Verify the details field is populated.
        assert_eq!(
            compiled.states["start"].details,
            "Extra context for first visit."
        );
        assert!(compiled.states["done"].details.is_empty());

        // Round-trip through JSON.
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);

        // Verify details is present in JSON for start but absent for done.
        let json_val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json_val["states"]["start"]["details"].is_string());
        assert!(json_val["states"]["done"].get("details").is_none());
    }

    // D5 integration: verify compile() propagates strict through to validate().
    // scenario-1 (strict=true): legacy-gate template fails compilation.
    // scenario-2 (strict=false): legacy-gate template compiles with a warning to stderr.
    #[test]
    fn compile_strict_true_errors_on_legacy_gate() {
        let src = r#"---
name: legacy
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: ./check.sh
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do work.

## done

Done.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("has no gates.* routing"),
            "expected D5 error, got: {}",
            msg
        );
        assert!(
            msg.contains("--allow-legacy-gates"),
            "error should hint at --allow-legacy-gates, got: {}",
            msg
        );
    }

    #[test]
    fn compile_strict_false_permits_legacy_gate() {
        let src = r#"---
name: legacy
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: ./check.sh
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do work.

## done

Done.
"#;
        let f = write_temp(src);
        // strict=false: warning to stderr, but compile returns Ok.
        compile(f.path(), false).unwrap();
    }

    // -------------------------------------------------------------------
    // Issue 7 — tasks accepts type, materialize_children hook, and the
    // narrow deny_unknown_fields on SourceState.
    // -------------------------------------------------------------------

    #[test]
    fn tasks_typed_accepts_field_compiles() {
        // Minimal batch parent template: plan_and_await has a tasks-typed
        // accepts field and transitions out on a structured condition. The
        // compiler must accept `type: tasks`.
        let src = r#"---
name: batch-parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
    transitions:
      - target: done
        when:
          tasks: submitted
  done:
    terminal: true
---

## plan_and_await

Submit the task list.

## done

All done.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        let state = &compiled.states["plan_and_await"];
        let accepts = state.accepts.as_ref().unwrap();
        assert_eq!(accepts["tasks"].field_type, "tasks");
        assert!(accepts["tasks"].required);
    }

    /// A minimal batch-parent template body that satisfies every Issue 8
    /// E-rule: accepts `tasks` (type tasks, required), has a
    /// children-complete gate, routes on it, and is non-terminal.
    /// Callers override the inner fields to trip specific rules.
    fn batch_parent_src(hook_extra: &str, gate_present: bool) -> String {
        let gate_block = if gate_present {
            "    gates:\n      cc:\n        type: children-complete\n"
        } else {
            ""
        };
        format!(
            r#"---
name: batch-parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
{gate}    materialize_children:
      from_field: tasks
      default_template: child.md
{extra}    transitions:
      - target: done
        when:
          gates.cc.all_complete: true
  done:
    terminal: true
---

## plan_and_await

Submit tasks.

## done

Done.
"#,
            gate = gate_block,
            extra = hook_extra,
        )
    }

    #[test]
    fn materialize_children_hook_parses_with_default_policy() {
        // The hook declares from_field and default_template only; the
        // failure_policy defaults to skip_dependents.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        let compiled = compile(&path, true).unwrap();
        let state = &compiled.states["plan_and_await"];
        let hook = state.materialize_children.as_ref().unwrap();
        assert_eq!(hook.from_field, "tasks");
        assert_eq!(hook.default_template, "child.md");
        assert_eq!(
            hook.failure_policy,
            crate::template::types::FailurePolicy::SkipDependents
        );
    }

    #[test]
    fn materialize_children_hook_accepts_continue_policy() {
        // failure_policy: continue is the explicit opt-out.
        let src = batch_parent_src("      failure_policy: continue\n", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        let compiled = compile(&path, true).unwrap();
        let hook = compiled.states["plan_and_await"]
            .materialize_children
            .as_ref()
            .unwrap();
        assert_eq!(
            hook.failure_policy,
            crate::template::types::FailurePolicy::Continue
        );
    }

    #[test]
    fn template_without_materialize_children_has_none() {
        // A plain (non-batch) template compiles with materialize_children
        // set to None on every state.
        let src = r#"---
name: plain
version: "1.0"
initial_state: work
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        assert!(compiled.states["work"].materialize_children.is_none());
        assert!(compiled.states["done"].materialize_children.is_none());
    }

    #[test]
    fn failure_and_skipped_marker_flags_parse() {
        // failure: true and skipped_marker: true are set on a terminal state.
        // Runtime validation of "meaningful only when terminal" lives with
        // compile rules (Issue 8); here we only verify the parse.
        let src = r#"---
name: parent
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: failed_marker
      - target: skipped
  failed_marker:
    terminal: true
    failure: true
  skipped:
    terminal: true
    skipped_marker: true
---

## start

Start.

## failed_marker

Failed.

## skipped

Skipped.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        assert!(compiled.states["failed_marker"].failure);
        assert!(!compiled.states["failed_marker"].skipped_marker);
        assert!(compiled.states["skipped"].skipped_marker);
        assert!(!compiled.states["skipped"].failure);
        // The non-terminal entry state leaves both flags false.
        assert!(!compiled.states["start"].failure);
        assert!(!compiled.states["start"].skipped_marker);
    }

    #[test]
    fn source_state_rejects_unknown_fields() {
        // deny_unknown_fields on SourceState catches typos at compile time.
        let src = r#"---
name: parent
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    materialize_childern: {}
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        // Use the full Debug/chain format — the underlying serde unknown-field
        // error lives in the error chain beneath the "invalid YAML" wrapper.
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("unknown field") || msg.contains("materialize_childern"),
            "expected unknown-field error, got: {}",
            msg
        );
    }

    #[test]
    fn source_materialize_children_rejects_unknown_fields() {
        // Inner spec also uses deny_unknown_fields.
        let src = r#"---
name: parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
    materialize_children:
      from_field: tasks
      default_template: child.md
      unknown_knob: 7
    transitions:
      - target: done
        when:
          tasks: submitted
  done:
    terminal: true
---

## plan_and_await

Submit.

## done

Done.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("unknown field") || msg.contains("unknown_knob"),
            "expected unknown-field error, got: {}",
            msg
        );
    }

    // ---------------------------------------------------------------------
    // Issue 8: E9 (default_template resolves) and F5 (skipped_marker
    // reachability) live in compile.rs because they need the source path.
    // ---------------------------------------------------------------------

    #[test]
    fn issue8_e9_valid_child_path_compiles() {
        // Positive case for E9: a valid child template next to the parent
        // compiles without error.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        compile(&path, true).expect("valid parent+child pair should compile");
    }

    #[test]
    fn issue8_e9_missing_child_file_is_rejected() {
        // E9 negative: child template does not exist on disk.
        let src = batch_parent_src("", true);
        // Write the parent without creating child.md.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("parent.md");
        std::fs::write(&path, &src).unwrap();
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
    }

    #[test]
    fn issue8_e9_uncompilable_child_surfaces_nested_error() {
        // E9 negative: child exists but has a compile error of its own.
        // The E9 error wraps the child's error message so authors can
        // trace the root cause.
        let bad_child = r#"---
name: broken
version: "1.0"
initial_state: missing
states:
  start:
    terminal: true
---

## start

Hi.
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", Some(bad_child));
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
        // The child's own error message should be in the chain.
        assert!(
            msg.contains("initial_state") || msg.contains("missing"),
            "expected nested child error, got: {}",
            msg
        );
    }

    #[test]
    fn issue8_e9_self_referential_default_template_rejected() {
        // Guard against a parent naming itself as its own default_template.
        let src = batch_parent_src("", true);
        // Rename so default_template (`child.md`) resolves back to the parent.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("child.md");
        std::fs::write(&path, &src).unwrap();
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
        assert!(
            msg.contains("parent template itself") || msg.contains("distinct"),
            "expected self-reference hint, got: {}",
            msg
        );
    }

    #[test]
    fn issue8_f5_child_with_skipped_marker_is_silent() {
        // Positive F5: the default child template includes a terminal
        // state with skipped_marker: true; compilation emits no F5 warning.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        // compile returns Ok; F5 is a stderr warning only when missing.
        compile(&path, true).expect("child with skipped_marker should compile cleanly");
    }

    #[test]
    fn issue8_f5_child_without_skipped_marker_compiles_with_warning() {
        // Negative F5: child lacks any skipped_marker terminal. Compile
        // still returns Ok because F5 is a warning, not an error.
        let child_no_marker = r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Work.

## done

Done.
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) =
            write_parent_with_child(&src, "parent.md", "child.md", Some(child_no_marker));
        compile(&path, true).expect("F5 is a warning, not an error");
    }

    #[test]
    fn issue8_f5_skipped_marker_unreachable_still_warns() {
        // Child has a skipped_marker terminal, but it is not reachable
        // from initial_state via any transition chain. F5 should fire,
        // and compile still returns Ok.
        let child_unreachable = r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
  orphan_skip:
    terminal: true
    skipped_marker: true
---

## start

Work.

## done

Done.

## orphan_skip

Skipped (but unreachable).
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) =
            write_parent_with_child(&src, "parent.md", "child.md", Some(child_unreachable));
        compile(&path, true).expect("F5 is a warning, not an error");
    }

    #[test]
    fn compiled_template_not_deny_unknown_fields() {
        // Decision 3 in the design: deny_unknown_fields is applied to
        // SourceState, NOT CompiledTemplate/TemplateState. A compile-cache
        // JSON with an extra field must still deserialize so that newer
        // binaries remain cache-compatible with older readers. This test
        // guards the invariant by directly constructing JSON with a future
        // field and deserializing.
        let json = serde_json::json!({
            "format_version": 1,
            "name": "future",
            "version": "9.9",
            "initial_state": "start",
            "states": {
                "start": {
                    "directive": "hi",
                    "terminal": true,
                    "some_future_field": ["yes"]
                }
            }
        });
        // If deny_unknown_fields were on CompiledTemplate/TemplateState this
        // would error; the test is that it does not.
        let _: CompiledTemplate = serde_json::from_value(json).unwrap();
    }
}
