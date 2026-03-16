# Research: Template Format v2 Rust Type Definitions

**Phase:** design  
**Role:** rust-types  
**Date:** 2026-03-15  
**Status:** Complete

## Summary

v2 Rust types remove `GATE_TYPE_FIELD_NOT_EMPTY` and `GATE_TYPE_FIELD_EQUALS` constants, keeping only `GATE_TYPE_COMMAND`. The `TemplateState` struct gains `accepts: Option<BTreeMap<String, FieldSchema>>`, `integration: Option<String>`, and transforms `transitions: Vec<String>` to `transitions: Vec<Transition>`. The `Transition` struct adds optional `when: Option<BTreeMap<String, serde_json::Value>>` for routing conditions. Compiled JSON v2 serializes to clean, flat structures with minimal defaults omitted via `skip_serializing_if`.

## Rust Type Definitions

### CompiledTemplate (v2)

```rust
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// A compiled template in FormatVersion=2 JSON format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledTemplate {
    pub format_version: u32,  // Will be 2
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

**Changes from v1:**
- `format_version` becomes 2 (was 1)
- All fields remain at the top level; no structural nesting

### TemplateState (v2)

```rust
/// A state declaration in a v2 compiled template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    pub directive: String,
    
    /// NEW in v2: Evidence field schema. If present, agent must submit matching data
    /// before transitioning. Generates the `expects` field in `koto next` output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<BTreeMap<String, FieldSchema>>,
    
    /// CHANGED in v2: From Vec<String> to structured transitions with conditions.
    /// Each transition specifies a target state and optional `when` conditions.
    pub transitions: Vec<Transition>,
    
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    
    /// Unchanged from v1: Koto-verifiable conditions (gates).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, Gate>,
    
    /// NEW in v2: Processing integration tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,
}
```

**Changes from v1:**
- Added: `accepts: Option<BTreeMap<String, FieldSchema>>`
- Added: `integration: Option<String>`
- Changed: `transitions: Vec<String>` → `transitions: Vec<Transition>`
- Unchanged: `directive`, `terminal`, `gates`

### Transition (NEW)

```rust
/// A structured transition with optional routing conditions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    /// Target state name.
    pub target: String,
    
    /// Optional condition map. Keys are field names (from the `accepts` schema);
    /// values are the expected evidence values that route to this target.
    /// 
    /// If `when` is None, this is an unconditional transition (for states with
    /// no `accepts` block, or as a default fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}
```

**Serialization example:**
```json
{
  "target": "deploy",
  "when": {
    "decision": "proceed"
  }
}
```

or unconditional:
```json
{
  "target": "next_step"
}
```

### FieldSchema (NEW)

```rust
/// Evidence field schema: declares what a single field in agent submissions looks like.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
    /// Field type: "string", "number", "boolean", "enum", etc.
    #[serde(rename = "type")]
    pub field_type: String,
    
    /// Whether this field is required in agent submissions.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    
    /// For enum fields: list of allowed values. Omitted for non-enum types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    
    /// Optional description for the field (shown to agents in `expects` output).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}
```

**Serialization examples:**

Enum field:
```json
{
  "type": "enum",
  "values": ["proceed", "escalate"],
  "required": true
}
```

String field:
```json
{
  "type": "string",
  "required": true,
  "description": "Reason for decision"
}
```

### Gate (v2)

```rust
/// Gate declaration (unchanged from v1, but context changes with removed field gates).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    #[serde(rename = "type")]
    pub gate_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub field: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Timeout in seconds (0 = use default of 30s).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timeout: u32,
}
```

**No structural change, but:**
- `field` and `value` fields remain for legacy field gates in v1 code only
- v2 templates and engines should only use `type: "command"` gates
- v1 gate types (`field_not_empty`, `field_equals`) removed from constants (see below)

### Gate Type Constants (v2)

**Removed (v1 only):**
```rust
// REMOVED in v2
pub const GATE_TYPE_FIELD_NOT_EMPTY: &str = "field_not_empty";
pub const GATE_TYPE_FIELD_EQUALS: &str = "field_equals";
```

**Kept (v2):**
```rust
// Only GATE_TYPE_COMMAND survives in v2
pub const GATE_TYPE_COMMAND: &str = "command";
```

### VariableDecl (unchanged)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableDecl {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}
```

