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
///
/// Note: `#[serde(deny_unknown_fields)]` is intentionally NOT applied here.
/// `CompiledTemplate` is loaded from the compile cache, which may be written
/// by a newer binary than the reader. Adding new fields to `CompiledTemplate`
/// must remain non-breaking for older binaries during version churn.
/// The strict unknown-field check lives on `SourceState` (the YAML
/// front-matter intermediate) instead — see `src/template/compile.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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
    /// Declares that this state spawns child workflows from a task-list
    /// evidence field when entered. Runtime behavior is implemented by the
    /// batch scheduler; this field is purely the compile-time contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_children: Option<MaterializeChildrenSpec>,
    /// When true and the state is terminal, marks this state as a failure
    /// outcome (propagated by the batch scheduler to the parent's
    /// `children-complete` gate output). Meaningful only when `terminal` is
    /// true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failure: bool,
    /// When true, the scheduler can synthesize child markers that land
    /// directly in this state to represent "skipped" children. Meaningful
    /// only when `terminal` is true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped_marker: bool,
    /// Optional auto-advance predicate. When all key-value pairs in this map
    /// match the available evidence (gate output + template variables), the
    /// engine auto-transitions from this state without waiting for agent
    /// evidence. Uses the same dot-path syntax as `when` clauses on transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if: Option<BTreeMap<String, serde_json::Value>>,
}

/// Template-level declaration that a state fans out child workflows from an
/// agent-submitted task list.
///
/// Placed on `TemplateState` alongside `gates`, `accepts`, and
/// `default_action`. The compiler validates the hook at load time; the
/// scheduler (lands in a later phase) reads the hook at runtime to
/// materialize children.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterializeChildrenSpec {
    /// Name of the `accepts` field on this state that carries the task
    /// list. The field's declared type must be `tasks`.
    pub from_field: String,
    /// Path to the default child template to use when a task entry omits
    /// `template`. The compiler validates that the path resolves.
    pub default_template: String,
    /// Policy for propagating failures to dependent tasks within the batch.
    /// Defaults to `SkipDependents`.
    #[serde(default = "default_failure_policy")]
    pub failure_policy: FailurePolicy,
}

/// Policy controlling how a batch handles failures of upstream tasks.
///
/// - `SkipDependents` (default): tasks whose `waits_on` chain transitively
///   includes a failed task are synthesized as terminal skip markers.
/// - `Continue`: a failure of one task does not prevent dependents from
///   running; each task is judged independently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    SkipDependents,
    Continue,
}

