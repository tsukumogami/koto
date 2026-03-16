# Design: Compiler Changes for Template Format v2 Compilation

**Phase:** design  
**Role:** phase3-compiler-changes  
**Date:** 2026-03-15  
**Status:** Complete

## Summary

Template format v2 compiler replaces `SourceState.transitions: Vec<String>` with `Vec<SourceTransition>` (each with `target` and optional `when` condition map), adds `accepts` (evidence field schema) and `integration` (processing tool tag) fields, and removes field gate validation (field_not_empty, field_equals). Validation rules expand to check mutual exclusivity of single-field when conditions, validate when-field references against accepts schema, and verify enum values. The `compile()` function sets `format_version: 2` and the compiler transforms source YAML structures into the v2 compiled schema via new deserialization types and validation logic.

---

## Investigation Scope

This design answers the compiler-specific questions:
1. How does `SourceState` change structurally?
2. What new validation rules are required?
3. What validation rules are removed?
4. How does the mutual exclusivity algorithm work concretely in Rust?
5. What are the concrete compiler error messages?
6. How does `compile()` change to produce `format_version: 2`?

---

## Current State: v1 Compiler Structure

### v1 Source Types (`src/template/compile.rs`)

```rust
#[derive(Debug, Deserialize)]
struct SourceState {
    #[serde(default)]
    transitions: Vec<String>,  // Simple list of target state names
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    gates: HashMap<String, SourceGate>,
}

#[derive(Debug, Deserialize)]
struct SourceGate {
    #[serde(rename = "type")]
    gate_type: String,
    #[serde(default)]
    field: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    timeout: u32,
}
```

### v1 Compiled Types (`src/template/types.rs`)

```rust
pub struct TemplateState {
    pub directive: String,
    pub transitions: Vec<String>,
    pub terminal: bool,
    pub gates: BTreeMap<String, Gate>,
}

pub struct Gate {
    pub gate_type: String,
    pub field: String,
    pub value: String,
    pub command: String,
    pub timeout: u32,
}
```

### v1 Gate Types

Constants defined in `types.rs`:
- `GATE_TYPE_FIELD_NOT_EMPTY = "field_not_empty"` — checks if a field exists
- `GATE_TYPE_FIELD_EQUALS = "field_equals"` — checks if a field equals a value
- `GATE_TYPE_COMMAND = "command"` — executes a command and checks exit code

### v1 Validation Rules (compile.rs: `compile_gate()` and `types.rs: `validate()`)

For field gates:
- `field_not_empty`: requires `field` to be non-empty
- `field_equals`: requires `field` to be non-empty

For command gates:
- `command`: requires `command` field to be non-empty

No transition mutual exclusivity validation exists in v1 (transitions are just strings).

---

## v2 Changes: SourceState Structure

### New Source Types (v2 compile.rs)

```rust
/// Structured transition with optional when condition.
#[derive(Debug, Deserialize)]
struct SourceTransition {
    target: String,
    #[serde(default)]
    when: Option<HashMap<String, serde_json::Value>>,
}

/// Field schema in accepts block.
#[derive(Debug, Deserialize)]
struct SourceFieldSchema {
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    values: Option<Vec<String>>,  // For enum fields
    #[serde(default)]
    description: String,
}

/// Updated SourceState for v2.
#[derive(Debug, Deserialize, Default)]
struct SourceState {
    #[serde(default)]
    transitions: Vec<SourceTransition>,  // CHANGED: from Vec<String>
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    gates: HashMap<String, SourceGate>,
    #[serde(default)]
    accepts: Option<HashMap<String, SourceFieldSchema>>,  // NEW: evidence schema
    #[serde(default)]
    integration: Option<String>,  // NEW: processing tool tag
}
```

### New Compiled Types (v2 types.rs)

```rust
/// Compiled structured transition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}

/// Field schema for accepts block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Updated TemplateState for v2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    pub directive: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<BTreeMap<String, FieldSchema>>,  // NEW
    pub transitions: Vec<Transition>,  // CHANGED: from Vec<String>
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,  // NEW
}

/// CompiledTemplate format_version bumps to 2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledTemplate {
    pub format_version: u32,  // Will be 2 for v2 templates
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub initial_state: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, VariableDecl>,
    pub states: BTreeMap<String, TemplateState>,
}
```

