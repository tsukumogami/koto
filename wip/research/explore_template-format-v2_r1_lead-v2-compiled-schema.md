# Research: Template Format v2 Compiled JSON Schema

**Phase:** explore  
**Role:** lead-v2-compiled-schema  
**Date:** 2026-03-15  
**Status:** Complete

## Summary

V2 compiled JSON schema transforms transitions from `transitions: Vec<String>` to structured transition objects with `target` and `when` condition maps. The `accepts` block (evidence field schema) serializes as a nested object mapping field names to schema definitions. The `integration` field (string routing tag) appears at the state level. No backward compatibility is needed; koto has no users and this is a clean break.

## Investigation Scope

This research answers the v2 compiled schema lead question:
- What's the exact v2 compiled JSON schema?
- How do `transitions` serialize from `Vec<String>` to structured objects?
- What does `accepts` look like in compiled JSON?
- Where does `integration` go?
- How do `when` conditions serialize?

## Current State (v1 Compiled Template)

### v1 Rust Types

From `src/template/types.rs`:

```rust
pub struct CompiledTemplate {
    pub format_version: u32,
    pub name: String,
    pub version: String,
    pub description: String,
    pub initial_state: String,
    pub variables: BTreeMap<String, VariableDecl>,
    pub states: BTreeMap<String, TemplateState>,
}

pub struct TemplateState {
    pub directive: String,
    pub transitions: Vec<String>,  // Simple list of target names
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

pub struct VariableDecl {
    pub description: String,
    pub required: bool,
    pub default: String,
}
```

### v1 Example JSON

A minimal v1 compiled template in JSON:

```json
{
  "format_version": 1,
  "name": "test-workflow",
  "version": "1.0",
  "description": "",
  "initial_state": "start",
  "variables": {},
  "states": {
    "start": {
      "directive": "Do the first task.",
      "transitions": ["done"],
      "terminal": false,
      "gates": {}
    },
    "done": {
      "directive": "All done.",
      "transitions": [],
      "terminal": true,
      "gates": {}
    }
  }
}
```

## Strategic Design Requirements

From `docs/designs/DESIGN-unified-koto-next.md`, the template v2 format must support:

### 1. Evidence Schema (`accepts` block)

```yaml
states:
  analyze_results:
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
      rationale:
        type: string
        required: true
```

**Purpose:** Declares what fields an agent must submit at this state. Generates the `expects` field in `koto next` output.

**Design notes:**
- Per-state schema, not global
- Drives payload validation in the advancement engine
- Field types include: `enum`, `string`, `number`, `boolean`
- `required` flag marks mandatory fields
- `values` array (enum-only) lists allowed values

### 2. Conditional Transitions (`when` conditions)

```yaml
transitions:
  - target: deploy
    when:
      decision: proceed
  - target: escalate_review
    when:
      decision: escalate
```

**Purpose:** Routes to different states based on agent-submitted evidence values, not just target names.

**Design notes:**
- Per-transition routing conditions
- Conditions are field→value maps (agent evidence must match)
- Compiler validates mutual exclusivity for single-field cases
- Multi-field conditions are author-responsible (documented limitation)
- Conditions are not gates (agent-submitted, not koto-verifiable)

### 3. Integration Field

```yaml
integration: delegate_review
```

**Purpose:** String tag identifying a processing integration to invoke at this state.

**Design notes:**
- Routing from template tag to actual tool is in user/project config, not template
- Template author names the integration; config binds it to a handler
- Graceful degradation: missing config entry doesn't fail template load, just degrades `koto next` output
- State can have both `accepts` (agent evidence) and `integration` (tool invocation)

### 4. Event-Sourced State File (Already Implemented in v1 Rust Code)

The v1 implementation in `src/engine/types.rs` already uses an event-sourced JSONL model:

```rust
pub struct StateFileHeader {
    pub schema_version: u32,
    pub workflow: String,
    pub template_hash: String,
    pub created_at: String,
}

pub enum EventPayload {
    WorkflowInitialized { template_path, variables },
    Transitioned { from, to, condition_type },
    EvidenceSubmitted { state, fields },
    DirectedTransition { from, to },
    IntegrationInvoked { state, integration, output },
    Rewound { from, to },
}

pub struct Event {
    pub seq: u64,
    pub timestamp: String,
    pub event_type: String,
    pub payload: EventPayload,
}
```

This is the state file model; the compiled template must declare what transitions and evidence each state expects.

## Proposed v2 Compiled Template Schema

### v2 Rust Types

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// The advancement engine uses `when` conditions to route evidence to the correct target.
    pub transitions: Vec<Transition>,
    
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    
    /// Unchanged from v1: Koto-verifiable conditions (gates).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, Gate>,
    
    /// NEW in v2: Processing integration tag. Routing from tag to actual tool is
    /// defined in user/project configuration, not in the template. If the integration
    /// is not configured, koto next degrades to returning the directive without
    /// integration output (no template load failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,
}

/// A structured transition with optional routing conditions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    /// Target state name.
    pub target: String,
    
    /// Optional condition map. Keys are field names (from the `accepts` schema);
    /// values are the expected evidence values that route to this target.
    /// 
    /// Example: { "decision": "proceed" } means this transition fires when
    /// the agent submits evidence with decision="proceed".
    /// 
    /// If `when` is None, this is an unconditional transition (for states with
    /// no `accepts` block, or as a default fallback; see compiler validation rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}

/// Evidence field schema: declares what a single field in agent submissions looks like.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
    /// Field type: "string", "number", "boolean", "enum", etc.
    #[serde(rename = "type")]
    pub field_type: String,
    
    /// Whether this field is required in agent submissions.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    
    /// For enum fields: list of allowed values.
    /// For string fields: omitted (or empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    
    /// Optional description for the field (shown to agents in `expects` output).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Unchanged from v1.
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
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timeout: u32,
}

/// Unchanged from v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableDecl {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

// Helper functions for serde skip_serializing_if
fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}
```

### v2 Serialization Changes

When serialized to JSON:

| Type | v1 | v2 | Example |
|------|----|----|---------|
| **format_version** | `1` | `2` | `"format_version": 2` |
| **transitions** | `["target1", "target2"]` | Array of objects | `[{"target": "deploy", "when": {...}}]` |
| **accepts** | N/A | Object mapping field names to schemas | `{"decision": {"type": "enum", "values": [...]}}` |
| **integration** | N/A | String tag | `"integration": "delegate_review"` |
| **when** | N/A | Object of field→value conditions | `{"decision": "proceed"}` |
| **gates** | Unchanged | Unchanged | `{"tests_passed": {"type": "command"}}` |

## Concrete v2 Compiled JSON Example

This corresponds to the YAML example in the strategic design:

```yaml
# Source YAML
states:
  analyze_results:
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
      rationale:
        type: string
        required: true
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: escalate_review
        when:
          decision: escalate
    gates:
      tests_passed:
        type: command
        command: ./check-ci.sh
    integration: delegate_review
```

Compiles to:

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
          "required": true
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

Key observations:
- `accepts` is an object with field names as keys, each containing a schema object
- `transitions` is now an array of objects (not strings) with `target` and optional `when`
- `when` condition values are simple JSON values (strings for enum matches, could be numbers/booleans for other types)
- States with no `accepts` block have `transitions` with no `when` (unconditional routing)
- `integration` is a simple string tag, not a nested object

## State with No Evidence (Unconditional Transitions)

A state with only koto-verifiable gates (no agent submission):

```yaml
states:
  wait_for_ci:
    directive: "Waiting for CI to pass..."
    gates:
      ci_check:
        type: command
        command: ./wait-for-ci.sh
    transitions:
      - target: next_step