/// Serde default helper: `FailurePolicy::SkipDependents`.
pub fn default_failure_policy() -> FailurePolicy {
    FailurePolicy::SkipDependents
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
    /// Completion condition for children-complete gates.
    ///
    /// Controls when a child workflow counts as complete. Currently only
    /// `"terminal"` is implemented (child reached a terminal state). The
    /// prefixes `"state:*"` and `"context:*"` are reserved for future use.
    /// Defaults to `"terminal"` when not specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<String>,
    /// Name prefix filter for children-complete gates.
    ///
    /// When set, only child workflows whose name starts with this prefix
    /// are considered by the gate. Enables multi-fanout scoping (e.g.,
    /// only research children, not all children).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_filter: Option<String>,
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
/// Gate type: check whether all child workflows have reached their completion condition.
pub const GATE_TYPE_CHILDREN_COMPLETE: &str = "children-complete";

/// Evidence namespace reserved for engine-injected gate output.
/// Agent submissions starting with this prefix are rejected (Feature 2, R7).
/// All `gates.*` key checks in advance.rs and types.rs use this constant.
pub const GATES_EVIDENCE_NAMESPACE: &str = "gates";

/// Prefix used in when-clause keys to match on the presence of an agent-submitted
/// evidence field. Paired with the sentinel value [`PRESENT_MATCHER_VALUE`] to form
/// expressions like `evidence.retry_failed: present` that fire when the named field
/// appears in any event since the last state transition (Issue #11).
pub const EVIDENCE_NAMESPACE: &str = "evidence";

/// Sentinel string value that, when it appears as the value of a when-clause entry
/// under the `evidence.<field>` key, triggers the presence matcher instead of the
/// default scalar-equality check.
pub const PRESENT_MATCHER_VALUE: &str = "present";

/// Returns true if `value` is the JSON string `"present"` used as the presence-matcher
/// sentinel. The check is case-sensitive so literal values like `"Present"` continue
/// to route via the value-equality path.
pub fn is_present_matcher(value: &serde_json::Value) -> bool {
    value.as_str() == Some(PRESENT_MATCHER_VALUE)
}

/// Namespace prefix for template variable existence checks in `when` clauses.
/// Keys like `vars.MY_VAR` reference declared template variables.
pub const VARS_NAMESPACE: &str = "vars";

/// Check if a `when` value is an `{is_set: bool}` matcher.
///
/// Returns `Some(true)` for `{"is_set": true}`, `Some(false)` for
/// `{"is_set": false}`, and `None` for any other shape.
pub fn is_is_set_matcher(value: &serde_json::Value) -> Option<bool> {
    let obj = value.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    obj.get("is_set")?.as_bool()
}

/// Return `true` when every key-value pair in `when` is satisfied by
/// `skip_conditions` acting as synthetic evidence.
///
/// Used by the compile-time E-SKIP-AMBIGUOUS check in `validate()` to determine
/// which conditional transitions the skip_if values would activate. The runtime
/// evaluator (`conditions_satisfied()` in `src/engine/advance.rs`) answers a
/// different question — whether actual runtime evidence satisfies the skip_if
/// predicate — so it does not call this function. What the two share is
/// `is_is_set_matcher`: both use it to interpret `{is_set: bool}` values for
/// the `vars.NAME` case, keeping the matching semantics aligned.
///
/// # Matching rules
///
/// - **`vars.NAME: {is_set: bool}`** — compile-time approximation: the
///   condition is satisfied when `skip_conditions` provides a set/unset signal
///   for the same variable key.  No variable store is available at compile
///   time; we look at whether `skip_conditions` contains an `is_set`-shaped
///   value (or any non-null value) for the key.
/// - **direct value equality** — the `skip_conditions` map must contain the
///   key with an equal JSON value.
pub(crate) fn skip_if_matches_when(
    skip_conditions: &BTreeMap<String, serde_json::Value>,
    when: &BTreeMap<String, serde_json::Value>,
) -> bool {
    let vars_prefix = format!("{}.", VARS_NAMESPACE);
    when.iter().all(|(field, expected)| {
        // vars.NAME: {is_set: bool} path.
        if field.starts_with(&vars_prefix) {
            if let Some(expected_set) = is_is_set_matcher(expected) {
                let is_set = skip_conditions
                    .get(field.as_str())
                    .or_else(|| {
                        let var_name = &field[vars_prefix.len()..];
                        skip_conditions.get(var_name)
                    })
                    .map(|v| {
                        if let Some(b) = is_is_set_matcher(v) {
                            b
                        } else {
                            !v.is_null()
                        }
                    })
                    .unwrap_or(false);
                return is_set == expected_set;
            }
        }
        // Direct value equality path.
        skip_conditions.get(field.as_str()) == Some(expected)
    })
}

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
        GATE_TYPE_CHILDREN_COMPLETE => Some(&[
            ("total", Number),
            ("completed", Number),
            ("pending", Number),
            ("all_complete", Boolean),
            ("error", Str),
        ]),
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
        GATE_TYPE_CHILDREN_COMPLETE => Some(serde_json::json!({
            "total": 0,
            "completed": 0,
            "pending": 0,
            "success": 0,
            "failed": 0,
            "skipped": 0,
            "blocked": 0,
            "spawn_failed": 0,
            "all_complete": true,
            "all_success": true,
            "any_failed": false,
            "any_skipped": false,
            "any_spawn_failed": false,
            "needs_attention": false,
            "children": [],
            "error": ""
        })),
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
///
/// `"tasks"` is a structured array type used by the batch child spawning
/// feature; the compiler knows its exact shape (see `item_schema` generation
/// in `src/cli/next_types.rs::derive_expects`), so templates declare
/// `type: tasks` without writing any schema by hand.
const VALID_FIELD_TYPES: &[&str] = &["enum", "string", "number", "boolean", "tasks"];

/// Field type marker for structured task-list evidence fields consumed by
/// the `materialize_children` hook.
pub const FIELD_TYPE_TASKS: &str = "tasks";

/// Runtime-injected variable names that are valid in templates but not
/// declared in the variables block. These are provided by the engine at
/// runtime (e.g., SESSION_DIR is the session directory path).
const RUNTIME_VARIABLE_NAMES: &[&str] = &["SESSION_DIR", "SESSION_NAME"];

impl CompiledTemplate {
    /// Validate the compiled template against all schema rules.
    ///
    /// When `strict` is `true`, a state that has gates but no `gates.*`
    /// when-clause references is a hard error (D5). When `strict` is `false`,
    /// the same condition emits a warning to stderr and validation continues.
    /// D4 (gate reachability / unreferenced-field warnings) is suppressed when
    /// `strict` is `false` — those warnings are aimed at template authors, not
    /// at agents running workflows.
    pub fn validate(&self, strict: bool) -> Result<(), String> {
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
                    GATE_TYPE_CHILDREN_COMPLETE => {
                        // Validate completion field.
                        if let Some(completion) = &gate.completion {
                            if completion != "terminal"
                                && !completion.starts_with("state:")
                                && !completion.starts_with("context:")
                            {
                                return Err(format!(
                                    "state {:?} gate {:?}: unknown completion prefix {:?}; \
                                     only \"terminal\" is supported (\"state:*\" and \"context:*\" are reserved)",
                                    state_name, gate_name, completion
                                ));
                            }
                            if completion.starts_with("state:") {
                                return Err(format!(
                                    "state {:?} gate {:?}: completion mode {:?} is reserved but not yet implemented",
                                    state_name, gate_name, completion
                                ));
                            }
                            if completion.starts_with("context:") {
                                return Err(format!(
                                    "state {:?} gate {:?}: completion mode {:?} is reserved but not yet implemented",
                                    state_name, gate_name, completion
                                ));
                            }
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
                             must be one of: enum, string, number, boolean, tasks",
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
            // Validate skip_if field rules (before evidence-routing validation so that
            // E-SKIP-AMBIGUOUS is evaluated independently of the mutual-exclusivity check).
            if let Some(skip_conditions) = &state.skip_if {
                // E-SKIP-TERMINAL: skip_if on a terminal state is unreachable.
                if state.terminal {
                    return Err(format!(
                        "E-SKIP-TERMINAL: state {:?}: skip_if cannot be declared on a terminal state; \
                         the terminal check fires before skip_if, making it unreachable\n  \
                         remedy: remove skip_if or make the state non-terminal",
                        state_name
                    ));
                }

                // E-SKIP-NO-TRANSITIONS: skip_if with no transitions has no target to advance to.
                if state.transitions.is_empty() {
                    return Err(format!(
                        "E-SKIP-NO-TRANSITIONS: state {:?}: skip_if requires at least one declared transition\n  \
                         remedy: add a transition target, or remove skip_if",
                        state_name
                    ));
                }

                // E-SKIP-AMBIGUOUS: when all transitions are conditional, simulate skip_if values
                // as synthetic evidence against each conditional transition's when clause.
                // Zero matches or more than one match is an error.
                //
                // When the state has a mix of conditional and unconditional transitions
                // (i.e. `all_conditional` is false), E-SKIP-AMBIGUOUS does not apply:
                // the unconditional transition acts as the fallback route and guarantees
                // that skip_if always has somewhere to advance. The ambiguity check is
                // only meaningful when every branch requires a specific condition to fire.
                let all_conditional = state.transitions.iter().all(|t| t.when.is_some());
                if all_conditional {
                    let matches: Vec<&str> = state
                        .transitions
                        .iter()
                        .filter(|t| {
                            if let Some(when) = &t.when {
                                skip_if_matches_when(skip_conditions, when)
                            } else {
                                false
                            }
                        })
                        .map(|t| t.target.as_str())
                        .collect();

                    match matches.len() {
                        0 => {
                            return Err(format!(
                                "E-SKIP-AMBIGUOUS: state {:?}: skip_if values match zero conditional transitions; \
                                 exactly one must match\n  \
                                 remedy: ensure skip_if values match the when clause of exactly one transition",
                                state_name
                            ));
                        }
                        1 => {} // exactly one match — valid
                        _ => {
                            return Err(format!(
                                "E-SKIP-AMBIGUOUS: state {:?}: skip_if values match more than one conditional transition {:?}; \
                                 exactly one must match\n  \
                                 remedy: refine skip_if values or when clauses so exactly one transition matches",
                                state_name,
                                matches
                            ));
                        }
                    }
                }

                // W-SKIP-GATE-ABSENT: warn when a skip_if key of the form gates.NAME.*
                // references a gate name not declared on this state.
                let gates_prefix = format!("{}.", GATES_EVIDENCE_NAMESPACE);
                for key in skip_conditions.keys() {
                    if key.starts_with(&gates_prefix) {
                        let segments: Vec<&str> = key.splitn(3, '.').collect();
                        if segments.len() >= 2 {
                            let gate_name_ref = segments[1];
                            if !state.gates.contains_key(gate_name_ref) {
                                eprintln!(
                                    "warning: W-SKIP-GATE-ABSENT: state {:?}: skip_if key {:?} references \
                                     gate {:?} which is not declared on this state; \
                                     the condition will be silently unmatchable at runtime\n  \
                                     remedy: declare a gate named {:?} on this state, or correct the key",
                                    state_name, key, gate_name_ref, gate_name_ref
                                );
                            }
                        }
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

        // Issue 8: compile rules for materialize_children (E1-E10) run
        // before D5/D4 so failures in the hook surface with rule-specific
        // messages rather than generic legacy-gate diagnostics. E9's
        // "resolves to a compilable template" check and F5 live in
        // `compile()` because they need the source path to resolve
        // relative template references.
        self.validate_materialize_children_errors()?;

        // D5: legacy gate detection. A state with gates but no `gates.*`
        // when-clause references uses the legacy boolean pass/block path. In
        // strict mode this is an error; in permissive mode it is a stderr
        // warning and validation continues.
        let gates_ns_prefix = format!("{}.", GATES_EVIDENCE_NAMESPACE);
        for (state_name, state) in &self.states {
            if state.gates.is_empty() {
                continue;
            }
            // Check whether any transition's when clause references gates.*.
            let has_gates_routing = state.transitions.iter().any(|t| {
                t.when
                    .as_ref()
                    .is_some_and(|w| w.keys().any(|k| k.starts_with(&gates_ns_prefix)))
            });
            if !has_gates_routing {
                for gate_name in state.gates.keys() {
                    if strict {
                        return Err(format!(
                            "state {:?}: gate {:?} has no gates.* routing\n  \
                             add a when clause referencing gates.{gate_name}.passed, \
                             gates.{gate_name}.error, ... \
                             or use --allow-legacy-gates to permit boolean pass/block behavior",
                            state_name, gate_name
                        ));
                    } else {
                        eprintln!(
                            "warning: state {:?}: gate {:?} has no gates.* routing (legacy behavior)",
                            state_name, gate_name
                        );
                    }
                }
            }
        }

        // D4: gate reachability check. Runs only after D2 and D3 passed for all
        // states above (any D2 or D3 error causes an early return before this point,
        // so reaching here guarantees the evidence maps are well-formed).
        for (state_name, state) in &self.states {
            self.validate_gate_reachability(state_name, state, strict)?;
        }

        // Issue 8: emit non-fatal warnings W1-W5. These never fail
        // compilation regardless of `strict`; they surface on stderr via
        // the same `eprintln!("warning: ...")` convention as D4/D5.
        for warning in self.collect_materialize_children_warnings() {
            eprintln!("warning: {}", warning);
        }

        // Issue #11: emit non-fatal warning W6 when the present-matcher sentinel
        // is used against a non-evidence path (e.g. `context.foo: present` or a
        // flat agent-evidence key). Same convention as W1-W5.
        for warning in self.collect_when_clause_warnings() {
            eprintln!("warning: {}", warning);
        }

        Ok(())
    }

    /// Validate compile-time error rules E1-E10 tied to the
    /// `materialize_children` hook.
    ///
    /// | Rule | Check |
    /// |------|-------|
    /// | E1   | `from_field` is non-empty |
    /// | E2   | `from_field` names a declared accepts field |
    /// | E3   | Referenced field has `type: tasks` |
    /// | E4   | Referenced field has `required: true` |
    /// | E5   | Declaring state is not terminal |
    /// | E6   | `failure_policy` is `skip_dependents` or `continue` (enforced by serde enum) |
    /// | E7   | State has at least one outgoing transition |
    /// | E8   | No two states reference the same `from_field` |
    /// | E9   | `default_template` is non-empty (resolution check lives in `compile()`) |
    /// | E10  | State with `materialize_children` must declare a `children-complete` gate |
    ///
    /// Returns on the first violation with an error message that names the
    /// offending state and a one-line remedy.
    fn validate_materialize_children_errors(&self) -> Result<(), String> {
        // E8 requires cross-state correlation of `from_field` values.
        let mut seen_from_fields: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // Deterministic iteration order: states are stored in a BTreeMap so
        // errors are reproducible across runs.
        for (state_name, state) in &self.states {
            let hook = match &state.materialize_children {
                Some(h) => h,
                None => continue,
            };

            // E1: from_field is non-empty.
            if hook.from_field.is_empty() {
                return Err(format!(
                    "E1: state {:?}: materialize_children.from_field must not be empty\n  \
                     remedy: set from_field to the name of a `tasks`-typed accepts field on this state",
                    state_name
                ));
            }

            // E5: Declaring state is not terminal.
            if state.terminal {
                return Err(format!(
                    "E5: state {:?}: materialize_children cannot be declared on a terminal state\n  \
                     remedy: move the hook to a non-terminal state that awaits evidence submission",
                    state_name
                ));
            }

            // E7: State has at least one outgoing transition.
            if state.transitions.is_empty() {
                return Err(format!(
                    "E7: state {:?}: materialize_children state must declare at least one outgoing transition\n  \
                     remedy: add a transition that fires when the children-complete gate completes",
                    state_name
                ));
            }

            // E2/E3/E4: from_field must be declared in accepts with
            // `type: tasks` and `required: true`.
            let accepts = state.accepts.as_ref().ok_or_else(|| {
                format!(
                    "E2: state {:?}: materialize_children.from_field {:?} is not declared in an accepts block\n  \
                     remedy: add an accepts block with field {:?} of type `tasks`",
                    state_name, hook.from_field, hook.from_field
                )
            })?;

            let schema = accepts.get(&hook.from_field).ok_or_else(|| {
                format!(
                    "E2: state {:?}: materialize_children.from_field {:?} is not a declared accepts field\n  \
                     remedy: declare {:?} in the accepts block with type `tasks`",
                    state_name, hook.from_field, hook.from_field
                )
            })?;

            // E3: Referenced field has type: tasks.
            if schema.field_type != FIELD_TYPE_TASKS {
                return Err(format!(
                    "E3: state {:?}: accepts field {:?} has type {:?}, expected \"tasks\"\n  \
                     remedy: change the field's type to \"tasks\" (structured task list)",
                    state_name, hook.from_field, schema.field_type
                ));
            }

            // E4: Referenced field has required: true.
            if !schema.required {
                return Err(format!(
                    "E4: state {:?}: accepts field {:?} must be required: true\n  \
                     remedy: set `required: true` on the accepts field backing the task list",
                    state_name, hook.from_field
                ));
            }

            // E6: failure_policy is skip_dependents or continue. The
            // `FailurePolicy` enum with `#[serde(rename_all = "snake_case")]`
            // already rejects unknown values at deserialization; matching
            // exhaustively here documents the contract for future variants.
            match hook.failure_policy {
                FailurePolicy::SkipDependents | FailurePolicy::Continue => {}
            }

            // E8: No two states share the same from_field.
            if let Some(prior) = seen_from_fields.get(&hook.from_field) {
                return Err(format!(
                    "E8: states {:?} and {:?} both reference from_field {:?}; \
                     a task-list field can back at most one materialize_children hook\n  \
                     remedy: use distinct accepts field names per hook (likely a copy-paste bug)",
                    prior, state_name, hook.from_field
                ));
            }
            seen_from_fields.insert(hook.from_field.clone(), state_name.clone());

            // E9 (partial): default_template is non-empty. The "resolves
            // to a compilable template" check lives in `compile()` because
            // it needs the source path to resolve relative references.
            if hook.default_template.is_empty() {
                return Err(format!(
                    "E9: state {:?}: materialize_children.default_template must not be empty\n  \
                     remedy: set default_template to a path (relative to the parent template's directory) of the child template",
                    state_name
                ));
            }

            // E10: State must also declare a children-complete gate.
            let has_children_complete_gate = state
                .gates
                .values()
                .any(|g| g.gate_type == GATE_TYPE_CHILDREN_COMPLETE);
            if !has_children_complete_gate {
                return Err(format!(
                    "E10: state {:?}: materialize_children requires a children-complete gate on the same state\n  \
                     remedy: add a gate with type `children-complete` so the state can observe child completion",
                    state_name
                ));
            }
        }

        Ok(())
    }

    /// Collect non-fatal warnings W1-W5 tied to `materialize_children`,
    /// `failure`, and `skipped_marker` usage.
    ///
    /// | Rule | Trigger |
    /// |------|---------|
    /// | W1 | State with `materialize_children` routes to a state reachable from the declaring state that does not observe the `children-complete` gate |
    /// | W2 | `children-complete.name_filter` is set but does not end with `.` (ergo not scoped to one parent) |
    /// | W3 | Terminal state whose name matches /block|fail|error/ lacks `failure: true` |
    /// | W4 | State with `materialize_children` routes only on `all_complete: true` without a second transition handling failures |
    /// | W5 | Terminal state with `failure: true` has no path writing `failure_reason` to context (v1 only checks the accepts-field path; templates relying on `default_action` or `context_assignments` may see false positives until those surfaces are checked) |
    ///
    /// Warnings are returned as formatted strings; callers emit them via
    /// stderr (`validate`) or collect them for tests.
    pub fn collect_materialize_children_warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();

        // W1: children-complete gate reachable from the declaring state.
        //
        // In the single-state fan-out shape (E10 enforces the gate lives on
        // the declaring state itself), the gate is trivially reachable from
        // the declaring state. W1 still catches the degenerate case where
        // the hook is declared but no gate on this state evaluates
        // `children-complete` via a `when` clause — meaning the gate output
        // is never observed by any transition.
        for (state_name, state) in &self.states {
            if state.materialize_children.is_none() {
                continue;
            }
            let gate_names: Vec<&String> = state
                .gates
                .iter()
                .filter_map(|(n, g)| (g.gate_type == GATE_TYPE_CHILDREN_COMPLETE).then_some(n))
                .collect();
            if gate_names.is_empty() {
                // E10 already fired; no W1 noise.
                continue;
            }
            let any_referenced = gate_names.iter().any(|gn| {
                let prefix = format!("{}.{}.", GATES_EVIDENCE_NAMESPACE, gn);
                state.transitions.iter().any(|t| {
                    t.when
                        .as_ref()
                        .is_some_and(|w| w.keys().any(|k| k.starts_with(&prefix)))
                })
            });
            if !any_referenced {
                warnings.push(format!(
                    "W1: state {:?}: children-complete gate is declared but no transition's when clause observes it\n  \
                     remedy: add a transition with a when clause referencing gates.<gate_name>.all_complete (or any children-complete output field)",
                    state_name
                ));
            }
        }

        // W2: children-complete.name_filter should end with `.` to scope
        // matching to children of a single parent (children are named
        // `<parent>.<task>`). We cannot know the parent name at compile
        // time, but a filter that lacks the trailing `.` would match by
        // prefix across parents and is almost certainly a bug.
        for (state_name, state) in &self.states {
            for (gate_name, gate) in &state.gates {
                if gate.gate_type != GATE_TYPE_CHILDREN_COMPLETE {
                    continue;
                }
                if let Some(filter) = &gate.name_filter {
                    if !filter.is_empty() && !filter.ends_with('.') {
                        warnings.push(format!(
                            "W2: state {:?} gate {:?}: name_filter {:?} does not end with \".\"; \
                             children-complete gates scope to one parent by matching the \"<parent>.\" prefix\n  \
                             remedy: append \".\" to the filter (e.g. {:?})",
                            state_name,
                            gate_name,
                            filter,
                            format!("{}.", filter)
                        ));
                    }
                }
            }
        }

        // W3: terminal state whose name contains "block"/"fail"/"error"
        // lacks `failure: true`.
        for (state_name, state) in &self.states {
            if !state.terminal {
                continue;
            }
            if state.failure {
                continue;
            }
            let lower = state_name.to_lowercase();
            let looks_failureish =
                lower.contains("block") || lower.contains("fail") || lower.contains("error");
            if looks_failureish {
                warnings.push(format!(
                    "W3: state {:?}: terminal state name suggests a failure outcome but `failure: true` is not set\n  \
                     remedy: set `failure: true` if this state represents a failure, or rename it if it represents a success",
                    state_name
                ));
            }
        }

        // W4: materialize_children state routes only on `all_complete: true`
        // without a second transition handling `any_failed > 0` or
        // `any_skipped > 0`. Failed or skipped children would silently
        // take the success branch.
        //
        // We look for transitions whose `when` clause references any of
        // the derived booleans `any_failed`, `any_skipped`,
        // `any_spawn_failed`, or `needs_attention` against any gate.
        for (state_name, state) in &self.states {
            if state.materialize_children.is_none() {
                continue;
            }
            let mut routes_on_all_complete = false;
            let mut has_failure_branch = false;
            for transition in &state.transitions {
                let when = match &transition.when {
                    Some(w) => w,
                    None => continue,
                };
                for key in when.keys() {
                    if !key.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)) {
                        continue;
                    }
                    // Key shape: gates.<gate>.<field>
                    let segments: Vec<&str> = key.splitn(3, '.').collect();
                    if segments.len() != 3 {
                        continue;
                    }
                    let field = segments[2];
                    match field {
                        "all_complete" | "all_success" => routes_on_all_complete = true,
                        "any_failed" | "any_skipped" | "any_spawn_failed" | "needs_attention" => {
                            has_failure_branch = true
                        }
                        _ => {}
                    }
                }
            }
            if routes_on_all_complete && !has_failure_branch {
                warnings.push(format!(
                    "W4: state {:?}: materialize_children routes only on all_complete/all_success \
                     with no branch handling failed or skipped children\n  \
                     remedy: add a transition guarded by gates.<gate>.any_failed or gates.<gate>.needs_attention to route failures somewhere meaningful",
                    state_name
                ));
            }
        }

        // W5: terminal state with `failure: true` has no path writing
        // `failure_reason` to context. The design calls out three ways the
        // context key can land:
        //   (a) the state's `accepts` block declares a `failure_reason` field
        //   (b) the state's `default_action` writes `failure_reason`
        //   (c) an upstream transition carries a `context_assignments`
        //       entry writing `failure_reason`
        //
        // Neither `default_action` nor `context_assignments` carry schema
        // metadata today (the runtime context-assignment surface lands
        // in a later phase), so for now we check (a) only. A future PR
        // extends this check once (b)/(c) have stable representations;
        // until then W5 over-warns on templates that rely on (b)/(c).
        //
        // TODO(issue-8/W5): widen the check once default_action and
        // context_assignments expose a writable-keys surface.
        for (state_name, state) in &self.states {
            if !(state.terminal && state.failure) {
                continue;
            }
            let has_failure_reason_accepts = state
                .accepts
                .as_ref()
                .is_some_and(|a| a.contains_key("failure_reason"));
            if !has_failure_reason_accepts {
                warnings.push(format!(
                    "W5: state {:?}: `failure: true` terminal state has no declared path writing the `failure_reason` context key; \
                     the batch view's per-child `reason` will fall back to the state name\n  \
                     remedy: add `failure_reason` to the state's accepts block (or write it via default_action / context_assignments)",
                    state_name
                ));
            }
        }

        warnings
    }

    /// Collect non-fatal warnings tied to when-clause matcher usage.
    ///
    /// | Rule | Trigger |
    /// |------|---------|
    /// | W6 | The string `"present"` appears as a when-clause value under a key that is not in the `evidence.<field>` namespace. The present matcher is only meaningful when checking whether an agent-submitted evidence field exists. A flat agent-evidence key, a `gates.*` path, or any other prefix with value `"present"` almost certainly means the template author intended presence matching but used the wrong path. |
    ///
    /// Warnings are returned as formatted strings; callers emit them via
    /// stderr (`validate`) or collect them for tests.
    pub fn collect_when_clause_warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();
        let evidence_prefix = format!("{}.", EVIDENCE_NAMESPACE);

        for (state_name, state) in &self.states {
            for transition in &state.transitions {
                let when = match &transition.when {
                    Some(w) => w,
                    None => continue,
                };
                for (field, value) in when {
                    if !is_present_matcher(value) {
                        continue;
                    }
                    if field.starts_with(&evidence_prefix) {
                        continue;
                    }
                    warnings.push(format!(
                        "W6: state {:?} transition to {:?}: when value {:?} is the presence-matcher \
                         sentinel but key {:?} is not in the evidence.<field> namespace\n  \
                         remedy: rewrite the key as evidence.<field> to check for field presence, or replace the value with the intended scalar for equality matching",
                        state_name,
                        transition.target,
                        PRESENT_MATCHER_VALUE,
                        field,
                    ));
                }
            }
        }

        warnings
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
    ///
    /// When `strict` is `false`, returns `Ok(())` immediately — the per-field
    /// warning loop is aimed at template authors and should not fire for agents.
    fn validate_gate_reachability(
        &self,
        state_name: &str,
        state: &TemplateState,
        strict: bool,
    ) -> Result<(), String> {
        // D4 suppression in permissive mode: suppress the per-field eprintln!
        // warning loop that is only useful for template authors.
        if !strict {
            return Ok(());
        }

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

            // Separate gates.* keys (engine-injected gate output), evidence.<field>
            // presence keys (Issue #11), vars.* keys (Issue #141), and flat agent
            // evidence keys. gates.* and vars.* keys bypass the accepts block
            // requirement because they are not agent-submitted evidence.
            // evidence.<field>: present checks only that the field appeared in any
            // submission since the last transition — it does not compare values,
            // so the field is not required to be declared in accepts.
            let gates_prefix = format!("{}.", GATES_EVIDENCE_NAMESPACE);
            let evidence_prefix = format!("{}.", EVIDENCE_NAMESPACE);
            let vars_prefix = format!("{}.", VARS_NAMESPACE);
            let gate_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| k.starts_with(&gates_prefix))
                .collect();
            let evidence_presence_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| k.starts_with(&evidence_prefix))
                .collect();
            let vars_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| k.starts_with(&vars_prefix))
                .collect();
            let agent_fields: Vec<(&String, &serde_json::Value)> = when
                .iter()
                .filter(|(k, _)| {
                    !k.starts_with(&gates_prefix)
                        && !k.starts_with(&evidence_prefix)
                        && !k.starts_with(&vars_prefix)
                })
                .collect();

            // Rule 5: when conditions that reference agent evidence require an accepts block.
            // Pure gates.*, evidence.<field>: present, and vars.* conditions are allowed
            // without an accepts block.
            if !agent_fields.is_empty() && !has_accepts {
                return Err(format!(
                    "state {:?} transition to {:?}: when conditions require an accepts block on the state",
                    state_name, transition.target
                ));
            }

            // Issue #11: validate evidence.<field> entries use the present matcher.
            // Any other value on an evidence.* key is an error — the namespace is
            // reserved for presence matching and has no equality semantics.
            for (field, value) in &evidence_presence_fields {
                let segments: Vec<&str> = field.splitn(3, '.').collect();
                if segments.len() != 2 || segments[1].is_empty() {
                    return Err(format!(
                        "state {:?}: when clause key {:?} has invalid format; expected \"evidence.<field>\"",
                        state_name, field.as_str()
                    ));
                }
                if !is_present_matcher(value) {
                    return Err(format!(
                        "state {:?} transition to {:?}: when value for evidence key {:?} must be {:?}; \
                         the evidence.<field> namespace only supports presence matching",
                        state_name, transition.target, field, PRESENT_MATCHER_VALUE
                    ));
                }
            }

            // Issue #141: validate vars.<name> entries use the {is_set: bool} matcher.
            // The vars.* namespace only supports existence checking, not equality.
            for (field, value) in &vars_fields {
                let segments: Vec<&str> = field.splitn(3, '.').collect();
                if segments.len() != 2 || segments[1].is_empty() {
                    return Err(format!(
                        "state {:?}: when clause key {:?} has invalid format; expected \"vars.<VARIABLE_NAME>\"",
                        state_name, field.as_str()
                    ));
                }
                let var_name = segments[1];
                // The variable must be declared in the template's variables block.
                if !self.variables.contains_key(var_name) {
                    return Err(format!(
                        "state {:?} transition to {:?}: when clause references undeclared variable {:?}; \
                         add it to the template's variables block",
                        state_name, transition.target, var_name
                    ));
                }
                if is_is_set_matcher(value).is_none() {
                    return Err(format!(
                        "state {:?} transition to {:?}: when value for vars key {:?} must be \
                         {{\"is_set\": true}} or {{\"is_set\": false}}; \
                         the vars.* namespace only supports existence matching",
                        state_name, transition.target, field
                    ));
                }
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        minimal_template().validate(true).unwrap();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
        assert!(err.contains("unsupported gate type"), "got: {}", err);
        assert!(err.contains("accepts/when"), "got: {}", err);
    }

    #[test]
    fn command_gate_still_works() {
        // A command gate with gates.* routing is valid in strict mode.
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
                completion: None,
                name_filter: None,
            },
        );
        let mut when = BTreeMap::new();
        when.insert("gates.ci.exit_code".to_string(), serde_json::json!(0));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate(true).unwrap();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
            },
        );
        let mut when = BTreeMap::new();
        when.insert("gates.ci_check.exit_code".to_string(), serde_json::json!(0));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        assert!(
            t.validate(true).is_ok(),
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        );
        assert!(
            t.validate(true).is_ok(),
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        let err = t.validate(true).unwrap_err();
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        let err = t.validate(true).unwrap_err();
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        t.validate(true).unwrap();
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        t.validate(true).unwrap();
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
        t.validate(true).unwrap();
    }

    #[test]
    fn integration_field_validates() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.integration = Some("delegate_review".to_string());
        t.validate(true).unwrap();
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
        t.validate(true).unwrap();
    }

    #[test]
    fn context_exists_gate_validates() {
        // Tests that a context-exists gate with a valid key passes gate-type validation.
        // Uses permissive mode so the test focuses on D1 gate validation, not D5.
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
                completion: None,
                name_filter: None,
            },
        );
        t.validate(false).unwrap();
    }

    #[test]
    fn rejects_undeclared_variable_ref_in_directive() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.directive = "Do {{TASK}} now".to_string();
        // No variable declared for TASK
        let err = t.validate(true).unwrap_err();
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
        t.validate(true).unwrap();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("variable reference '{{MISSING}}'"),
            "got: {}",
            err
        );
    }

    #[test]
    fn accepts_declared_variable_ref_in_gate_command() {
        // Tests that a declared variable in a gate command passes variable validation.
        // Uses permissive mode so the test focuses on variable resolution, not D5.
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
                completion: None,
                name_filter: None,
            },
        );
        t.validate(false).unwrap();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("context-exists gate must have a non-empty key"),
            "got: {}",
            err
        );
    }

    #[test]
    fn context_matches_gate_validates() {
        // Tests that a context-matches gate with valid key and pattern passes gate-type
        // validation. Uses permissive mode so the test focuses on D1 gate validation, not D5.
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
                completion: None,
                name_filter: None,
            },
        );
        t.validate(false).unwrap();
    }

    #[test]
    fn lowercase_braces_not_treated_as_variable_refs() {
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        state.directive = "Use {{name}} style".to_string();
        // Lowercase is not a variable ref, should pass without declaring it
        t.validate(true).unwrap();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        t.validate(true).unwrap();
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
        t.validate(true).unwrap();
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
        let gate: Gate = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(gate.override_default.is_some());
        let v = gate.override_default.as_ref().unwrap();
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["error"], "");

        // Serialize back and confirm the value is preserved.
        let out = serde_yaml_ng::to_string(&gate).unwrap();
        assert!(
            out.contains("override_default"),
            "override_default should appear in output: {}",
            out
        );

        let gate2: Gate = serde_yaml_ng::from_str(&out).unwrap();
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
        let gate: Gate = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(gate.override_default.is_none());

        let out = serde_yaml_ng::to_string(&gate).unwrap();
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
            completion: None,
            name_filter: None,
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
            completion: None,
            name_filter: None,
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
            completion: None,
            name_filter: None,
        }
    }

    #[test]
    fn override_default_valid_command() {
        // D2 check: a well-formed override_default for a command gate passes.
        // Uses permissive mode so the test can focus on D2 without requiring
        // gates.* routing (D5 is the template-author check, not D2).
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::json!({"exit_code": 0, "error": ""}))),
        );
        t.validate(false).unwrap();
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
        t.validate(false).unwrap();
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
        t.validate(false).unwrap();
    }

    #[test]
    fn override_default_non_object_null_rejected() {
        let mut t = minimal_template();
        t.states.get_mut("start").unwrap().gates.insert(
            "ci_check".to_string(),
            make_command_gate(Some(serde_json::Value::Null)),
        );
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        // D2 check: a gate with no override_default passes D2 regardless of strict mode.
        // Uses permissive mode so the test can focus on D2.
        let mut t = minimal_template();
        t.states
            .get_mut("start")
            .unwrap()
            .gates
            .insert("ci_check".to_string(), make_command_gate(None));
        t.validate(false).unwrap();
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
            GATE_TYPE_CHILDREN_COMPLETE,
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
                completion: None,
                name_filter: None,
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        let err = t.validate(true).unwrap_err();
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
        t.validate(true).unwrap();
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
                completion: None,
                name_filter: None,
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
        t.validate(true).unwrap();
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
                completion: None,
                name_filter: None,
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
        t.validate(true).unwrap();
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
        t.validate(true).unwrap();
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
        t.validate(true).unwrap();
    }

    // AC4/AC13: reachable state with no override_default uses builtin default.
    #[test]
    fn reachability_reachable_state_uses_builtin_default() {
        // No override_default; builtin default for command is {exit_code: 0, error: ""}.
        // pass_exit_code=0 matches → at least one transition fires → no error.
        let t = template_with_command_gate_transitions("ci_check", None, 0);
        t.validate(true).unwrap();
    }

    // AC3/AC12: dead-end state with no firing transition is rejected.
    #[test]
    fn reachability_dead_end_state_rejected() {
        // No override_default; builtin default exit_code=0 does NOT match pass_exit_code=99.
        // fail transition needs exit_code==1, also doesn't match exit_code=0.
        // So no transition fires → dead-end error.
        let t = template_with_command_gate_transitions("ci_check", None, 99);
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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
                completion: None,
                name_filter: None,
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
        t.validate(true).unwrap();
    }

    // AC6/AC15: state with no gates.* when-clause references compiles cleanly.
    #[test]
    fn reachability_no_gates_when_references_unaffected() {
        // This is just the minimal_template() with no gate references.
        minimal_template().validate(true).unwrap();
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        );
        let err = t.validate(true).unwrap_err();
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

    // AC11: unreferenced gate field does not cause validate() to return an error.
    // The warning content (state, gate, field names) is verified by the functional test
    // `gate_contract_unreferenced_field_warning` in tests/integration_test.rs (AC16).
    #[test]
    fn reachability_unreferenced_field_emits_warning() {
        // Gate has both "exit_code" and "error" fields. Only "exit_code" is referenced.
        // validate() must succeed (warning is non-fatal).
        let t = template_with_command_gate_transitions(
            "ci_check",
            Some(serde_json::json!({"exit_code": 0, "error": ""})),
            0,
        );
        t.validate(true).unwrap();
    }

    // -----------------------------------------------------------------------
    // D5: legacy gate detection — strict mode errors, permissive mode warns
    // -----------------------------------------------------------------------

    /// Build a minimal template with a gate but no gates.* when-clause references
    /// (legacy gate behavior).
    fn template_with_legacy_gate() -> CompiledTemplate {
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
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );
        t
    }

    // scenario-1: validate(strict=true) returns Err for a template with a gate
    // and no gates.* when references.
    #[test]
    fn d5_strict_mode_errors_on_legacy_gate() {
        let t = template_with_legacy_gate();
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("gate \"ci_check\" has no gates.* routing"),
            "expected D5 error, got: {}",
            err
        );
        assert!(
            err.contains("--allow-legacy-gates"),
            "error should hint at --allow-legacy-gates, got: {}",
            err
        );
    }

    // scenario-2: validate(strict=false) returns Ok for the same template
    // (warning goes to stderr; not testable here, but the Ok return is verified).
    #[test]
    fn d5_permissive_mode_returns_ok_on_legacy_gate() {
        let t = template_with_legacy_gate();
        assert!(
            t.validate(false).is_ok(),
            "permissive mode should return Ok for legacy-gate template"
        );
    }

    // scenario-3: a template with no gates compiles successfully in both modes.
    #[test]
    fn d5_no_gate_template_passes_both_modes() {
        let t = minimal_template();
        assert!(
            t.validate(true).is_ok(),
            "strict mode: no-gate template should pass"
        );
        assert!(
            t.validate(false).is_ok(),
            "permissive mode: no-gate template should pass"
        );
    }

    // scenario-4: validate_gate_reachability returns Ok early in permissive mode
    // without emitting per-field warnings. Template has a dead-end gate state
    // (no transition fires) that would error in strict mode but not in permissive mode.
    #[test]
    fn d4_suppressed_in_permissive_mode() {
        // Dead-end gate template: no transition fires with builtin defaults.
        // In strict mode this is a D4 error; in permissive mode D4 is suppressed.
        let t = template_with_command_gate_transitions(
            "ci_check", None,
            99, // builtin default exit_code=0 does not match 99, no transition fires
        );
        // strict=true: D4 fires (dead-end state).
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("no transition fires"),
            "expected D4 error, got: {}",
            err
        );
        // strict=false: D4 suppressed (early return in validate_gate_reachability).
        assert!(
            t.validate(false).is_ok(),
            "permissive mode should suppress D4 dead-end error"
        );
    }

    // scenario-5: compile() propagates strict through to validate().
    // Tested indirectly: compile.rs tests call compile(path, true/false) and
    // the D5 check fires (or doesn't) based on the strict argument.
    // This unit test confirms via validate() directly that strict is wired through.
    #[test]
    fn d5_strict_parameter_controls_error_vs_warning() {
        let t = template_with_legacy_gate();
        // Strict: error.
        assert!(t.validate(true).is_err(), "strict=true must return Err");
        // Permissive: no error.
        assert!(t.validate(false).is_ok(), "strict=false must return Ok");
    }

    // -----------------------------------------------------------------------
    // Issue 7: tasks accepts type, materialize_children hook, new TemplateState
    // fields, and narrow deny_unknown_fields.
    // -----------------------------------------------------------------------

    #[test]
    fn template_state_defaults_for_new_fields() {
        // Default construction leaves the three Issue-7 fields at their zero
        // values: None, false, false.
        let state = TemplateState::default();
        assert!(state.materialize_children.is_none());
        assert!(!state.failure);
        assert!(!state.skipped_marker);
    }

    #[test]
    fn failure_policy_serializes_as_snake_case() {
        // skip_dependents / continue, matching the design's Key Interfaces.
        let v = serde_json::to_value(FailurePolicy::SkipDependents).unwrap();
        assert_eq!(v, serde_json::json!("skip_dependents"));
        let v = serde_json::to_value(FailurePolicy::Continue).unwrap();
        assert_eq!(v, serde_json::json!("continue"));

        // Round-trip via JSON.
        let decoded: FailurePolicy = serde_json::from_str("\"skip_dependents\"").unwrap();
        assert_eq!(decoded, FailurePolicy::SkipDependents);
        let decoded: FailurePolicy = serde_json::from_str("\"continue\"").unwrap();
        assert_eq!(decoded, FailurePolicy::Continue);
    }

    #[test]
    fn default_failure_policy_is_skip_dependents() {
        assert_eq!(default_failure_policy(), FailurePolicy::SkipDependents);
    }

    #[test]
    fn materialize_children_spec_deserializes_with_default_policy() {
        // When failure_policy is omitted, the serde default kicks in.
        let json = serde_json::json!({
            "from_field": "tasks",
            "default_template": "child.md"
        });
        let spec: MaterializeChildrenSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.from_field, "tasks");
        assert_eq!(spec.default_template, "child.md");
        assert_eq!(spec.failure_policy, FailurePolicy::SkipDependents);
    }

    #[test]
    fn materialize_children_spec_rejects_unknown_fields() {
        // deny_unknown_fields is applied to MaterializeChildrenSpec — a typo
        // like `default_tempalte` must be rejected at deserialization.
        let json = serde_json::json!({
            "from_field": "tasks",
            "default_template": "child.md",
            "unknown_field": 42,
        });
        let err = serde_json::from_value::<MaterializeChildrenSpec>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown-field error, got: {}",
            err
        );
    }

    #[test]
    fn tasks_field_type_passes_validation() {
        // A tasks-typed accepts field compiles and validates successfully.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "tasks".to_string(),
            FieldSchema {
                field_type: "tasks".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        t.validate(true).unwrap();
    }

    #[test]
    fn invalid_field_type_error_lists_tasks() {
        // The error message enumerating valid field types must include
        // "tasks" so template authors see it as an option.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "bad".to_string(),
            FieldSchema {
                field_type: "nonsense".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
            },
        );
        state.accepts = Some(accepts);
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("tasks"),
            "expected tasks in error, got: {}",
            err
        );
    }

    #[test]
    fn template_state_deserializes_without_new_fields_for_cache_compat() {
        // CompiledTemplate is loaded from the compile cache. Older binaries
        // wrote state files that do not include materialize_children,
        // failure, or skipped_marker — those must parse unchanged thanks to
        // serde(default) on each field.
        let json = serde_json::json!({
            "directive": "do a thing",
        });
        let state: TemplateState = serde_json::from_value(json).unwrap();
        assert_eq!(state.directive, "do a thing");
        assert!(state.materialize_children.is_none());
        assert!(!state.failure);
        assert!(!state.skipped_marker);
    }

    #[test]
    fn template_state_tolerates_unknown_fields_for_forward_compat() {
        // deny_unknown_fields is NOT on TemplateState: a compile cache
        // written by a newer binary that added a new field must still parse
        // cleanly with the current binary. This is the mirror test of
        // SourceState's stricter behavior.
        let json = serde_json::json!({
            "directive": "do a thing",
            "some_future_field": "surprise",
        });
        let state: TemplateState = serde_json::from_value(json).unwrap();
        assert_eq!(state.directive, "do a thing");
    }

    #[test]
    fn template_state_round_trips_with_new_fields_set() {
        // failure: true and skipped_marker: true round-trip through JSON.
        let state = TemplateState {
            directive: "done".to_string(),
            terminal: true,
            failure: true,
            skipped_marker: true,
            skip_if: None,
            ..TemplateState::default()
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["failure"], serde_json::json!(true));
        assert_eq!(json["skipped_marker"], serde_json::json!(true));
        let restored: TemplateState = serde_json::from_value(json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn materialize_children_spec_serializes_skip_dependents_by_default() {
        let spec = MaterializeChildrenSpec {
            from_field: "tasks".to_string(),
            default_template: "child.md".to_string(),
            failure_policy: FailurePolicy::SkipDependents,
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["from_field"], "tasks");
        assert_eq!(json["default_template"], "child.md");
        assert_eq!(json["failure_policy"], "skip_dependents");
    }

    // ---------------------------------------------------------------------
    // Issue 8: compile rules E1-E10 (errors) and W1-W5 (warnings).
    //
    // These tests exercise validate() directly on a handwritten
    // CompiledTemplate so each rule can fire in isolation without relying
    // on the YAML parser. E9's "resolves to a compilable template" check
    // and F5 (skipped_marker reachability) live in `compile::tests`
    // because they need the source path.
    // ---------------------------------------------------------------------

    /// Build a minimum batch-parent template: one non-terminal declaring
    /// state with `materialize_children` and a children-complete gate,
    /// plus a terminal `done` state. Callers mutate the result to trip
    /// individual rules.
    ///
    /// Intentionally omits a failure-branch transition so W4-negative
    /// coverage has something to fire on. Add a second transition on
    /// `gates.cc.any_failed` for a W4-silent fixture.
    fn minimal_batch_parent() -> CompiledTemplate {
        let mut accepts: BTreeMap<String, FieldSchema> = BTreeMap::new();
        accepts.insert(
            "tasks".to_string(),
            FieldSchema {
                field_type: FIELD_TYPE_TASKS.to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        let mut gates: BTreeMap<String, Gate> = BTreeMap::new();
        gates.insert(
            "cc".to_string(),
            Gate {
                gate_type: GATE_TYPE_CHILDREN_COMPLETE.to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );
        let mut when: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        when.insert("gates.cc.all_complete".to_string(), serde_json::json!(true));
        let plan = TemplateState {
            directive: "Plan.".to_string(),
            details: String::new(),
            transitions: vec![Transition {
                target: "done".to_string(),
                when: Some(when),
            }],
            terminal: false,
            gates,
            accepts: Some(accepts),
            integration: None,
            default_action: None,
            materialize_children: Some(MaterializeChildrenSpec {
                from_field: "tasks".to_string(),
                default_template: "child.md".to_string(),
                failure_policy: FailurePolicy::SkipDependents,
            }),
            failure: false,
            skipped_marker: false,
            skip_if: None,
        };
        let done = TemplateState {
            directive: "Done.".to_string(),
            details: String::new(),
            transitions: vec![],
            terminal: true,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: false,
            skipped_marker: false,
            skip_if: None,
        };
        let mut states = BTreeMap::new();
        states.insert("plan".to_string(), plan);
        states.insert("done".to_string(), done);
        CompiledTemplate {
            format_version: 1,
            name: "batch-parent".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "plan".to_string(),
            variables: BTreeMap::new(),
            states,
        }
    }

    /// Positive baseline: a valid batch parent passes validate() cleanly.
    #[test]
    fn issue8_minimal_batch_parent_passes() {
        let t = minimal_batch_parent();
        t.validate(true)
            .expect("valid batch parent should validate");
    }

    #[test]
    fn issue8_e1_empty_from_field_rejected() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .materialize_children
            .as_mut()
            .unwrap()
            .from_field = String::new();
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E1:"), "got: {}", err);
        assert!(err.contains("plan"), "error mentions state name: {}", err);
    }

    #[test]
    fn issue8_e2_unknown_from_field_rejected() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .materialize_children
            .as_mut()
            .unwrap()
            .from_field = "not_a_field".to_string();
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E2:"), "got: {}", err);
        assert!(
            err.contains("not_a_field"),
            "error mentions bad field: {}",
            err
        );
    }

    #[test]
    fn issue8_e3_wrong_field_type_rejected() {
        let mut t = minimal_batch_parent();
        let accepts = t.states.get_mut("plan").unwrap().accepts.as_mut().unwrap();
        accepts.get_mut("tasks").unwrap().field_type = "string".to_string();
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E3:"), "got: {}", err);
    }

    #[test]
    fn issue8_e4_not_required_field_rejected() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .accepts
            .as_mut()
            .unwrap()
            .get_mut("tasks")
            .unwrap()
            .required = false;
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E4:"), "got: {}", err);
    }

    #[test]
    fn issue8_e5_terminal_declaring_state_rejected() {
        let mut t = minimal_batch_parent();
        t.states.get_mut("plan").unwrap().terminal = true;
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E5:"), "got: {}", err);
    }

    #[test]
    fn issue8_e6_valid_policies_pass() {
        // Positive test: both enum values are accepted.
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .materialize_children
            .as_mut()
            .unwrap()
            .failure_policy = FailurePolicy::Continue;
        t.validate(true).expect("continue policy should validate");
    }

    #[test]
    fn issue8_e7_no_outgoing_transition_rejected() {
        let mut t = minimal_batch_parent();
        t.states.get_mut("plan").unwrap().transitions.clear();
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E7:"), "got: {}", err);
    }

    #[test]
    fn issue8_e8_duplicate_from_field_rejected() {
        let mut t = minimal_batch_parent();
        // Add a second state referencing the same from_field.
        let mut accepts2: BTreeMap<String, FieldSchema> = BTreeMap::new();
        accepts2.insert(
            "tasks".to_string(),
            FieldSchema {
                field_type: FIELD_TYPE_TASKS.to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );
        let mut gates2: BTreeMap<String, Gate> = BTreeMap::new();
        gates2.insert(
            "cc2".to_string(),
            Gate {
                gate_type: GATE_TYPE_CHILDREN_COMPLETE.to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );
        let mut when: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        when.insert(
            "gates.cc2.all_complete".to_string(),
            serde_json::json!(true),
        );
        let plan2 = TemplateState {
            directive: "Second plan.".to_string(),
            details: String::new(),
            transitions: vec![Transition {
                target: "done".to_string(),
                when: Some(when),
            }],
            terminal: false,
            gates: gates2,
            accepts: Some(accepts2),
            integration: None,
            default_action: None,
            materialize_children: Some(MaterializeChildrenSpec {
                from_field: "tasks".to_string(),
                default_template: "child.md".to_string(),
                failure_policy: FailurePolicy::SkipDependents,
            }),
            failure: false,
            skipped_marker: false,
            skip_if: None,
        };
        t.states.insert("plan2".to_string(), plan2);
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E8:"), "got: {}", err);
    }

    #[test]
    fn issue8_e9_empty_default_template_rejected() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .materialize_children
            .as_mut()
            .unwrap()
            .default_template = String::new();
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E9:"), "got: {}", err);
    }

    #[test]
    fn issue8_e10_missing_children_complete_gate_rejected() {
        let mut t = minimal_batch_parent();
        t.states.get_mut("plan").unwrap().gates.clear();
        // Without the gate, the transition's when clause references a
        // gate that isn't declared — that's a D3 error that fires before
        // E10. Rewrite the transition to drop the gate reference so E10
        // is the first error encountered.
        t.states.get_mut("plan").unwrap().transitions = vec![Transition {
            target: "done".to_string(),
            when: None,
        }];
        let err = t.validate(true).unwrap_err();
        assert!(err.starts_with("E10:"), "got: {}", err);
    }

    // ----- Warnings --------------------------------------------------

    #[test]
    fn issue8_w1_no_gate_observed_emits_warning() {
        // Drop the when clause so no transition observes the
        // children-complete gate. D5 (legacy-gate) would fire in strict
        // mode because of the untouched gate; W1 is an independent
        // Issue-8 warning that we surface via direct collection here to
        // exercise the W1 rule in isolation.
        let mut t = minimal_batch_parent();
        t.states.get_mut("plan").unwrap().transitions = vec![Transition {
            target: "done".to_string(),
            when: None,
        }];
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("W1:")),
            "expected W1 warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w1_not_fired_when_gate_observed() {
        // Positive: when the gate is observed via a when clause, W1 is silent.
        let t = minimal_batch_parent();
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            !warnings.iter().any(|w| w.starts_with("W1:")),
            "W1 should be quiet; got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w2_name_filter_missing_dot_emits_warning() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .gates
            .get_mut("cc")
            .unwrap()
            .name_filter = Some("myparent".to_string());
        t.validate(true).expect("W2 should not fail validation");
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("W2:")),
            "expected W2 warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w2_name_filter_with_dot_is_silent() {
        let mut t = minimal_batch_parent();
        t.states
            .get_mut("plan")
            .unwrap()
            .gates
            .get_mut("cc")
            .unwrap()
            .name_filter = Some("myparent.".to_string());
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            !warnings.iter().any(|w| w.starts_with("W2:")),
            "W2 should be quiet; got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w3_failureish_name_without_failure_flag_warns() {
        let mut t = minimal_batch_parent();
        // Add a terminal state named "blocked" without failure: true.
        let blocked = TemplateState {
            directive: "Blocked.".to_string(),
            details: String::new(),
            transitions: vec![],
            terminal: true,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: false,
            skipped_marker: false,
            skip_if: None,
        };
        t.states.insert("blocked".to_string(), blocked);
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("W3:") && w.contains("blocked")),
            "expected W3 for \"blocked\", got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w3_failure_flag_silences_warning() {
        let mut t = minimal_batch_parent();
        let failed = TemplateState {
            directive: "Failed.".to_string(),
            details: String::new(),
            transitions: vec![],
            terminal: true,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: true,
            skipped_marker: false,
            skip_if: None,
        };
        t.states.insert("failed".to_string(), failed);
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            !warnings.iter().any(|w| w.starts_with("W3:")),
            "W3 should be quiet when failure: true is set; got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w4_only_all_complete_without_failure_branch_warns() {
        // The baseline minimal_batch_parent routes only on all_complete: true
        // with no failure branch. W4 should fire.
        let t = minimal_batch_parent();
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("W4:")),
            "expected W4 from all_complete-only routing, got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w4_with_failure_branch_silent() {
        // Positive-negative coverage for W4: when a materialize_children
        // state pairs its `gates.<gate>.all_complete` transition with a
        // second transition guarded by `gates.<gate>.any_failed`, W4 must
        // stay silent.
        //
        // We call `collect_materialize_children_warnings()` directly
        // rather than `validate()`, so D3's when-clause field validator
        // (which would reject `any_failed` because it isn't in the
        // children-complete gate schema today) never runs. The BTreeMap
        // is built by hand so we control the exact keys W4 inspects.
        let mut t = minimal_batch_parent();
        let mut failure_when: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        failure_when.insert("gates.cc.any_failed".to_string(), serde_json::json!(true));
        t.states
            .get_mut("plan")
            .unwrap()
            .transitions
            .push(Transition {
                target: "done".to_string(),
                when: Some(failure_when),
            });
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            !warnings.iter().any(|w| w.starts_with("W4:")),
            "W4 should be silent when a failure branch on gates.cc.any_failed is present; got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w5_failure_terminal_without_failure_reason_warns() {
        let mut t = minimal_batch_parent();
        // Add a terminal failure state with no failure_reason writer.
        let failed = TemplateState {
            directive: "Failed.".to_string(),
            details: String::new(),
            transitions: vec![],
            terminal: true,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: true,
            skipped_marker: false,
            skip_if: None,
        };
        t.states.insert("failed".to_string(), failed);
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("W5:")),
            "expected W5, got: {:?}",
            warnings
        );
    }

    #[test]
    fn issue8_w5_failure_reason_in_accepts_silences_warning() {
        let mut t = minimal_batch_parent();
        let mut failure_accepts: BTreeMap<String, FieldSchema> = BTreeMap::new();
        failure_accepts.insert(
            "failure_reason".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
            },
        );
        let failed = TemplateState {
            directive: "Failed.".to_string(),
            details: String::new(),
            transitions: vec![],
            terminal: true,
            gates: BTreeMap::new(),
            accepts: Some(failure_accepts),
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: true,
            skipped_marker: false,
            skip_if: None,
        };
        t.states.insert("failed".to_string(), failed);
        let warnings = t.collect_materialize_children_warnings();
        assert!(
            !warnings.iter().any(|w| w.starts_with("W5:")),
            "W5 should be quiet when failure_reason is in accepts; got: {:?}",
            warnings
        );
    }

    // ---------------------------------------------------------------------
    // Issue #11: when-clause evidence.<field>: present matcher (compile)
    // ---------------------------------------------------------------------

    #[test]
    fn is_present_matcher_case_sensitive() {
        assert!(is_present_matcher(&serde_json::json!("present")));
        assert!(!is_present_matcher(&serde_json::json!("Present")));
        assert!(!is_present_matcher(&serde_json::json!("PRESENT")));
        assert!(!is_present_matcher(&serde_json::json!("presence")));
        // Non-string values never match the sentinel.
        assert!(!is_present_matcher(&serde_json::json!(true)));
        assert!(!is_present_matcher(&serde_json::json!(0)));
        assert!(!is_present_matcher(&serde_json::json!(null)));
    }

    #[test]
    fn evidence_present_when_clause_accepted() {
        // A transition using `evidence.retry_failed: present` should validate
        // without an accepts block — presence matching does not depend on the
        // declared schema.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "evidence.retry_failed".to_string(),
            serde_json::json!("present"),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        assert!(
            t.validate(true).is_ok(),
            "evidence.<field>: present should validate without an accepts block"
        );
    }

    #[test]
    fn evidence_non_present_value_is_rejected() {
        // `evidence.<field>` is reserved for presence matching; any other value
        // is a hard error.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "evidence.retry_failed".to_string(),
            serde_json::json!("done"),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("only supports presence matching"),
            "got: {}",
            err
        );
    }

    #[test]
    fn evidence_empty_field_name_is_rejected() {
        // `evidence.` with nothing after the dot is not a valid path.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("evidence.".to_string(), serde_json::json!("present"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("invalid format") && err.contains("evidence.<field>"),
            "got: {}",
            err
        );
    }

    #[test]
    fn w6_warns_on_present_outside_evidence_namespace() {
        // `context.foo: present` is not a legal placement of the present matcher.
        // Compile should surface W6 without failing.
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
        // Use a flat agent-evidence key that is declared in accepts so the
        // transition passes D* rules; the only non-standard thing is the
        // `"present"` value on a non-evidence.* path.
        when.insert("decision".to_string(), serde_json::json!("present"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];

        // Validation succeeds (W6 is non-fatal).
        t.validate(true).unwrap();

        // But the collected warnings must include W6.
        let warnings = t.collect_when_clause_warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("W6:")),
            "expected W6 warning when \"present\" appears outside evidence.<field>; got: {:?}",
            warnings
        );
    }

    #[test]
    fn w6_silent_for_evidence_present_and_value_equality() {
        // No W6 warning should fire when "present" is used correctly under
        // evidence.<field>, and none for ordinary value-equality matchers.
        // Build two separate templates so mutual-exclusivity rules don't
        // entangle the two cases.

        // Case 1: evidence.<field>: present used correctly.
        let mut t = minimal_template();
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "evidence.retry_failed".to_string(),
            serde_json::json!("present"),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate(true).unwrap();
        let warnings = t.collect_when_clause_warnings();
        assert!(
            warnings.is_empty(),
            "expected no W6 warnings for correct evidence.<field>: present usage; got: {:?}",
            warnings
        );

        // Case 2: value-equality matcher on a flat agent-evidence field.
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
        when.insert("decision".to_string(), serde_json::json!("approve"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        t.validate(true).unwrap();
        let warnings = t.collect_when_clause_warnings();
        assert!(
            warnings.is_empty(),
            "expected no W6 warnings for value-equality matchers; got: {:?}",
            warnings
        );
    }

    // ---------------------------------------------------------------------
    // Issue #141: is_set matcher and vars.* validation
    // ---------------------------------------------------------------------

    #[test]
    fn is_is_set_matcher_true() {
        assert_eq!(
            is_is_set_matcher(&serde_json::json!({"is_set": true})),
            Some(true)
        );
    }

    #[test]
    fn is_is_set_matcher_false() {
        assert_eq!(
            is_is_set_matcher(&serde_json::json!({"is_set": false})),
            Some(false)
        );
    }

    #[test]
    fn is_is_set_matcher_rejects_non_object() {
        assert_eq!(is_is_set_matcher(&serde_json::json!("is_set")), None);
        assert_eq!(is_is_set_matcher(&serde_json::json!(true)), None);
        assert_eq!(is_is_set_matcher(&serde_json::json!(42)), None);
        assert_eq!(is_is_set_matcher(&serde_json::json!(null)), None);
    }

    #[test]
    fn is_is_set_matcher_rejects_extra_keys() {
        assert_eq!(
            is_is_set_matcher(&serde_json::json!({"is_set": true, "extra": 1})),
            None
        );
    }

    #[test]
    fn is_is_set_matcher_rejects_non_bool_value() {
        assert_eq!(
            is_is_set_matcher(&serde_json::json!({"is_set": "yes"})),
            None
        );
    }

    /// Helper: build a template with a declared optional variable and transitions
    /// using vars.* when clauses.
    fn template_with_var(var_name: &str) -> CompiledTemplate {
        let mut t = minimal_template();
        t.variables.insert(
            var_name.to_string(),
            VariableDecl {
                description: String::new(),
                required: false,
                default: String::new(),
            },
        );
        t
    }

    #[test]
    fn vars_is_set_true_and_false_validate() {
        let mut t = template_with_var("OPT_VAR");
        // Add a middle state for the second branch.
        t.states.insert(
            "alt".to_string(),
            TemplateState {
                directive: "Alt path.".to_string(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                ..Default::default()
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut when_set = BTreeMap::new();
        when_set.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );
        let mut when_unset = BTreeMap::new();
        when_unset.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": false}),
        );
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_set),
            },
            Transition {
                target: "alt".to_string(),
                when: Some(when_unset),
            },
        ];
        assert!(
            t.validate(true).is_ok(),
            "vars.* with is_set true/false should validate"
        );
    }

    #[test]
    fn vars_equality_value_is_rejected() {
        let mut t = template_with_var("FOO");
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert("vars.FOO".to_string(), serde_json::json!("bar"));
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("only supports existence matching"),
            "got: {}",
            err
        );
    }

    #[test]
    fn vars_undeclared_variable_is_rejected() {
        let mut t = minimal_template(); // no variables declared
        let state = t.states.get_mut("start").unwrap();
        let mut when = BTreeMap::new();
        when.insert(
            "vars.UNKNOWN".to_string(),
            serde_json::json!({"is_set": true}),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        let err = t.validate(true).unwrap_err();
        assert!(
            err.contains("undeclared variable") && err.contains("UNKNOWN"),
            "got: {}",
            err
        );
    }

    #[test]
    fn vars_is_set_mutual_exclusivity_disjoint() {
        // {is_set: true} and {is_set: false} on the same field should NOT
        // trigger a mutual exclusivity error (they are disjoint).
        let mut t = template_with_var("OPT_VAR");
        t.states.insert(
            "alt".to_string(),
            TemplateState {
                directive: "Alt path.".to_string(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                ..Default::default()
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut when_set = BTreeMap::new();
        when_set.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );
        let mut when_unset = BTreeMap::new();
        when_unset.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": false}),
        );
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_set),
            },
            Transition {
                target: "alt".to_string(),
                when: Some(when_unset),
            },
        ];
        assert!(
            t.validate(true).is_ok(),
            "is_set true/false should be disjoint and not trigger mutual exclusivity error"
        );
    }

    #[test]
    fn vars_is_set_identical_flags_conflict() {
        // Two transitions with {is_set: true} on the same field should trigger
        // a mutual exclusivity error.
        let mut t = template_with_var("OPT_VAR");
        t.states.insert(
            "alt".to_string(),
            TemplateState {
                directive: "Alt path.".to_string(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                ..Default::default()
            },
        );
        let state = t.states.get_mut("start").unwrap();
        let mut when_a = BTreeMap::new();
        when_a.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );
        let mut when_b = BTreeMap::new();
        when_b.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );
        state.transitions = vec![
            Transition {
                target: "done".to_string(),
                when: Some(when_a),
            },
            Transition {
                target: "alt".to_string(),
                when: Some(when_b),
            },
        ];
        let err = t.validate(true).unwrap_err();
        assert!(err.contains("not mutually exclusive"), "got: {}", err);
    }

    #[test]
    fn vars_does_not_require_accepts_block() {
        // vars.* conditions should not require an accepts block (they are not
        // agent-submitted evidence).
        let mut t = template_with_var("OPT_VAR");
        let state = t.states.get_mut("start").unwrap();
        assert!(state.accepts.is_none(), "precondition: no accepts block");
        let mut when = BTreeMap::new();
        when.insert(
            "vars.OPT_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );
        state.transitions = vec![Transition {
            target: "done".to_string(),
            when: Some(when),
        }];
        assert!(
            t.validate(true).is_ok(),
            "vars.* should not require an accepts block"
        );
    }
}
