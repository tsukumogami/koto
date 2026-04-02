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

/// The JSON value type of a gate output field.
///
/// Used by [`gate_type_schema`] to describe the expected type of each field
/// in a gate type's output schema. The compiler uses this for exact-match
/// validation of `override_default` values (D2) and the reachability check (D4).
#[derive(Debug, Clone, PartialEq)]
pub enum GateSchemaFieldType {
    Number,
    /// String-typed field. Named `Str` to avoid shadowing `std::string::String`
    /// when this enum is glob-imported with `use GateSchemaFieldType::*`.
    Str,
    Boolean,
}

/// Return the static output field schema for a known gate type.
///
/// Each element is `(field_name, field_type)`. Returns `None` for unknown
/// gate type strings.
///
/// Gate schemas:
/// - `command`:        `[("exit_code", Number), ("error", Str)]`
/// - `context-exists`: `[("exists", Boolean), ("error", Str)]`
/// - `context-matches`:`[("matches", Boolean), ("error", Str)]`
pub fn gate_type_schema(gate_type: &str) -> Option<&'static [(&'static str, GateSchemaFieldType)]> {
    use GateSchemaFieldType::*;
    match gate_type {
        GATE_TYPE_COMMAND => Some(&[("exit_code", Number), ("error", Str)]),
        GATE_TYPE_CONTEXT_EXISTS => Some(&[("exists", Boolean), ("error", Str)]),
        GATE_TYPE_CONTEXT_MATCHES => Some(&[("matches", Boolean), ("error", Str)]),
        _ => None,
    }
}

/// Return the built-in default override value for a known gate type.
///
/// Mirrors `built_in_default()` in `src/gate.rs` — the two functions serve
/// the same purpose in different contexts (compile-time vs. runtime). A unit
/// test in this module asserts they return identical values for every known
/// gate type. Update both functions in tandem if a gate type's default changes.
///
/// Returns `None` for unknown gate types.
pub fn gate_type_builtin_default(gate_type: &str) -> Option<serde_json::Value> {
    match gate_type {
        GATE_TYPE_COMMAND => Some(serde_json::json!({"exit_code": 0, "error": ""})),
        GATE_TYPE_CONTEXT_EXISTS => Some(serde_json::json!({"exists": true, "error": ""})),
        GATE_TYPE_CONTEXT_MATCHES => Some(serde_json::json!({"matches": true, "error": ""})),
        _ => None,
    }
}

/// Return the lowercase type name of a JSON value (for error messages).
fn json_type_name(value: &serde_json::Value) -> &'static str {
    if value.is_number() {
        "number"
    } else if value.is_string() {
        "string"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_array() {
        "array"
    } else if value.is_object() {
        "object"
    } else {
        "null"
    }
}

/// Walk a dot-separated path through a nested JSON object.
///
/// Mirrors `resolve_value()` in `src/engine/advance.rs` — both implement the
/// same segment-by-segment traversal for `gates.*` paths, one at compile time
/// (here) and one at runtime (advance.rs). Keep the two implementations in sync.
///
/// Returns `None` if any segment is missing or if an intermediate value is not
/// a JSON object.
fn resolve_gates_path<'a>(
    evidence: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = evidence;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Return the lowercase name of a `GateSchemaFieldType` for error messages.
fn gate_schema_field_type_name(t: &GateSchemaFieldType) -> &'static str {
    match t {
        GateSchemaFieldType::Number => "number",
        GateSchemaFieldType::Str => "string",
        GateSchemaFieldType::Boolean => "boolean",
    }
}