---

## Validation Rules: Added, Changed, Removed

### Validation Rules Removed (v1 → not in v2)

These gate type validations are **removed entirely** because field gates are removed from the v2 format:

1. **field_not_empty gate validation**
   - v1: `compile_gate()` checks `gate.field` is non-empty for field_not_empty gates
   - v2: Field gates don't exist; validation removed

2. **field_equals gate validation**
   - v1: `compile_gate()` checks `gate.field` is non-empty for field_equals gates
   - v2: Field gates don't exist; validation removed

3. **Gate type constants** (optional cleanup)
   - v1: `GATE_TYPE_FIELD_NOT_EMPTY` and `GATE_TYPE_FIELD_EQUALS` are constants
   - v2: Can remain in code for compatibility, but compiler won't accept them in `compile_gate()`

### Validation Rules Changed

1. **Transition targets validation**
   - v1: Validates `state.transitions: Vec<String>` are all defined states
   - v2: Validates `state.transitions: Vec<Transition>` — each transition's `target` field is defined
   - **Same logic, different field access**: `target in &state.transitions[i].target`

### Validation Rules Added (v2 only)

1. **When condition field existence check**
   - **Rule**: Every field name in a `when` condition must exist in the state's `accepts` block
   - **Logic**: For each transition with `when: {field_name: value, ...}`, validate that `field_name` is a key in `state.accepts`
   - **Error**: "state 'X' transition to 'Y' references unknown field 'Z' in when condition"
   - **Location**: `compile()` during state compilation, or in `validate()`

2. **Enum value validation**
   - **Rule**: For enum-type fields, `when` condition values must be in the declared `values` list
   - **Logic**: If `accepts[field_name].type == "enum"` and `values` is Some, check that `transition.when[field_name]` is a string matching one of the values
   - **Error**: "state 'X' transition condition value 'V' not in enum values ['A', 'B', 'C'] for field 'F'"
   - **Location**: `validate()` method

3. **Empty when condition check**
   - **Rule**: A transition with a `when` block must have at least one field
   - **Logic**: If `transition.when.is_some()` and `transition.when.as_ref().unwrap().is_empty()`, error
   - **Error**: "state 'X' transition to 'Y' has empty when condition"
   - **Location**: `compile()` during transition parsing

4. **Mutual exclusivity (single-field case)**
   - **Rule**: Two transitions from the same state cannot have `when` conditions on the same field with the same value
   - **Logic**: Group single-field transitions by field name; within each group, check for duplicate values
   - **Error**: "state 'X': transitions 'Y' and 'Z' both match when field='value'"
   - **Location**: New method `validate_transition_mutual_exclusivity()` called from `validate()`

5. **Accepts block consistency**
   - **Rule**: If a state has transitions with `when` conditions, the state must have an `accepts` block
   - **Logic**: For each transition with `when.is_some()`, verify `state.accepts.is_some()`
   - **Error**: "state 'X' transition to 'Y' has when conditions but no accepts block"
   - **Location**: `validate()` method

---

## Concrete Mutual Exclusivity Algorithm (Rust)

### Pseudocode Algorithm

```
fn validate_transition_mutual_exclusivity(
    state_name: &str,
    state: &TemplateState,
) -> Result<(), String> {
    // 1. Extract all single-field transitions
    let single_field_transitions: Vec<_> = state
        .transitions
        .iter()
        .filter(|t| {
            t.when.as_ref().map_or(false, |w| w.len() == 1)
        })
        .collect();

    // 2. If 0 or 1 transition with when: no conflict possible
    if single_field_transitions.len() <= 1 {
        return Ok(());
    }

    // 3. Group by field name: field_name -> [(transition.target, value)]
    let mut field_conditions: HashMap<String, Vec<(&str, Option<String>)>> = HashMap::new();
    
    for transition in &single_field_transitions {
        if let Some(when_map) = &transition.when {
            // Single-field: exactly one entry
            for (field_name, condition_value) in when_map {
                let value_str = condition_value.as_str().map(|s| s.to_string());
                field_conditions
                    .entry(field_name.clone())
                    .or_insert_with(Vec::new)
                    .push((transition.target.as_str(), value_str));
            }
        }
    }

    // 4. Check for duplicate values within each field group
    for (field_name, transitions) in field_conditions {
        let mut seen_values: HashSet<String> = HashSet::new();
        
        for (target, value) in &transitions {
            if let Some(v) = value {
                if seen_values.contains(v) {
                    // Found duplicate: find all transitions with this value
                    let conflicting_targets: Vec<&str> = transitions
                        .iter()
                        .filter(|(_, val)| val.as_ref() == Some(v))
                        .map(|(t, _)| *t)
                        .collect();
                    
                    return Err(format!(
                        "state {:?}: transitions {:?} both match when {}={}",
                        state_name,
                        conflicting_targets,
                        field_name,
                        v
                    ));
                }
                seen_values.insert(v.clone());
            }
        }
    }

    Ok(())
}
```

