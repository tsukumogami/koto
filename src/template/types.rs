use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use regex::Regex;

/// Regex for variable references in template strings: `{{KEY}}` where KEY is
/// uppercase letters, digits, and underscores.
pub const VAR_REF_PATTERN: &str = r"\{\{([A-Z][A-Z0-9_]*)\}\}";

/// Extract all `{{KEY}}` references from a string.
/// Used by compile-time validation and runtime substitution.
pub fn extract_refs(input: &str) -> Vec<String> {
    let re = Regex::new(VAR_REF_PATTERN).expect("VAR_REF_PATTERN is a valid regex");
    re.captures_iter(input)
        .map(|caps| caps[1].to_string())
        .collect()
}

/// A compiled template in FormatVersion=1 JSON format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledTemplate {
    pub format_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub initial_state: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, VariableDecl>,
    pub states: BTreeMap<String, TemplateState>,
}

/// A variable declaration in a compiled template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableDecl {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

/// A state declaration in a compiled template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    pub directive: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<BTreeMap<String, FieldSchema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_action: Option<ActionDecl>,
}

/// A structured transition with an optional condition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}

/// Schema for an evidence field declared in an `accepts` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A gate declaration in a compiled template state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    #[serde(rename = "type")]
    pub gate_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Timeout in seconds (0 = use default of 30s).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timeout: u32,
    /// Context key for context-exists and context-matches gates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Regex pattern for context-matches gates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// Optional instance-level override default value for this gate.
    ///
    /// When present, this value is used as the override default instead of
    /// (or in addition to) the built-in default for the gate type. The
    /// `koto overrides record` command resolves the override value by
    /// checking `--with-data`, then this field, then `built_in_default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_default: Option<serde_json::Value>,
}

/// A default action declaration for a template state.
///
/// When present on a state, the engine executes this command automatically
/// on state entry (unless override evidence exists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionDecl {
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_confirmation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling: Option<PollingConfig>,
}

/// Polling configuration for actions that need repeated execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollingConfig {
    pub interval_secs: u32,
    pub timeout_secs: u32,
}

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Gate type: shell command.
pub const GATE_TYPE_COMMAND: &str = "command";
/// Gate type: check whether a context key exists.
pub const GATE_TYPE_CONTEXT_EXISTS: &str = "context-exists";
/// Gate type: check whether context content matches a regex pattern.
pub const GATE_TYPE_CONTEXT_MATCHES: &str = "context-matches";

/// Evidence namespace reserved for engine-injected gate output.
/// Agent submissions starting with this prefix are rejected (Feature 2, R7).
/// All `gates.*` key checks in advance.rs and types.rs use this constant.
pub const GATES_EVIDENCE_NAMESPACE: &str = "gates";

/// Valid field types for FieldSchema.
const VALID_FIELD_TYPES: &[&str] = &["enum", "string", "number", "boolean"];

/// Runtime-injected variable names that are valid in templates but not
/// declared in the variables block. These are provided by the engine at
/// runtime (e.g., SESSION_DIR is the session directory path).
const RUNTIME_VARIABLE_NAMES: &[&str] = &["SESSION_DIR", "SESSION_NAME"];