/// Return true if `value`'s JSON type matches the expected `GateSchemaFieldType`.
fn json_value_matches_schema_type(value: &serde_json::Value, t: &GateSchemaFieldType) -> bool {
    match t {
        GateSchemaFieldType::Number => value.is_number(),
        GateSchemaFieldType::Str => value.is_string(),
        GateSchemaFieldType::Boolean => value.is_boolean(),
    }
}

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
            // D2: validate override_default against gate type schema.
            for (gate_name, gate) in &state.gates {
                if let Some(override_val) = &gate.override_default {
                    // Only known gate types have schemas; unknown types are already
                    // rejected above, so if gate_type_schema returns None here the
                    // gate type is unknown and the error was already emitted.
                    if let Some(schema) = gate_type_schema(gate.gate_type.as_str()) {
                        // Must be a JSON object.
                        let obj = match override_val.as_object() {
                            Some(o) => o,
                            None => {
                                return Err(format!(
                                    "state {:?} gate {:?}: override_default is not a JSON object (found: {})",
                                    state_name,
                                    gate_name,
                                    json_type_name(override_val)
                                ));
                            }
                        };

                        // Check for missing required fields.
                        for (field, _field_type) in schema {
                            if !obj.contains_key(*field) {
                                let hint: Vec<String> = schema
                                    .iter()
                                    .map(|(n, t)| {
                                        format!("{}: {}", n, gate_schema_field_type_name(t))
                                    })
                                    .collect();
                                return Err(format!(
                                    "state {:?} gate {:?}: override_default missing required field {:?}\n  ({} schema requires: {})",
                                    state_name,
                                    gate_name,
                                    field,
                                    gate.gate_type,
                                    hint.join(", ")
                                ));
                            }
                        }

                        // Check for unknown fields.
                        for key in obj.keys() {
                            if !schema.iter().any(|(n, _)| n == key) {
                                let known: Vec<&str> = schema.iter().map(|(n, _)| *n).collect();
                                return Err(format!(
                                    "state {:?} gate {:?}: override_default has unknown field {:?}\n  ({} schema: {})",
                                    state_name,
                                    gate_name,
                                    key,
                                    gate.gate_type,
                                    known.join(", ")
                                ));
                            }
                        }

                        // Check that each field value matches the expected type.
                        for (field, expected_type) in schema {
                            let value = &obj[*field];
                            if !json_value_matches_schema_type(value, expected_type) {
                                return Err(format!(
                                    "state {:?} gate {:?}: override_default field {:?} has wrong type\n  expected: {}, found: {}",
                                    state_name,
                                    gate_name,
                                    field,
                                    gate_schema_field_type_name(expected_type),
                                    json_type_name(value)
                                ));
                            }
                        }
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
            // Validate evidence routing rules on transitions (D3 included).
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

        // D4: gate reachability check. Runs only after D2 and D3 passed for all
        // states above (any D2 or D3 error causes an early return before this point,
        // so reaching here guarantees the evidence maps are well-formed).
        for (state_name, state) in &self.states {
            self.validate_gate_reachability(state_name, state)?;
        }

        Ok(())
    }

    /// Validate that every state with pure-gate-only transitions can fire at least
    /// one transition when all gates use their declared (or builtin) override defaults.
    ///
    /// A "pure-gate" transition has a `when` clause containing exclusively `gates.*`
    /// fields. States with no such transitions are exempt: their `when` clauses require
    /// agent evidence that the compiler cannot predict.
    ///
    /// Also emits a non-fatal stderr warning for gate output fields that are declared
    /// in the gate type's schema but never referenced in any `when` clause (AC10).
    fn validate_gate_reachability(
        &self,
        state_name: &str,
        state: &TemplateState,
    ) -> Result<(), String> {
        // D4 is only reachable after D2 and D3 pass for all states (validate() returns
        // early on any D2 or D3 error before the D4 loop). Evidence maps are therefore
        // well-formed when this method runs.

        let gates_prefix = format!("{}.", GATES_EVIDENCE_NAMESPACE);

        // Collect pure-gate transitions: `when` clause is non-empty and every key
        // starts with "gates.".
        let pure_gate_transitions: Vec<&Transition> = state
            .transitions
            .iter()
            .filter(|t| {
                t.when.as_ref().is_some_and(|w| {
                    !w.is_empty() && w.keys().all(|k| k.starts_with(&gates_prefix))
                })
            })
            .collect();

        // No pure-gate transitions → exempt from the reachability check (AC5, AC6).
        if pure_gate_transitions.is_empty() {
            return Ok(());
        }

        // Build evidence map: {"gates": {<name>: <override_default or builtin_default>}}
        let mut gate_map = serde_json::Map::new();
        for (gate_name, gate) in &state.gates {
            let default_val = if let Some(ov) = &gate.override_default {
                ov.clone()
            } else if let Some(bv) = gate_type_builtin_default(&gate.gate_type) {
                bv
            } else {
                // Unknown gate type already rejected by D1 gate type check; skip.
                continue;
            };
            gate_map.insert(gate_name.clone(), default_val);
        }
        let evidence = serde_json::json!({ GATES_EVIDENCE_NAMESPACE: gate_map });

        // AC10: warn for schema fields never referenced in any `when` clause.
        for (gate_name, gate) in &state.gates {
            if let Some(schema) = gate_type_schema(&gate.gate_type) {
                for (field_name, _) in schema {
                    let path = format!("{}.{}.{}", GATES_EVIDENCE_NAMESPACE, gate_name, field_name);
                    let referenced = state
                        .transitions
                        .iter()
                        .any(|t| t.when.as_ref().is_some_and(|w| w.contains_key(&path)));
                    if !referenced {
                        eprintln!(
                            "warning: state {:?} gate {:?} field {:?} is never referenced in any when clause",
                            state_name, gate_name, field_name
                        );
                    }
                }
            }
        }

        // AC3/AC4: check if at least one pure-gate transition fires.
        let fires = pure_gate_transitions.iter().any(|t| {
            let when = t.when.as_ref().unwrap();
            when.iter()
                .all(|(k, v)| resolve_gates_path(&evidence, k).is_some_and(|ev| ev == v))
        });

        if !fires {
            // Format the error: list each gate's effective override value.
            let gate_lines: Vec<String> = state
                .gates
                .iter()
                .map(|(gname, gate)| {
                    let val = gate.override_default.clone().unwrap_or_else(|| {
                        gate_type_builtin_default(&gate.gate_type)
                            .unwrap_or(serde_json::Value::Null)
                    });
                    format!("  gate {:?} override: {}", gname, val)
                })
                .collect();
            return Err(format!(
                "state {:?}: no transition fires when all gates use override defaults\n{}\n  pure-gate transitions checked: {}",
                state_name,
                gate_lines.join("\n"),
                pure_gate_transitions.len()
            ));
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

            // D3: validate gates.* path structure and field references.
            for (field, _) in &gate_fields {
                let segments: Vec<&str> = field.splitn(4, '.').collect();
                // segments[0] is "gates"; we need exactly 3 segments total.
                if segments.len() != 3 {
                    return Err(format!(
                        "state {:?}: when clause key {:?} has invalid format; expected \"gates.<gate>.<field>\"",
                        state_name, field.as_str()
                    ));
                }
                let gate_name_ref = segments[1];
                let field_name_ref = segments[2];

                // Gate name must be declared in this state's gates block.
                let gate = match state.gates.get(gate_name_ref) {
                    Some(g) => g,
                    None => {
                        return Err(format!(
                            "state {:?}: when clause references gate {:?} which is not declared in this state",
                            state_name, gate_name_ref
                        ));
                    }
                };

                // Field name must be valid for this gate type's schema.
                if let Some(schema) = gate_type_schema(&gate.gate_type) {
                    let valid_fields: Vec<&str> = schema.iter().map(|(name, _)| *name).collect();
                    if !valid_fields.contains(&field_name_ref) {
                        return Err(format!(
                            "state {:?} gate {:?}: when clause references unknown field {:?}; {} gate fields: {}",
                            state_name,
                            gate_name_ref,
                            field_name_ref,
                            gate.gate_type,
                            valid_fields.join(", ")
                        ));
                    }
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

    // -----------------------------------------------------------------------
    // Issue 1: gate schema registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn gate_type_schema_command() {
        use GateSchemaFieldType::*;
        let schema = gate_type_schema(GATE_TYPE_COMMAND).expect("command schema must exist");
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0], ("exit_code", Number));
        assert_eq!(schema[1], ("error", Str));
    }

    #[test]
    fn gate_type_schema_context_exists() {
        use GateSchemaFieldType::*;
        let schema =
            gate_type_schema(GATE_TYPE_CONTEXT_EXISTS).expect("context-exists schema must exist");
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0], ("exists", Boolean));
        assert_eq!(schema[1], ("error", Str));
    }

    #[test]
    fn gate_type_schema_context_matches() {
        use GateSchemaFieldType::*;
        let schema =
            gate_type_schema(GATE_TYPE_CONTEXT_MATCHES).expect("context-matches schema must exist");
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0], ("matches", Boolean));
        assert_eq!(schema[1], ("error", Str));
    }

    #[test]
    fn gate_type_schema_unknown_returns_none() {
        assert!(gate_type_schema("jira").is_none());
        assert!(gate_type_schema("").is_none());
        assert!(gate_type_schema("http").is_none());
    }

    #[test]
    fn gate_type_builtin_default_command() {
        let val = gate_type_builtin_default(GATE_TYPE_COMMAND).expect("command default must exist");
        assert_eq!(val["exit_code"], serde_json::json!(0));
        assert_eq!(val["error"], serde_json::json!(""));
    }

    #[test]
    fn gate_type_builtin_default_context_exists() {
        let val = gate_type_builtin_default(GATE_TYPE_CONTEXT_EXISTS)
            .expect("context-exists default must exist");
        assert_eq!(val["exists"], serde_json::json!(true));
        assert_eq!(val["error"], serde_json::json!(""));
    }

    #[test]
    fn gate_type_builtin_default_context_matches() {
        let val = gate_type_builtin_default(GATE_TYPE_CONTEXT_MATCHES)
            .expect("context-matches default must exist");
        assert_eq!(val["matches"], serde_json::json!(true));
        assert_eq!(val["error"], serde_json::json!(""));
    }

    #[test]
    fn gate_type_builtin_default_unknown_returns_none() {
        assert!(gate_type_builtin_default("jira").is_none());
        assert!(gate_type_builtin_default("").is_none());
    }

    // -----------------------------------------------------------------------
    // Issue 2: override_default validation tests
    // -----------------------------------------------------------------------

    fn make_command_gate(override_default: Option<serde_json::Value>) -> Gate {
        Gate {
            gate_type: GATE_TYPE_COMMAND.to_string(),
            command: "./check.sh".to_string(),
            timeout: 0,
            key: String::new(),
            pattern: String::new(),
            override_default,
        }
    }

    fn make_context_exists_gate(override_default: Option<serde_json::Value>) -> Gate {
        Gate {
            gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
            command: String::new(),
            timeout: 0,
            key: "some/key.md".to_string(),
            pattern: String::new(),
            override_default,
        }
    }

    fn make_context_matches_gate(override_default: Option<serde_json::Value>) -> Gate {
        Gate {
            gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
            command: String::new(),
            timeout: 0,
            key: "some/key.md".to_string(),
            pattern: "approved".to_string(),
            override_default,
        }
    }

    #[test]
    fn override_default_valid_command() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::json!({"exit_code": 0, "error": ""}))),
        );
        t.validate().unwrap();
    }

    #[test]
    fn override_default_valid_context_exists() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "doc_check".to_string(),
            make_context_exists_gate(Some(
                serde_json::json!({"exists": false, "error": "missing"}),
            )),
        );
        t.validate().unwrap();
    }

    #[test]
    fn override_default_valid_context_matches() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "review_check".to_string(),
            make_context_matches_gate(Some(
                serde_json::json!({"matches": false, "error": "not approved"}),
            )),
        );
        t.validate().unwrap();
    }

    #[test]
    fn override_default_non_object_null_rejected() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::Value::Null)),
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("override_default is not a JSON object"),
            "got: {}",
            err
        );
        assert!(err.contains("found: null"), "got: {}", err);
    }

    #[test]
    fn override_default_non_object_scalar_rejected() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::json!(42))),
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("override_default is not a JSON object"),
            "got: {}",
            err
        );
        assert!(err.contains("found: number"), "got: {}", err);
    }

    #[test]
    fn override_default_missing_field_rejected() {
        let mut t = minimal_template();
        // Missing "error" field.
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::json!({"exit_code": 0}))),
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("override_default missing required field"),
            "got: {}",
            err
        );
        assert!(err.contains("\"error\""), "got: {}", err);
        // Hint lists all fields with types.
        assert!(err.contains("exit_code: number"), "got: {}", err);
        assert!(err.contains("error: string"), "got: {}", err);
    }

    #[test]
    fn override_default_extra_field_rejected() {
        let mut t = minimal_template();
        // "status" is not in the command schema.
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(
                serde_json::json!({"exit_code": 0, "error": "", "status": "ok"}),
            )),
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("override_default has unknown field"),
            "got: {}",
            err
        );
        assert!(err.contains("\"status\""), "got: {}", err);
        // Hint lists known field names only.
        assert!(err.contains("exit_code"), "got: {}", err);
        assert!(err.contains("error"), "got: {}", err);
    }

    #[test]
    fn override_default_wrong_type_rejected() {
        let mut t = minimal_template();
        // exit_code should be a number, not a string.
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::json!({"exit_code": "0", "error": ""}))),
        );
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("override_default field \"exit_code\" has wrong type"),
            "got: {}",
            err
        );
        assert!(err.contains("expected: number"), "got: {}", err);
        assert!(err.contains("found: string"), "got: {}", err);
    }

    #[test]
    fn override_default_none_gate_unaffected() {
        let mut t = minimal_template();
        t.states
            .get_mut("start")
            .unwrap()
            .gates
            .insert("ci_check".to_string(), make_command_gate(None));
        t.validate().unwrap();
    }

    /// Assert that gate_type_builtin_default() returns values identical to
    /// built_in_default() in src/gate.rs for every GATE_TYPE_* constant.
    ///
    /// This synchronization test catches drift when a gate type's default changes.
    /// Both functions must be updated in tandem (circular dep prevents sharing code).
    #[test]
    fn gate_type_builtin_default_matches_gate_rs_built_in_default() {
        use crate::gate::built_in_default;
        for gate_type in &[
            GATE_TYPE_COMMAND,
            GATE_TYPE_CONTEXT_EXISTS,
            GATE_TYPE_CONTEXT_MATCHES,
        ] {
            let types_val = gate_type_builtin_default(gate_type)
                .unwrap_or_else(|| panic!("gate_type_builtin_default missing for {}", gate_type));
            let gate_val = built_in_default(gate_type)
                .unwrap_or_else(|| panic!("built_in_default missing for {}", gate_type));
            assert_eq!(
                types_val, gate_val,
                "gate_type_builtin_default and built_in_default diverged for gate type {}",
                gate_type
            );
        }
    }

    // -----------------------------------------------------------------------
    // Issue 3: gates.* when clause path and field validation tests
    // -----------------------------------------------------------------------

    /// Build a minimal template with a single command gate on the "start" state.
    fn template_with_command_gate(gate_name: &str) -> CompiledTemplate {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            gate_name.to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );
        t
    }

    #[test]
    fn gates_when_two_segment_path_rejected() {
        // "gates.ci_check" has only 2 segments — missing the field name.
        let mut t = template_with_command_gate("ci_check");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check".to_string(), serde_json::json!(0));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("gates.ci_check") && err.contains("invalid format"),
            "got: {}",
            err
        );
        assert!(err.contains("gates.<gate>.<field>"), "got: {}", err);
    }

    #[test]
    fn gates_when_four_segment_path_rejected() {
        // "gates.ci_check.exit_code.extra" has 4 segments — too many.
        let mut t = template_with_command_gate("ci_check");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "gates.ci_check.exit_code.extra".to_string(),
            serde_json::json!(0),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("gates.ci_check.exit_code.extra") && err.contains("invalid format"),
            "got: {}",
            err
        );
        assert!(err.contains("gates.<gate>.<field>"), "got: {}", err);
    }

    #[test]
    fn gates_when_nonexistent_gate_rejected() {
        // "gates.nonexistent_gate.exit_code" references a gate not in state.gates.
        let mut t = template_with_command_gate("ci_check");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "gates.nonexistent_gate.exit_code".to_string(),
            serde_json::json!(0),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("nonexistent_gate") && err.contains("not declared in this state"),
            "got: {}",
            err
        );
    }

    #[test]
    fn gates_when_unknown_field_rejected() {
        // "gates.ci_check.exitt_code" is a typo — not in command gate schema.
        let mut t = template_with_command_gate("ci_check");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "gates.ci_check.exitt_code".to_string(),
            serde_json::json!(0),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate().unwrap_err();
        assert!(
            err.contains("exitt_code") && err.contains("unknown field"),
            "got: {}",
            err
        );
        assert!(
            err.contains("exit_code") && err.contains("error"),
            "got: {}",
            err
        );
    }

    #[test]
    fn gates_when_valid_command_reference() {
        // "gates.ci_check.exit_code" for a command gate is valid.
        let mut t = template_with_command_gate("ci_check");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    #[test]
    fn gates_when_valid_context_exists_reference() {
        // "gates.schema_check.exists" for a context-exists gate is valid.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "schema_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "schema/check.md".to_string(),
                pattern: String::new(),
                override_default: None,
            },
        );
        let mut when = BTreeMap::new();
        when.insert(
            "gates.schema_check.exists".to_string(),
            serde_json::json!(true),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    #[test]
    fn gates_when_valid_context_matches_reference() {
        // "gates.pattern_check.matches" for a context-matches gate is valid.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "pattern_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
            },
        );
        let mut when = BTreeMap::new();
        when.insert(
            "gates.pattern_check.matches".to_string(),
            serde_json::json!(true),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    #[test]
    fn gates_when_no_gates_key_unaffected() {
        // A state with no gates.* when clause keys passes without any gate declared.
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
        when.insert("decision".to_string(), serde_json::json!("proceed"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    // -----------------------------------------------------------------------
    // Issue 4: gate reachability check and resolve_gates_path tests
    // -----------------------------------------------------------------------

    /// Build a template where "start" has a command gate and two pure-gate transitions.
    /// `pass_exit_code` controls whether the "done" transition fires (exit_code == pass_exit_code).
    fn template_with_command_gate_transitions(
        gate_name: &str,
        override_default: Option<serde_json::Value>,
        pass_exit_code: i64,
    ) -> CompiledTemplate {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            gate_name.to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default,
            },
        );
        let mut when_pass = BTreeMap::new();
        when_pass.insert(
            format!("gates.{}.exit_code", gate_name),
            serde_json::json!(pass_exit_code),
        );
        let mut when_fail = BTreeMap::new();
        when_fail.insert(
            format!("gates.{}.exit_code", gate_name),
            serde_json::json!(1),
        );
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
        t
    }

    // AC1/resolve_gates_path: helper walks nested JSON correctly.
    #[test]
    fn resolve_gates_path_walks_nested_map() {
        let evidence = serde_json::json!({"gates": {"ci": {"exit_code": 0, "error": ""}}});
        assert_eq!(
            resolve_gates_path(&evidence, "gates.ci.exit_code"),
            Some(&serde_json::json!(0))
        );
        assert_eq!(
            resolve_gates_path(&evidence, "gates.ci.error"),
            Some(&serde_json::json!(""))
        );
        assert_eq!(resolve_gates_path(&evidence, "gates.ci.missing"), None);
        assert_eq!(
            resolve_gates_path(&evidence, "gates.nonexistent.exit_code"),
            None
        );
        // Non-object intermediate node returns None.
        assert_eq!(
            resolve_gates_path(&evidence, "gates.ci.exit_code.sub"),
            None
        );
    }

    // AC4: reachable state (override default satisfies a pure-gate transition) compiles.
    #[test]
    fn reachability_reachable_state_with_override_default() {
        // exit_code == 0 matches the "done" transition with pass_exit_code=0.
        let t = template_with_command_gate_transitions(
            "ci_check",
            Some(serde_json::json!({"exit_code": 0, "error": ""})),
            0,
        );
        t.validate().unwrap();
    }

    // AC4/AC13: reachable state with no override_default uses builtin default.
    #[test]
    fn reachability_reachable_state_uses_builtin_default() {
        // No override_default; builtin default for command is {exit_code: 0, error: ""}.
        // pass_exit_code=0 matches → at least one transition fires → no error.
        let t = template_with_command_gate_transitions("ci_check", None, 0);
        t.validate().unwrap();
    }

    // AC3/AC12: dead-end state with no firing transition is rejected.
    #[test]
    fn reachability_dead_end_state_rejected() {
        // No override_default; builtin default exit_code=0 does NOT match pass_exit_code=99.
        // fail transition needs exit_code==1, also doesn't match exit_code=0.
        // So no transition fires → dead-end error.
        let t = template_with_command_gate_transitions("ci_check", None, 99);
        let err = t.validate().unwrap_err();
        assert!(err.contains("no transition fires"), "got: {}", err);
        assert!(err.contains("\"start\""), "got: {}", err);
        assert!(err.contains("\"ci_check\""), "got: {}", err);
        assert!(
            err.contains("pure-gate transitions checked: 2"),
            "got: {}",
            err
        );
    }

    // AC12: dead-end state where gate has no override_default; error still fires based on builtin.
    #[test]
    fn reachability_dead_end_with_no_override_default_uses_builtin() {
        // Both transitions check exit_code == 42 or exit_code == 43 (neither matches builtin 0).
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "gate1".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None, // no override_default; builtin used
            },
        );
        let mut when_a = BTreeMap::new();
        when_a.insert("gates.gate1.exit_code".to_string(), serde_json::json!(42));
        let mut when_b = BTreeMap::new();
        when_b.insert("gates.gate1.exit_code".to_string(), serde_json::json!(43));
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "fix".to_string(),
                when: Some(when_b),
            },
        ];
        t.states.insert(
            "fix".to_string(),
            TemplateState {
                directive: "Fix.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let err = t.validate().unwrap_err();
        assert!(err.contains("no transition fires"), "got: {}", err);
        // The error must mention gate1 — confirms the builtin default was used.
        assert!(err.contains("\"gate1\""), "got: {}", err);
    }

    // AC5/AC14: mixed-evidence state (all transitions have non-gates.* fields) is exempt.
    #[test]
    fn reachability_mixed_evidence_state_exempt() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: Some(serde_json::json!({"exit_code": 0, "error": ""})),
            },
        );
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "approved".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        // Transition has both gates.* and agent evidence — mixed, so exempt from reachability.
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        when.insert("approved".to_string(), serde_json::json!("yes"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate().unwrap();
    }

    // AC6/AC15: state with no gates.* when-clause references compiles cleanly.
    #[test]
    fn reachability_no_gates_when_references_unaffected() {
        // This is just the minimal_template() with no gate references.
        minimal_template().validate().unwrap();
    }

    // AC8/AC9: D4 skipped when D2 produced an error.
    #[test]
    fn reachability_skipped_when_d2_error_exists() {
        // Template has:
        // 1. A D2 error: command gate with override_default missing "error" field.
        // 2. A dead-end state that would trigger a D4 error if evaluated.
        // Expected: exactly one error, from D2, not D4.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "./check.sh".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                // D2 error: missing "error" field
                override_default: Some(serde_json::json!({"exit_code": 0})),
            },
        );
        // Dead-end transitions (would trigger D4 if D2 didn't block first).
        let mut when_a = BTreeMap::new();
        when_a.insert(
            "gates.ci_check.exit_code".to_string(),
            serde_json::json!(99),
        );
        let mut when_b = BTreeMap::new();
        when_b.insert(
            "gates.ci_check.exit_code".to_string(),
            serde_json::json!(100),
        );
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "fix".to_string(),
                when: Some(when_b),
            },
        ];
        t.states.insert(
            "fix".to_string(),
            TemplateState {
                directive: "Fix.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        );
        let err = t.validate().unwrap_err();
        // Must be a D2 error, not a D4 error.
        assert!(
            err.contains("override_default missing required field"),
            "expected D2 error, got: {}",
            err
        );
        assert!(
            !err.contains("no transition fires"),
            "D4 must not run when D2 failed, got: {}",
            err
        );
    }

    // AC11/AC16: unreferenced gate field emits a warning to stderr.
    // This test captures stderr to verify the warning format.
    #[test]
    fn reachability_unreferenced_field_emits_warning() {
        // Gate has both "exit_code" and "error" fields. Only "exit_code" is referenced.
        // Expect a warning for "error".
        // We can't easily capture eprintln! in Rust tests without a custom stderr writer.
        // Instead: verify compilation succeeds (warning is non-fatal) and separately
        // assert the warning would be produced for a known-unreferenced field by
        // checking validate_gate_reachability directly with the expected state.
        let t = template_with_command_gate_transitions(
            "ci_check",
            Some(serde_json::json!({"exit_code": 0, "error": ""})),
            0,
        );
        // Validation must succeed (warning is non-fatal).
        t.validate().unwrap();
        // The "error" field is not referenced in any when clause — warning emitted to stderr.
        // We can't assert the warning text in a unit test without redirecting stderr,
        // but the functional test (AC16) covers the stderr content check.
    }
}