### Rust Implementation in types.rs

```rust
impl CompiledTemplate {
    /// Validate the compiled template, including v2 mutual exclusivity.
    pub fn validate(&self) -> Result<(), String> {
        // Existing v1 checks: format_version, name, version, initial_state, states exist
        if self.format_version != 2 {
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

        // Validate all states
        for (state_name, state) in &self.states {
            if state.directive.is_empty() {
                return Err(format!("state {:?} has empty directive", state_name));
            }

            // NEW v2: Validate transitions (now structured)
            for transition in &state.transitions {
                if !self.states.contains_key(&transition.target) {
                    return Err(format!(
                        "state {:?} references undefined transition target {:?}",
                        state_name, transition.target
                    ));
                }

                // NEW v2: Validate when conditions against accepts schema
                if let Some(when_conditions) = &transition.when {
                    if when_conditions.is_empty() {
                        return Err(format!(
                            "state {:?} transition to {:?} has empty when condition",
                            state_name, transition.target
                        ));
                    }

                    if state.accepts.is_none() {
                        return Err(format!(
                            "state {:?} transition to {:?} has when conditions but no accepts block",
                            state_name, transition.target
                        ));
                    }

                    let accepts = state.accepts.as_ref().unwrap();
                    for (field_name, condition_value) in when_conditions {
                        // Check field exists in accepts
                        if !accepts.contains_key(field_name) {
                            return Err(format!(
                                "state {:?} transition to {:?} references unknown field {:?} in when condition",
                                state_name, transition.target, field_name
                            ));
                        }

                        // For enum fields, validate value is in allowed values
                        let field_schema = &accepts[field_name];
                        if field_schema.field_type == "enum" {
                            if let Some(allowed_values) = &field_schema.values {
                                if let Some(cond_str) = condition_value.as_str() {
                                    if !allowed_values.contains(&cond_str.to_string()) {
                                        return Err(format!(
                                            "state {:?} transition condition value {:?} not in enum values {:?} for field {:?}",
                                            state_name, cond_str, allowed_values, field_name
                                        ));
                                    }
                                } else {
                                    return Err(format!(
                                        "state {:?} transition condition for enum field {:?} must be a string",
                                        state_name, field_name
                                    ));
                                }
                            }
                        }
                    }
                }
            }

            // NEW v2: Validate mutual exclusivity of single-field when conditions
            self.validate_transition_mutual_exclusivity(state_name, state)?;

            // v1 gate validation (unchanged except field gates are removed)
            for (gate_name, gate) in &state.gates {
                match gate.gate_type.as_str() {
                    // field_not_empty and field_equals are NOT accepted in v2
                    GATE_TYPE_COMMAND => {
                        if gate.command.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: command must not be empty",
                                state_name, gate_name
                            ));
                        }
                    }
                    unknown => {
                        return Err(format!(
                            "state {:?} gate {:?}: unknown type {:?}",
                            state_name, gate_name, unknown
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate mutual exclusivity of single-field when conditions.
    fn validate_transition_mutual_exclusivity(
        &self,
        state_name: &str,
        state: &TemplateState,
    ) -> Result<(), String> {
        use std::collections::{HashMap, HashSet};

        // Filter to transitions with single-field when conditions
        let single_field_transitions: Vec<_> = state
            .transitions
            .iter()
            .filter(|t| t.when.as_ref().map_or(false, |w| w.len() == 1))
            .collect();

        // If 0 or 1 single-field transition: no conflict possible
        if single_field_transitions.len() <= 1 {
            return Ok(());
        }

        // Group by field name
        let mut field_conditions: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();

        for transition in &single_field_transitions {
            if let Some(when_map) = &transition.when {
                for (field_name, condition_value) in when_map {
                    let value_str = condition_value
                        .as_str()
                        .map(|s| s.to_string());
                    field_conditions
                        .entry(field_name.clone())
                        .or_insert_with(Vec::new)
                        .push((transition.target.clone(), value_str));
                }
            }
        }

        // Check for duplicate values within each field
        for (field_name, transitions) in field_conditions {
            let mut seen_values: HashSet<String> = HashSet::new();

            for (target, value) in &transitions {
                if let Some(v) = value {
                    if seen_values.contains(v) {
                        // Find all conflicting targets
                        let conflicting: Vec<_> = transitions
                            .iter()
                            .filter(|(_, val)| val.as_ref() == Some(v))
                            .map(|(t, _)| t.as_str())
                            .collect();

                        return Err(format!(
                            "state {:?}: transitions {:?} both match when {}={}",
                            state_name, conflicting, field_name, v
                        ));
                    }
                    seen_values.insert(v.clone());
                }
            }
        }

        Ok(())
    }
}
```