```

Compiles to:

```json
{
  "directive": "Waiting for CI to pass...",
  "transitions": [
    {
      "target": "next_step"
    }
  ],
  "terminal": false,
  "gates": {
    "ci_check": {
      "type": "command",
      "command": "./wait-for-ci.sh"
    }
  }
}
```

No `accepts`, no `when` condition on the transition (auto-advance when gates pass).

## State with Only Evidence (No Gates)

A state that only needs agent submission, no koto verification:

```yaml
states:
  capture_input:
    accepts:
      plan_text:
        type: string
        required: true
    transitions:
      - target: analyze
        when:
          plan_text: "non-empty"  # Placeholder; actual validation is schema-based
```

Compiles to:

```json
{
  "directive": "Provide your implementation plan.",
  "accepts": {
    "plan_text": {
      "type": "string",
      "required": true
    }
  },
  "transitions": [
    {
      "target": "analyze",
      "when": {}
    }
  ],
  "terminal": false,
  "gates": {}
}
```

## Integration Field Semantics

The `integration` field in compiled JSON is a **string tag**, not a structured object:

```json
{
  "directive": "Analyzing code...",
  "integration": "code_analysis_tool",
  "accepts": { ... },
  "transitions": [ ... ]
}
```

**At runtime:**
1. The advancement engine reads `integration: "code_analysis_tool"`
2. Looks up this tag in user/project configuration (e.g., a config file mapping `code_analysis_tool` → command path)
3. Invokes the tool and records output in an `integration_invoked` event
4. Returns `koto next` output with the integration info nested under an `integration` object (different from template; output shows execution result)

**No integration config entry:**
- The compiled template loads successfully
- At runtime, `koto next` detects missing config and degrades: returns the directive with `integration.available: false`
- No template load-time failure (graceful degradation per PRD R17)

## Compiler Validation Rules (v2)

The v2 template compiler must enforce:

### 1. Mutual Exclusivity (Single-Field Cases)

If two transitions have `when` conditions on the same field:
```yaml
transitions:
  - target: a
    when:
      status: ready
  - target: b
    when:
      status: waiting
```

The compiler validates that values are disjoint: `{ready, waiting}` → no overlap → OK.

If values overlap, the compiler rejects with an error naming both transitions.

### 2. Multi-Field Conditions (Author Responsible)

If transitions test different fields, compiler cannot validate:
```yaml
transitions:
  - target: a
    when:
      status: ready
      approval_count: 2  # Different field
  - target: b
    when:
      status: ready      # Same value, different other conditions
      approval_count: 1
```

Compiler documents: "Could not statically verify mutual exclusivity for multi-field conditions; author must ensure non-overlapping semantics."

(Design note suggests considering explicit `exclusive_with` annotation for future; not in v2 scope.)

### 3. Accepts Schema Completeness

Every field in a `when` condition must exist in the `accepts` schema:

```yaml
accepts:
  decision: { type: enum, values: [a, b] }
transitions:
  - target: x
    when:
      decision: a
      unknown_field: value  # ERROR: unknown_field not in accepts
```

Compiler error: "Transition targets unknown field 'unknown_field' not declared in accepts"

### 4. Enum Value Validation

For enum fields, `when` values must be in the declared `values` list:

```yaml
accepts:
  status: { type: enum, values: [ready, waiting] }
transitions:
  - target: x
    when:
      status: invalid_value  # ERROR: not in [ready, waiting]