impl CompiledTemplate {
    /// Validate the compiled template against all schema rules.
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != 1 {
            return Err(format!(
                "unsupported format version: {}",
                self.format_version
            ));
        }
        if self.name.is_empty() {
            return Err("missing required field: name".to_string());
        }
        if self.version.is_empty() {
            return Err("missing required field: version".to_string());
        }
        if self.initial_state.is_empty() {
            return Err("missing required field: initial_state".to_string());
        }
        if self.states.is_empty() {
            return Err("template has no states".to_string());
        }
        if !self.states.contains_key(&self.initial_state) {
            return Err(format!(
                "initial_state {:?} is not a declared state",
                self.initial_state
            ));
        }
        for (state_name, state) in &self.states {
            if state.directive.is_empty() {
                return Err(format!("state {:?} has empty directive", state_name));
            }
            // Validate transition targets exist.
            for transition in &state.transitions {
                if !self.states.contains_key(&transition.target) {
                    return Err(format!(
                        "state {:?} references undefined transition target {:?}",
                        state_name, transition.target
                    ));
                }
            }
            // Validate gates.
            for (gate_name, gate) in &state.gates {
                match gate.gate_type.as_str() {
                    GATE_TYPE_COMMAND => {
                        if gate.command.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: command must not be empty",
                                state_name, gate_name
                            ));
                        }
                    }
                    GATE_TYPE_CONTEXT_EXISTS => {
                        if gate.key.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: context-exists gate must have a non-empty key",
                                state_name, gate_name
                            ));
                        }
                    }
                    GATE_TYPE_CONTEXT_MATCHES => {
                        if gate.key.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: context-matches gate must have a non-empty key",
                                state_name, gate_name
                            ));
                        }
                        if gate.pattern.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: context-matches gate must have a non-empty pattern",
                                state_name, gate_name
                            ));
                        }
                        // Validate that the pattern is a valid regex.
                        if let Err(e) = regex::Regex::new(&gate.pattern) {
                            return Err(format!(
                                "state {:?} gate {:?}: invalid regex pattern {:?}: {}",
                                state_name, gate_name, gate.pattern, e
                            ));
                        }
                    }
                    other => {
                        return Err(format!(
                            "state {:?} gate {:?}: unsupported gate type {:?}. \
                             Field-based gates have been replaced by accepts/when. \
                             Use accepts blocks for evidence schema and when conditions for routing.",
                            state_name, gate_name, other
                        ));
                    }
                }
            }
            // Validate accepts block field schemas.
            if let Some(accepts) = &state.accepts {
                for (field_name, schema) in accepts {
                    if !VALID_FIELD_TYPES.contains(&schema.field_type.as_str()) {
                        return Err(format!(
                            "state {:?} accepts field {:?}: invalid field_type {:?}, \
                             must be one of: enum, string, number, boolean",
                            state_name, field_name, schema.field_type
                        ));
                    }
                    if schema.field_type == "enum" && schema.values.is_empty() {
                        return Err(format!(
                            "state {:?} accepts field {:?}: enum fields must have a non-empty values list",
                            state_name, field_name
                        ));
                    }
                }
            }
            // Validate evidence routing rules on transitions.
            self.validate_evidence_routing(state_name, state)?;

            // Validate variable references in directives.
            for ref_name in extract_refs(&state.directive) {
                if !self.variables.contains_key(&ref_name)
                    && !RUNTIME_VARIABLE_NAMES.contains(&ref_name.as_str())
                {
                    return Err(format!(
                        "state '{}': variable reference '{{{{{}}}}}' is not declared in the template's variables block",
                        state_name, ref_name
                    ));
                }
            }

            // Validate variable references in gate commands.
            for gate in state.gates.values() {
                for ref_name in extract_refs(&gate.command) {
                    if !self.variables.contains_key(&ref_name)
                        && !RUNTIME_VARIABLE_NAMES.contains(&ref_name.as_str())
                    {
                        return Err(format!(
                            "state '{}': variable reference '{{{{{}}}}}' is not declared in the template's variables block",
                            state_name, ref_name
                        ));
                    }
                }
            }

            // Validate default_action.
            if let Some(action) = &state.default_action {
                // Reject states with both integration and default_action.
                if state.integration.is_some() {
                    return Err(format!(
                        "state {:?}: cannot have both integration and default_action",
                        state_name
                    ));
                }
                // Reject empty action commands.
                if action.command.is_empty() {
                    return Err(format!(
                        "state {:?}: default_action command must not be empty",
                        state_name
                    ));
                }
                // Validate variable references in action command.
                for ref_name in extract_refs(&action.command) {
                    if !self.variables.contains_key(&ref_name)
                        && !RUNTIME_VARIABLE_NAMES.contains(&ref_name.as_str())
                    {
                        return Err(format!(
                            "state '{}': variable reference '{{{{{}}}}}' in default_action command is not declared in the template's variables block",
                            state_name, ref_name
                        ));
                    }
                }
                // Validate variable references in action working_dir.
                for ref_name in extract_refs(&action.working_dir) {
                    if !self.variables.contains_key(&ref_name)
                        && !RUNTIME_VARIABLE_NAMES.contains(&ref_name.as_str())
                    {
                        return Err(format!(
                            "state '{}': variable reference '{{{{{}}}}}' in default_action working_dir is not declared in the template's variables block",
                            state_name, ref_name
                        ));
                    }
                }
                // Require polling.timeout_secs > 0 when polling is declared.
                if let Some(polling) = &action.polling {
                    if polling.timeout_secs == 0 {
                        return Err(format!(
                            "state {:?}: default_action polling.timeout_secs must be greater than 0",
                            state_name
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate evidence routing rules for a single state.
    fn validate_evidence_routing(
        &self,
        state_name: &str,
        state: &TemplateState,
    ) -> Result<(), String> {
        let has_accepts = state.accepts.is_some();

        // Collect transitions that have when conditions.
        let conditional: Vec<&Transition> = state
            .transitions
            .iter()
            .filter(|t| t.when.is_some())
            .collect();

        for transition in &conditional {
            let when = transition.when.as_ref().unwrap();

            // Rule 3: Empty when blocks are rejected.
            if when.is_empty() {
                return Err(format!(
                    "state {:?} transition to {:?}: when block must not be empty",
                    state_name, transition.target
                ));
            }

            // Separate gates.* keys (engine-injected gate output) from agent evidence keys.
            // gates.* keys bypass the accepts block requirement and field-presence checks
            // because they are populated automatically by the advance loop, not by agents.
            let agent_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| !k.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)))
                .collect();
            let gate_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)))
                .collect();

            // Rule 5: when conditions that reference agent evidence require an accepts block.
            // Pure gates.* conditions are allowed without an accepts block.
            if !agent_fields.is_empty() && !has_accepts {
                return Err(format!(
                    "state {:?} transition to {:?}: when conditions require an accepts block on the state",
                    state_name, transition.target
                ));
            }

            // Rule 6 applied to gates.* fields: values must be JSON scalars.
            for (field, value) in &gate_fields {
                if value.is_array() || value.is_object() {
                    return Err(format!(
                        "state {:?} transition to {:?}: when value for field {:?} must be a scalar \
                         (string, number, or boolean), not an array or object",
                        state_name, transition.target, field
                    ));
                }
            }

            if !agent_fields.is_empty() {
                let accepts = state.accepts.as_ref().unwrap();

                for (field, value) in &agent_fields {
                    // Rule 1: agent evidence when fields must reference fields declared in accepts.
                    if !accepts.contains_key(*field) {
                        return Err(format!(
                            "state {:?} transition to {:?}: when field {:?} is not declared in accepts",
                            state_name, transition.target, field
                        ));
                    }

                    // Rule 6: when values must be JSON scalars.
                    if value.is_array() || value.is_object() {
                        return Err(format!(
                            "state {:?} transition to {:?}: when value for field {:?} must be a scalar \
                             (string, number, or boolean), not an array or object",
                            state_name, transition.target, field
                        ));
                    }

                    // Rule 2: when values for enum fields must appear in the values list.
                    let schema = &accepts[*field];
                    if schema.field_type == "enum" {
                        let value_str = match value.as_str() {
                            Some(s) => s.to_string(),
                            None => value.to_string(),
                        };
                        if !schema.values.contains(&value_str) {
                            return Err(format!(
                                "state {:?} transition to {:?}: when value {:?} for enum field {:?} \
                                 is not in allowed values {:?}",
                                state_name, transition.target, value_str, field, schema.values
                            ));
                        }
                    }
                }
            }
        }

        // Rule 4: Pairwise mutual exclusivity.
        if conditional.len() >= 2 {
            for i in 0..conditional.len() {
                for j in (i + 1)..conditional.len() {
                    let when_a = conditional[i].when.as_ref().unwrap();
                    let when_b = conditional[j].when.as_ref().unwrap();

                    let mut has_shared_field = false;
                    let mut has_disjoint_value = false;

                    for (field, val_a) in when_a {
                        if let Some(val_b) = when_b.get(field) {
                            has_shared_field = true;
                            if val_a != val_b {
                                has_disjoint_value = true;
                                break;
                            }
                        }
                    }

                    if !has_shared_field || !has_disjoint_value {
                        let reason = if !has_shared_field {
                            "transitions share no fields, so both could match the same evidence"
                        } else {
                            "all shared fields have identical values, so both transitions would match"
                        };
                        return Err(format!(
                            "state {:?}: transitions to {:?} and {:?} are not mutually exclusive: {}",
                            state_name,
                            conditional[i].target,
                            conditional[j].target,
                            reason
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_template() -> CompiledTemplate {
        let mut states = BTreeMap::new();
        states.insert(
            "start".to_string(),
            TemplateState {
                directive: "Begin.".to_string(),
                details: String::new(),
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
                details: String::new(),
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
    fn valid_minimal_template_passes() {
        minimal_template().validate().unwrap();
    }

    #[test]
    fn rejects_field_not_empty_gate() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "field_not_empty".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(err.contains("unsupported gate type"), "got: {}", err);
        assert!(err.contains("accepts/when"), "got: {}", err);
    }

    #[test]
    fn rejects_field_equals_gate() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "field_equals".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(err.contains("unsupported gate type"), "got: {}", err);
        assert!(err.contains("accepts/when"), "got: {}", err);
    }

    #[test]
    fn command_gate_still_works() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "ci".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check-ci.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        t.validate().unwrap();
    }

    #[test]
    fn rejects_empty_when_block() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.accepts = Some(BTreeMap::new());
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(BTreeMap::new()),
        }];
        let err = t.validate().unwrap_err();
        assert!(err.contains("when block must not be empty"), "got: {}", err);
    }

    #[test]
    fn rejects_when_without_accepts() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("proceed"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("when conditions require an accepts block"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_when_field_not_in_accepts() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let mut when = BTreeMap::new();
        when.insert("nonexistent".to_string(), serde_json::json!("val"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(err.contains("not declared in accepts"), "got: {}", err);
    }

    #[test]
    fn gates_only_when_does_not_require_accepts() {
        // A when clause with only gates.* keys is valid without an accepts block.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "exit 0".to_string(),
                key: String::new(),
                pattern: String::new(),
                timeout: 0,
                override_default: None,
            },
        );
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        assert!(
            t.validate().is_ok(),
            "gates.* when clause should not require accepts"
        );
    }

    #[test]
    fn gates_only_when_with_multiple_transitions_allowed() {
        // Multiple transitions using only gates.* keys with distinct values are valid.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "exit 0".to_string(),
                key: String::new(),
                pattern: String::new(),
                timeout: 0,
                override_default: None,
            },
        );
        let mut when_pass = BTreeMap::new();
        when_pass.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        let mut when_fail = BTreeMap::new();
        when_fail.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(1));
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_pass),
            },
            Transition {
                target: "fix".to_string(),
                when: Some(when_fail),
            },
        ];
        // Add the "fix" terminal state so the template is valid.
        t.states.insert(
            "fix".to_string(),
            TemplateState {
                directive: "Fix the issue.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        assert!(
            t.validate().is_ok(),
            "two gates.* when transitions should be valid"
        );
    }

    #[test]
    fn mixed_gates_and_agent_when_requires_accepts() {
        // A when clause mixing gates.* and agent evidence keys still requires an accepts block.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        when.insert("decision".to_string(), serde_json::json!("approve"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("when conditions require an accepts block"),
            "got: {}",
            err
        );
    }

    #[test]
    fn gates_when_rejects_non_scalar_value() {
        // gates.* when values must still be scalars.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "gates.ci_check.output".to_string(),
            serde_json::json!({"nested": "object"}),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(err.contains("must be a scalar"), "got: {}", err);
    }

    #[test]
    fn rejects_non_scalar_when_value() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "data".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let mut when = BTreeMap::new();
        when.insert("data".to_string(), serde_json::json!(["a", "b"]));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(err.contains("must be a scalar"), "got: {}", err);
    }

    #[test]
    fn rejects_enum_value_not_in_list() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string(), "escalate".to_string()],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("invalid_value"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(err.contains("not in allowed values"), "got: {}", err);
    }

    #[test]
    fn rejects_invalid_field_type() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "data".to_string(),
            FieldSchema {
                field_type: "invalid_type".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let err = t.validate().unwrap_err();
        assert!(err.contains("invalid field_type"), "got: {}", err);
    }

    #[test]
    fn rejects_enum_without_values() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("enum fields must have a non-empty values list"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_non_exclusive_transitions_no_shared_field() {
        let mut t = minimal_template();
        t.states.insert(
            "other".to_string(),
            TemplateState {
                directive: "Other.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        accepts.insert(
            "priority".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);

        let mut when_a = BTreeMap::new();
        when_a.insert("decision".to_string(), serde_json::json!("proceed"));
        let mut when_b = BTreeMap::new();
        when_b.insert("priority".to_string(), serde_json::json!("high"));

        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "other".to_string(),
                when: Some(when_b),
            },
        ];
        let err = t.validate().unwrap_err();
        assert!(err.contains("not mutually exclusive"), "got: {}", err);
        assert!(err.contains("share no fields"), "got: {}", err);
    }

    #[test]
    fn rejects_non_exclusive_transitions_same_values() {
        let mut t = minimal_template();
        t.states.insert(
            "other".to_string(),
            TemplateState {
                directive: "Other.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string()],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);

        let mut when_a = BTreeMap::new();
        when_a.insert("decision".to_string(), serde_json::json!("proceed"));
        let mut when_b = BTreeMap::new();
        when_b.insert("decision".to_string(), serde_json::json!("proceed"));

        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "other".to_string(),
                when: Some(when_b),
            },
        ];
        let err = t.validate().unwrap_err();
        assert!(err.contains("not mutually exclusive"), "got: {}", err);
        assert!(err.contains("identical values"), "got: {}", err);
    }

    #[test]
    fn accepts_exclusive_transitions() {
        let mut t = minimal_template();
        t.states.insert(
            "other".to_string(),
            TemplateState {
                directive: "Other.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string(), "escalate".to_string()],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);

        let mut when_a = BTreeMap::new();
        when_a.insert("decision".to_string(), serde_json::json!("proceed"));
        let mut when_b = BTreeMap::new();
        when_b.insert("decision".to_string(), serde_json::json!("escalate"));

        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "other".to_string(),
                when: Some(when_b),
            },
        ];
        t.validate().unwrap();
    }

    #[test]
    fn multi_field_exclusive_transitions() {
        let mut t = minimal_template();
        t.states.insert(
            "other".to_string(),
            TemplateState {
                directive: "Other.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string()],
                description: String::new(),
            },
        );
        accepts.insert(
            "priority".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["high".to_string(), "low".to_string()],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);

        let mut when_a = BTreeMap::new();
        when_a.insert("decision".to_string(), serde_json::json!("proceed"));
        when_a.insert("priority".to_string(), serde_json::json!("high"));
        let mut when_b = BTreeMap::new();
        when_b.insert("decision".to_string(), serde_json::json!("proceed"));
        when_b.insert("priority".to_string(), serde_json::json!("low"));

        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "other".to_string(),
                when: Some(when_b),
            },
        ];
        // Exclusive on priority even though decision overlaps.
        t.validate().unwrap();
    }

    #[test]
    fn valid_enum_when_values_pass() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string(), "escalate".to_string()],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("proceed"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    #[test]
    fn integration_field_validates() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.integration = Some("delegate_review".to_string());
        t.validate().unwrap();
    }

    #[test]
    fn accepts_with_unconditional_transitions() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "data".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        // No when condition -- unconditional transition is fine.
        t.validate().unwrap();
    }

    #[test]
    fn context_exists_gate_validates() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "research".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "research/r1/lead.md".to_string(),
                pattern: String::new(),
                override_default: None,
            },
        );
        t.validate().unwrap();
    }

    #[test]
    fn rejects_undeclared_variable_ref_in_directive() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.directive = "Do {{TASK}} now".to_string();
        // No variable declared for TASK
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("variable reference '{{TASK}}'"),
            "got: {}",
            err
        );
        assert!(err.contains("not declared"), "got: {}", err);
    }

    #[test]
    fn accepts_declared_variable_ref_in_directive() {
        let mut t = minimal_template();
        t.variables.insert(
            "TASK".to_string(),
            VariableDecl {
                description: String::new(),
                required: true,
                default: String::new(),
            },
        );
        let state = t.states.get_mut("start").unwrap();
        state.directive = "Do {{TASK}} now".to_string();
        t.validate().unwrap();
    }

    #[test]
    fn rejects_undeclared_variable_ref_in_gate_command() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "echo {{MISSING}}".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("variable reference '{{MISSING}}'"),
            "got: {}",
            err
        );
    }

    #[test]
    fn accepts_declared_variable_ref_in_gate_command() {
        let mut t = minimal_template();
        t.variables.insert(
            "BRANCH".to_string(),
            VariableDecl {
                description: String::new(),
                required: false,
                default: "main".to_string(),
            },
        );
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "git checkout {{BRANCH}}".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        t.validate().unwrap();
    }

    #[test]
    fn context_exists_gate_rejects_empty_key() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "research".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("context-exists gate must have a non-empty key"),
            "got: {}",
            err
        );
    }

    #[test]
    fn context_matches_gate_validates() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
            },
        );
        t.validate().unwrap();
    }

    #[test]
    fn lowercase_braces_not_treated_as_variable_refs() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.directive = "Use {{name}} style".to_string();
        // Lowercase is not a variable ref, should pass without declaring it
        t.validate().unwrap();
    }

    #[test]
    fn context_matches_gate_rejects_empty_key() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: "## Approved".to_string(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("context-matches gate must have a non-empty key"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_integration_and_default_action() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.integration = Some("my-runner".to_string());
        state.default_action = Some(ActionDecl {
            command: "echo hi".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        });
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("cannot have both integration and default_action"),
            "got: {}",
            err
        );
    }

    #[test]
    fn context_matches_gate_rejects_empty_pattern() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("context-matches gate must have a non-empty pattern"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_empty_default_action_command() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: String::new(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        });
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("default_action command must not be empty"),
            "got: {}",
            err
        );
    }

    #[test]
    fn context_matches_gate_rejects_invalid_regex() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "[invalid".to_string(),
                override_default: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(err.contains("invalid regex pattern"), "got: {}", err);
    }

    #[test]
    fn rejects_undeclared_variable_in_action_command() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: "echo {{MISSING}}".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        });
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("variable reference '{{MISSING}}'")
                && err.contains("default_action command"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_undeclared_variable_in_action_working_dir() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: "echo ok".to_string(),
            working_dir: "/tmp/{{MISSING}}".to_string(),
            requires_confirmation: false,
            polling: None,
        });
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("variable reference '{{MISSING}}'")
                && err.contains("default_action working_dir"),
            "got: {}",
            err
        );
    }

    #[test]
    fn rejects_polling_with_zero_timeout() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: "echo check".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: Some(PollingConfig {
                interval_secs: 10,
                timeout_secs: 0,
            }),
        });
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("polling.timeout_secs must be greater than 0"),
            "got: {}",
            err
        );
    }

    #[test]
    fn valid_default_action_passes() {
        let mut t = minimal_template();
        t.variables.insert(
            "BRANCH".to_string(),
            VariableDecl {
                description: String::new(),
                required: false,
                default: "main".to_string(),
            },
        );
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: "git checkout {{BRANCH}}".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        });
        t.validate().unwrap();
    }

    #[test]
    fn valid_default_action_with_polling_passes() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.default_action = Some(ActionDecl {
            command: "gh pr checks --watch".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: Some(PollingConfig {
                interval_secs: 30,
                timeout_secs: 1800,
            }),
        });
        t.validate().unwrap();
    }

    #[test]
    fn action_decl_serde_round_trip() {
        let action = ActionDecl {
            command: "echo test".to_string(),
            working_dir: "/tmp".to_string(),
            requires_confirmation: true,
            polling: Some(PollingConfig {
                interval_secs: 10,
                timeout_secs: 300,
            }),
        };
        let json = serde_json::to_string(&action).unwrap();
        let restored: ActionDecl = serde_json::from_str(&json).unwrap();
        assert_eq!(action, restored);
    }

    #[test]
    fn action_decl_serde_minimal() {
        let action = ActionDecl {
            command: "echo test".to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        // Optional/empty fields should be omitted.
        assert!(!json.contains("working_dir"));
        assert!(!json.contains("requires_confirmation"));
        assert!(!json.contains("polling"));
        let restored: ActionDecl = serde_json::from_str(&json).unwrap();
        assert_eq!(action, restored);
    }

    #[test]
    fn gate_override_default_present_round_trips_via_serde_yaml() {
        // A gate YAML with override_default set should round-trip exactly.
        let yaml = r#"
type: command
command: "./check.sh"
override_default:
  exit_code: 1
  error: ""
"#;
        let gate: Gate = serde_yml::from_str(yaml).unwrap();
        assert!(gate.override_default.is_some());
        let v = gate.override_default.as_ref().unwrap();
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["error"], "");

        // Serialize back and confirm the value is preserved.
        let out = serde_yml::to_string(&gate).unwrap();
        assert!(
            out.contains("override_default"),
            "override_default should appear in output: {}",
            out
        );

        let gate2: Gate = serde_yml::from_str(&out).unwrap();
        assert_eq!(gate.override_default, gate2.override_default);
    }

    #[test]
    fn gate_override_default_absent_deserializes_as_none_and_omits_key() {
        // A gate YAML without override_default should deserialize with None
        // and serialize without the key.
        let yaml = r#"
type: command
command: "./check.sh"
"#;
        let gate: Gate = serde_yml::from_str(yaml).unwrap();
        assert!(gate.override_default.is_none());

        let out = serde_yml::to_string(&gate).unwrap();
        assert!(
            !out.contains("override_default"),
            "override_default should be absent from output: {}",
            out
        );
    }
}