---

## Concrete Compiler Errors

### 1. Non-Deterministic Transition (Duplicate Values)

**Condition**: Two transitions from the same state test the same field with the same value.

**YAML**:
```yaml
states:
  analyze:
    accepts:
      decision: { type: enum, values: [proceed, escalate] }
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: review
        when:
          decision: proceed  # Same value as first transition
```

**Error Message**:
```
error: state "analyze": transitions ["deploy", "review"] both match when decision=proceed
  make these transitions mutually exclusive by using different values for the "decision" field
  or combine them into a single transition with fallback logic
```

**Error Location**: `validate_transition_mutual_exclusivity()` in types.rs during `validate()` call

**Exit Code**: Non-zero (template rejected)

### 2. Unknown Field in When Condition

**Condition**: A transition's `when` condition references a field not declared in `accepts`.

**YAML**:
```yaml
states:
  analyze:
    accepts:
      decision: { type: enum, values: [proceed, escalate] }
    transitions:
      - target: deploy
        when:
          unknown_field: value  # Not in accepts
```

**Error Message**:
```
error: state "analyze" transition to "deploy" references unknown field "unknown_field" in when condition
  field must be declared in the accepts block for this state
  available fields: decision
```

**Error Location**: `validate()` method in types.rs when checking `when` conditions

**Exit Code**: Non-zero (template rejected)

### 3. Enum Value Not in Values List

**Condition**: A `when` condition specifies a value not in the enum's `values` list.

**YAML**:
```yaml
states:
  analyze:
    accepts:
      decision: { type: enum, values: [proceed, escalate] }
    transitions:
      - target: deploy
        when:
          decision: invalid_option  # Not in [proceed, escalate]
```

**Error Message**:
```
error: state "analyze" transition condition value "invalid_option" not in enum values ["proceed", "escalate"] for field "decision"
  update the condition to use one of the allowed enum values
```

**Error Location**: `validate()` method when validating enum field conditions

**Exit Code**: Non-zero (template rejected)

### 4. When Condition on Non-Existent Accepts

**Condition**: A transition has a `when` condition but the state has no `accepts` block.

**YAML**:
```yaml
states:
  process:
    # No accepts block
    transitions:
      - target: next
        when:
          status: ready  # Where does 'status' come from?
```

**Error Message**:
```
error: state "process" transition to "next" has when conditions but no accepts block
  either add an accepts block declaring the fields used in when conditions
  or remove the when conditions from the transitions
```

**Error Location**: `validate()` method when checking `when` conditions

**Exit Code**: Non-zero (template rejected)

### 5. Empty When Condition

**Condition**: A transition has an empty `when` block.

**YAML**:
```yaml
states:
  process:
    accepts:
      status: { type: string }
    transitions:
      - target: next
        when: {}  # Empty!
```

**Error Message**:
```
error: state "process" transition to "next" has empty when condition
  add at least one field to the when condition (e.g., when: {status: ready})
  or remove the when block if this transition should not require evidence
```