```

Compiler error: "Condition value 'invalid_value' not in enum values for field 'status'"

## Transition Structure Decision: Flat vs. Nested When

**Decision:** Flat `when` conditions, not nested objects.

Two options considered:

**Option A: Flat (Chosen)**
```json
{
  "target": "deploy",
  "when": {
    "decision": "proceed"
  }
}
```

Pros:
- Simpler JSON structure
- Matches YAML source closely
- Shorter serialization

**Option B: Nested conditions object**
```json
{
  "target": "deploy",
  "when": {
    "conditions": [
      { "field": "decision", "operator": "equals", "value": "proceed" }
    ]
  }
}
```

Pros:
- More extensible (allows operators like "contains", "greater_than")
- Clearer field/operator/value separation

Chosen Option A because:
- v2 scope is single-field equality conditions only
- YAML source is already flat
- Compiler doesn't need to store operators (equality is implicit)
- Future: if operators are needed, a Phase N upgrade can revise this

## Integration with koto next Output

The compiled template's `accepts` and `transitions` drive the `expects` field in `koto next` output.

**Template (v2 compiled):**
```json
{
  "accepts": {
    "decision": { "type": "enum", "values": ["proceed", "escalate"] },
    "rationale": { "type": "string", "required": true }
  },
  "transitions": [
    { "target": "deploy", "when": { "decision": "proceed" } },
    { "target": "escalate_review", "when": { "decision": "escalate" } }
  ]
}
```

**koto next output:**
```json
{
  "action": "execute",
  "state": "analyze_results",
  "directive": "Review the test output...",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": { "type": "enum", "values": ["proceed", "escalate"], "required": true },
      "rationale": { "type": "string", "required": true }
    },
    "options": [
      { "target": "deploy", "when": { "decision": "proceed" } },
      { "target": "escalate_review", "when": { "decision": "escalate" } }
    ]
  },
  "error": null
}
```

The `expects.options` array mirrors the compiled `transitions` array, giving agents the full routing map.

## Backward Compatibility

**Not needed. koto has no users and no released versions.**

Per `/home/dangazineu/dev/workspace/tsuku/tsuku-2/CLAUDE.md`:
> "Conventions: Never add AI attribution or co-author lines to commits or PRs"
> (And per the DESIGN document) "Both the state file format and the template format are breaking changes. This is intentional — koto has no released users and no existing workflows to preserve."

v2 compiled templates will have `format_version: 2`. Implementations can add a check:
```rust
if compiled.format_version != 2 {
    return Err(format!("unsupported format version: {}", compiled.format_version));
}
```

Old v1 code (if retained for reference) remains with `format_version: 1`.

## Validation Algorithm (CompiledTemplate::validate)

The v2 `validate()` method must extend v1:

```rust
impl CompiledTemplate {
    pub fn validate(&self) -> Result<(), String> {
        // From v1: required fields, format_version, initial_state existence
        if self.format_version != 2 {
            return Err(format!("unsupported format version: {}", self.format_version));
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

                // NEW v2: If transition has `when` conditions, validate against `accepts`
                if let Some(when_conditions) = &transition.when {
                    if state.accepts.is_none() {
                        return Err(format!(
                            "state {:?} transition to {:?} has `when` conditions but no `accepts` block",
                            state_name, transition.target
                        ));
                    }
                    let accepts = state.accepts.as_ref().unwrap();
                    for (field_name, condition_value) in when_conditions {
                        if !accepts.contains_key(field_name) {
                            return Err(format!(
                                "state {:?} transition to {:?} references unknown field {:?} in `when` condition",
                                state_name, transition.target, field_name
                            ));
                        }
                        
                        // Validate enum values if applicable
                        let field_schema = &accepts[field_name];
                        if field_schema.field_type == "enum" {
                            if let Some(allowed_values) = &field_schema.values {
                                // condition_value should be a string matching one of the allowed values
                                if let Some(cond_str) = condition_value.as_str() {
                                    if !allowed_values.contains(&cond_str.to_string()) {
                                        return Err(format!(
                                            "state {:?} transition condition has invalid enum value {:?} for field {:?}",
                                            state_name, cond_str, field_name
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

            // Validate mutual exclusivity (single-field case only)
            self.validate_transition_mutual_exclusivity(state_name, state)?;

            // NEW v2: Validate gates (unchanged from v1)
            for (gate_name, gate) in &state.gates {
                match gate.gate_type.as_str() {
                    GATE_TYPE_FIELD_NOT_EMPTY => {
                        if gate.field.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: missing required field \"field\"",
                                state_name, gate_name
                            ));
                        }
                    }
                    GATE_TYPE_FIELD_EQUALS => {
                        if gate.field.is_empty() {
                            return Err(format!(
                                "state {:?} gate {:?}: missing required field \"field\"",
                                state_name, gate_name
                            ));
                        }
                    }
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

    fn validate_transition_mutual_exclusivity(
        &self,
        state_name: &str,
        state: &TemplateState,
    ) -> Result<(), String> {
        // Only validate single-field conditions (multi-field is author-responsible)
        
        // Build a map of field -> [transition targets with conditions on that field]
        let mut field_conditions: std::collections::HashMap<String, Vec<(&str, Option<String>)>> =
            std::collections::HashMap::new();

        for transition in &state.transitions {
            if let Some(when_map) = &transition.when {
                if when_map.len() == 1 {
                    // Single-field condition: validate disjointness
                    for (field_name, condition_value) in when_map {
                        let value_str = condition_value.as_str().map(|s| s.to_string());
                        field_conditions
                            .entry(field_name.clone())
                            .or_insert_with(Vec::new)
                            .push((transition.target.as_str(), value_str));
                    }
                }
                // Multi-field conditions are not validated here
            }
        }

        // Check for duplicate values within a field
        for (field_name, transitions) in field_conditions {
            let mut seen_values = std::collections::HashSet::new();
            for (_target, value) in &transitions {
                if let Some(v) = value {
                    if seen_values.contains(v) {
                        // Find the conflicting targets
                        let targets: Vec<_> = transitions
                            .iter()
                            .filter(|(_, val)| val.as_ref() == Some(v))
                            .map(|(t, _)| *t)
                            .collect();
                        return Err(format!(
                            "state {:?}: transitions {:?} have overlapping conditions on field {:?} with value {:?}",
                            state_name, targets, field_name, v
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

## Compile Pipeline Changes

The v2 `compile()` function in `src/template/compile.rs` must:

1. **Parse v2 YAML structure:**
   - New `accepts` block in source `SourceState`
   - New `integration` field in source `SourceState`
   - New `when` conditions in source `SourceTransition`

2. **Transform source to compiled:**
   ```rust
   #[derive(Debug, Deserialize)]
   struct SourceState {
       #[serde(default)]
       transitions: Vec<SourceTransition>,  // Changed from Vec<String>
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
       when: Option<HashMap<String, serde_json::Value>>,
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
   ```

3. **Convert SourceState to TemplateState:**
   ```rust
   let compiled_state = TemplateState {
       directive: directives.get(state_name).cloned().unwrap_or_default(),
       accepts: source_state.accepts.as_ref().map(|accepts_map| {
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
               .collect()
       }),
       transitions: source_state
           .transitions
           .iter()
           .map(|t| Transition {
               target: t.target.clone(),
               when: t.when.clone(),
           })
           .collect(),
       terminal: source_state.terminal,
       gates: compiled_gates,
       integration: source_state.integration.clone(),
   };
   ```

4. **Update format_version:**
   ```rust
   Ok(CompiledTemplate {
       format_version: 2,  // Changed from 1
       name: fm.name,
       // ... rest unchanged
   })
   ```

## Testing Strategy

Test cases for v2 compiled template:

1. **Basic structure:**
   - Compiles v2 YAML with `accepts`, `when`, `integration`
   - JSON round-trips (serialize → deserialize → serialize matches)

2. **Transition validation:**
   - Rejects undefined transition targets
   - Rejects `when` conditions on fields not in `accepts`
   - Rejects enum condition values not in `values` list

3. **Mutual exclusivity:**
   - Accepts disjoint single-field conditions
   - Rejects overlapping single-field conditions
   - Allows multi-field conditions (no compile-time validation)

4. **Accepts schema:**
   - Compiles all field types (enum, string, number, boolean)
   - Validates enum with `values` array
   - Validates required flag

5. **Integration field:**
   - Compiles successfully with or without `integration`
   - Stores string tag exactly as provided
   - Does not fail if config doesn't exist (runtime concern, not compile-time)

6. **Backward compat check:**
   - v1 code (if retained) rejects v2 format_version: 2
   - v2 code rejects format_version: 1 (or implements v1 compatibility if needed)

## Example Test Case

```rust
#[test]
fn v2_compiles_with_accepts_and_when() {
    let src = r#"---
name: branching-workflow
version: "2.0"
initial_state: decide

states:
  decide:
    accepts:
      choice:
        type: enum
        values: [path_a, path_b]
        required: true
      explanation:
        type: string
        required: true
    transitions:
      - target: execute_a
        when:
          choice: path_a
      - target: execute_b
        when:
          choice: path_b
    gates:
      input_ready:
        type: field_not_empty
        field: choice
  execute_a:
    transitions:
      - target: done
    terminal: false
  execute_b:
    transitions:
      - target: done
    terminal: false
  done:
    terminal: true
---

## decide
Choose your path.

## execute_a
Executing path A.

## execute_b
Executing path B.

## done
Complete.
"#;

    let f = write_temp(src);
    let compiled = compile(f.path()).unwrap();

    assert_eq!(compiled.format_version, 2);
    
    let decide_state = &compiled.states["decide"];
    assert!(decide_state.accepts.is_some());
    let accepts = decide_state.accepts.as_ref().unwrap();
    assert!(accepts.contains_key("choice"));
    assert_eq!(accepts["choice"].field_type, "enum");
    assert_eq!(accepts["choice"].values, Some(vec!["path_a".to_string(), "path_b".to_string()]));
    
    assert_eq!(decide_state.transitions.len(), 2);
    assert_eq!(decide_state.transitions[0].target, "execute_a");
    assert_eq!(
        decide_state.transitions[0].when,
        Some(vec![("choice".to_string(), serde_json::json!("path_a"))].into_iter().collect())
    );
}
```

## Questions for Tactical Sub-Designs

### Phase 2: Template Format v2 Compilation

1. **Operator extensibility:** If Phase N needs comparison operators (not just equality), should v2 reserve a structure that allows future `operator` fields, or is flat equality sufficient for now?

2. **Complex conditions:** The design reserves multi-field condition validation for authors. Should v2 provide an optional `exclusive_with` annotation for authors to explicitly declare mutual exclusivity, or is documentation sufficient?

3. **Field type extensibility:** Are "enum", "string", "number", "boolean" the complete set, or should v2 allow custom types via a registry? (Probably custom types out of scope.)

4. **Integration absence:** When an integration is declared in template but not configured, should `koto next` output include integration info at all, or include it with `available: false`? (Strategic design says `available: false`, confirm in Phase 3.)

### Phase 3: CLI Output Contract

1. **Expects field structure:** Confirm that `expects.options` mirrors the compiled `transitions` array exactly, or should agent-facing options be filtered/transformed somehow?

2. **Error for stale transitions:** If agent submits evidence for a transition that no longer exists (template changed), what error code? Template hash mismatch should catch this, but confirm strategy.

3. **Directive rendering:** If directive text includes variable interpolation like `{{FIELD_VALUE}}`, where in the pipeline is that substitution applied? (Probably agent submission time, not compile time.)

## Conclusion

The v2 compiled template schema cleanly separates:
- **What states expect** (`accepts`): Evidence field schema, self-describing for `koto next` output
- **How evidence routes** (`when` conditions): Per-transition conditional routing
- **What to do** (`integration`): String tag for user-configured tool invocation
- **How to verify** (`gates`): Unchanged koto-verifiable conditions

The schema is straightforward to serialize to JSON, validate, and extend in future phases. No backward compatibility is needed. The tactical sub-designs for compilation, CLI output, and the advancement engine can proceed in parallel once this schema is accepted.

