use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
}

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The only supported gate type.
pub const GATE_TYPE_COMMAND: &str = "command";

/// Valid field types for FieldSchema.
const VALID_FIELD_TYPES: &[&str] = &["enum", "string", "number", "boolean"];

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
            // Validate gates: only command gates are allowed.
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

            // Rule 5: when conditions require the state to have an accepts block.
            if !has_accepts {
                return Err(format!(
                    "state {:?} transition to {:?}: when conditions require an accepts block on the state",
                    state_name, transition.target
                ));
            }

            let accepts = state.accepts.as_ref().unwrap();

            for (field, value) in when {
                // Rule 1: when fields must reference fields declared in accepts.
                if !accepts.contains_key(field) {
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
                let schema = &accepts[field];
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
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
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
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
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
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
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
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
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
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
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
}