**Error Location**: `validate()` method when validating `when` conditions

**Exit Code**: Non-zero (template rejected)

### 6. Field Gate Type Not Supported in v2

**Condition**: A state gate uses `field_not_empty` or `field_equals` (removed in v2).

**YAML**:
```yaml
states:
  process:
    gates:
      check_field:
        type: field_not_empty  # Not allowed in v2
        field: status
```

**Error Message**:
```
error: state "process" gate "check_field": unknown type "field_not_empty"
  field_not_empty and field_equals gates are not supported in format version 2
  use the accepts/when constructs instead to declare and route on agent-submitted evidence
  example: add an accepts block for the field and transitions with when conditions
```

**Error Location**: `validate()` method when checking gate types

**Exit Code**: Non-zero (template rejected)

### 7. When Field Value Type Mismatch

**Condition**: A `when` condition provides a non-string value for an enum field.

**YAML** (if deserialized from YAML allowing numbers):
```yaml
states:
  analyze:
    accepts:
      priority: { type: enum, values: [high, medium, low] }
    transitions:
      - target: urgent
        when:
          priority: 1  # Number instead of string
```

**Error Message**:
```
error: state "analyze" transition condition for enum field "priority" must be a string
  the enum value in when must match one of: [high, medium, low]
  update the condition to use a string value (e.g., when: {priority: high})
```

**Error Location**: `validate()` method when validating enum field type

**Exit Code**: Non-zero (template rejected)

---

## Compile Function Changes

### Current v1 compile() function signature and flow

```rust
pub fn compile(source_path: &Path) -> anyhow::Result<CompiledTemplate> {
    // 1. Read file
    let content = std::fs::read_to_string(source_path)?;
    
    // 2. Split frontmatter
    let (frontmatter_str, body) = split_frontmatter(&content)?;
    
    // 3. Deserialize YAML frontmatter to SourceFrontmatter
    let fm: SourceFrontmatter = serde_yml::from_str(frontmatter_str)?;
    
    // 4. Validate required frontmatter fields
    if fm.name.is_empty() { ... }
    // etc.
    
    // 5. Extract directives from markdown body
    let directives = extract_directives(&fm.states, body);
    
    // 6. Build compiled states
    let mut compiled_states: BTreeMap<String, TemplateState> = BTreeMap::new();
    for (state_name, source_state) in &fm.states {
        let directive = directives.get(state_name)?;
        
        // Compile gates
        let mut compiled_gates: BTreeMap<String, Gate> = BTreeMap::new();
        for (gate_name, source_gate) in &source_state.gates {
            let gate = compile_gate(state_name, gate_name, source_gate)?;
            compiled_gates.insert(gate_name.clone(), gate);
        }
        
        compiled_states.insert(
            state_name.clone(),
            TemplateState {
                directive,
                transitions: source_state.transitions.clone(),  // Just copy strings
                terminal: source_state.terminal,
                gates: compiled_gates,
            },
        );
    }
    
    // 7. Validate transition targets exist
    for (state_name, state) in &compiled_states {
        for target in &state.transitions {
            if !compiled_states.contains_key(target) { ... }
        }
    }
    
    // 8. Validate initial_state exists
    if !compiled_states.contains_key(&fm.initial_state) { ... }
    
    // 9. Build variables
    let variables: BTreeMap<String, VariableDecl> = fm.variables.into_iter().map(...).collect();
    
    // 10. Return CompiledTemplate with format_version: 1
    Ok(CompiledTemplate {
        format_version: 1,
        name: fm.name,
        version: fm.version,
        description: fm.description,
        initial_state: fm.initial_state,
        variables,
        states: compiled_states,
    })
}
```

### v2 compile() changes

The flow remains similar, but with structural changes:

```rust
pub fn compile(source_path: &Path) -> anyhow::Result<CompiledTemplate> {
    // 1. Read file
    let content = std::fs::read_to_string(source_path)?;
    
    // 2. Split frontmatter
    let (frontmatter_str, body) = split_frontmatter(&content)?;
    
    // 3. Deserialize YAML frontmatter
    // SourceFrontmatter is unchanged; SourceState has new fields
    let fm: SourceFrontmatter = serde_yml::from_str(frontmatter_str)?;
    
    // 4. Validate required frontmatter fields (unchanged)
    if fm.name.is_empty() { ... }
    
    // 5. Extract directives from markdown body (unchanged)
    let directives = extract_directives(&fm.states, body);
    
    // 6. Build compiled states (CHANGED structure for v2)
    let mut compiled_states: BTreeMap<String, TemplateState> = BTreeMap::new();
    for (state_name, source_state) in &fm.states {
        let directive = directives.get(state_name)?;
        
        // Compile gates (changed: only accept command gates)
        let mut compiled_gates: BTreeMap<String, Gate> = BTreeMap::new();
        for (gate_name, source_gate) in &source_state.gates {
            let gate = compile_gate_v2(state_name, gate_name, source_gate)?;  // NEW: v2 version
            compiled_gates.insert(gate_name.clone(), gate);
        }
        
        // NEW: Compile accepts block to FieldSchema map
        let compiled_accepts = source_state.accepts.as_ref().map(|accepts_map| {
            accepts_map
                .iter()
                .map(|(name, schema)| {
                    (
                        name.clone(),
                        FieldSchema {
                            field_type: schema.field_type.clone(),
                            required: schema.required,
                            values: schema.values.clone(),
                            description: schema.description.clone(),
                        },
                    )
                })
                .collect::<BTreeMap<String, FieldSchema>>()
        });
        
        // NEW: Transform SourceTransition list to Transition list
        let compiled_transitions = source_state
            .transitions
            .iter()
            .map(|t| Transition {
                target: t.target.clone(),
                when: t.when.clone(),
            })
            .collect::<Vec<Transition>>();
        
        compiled_states.insert(
            state_name.clone(),
            TemplateState {
                directive,
                accepts: compiled_accepts,  // NEW
                transitions: compiled_transitions,  // CHANGED: from Vec<String>
                terminal: source_state.terminal,
                gates: compiled_gates,
                integration: source_state.integration.clone(),  // NEW
            },
        );
    }
    
    // 7. Validate transition targets exist (CHANGED: access .target field)
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
    
    // 8. Validate initial_state exists (unchanged)
    if !compiled_states.contains_key(&fm.initial_state) { ... }
    
    // 9. Build variables (unchanged)
    let variables: BTreeMap<String, VariableDecl> = fm.variables.into_iter().map(...).collect();
    
    // 10. Return CompiledTemplate with format_version: 2
    Ok(CompiledTemplate {
        format_version: 2,  // CHANGED: from 1
        name: fm.name,
        version: fm.version,
        description: fm.description,
        initial_state: fm.initial_state,
        variables,
        states: compiled_states,
    })
}
```

### New compile_gate_v2() function

```rust
fn compile_gate_v2(state_name: &str, gate_name: &str, source: &SourceGate) -> anyhow::Result<Gate> {
    match source.gate_type.as_str() {
        // v2 only supports command gates
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
                field: String::new(),
                value: String::new(),
                command: source.command.clone(),
                timeout: source.timeout,
            })
        }
        // Reject field gates
        GATE_TYPE_FIELD_NOT_EMPTY | GATE_TYPE_FIELD_EQUALS => {
            return Err(anyhow!(
                "state {:?} gate {:?}: field gate type {:?} is not supported in format version 2\
                \nuse the accepts/when constructs instead to declare and route on agent-submitted evidence",
                state_name,
                gate_name,
                source.gate_type
            ));
        }
        unknown => Err(anyhow!(
            "state {:?} gate {:?}: unknown type {:?}",
            state_name,
            gate_name,
            unknown
        )),
    }
}
```

---

## Testing Strategy for Compiler Changes

### Test Cases (additions to compile.rs tests)

1. **Basic v2 template with accepts and when**
   - Compiles successfully with `accepts` block and structured transitions
   - Sets `format_version: 2`

2. **When condition field validation**
   - Rejects `when` field not in `accepts`
   - Error message names the unknown field

3. **Enum value validation**
   - Accepts enum value in `values` list
   - Rejects enum value not in list
   - Error message shows allowed values