### Helper Functions

```rust
fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}
```

These functions drive `skip_serializing_if` to omit default/empty values from JSON output.

## Compiled JSON v2 Schema

### Minimal Template

```json
{
  "format_version": 2,
  "name": "simple",
  "version": "1.0",
  "initial_state": "start",
  "states": {
    "start": {
      "directive": "Begin here.",
      "transitions": [
        {
          "target": "done"
        }
      ],
      "terminal": false,
      "gates": {}
    },
    "done": {
      "directive": "Finished.",
      "transitions": [],
      "terminal": true,
      "gates": {}
    }
  }
}
```

**Observations:**
- `description` omitted (empty string)
- `variables` omitted (empty map)
- `accepts` omitted (None)
- `integration` omitted (None)
- `when` omitted for unconditional transitions (None)

### Full Template with Accepts, When, Integration, Gates

```json
{
  "format_version": 2,
  "name": "review-workflow",
  "version": "1.0",
  "description": "Multi-path code review workflow",
  "initial_state": "analyze_results",
  "variables": {},
  "states": {
    "analyze_results": {
      "directive": "Review the test output and decide the next step.",
      "accepts": {
        "decision": {
          "type": "enum",
          "values": ["proceed", "escalate"],
          "required": true
        },
        "rationale": {
          "type": "string",
          "required": true,
          "description": "Explain your decision"
        }
      },
      "transitions": [
        {
          "target": "deploy",
          "when": {
            "decision": "proceed"
          }
        },
        {
          "target": "escalate_review",
          "when": {
            "decision": "escalate"
          }
        }
      ],
      "terminal": false,
      "gates": {
        "tests_passed": {
          "type": "command",
          "command": "./check-ci.sh"
        }
      },
      "integration": "delegate_review"
    },
    "deploy": {
      "directive": "Deploy to production.",
      "transitions": [
        {
          "target": "complete"
        }
      ],
      "terminal": false,
      "gates": {}
    },
    "escalate_review": {
      "directive": "Escalate to senior review team.",
      "transitions": [
        {
          "target": "complete"
        }
      ],
      "terminal": false,
      "gates": {}
    },
    "complete": {
      "directive": "Review workflow complete.",
      "transitions": [],
      "terminal": true,
      "gates": {}
    }
  }
}
```

**Key characteristics:**
- `accepts` is a map of field names to schema objects
- `transitions` is an array of transition objects (not strings)
- `when` inside transitions is a flat key→value map (no operators)
- `integration` is a plain string tag
- `gates` map unchanged from v1 (but only `type: "command"` used)

## When Condition Value Handling

The `when` field uses `serde_json::Value` to handle arbitrary JSON types:

```rust
pub when: Option<BTreeMap<String, serde_json::Value>>,
```

### Serialization

For enum matching, values serialize as JSON strings:
```json
{
  "decision": "proceed"
}
```

For numeric/boolean conditions (future extensibility), values can be numbers or booleans:
```json
{
  "approval_count": 2,
  "is_urgent": true
}
```

### Deserialization

During validation, the compiler checks:

```rust
// For enum fields, extract as string
if let Some(cond_str) = condition_value.as_str() {
    if !allowed_values.contains(&cond_str.to_string()) {
        return Err(...);
    }
}
```

This allows v2 to remain extensible: future phases can support numeric comparisons without breaking the type.

## Serialization Behavior (skip_serializing_if)

The v2 schema uses `skip_serializing_if` to minimize JSON output:

| Field | Condition | Serialized |
|-------|-----------|-----------|
| `description` | Not empty | Yes |
| `variables` | Not empty | Yes |
| `accepts` | Some(...) | Yes |
| `integration` | Some(...) | Yes |
| `when` | Some(...) | Yes |
| `terminal` | true | Always |
| `gates` | Not empty | Always |
| `required` (in FieldSchema) | true | Always |
| `values` (in FieldSchema) | Some(...) | Yes |
| `field_type` (in FieldSchema) | N/A | Always |
| `target` (in Transition) | N/A | Always |
| `timeout` (in Gate) | Not 0 | Yes |

## Validation Rules (v2 Compile Time)

### 1. Transition Target Validation

All `transition.target` values must exist as declared states:

```rust
for transition in &state.transitions {
    if !self.states.contains_key(&transition.target) {
        return Err(...);
    }
}
```

### 2. When Condition Field Validation

Every field in a `when` map must exist in the state's `accepts` block:

```rust
if let Some(when_conditions) = &transition.when {
    if state.accepts.is_none() {
        return Err(...);  // when conditions without accepts block
    }
    let accepts = state.accepts.as_ref().unwrap();
    for (field_name, _) in when_conditions {
        if !accepts.contains_key(field_name) {
            return Err(...);  // unknown field in when condition
        }
    }
}
```

### 3. Enum Value Validation

For enum fields, `when` values must be in the declared `values` list:

```rust
if field_schema.field_type == "enum" {
    if let Some(allowed_values) = &field_schema.values {
        if let Some(cond_str) = condition_value.as_str() {
            if !allowed_values.contains(&cond_str.to_string()) {
                return Err(...);
            }
        }
    }
}
```

### 4. Mutual Exclusivity (Single-Field Only)

For transitions with single-field `when` conditions on the same field, values must be disjoint:

```rust
// Build field -> [(target, value)] map
for transition in &state.transitions {
    if let Some(when_map) = &transition.when {
        if when_map.len() == 1 {  // Only validate single-field
            for (field_name, condition_value) in when_map {
                let value_str = condition_value.as_str();
                field_conditions.entry(field_name.clone())
                    .or_insert_with(Vec::new)
                    .push((transition.target.as_str(), value_str));
            }
        }
    }
}

// Check for duplicate values
for (field_name, transitions) in field_conditions {
    let mut seen_values = HashSet::new();
    for (_target, value) in &transitions {
        if let Some(v) = value {
            if seen_values.contains(v) {
                return Err(...);  // overlapping condition values
            }
            seen_values.insert(v.clone());
        }
    }
}
```

Multi-field conditions are not validated at compile time (author responsible).

## Gate Type Usage (v2)

### Commands Only

In v2 templates and compiled output, only `type: "command"` gates are valid:

```json
{
  "gates": {
    "tests_passed": {
      "type": "command",
      "command": "./check-ci.sh",
      "timeout": 60
    }
  }
}
```

### Field Gates (Removed)

The following are no longer supported in v2:

```json
{
  "type": "field_not_empty",
  "field": "status"
}
```

```json
{
  "type": "field_equals",
  "field": "approval",
  "value": "approved"
}
```

**Rationale:** Field-based verification is now expressed via `accepts` schema (what agent must provide) and `when` conditions (how to route). Koto gates are for external verification only.

## Source Type Definitions (Compiler Input)

The v2 `compile()` function parses YAML into these source types:

```rust
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
struct SourceState {
    #[serde(default)]
    transitions: Vec<SourceTransition>,  // NEW: was Vec<String>
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    gates: HashMap<String, SourceGate>,
    #[serde(default)]
    accepts: Option<HashMap<String, SourceFieldSchema>>,  // NEW
    #[serde(default)]
    integration: Option<String>,  // NEW
}

#[derive(Debug, Deserialize)]
struct SourceTransition {
    target: String,
    #[serde(default)]
    when: Option<HashMap<String, serde_json::Value>>,  // NEW
}

#[derive(Debug, Deserialize)]
struct SourceFieldSchema {
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    values: Option<Vec<String>>,
    #[serde(default)]
    description: String,
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

## Compile Transformation

The v2 compiler transforms source to compiled types:

1. **SourceState → TemplateState:**
   - `transitions: Vec<SourceTransition>` → `transitions: Vec<Transition>`
   - `accepts: Option<HashMap>` → `accepts: Option<BTreeMap<String, FieldSchema>>`
   - `integration: Option<String>` → `integration: Option<String>` (copy)
   - `gates: HashMap` → `gates: BTreeMap<String, Gate>` (copy)

2. **SourceTransition → Transition:**
   - `target: String` → `target: String` (copy)
   - `when: Option<HashMap>` → `when: Option<BTreeMap>` (copy, preserve serde_json::Value)

3. **SourceFieldSchema → FieldSchema:**
   - `field_type` → `field_type` (copy)
   - `required` → `required` (copy)
   - `values` → `values` (copy)
   - `description` → `description` (copy)

## Integration Field Semantics

The `integration` field is a **string tag** used for routing:

At runtime:
1. Engine reads `integration: "code_analysis_tool"` from compiled template
2. Looks up this tag in user/project configuration
3. Invokes the configured tool
4. Records an `IntegrationInvoked` event (from engine/types.rs)
5. Returns `koto next` output with integration status

From `src/engine/types.rs`, the event payload is:
```rust
IntegrationInvoked {
    state: String,
    integration: String,
    output: serde_json::Value,
},
```

## Format Version Bump

v2 introduces `format_version: 2`. Validation must check:

```rust
if self.format_version != 2 {
    return Err(format!("unsupported format version: {}", self.format_version));
}
```

Separate v1 and v2 code paths can coexist, with v2 being the new default.

## Backward Compatibility

**Not required.** Per codebase policy:
- koto has no released versions
- koto has no external users
- Format versions are intentionally breaking changes

v2 code can remove v1 gate type constants entirely. If v1 code is retained for reference, it remains isolated.

## Changes Summary

### Added to types.rs

```rust
// NEW: Transition struct
pub struct Transition { ... }

// NEW: FieldSchema struct
pub struct FieldSchema { ... }

// REMOVED: Gate type constants (v1 only)
// pub const GATE_TYPE_FIELD_NOT_EMPTY: &str = ...;
// pub const GATE_TYPE_FIELD_EQUALS: &str = ...;

// KEPT: Command gate constant (v2)
pub const GATE_TYPE_COMMAND: &str = "command";
```

### Modified in types.rs

```rust
// TemplateState gains three fields:
pub struct TemplateState {
    // ... existing fields ...
    pub accepts: Option<BTreeMap<String, FieldSchema>>,  // NEW
    pub transitions: Vec<Transition>,  // CHANGED from Vec<String>
    pub integration: Option<String>,  // NEW
}

// CompiledTemplate format_version changes:
pub format_version: u32,  // Now 2 instead of 1
```

### Added to compile.rs

```rust
// NEW source type
struct SourceTransition { ... }

// NEW source type
struct SourceFieldSchema { ... }

// SourceState gains two fields:
struct SourceState {
    transitions: Vec<SourceTransition>,  // CHANGED from Vec<String>
    accepts: Option<HashMap<String, SourceFieldSchema>>,  // NEW
    integration: Option<String>,  // NEW
}

// Compilation logic for new types
fn compile_transitions(...) -> Vec<Transition> { ... }
fn compile_field_schema(...) -> FieldSchema { ... }
```

## Conclusion

The v2 Rust type system is minimal and clean:
- Removes field gate logic from types (kept only for backward reference)
- Adds `Transition` and `FieldSchema` structs to represent v2 concepts
- Uses `serde_json::Value` for flexible condition values
- Leverages `skip_serializing_if` for compact JSON output
- Validation rules are straightforward and match the strategic design
- Integration with state file events is via string tags, not typed references

The types map directly to YAML source structure, making the compiler straightforward.