4. **Mutual exclusivity validation**
   - Accepts disjoint single-field conditions (e.g., `decision: proceed` vs `decision: escalate`)
   - Rejects overlapping single-field conditions (same value)
   - Error message names conflicting transitions

5. **Empty when validation**
   - Rejects transition with empty `when: {}`

6. **When without accepts**
   - Rejects transition with `when` but state has no `accepts`

7. **Field gate rejection in v2**
   - Rejects `type: field_not_empty` with helpful error message
   - Rejects `type: field_equals` with helpful error message

8. **Integration field passthrough**
   - Compiles integration field as Option<String>
   - Stores string verbatim

9. **Multi-field when conditions**
   - Compiles successfully (no validation error)
   - Sets transition.when with multiple keys

10. **Variable interpolation in directives**
    - Unchanged from v1: directives with `{{VAR}}` compile as-is

11. **JSON round-trip**
    - v2 template serializes and deserializes correctly
    - format_version stays 2

### Example Test Case

```rust
#[test]
fn v2_mutual_exclusivity_valid() {
    let src = r#"---
name: test
version: "1.0"
initial_state: choose
states:
  choose:
    accepts:
      decision: { type: enum, values: [a, b] }
    transitions:
      - target: path_a
        when: { decision: a }
      - target: path_b
        when: { decision: b }
  path_a:
    transitions: [end]
  path_b:
    transitions: [end]
  end:
    terminal: true
---
## choose
Pick a path.
## path_a
Path A.
## path_b
Path B.
## end
Done.
"#;
    let f = write_temp(src);
    let result = compile(f.path());
    assert!(result.is_ok());
    let compiled = result.unwrap();
    assert_eq!(compiled.format_version, 2);
}

#[test]
fn v2_mutual_exclusivity_conflict() {
    let src = r#"---
name: test
version: "1.0"
initial_state: choose
states:
  choose:
    accepts:
      decision: { type: enum, values: [a, b] }
    transitions:
      - target: path_a
        when: { decision: a }
      - target: path_b
        when: { decision: a }  // Same value!
  path_a:
    transitions: [end]
  path_b:
    transitions: [end]
  end:
    terminal: true
---
## choose
Pick a path.
## path_a
Path A.
## path_b
Path B.
## end
Done.
"#;
    let f = write_temp(src);
    let compiled = compile(f.path()).unwrap();
    let err = compiled.validate().unwrap_err();
    assert!(err.contains("both match when") && err.contains("decision"));
}

#[test]
fn v2_field_gate_rejected() {
    let src = r#"---
name: test
version: "1.0"
initial_state: process
states:
  process:
    terminal: true
    gates:
      check:
        type: field_not_empty
        field: status
---
## process
Do something.
"#;
    let f = write_temp(src);
    let err = compile(f.path()).unwrap_err();
    assert!(err.to_string().contains("not supported"));
}
```

---

## Summary

**SourceState transformation:**
- `transitions: Vec<String>` → `Vec<SourceTransition>` (each with `target` and optional `when`)
- Add `accepts: Option<HashMap<String, SourceFieldSchema>>`
- Add `integration: Option<String>`

**Validation rules removed:**
- `field_not_empty` gate validation
- `field_equals` gate validation

**Validation rules added:**
- When condition fields must exist in `accepts`
- Enum when values must be in field's `values` list
- Empty when conditions rejected
- Mutual exclusivity: single-field when conditions must have disjoint values
- When conditions require an `accepts` block

**Compiler changes:**
- `compile_gate()` becomes `compile_gate_v2()`: only accepts GATE_TYPE_COMMAND
- Transition compilation: transform Vec<SourceTransition> to Vec<Transition>
- Accepts compilation: convert HashMap<String, SourceFieldSchema> to BTreeMap<String, FieldSchema>
- New method `validate_transition_mutual_exclusivity()` for single-field conditions
- `format_version` set to 2

**Concrete errors:**
- "transitions X and Y both match when field=value"
- "transition references unknown field F in when condition"
- "condition value V not in enum values [A, B, C]"
- "transition has when conditions but no accepts block"
- "transition has empty when condition"
- "field gate type X not supported in format version 2"

The compiler design is straightforward: structure changes follow the v2 schema, validation is mostly about checking field/value consistency, and mutual exclusivity leverages grouping and deduplication on single-field cases.

