# Research: Mutual Exclusivity Validation in Template Format v2 Compiler

## Problem Statement

koto template format v2 introduces `when` conditions on per-transition basis. Each transition from a given state can declare a `when` condition that routes evidence-based branching. The strategic design says:

> The template compiler validates that per-transition `when` conditions on the same state are mutually exclusive (same field, disjoint values) and rejects templates that are non-deterministic. Single-field conditions can be verified; multi-field conditions are the author's responsibility.

This investigation defines the validation algorithm, error messages, and edge case handling.

## Strategic Design Requirements

From `DESIGN-unified-koto-next.md`:

1. **Compiler validates mutual exclusivity** for single-field `when` conditions
2. **Single-field case**: Two transitions test the same field against disjoint values (e.g., `decision: proceed` vs `decision: escalate`)
3. **Multi-field case**: Template authors are responsible; compiler cannot verify (e.g., transition A tests `field1: valueA`, transition B tests `field2: valueB`)
4. **Non-deterministic templates are rejected** with clear error messages
5. **Compiler warning for heading collisions** (in v1 format) — v2 may have equivalent collision scenarios

## Current Implementation State

The v1 Rust compiler (`src/template/compile.rs`) validates:
- Required fields in YAML frontmatter (name, version, initial_state)
- Transition targets exist
- State directives are present
- Gate types and required fields

**Missing**: No mutual exclusivity validation exists yet (v1 format used simple `transitions: []string`).

## Format: `when` Conditions Specification

Based on the strategic design and scope exploration, v2 transitions have:

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

    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: escalate_review
        when:
          decision: escalate
```

**Structure of `when`**:
- `when` is a map of `field_name: value_or_condition`
- Single-field case: exactly one key in the map (e.g., `{decision: proceed}`)
- Multi-field case: multiple keys in the map (e.g., `{field1: value1, field2: value2}`)

## Mutual Exclusivity Algorithm

### Definition: Single-Field Mutual Exclusivity

Two transitions are mutually exclusive on a single field if:

1. Both have `when` conditions
2. Both `when` conditions contain exactly one key
3. Both conditions test the **same field** (same key name)
4. The values are **disjoint** (not equal)

Example: `decision: proceed` and `decision: escalate` on the same field `decision` are mutually exclusive.

### Definition: Multi-Field Mutual Exclusivity

Two transitions are **not statically verifiable** if:

1. At least one `when` condition has multiple fields, OR
2. The conditions test different fields (no overlap)

Example: Transition A has `{decision: proceed, priority: high}` and Transition B has `{decision: proceed, priority: low}` — cannot verify statically that both won't match (requires runtime evidence values).

### Algorithm: Single-Field Validation

For each state with transitions:

1. **Extract all single-field transitions**: Collect transitions where `when.keys().len() == 1`
2. **Group by field**: Group single-field transitions by the field name they test
3. **Check disjoint values within each group**:
   - For each group (same field), compare all pairs of values
   - If any two values are equal (e.g., both `decision: proceed`), reject with error
   - If all values are distinct, the group is valid
4. **Report unverifiable multi-field transitions**: Flag transitions with `when.keys().len() > 1` or mixed single/multi-field on the same state

### Complexity and Limitations

- **Single-field only**: The compiler can verify `decision: proceed` vs `decision: escalate` are disjoint
- **Cannot verify enum membership**: If a field is declared as `values: [proceed, escalate]` but a transition specifies `when: {decision: proceed}`, the compiler assumes the value is valid (validation happens at submission time)
- **No negative conditions**: `when` does not support "not equal" or exclusion logic
- **No OR logic**: A single transition cannot declare `when: {decision: [proceed, escalate]}` (OR of multiple values). Each transition tests a single specific value.
- **Enum overlap**: If two transitions declare `when: {decision: proceed}`, both are satisfied by the same evidence — conflict, not exclusivity
- **Cannot detect logical overlap in multi-field**: If transition A requires `{field1: proceed, field2: high}` and transition B requires `{field1: proceed, field2: low}`, both could be true if submitted evidence provides both fields with matching values

## Error Messages

### 1. Non-deterministic Transition: Duplicate Values

**Condition**: Two transitions from the same state test the same field with the same value.

```yaml
transitions:
  - target: deploy
    when:
      decision: proceed
  - target: escalate_review
    when:
      decision: proceed
```

**Error**:
```
error: state "analyze_results": transitions "deploy" and "escalate_review" both match when decision=proceed
  → make these transitions mutually exclusive by using different values for the "decision" field
  → or combine them into a single transition with fallback logic
```

### 2. Empty `when` Condition

**Condition**: A transition has a `when` block but it's empty (no fields).

```yaml
transitions:
  - target: next_state
    when: {}
```

**Error**:
```
error: state "current": transition "next_state" has empty when condition
  → add at least one field to the when condition (e.g., when: {decision: proceed})
  → or remove the when block if this transition should not require evidence
```

### 3. Missing Field in Some Transitions

**Condition**: Some transitions from a state have `when`, others don't (mixing unconditional and conditional).

```yaml
transitions:
  - target: next_state
    when:
      decision: proceed
  - target: fallback_state
    # no when condition
```

**Warning** (not error):
```
warning: state "current": transition "next_state" requires evidence (decision=proceed)
  but transition "fallback_state" has no when condition
  → if "fallback_state" is intended as a fallback/catch-all, consider removing its when condition from all transitions
  → if both require conditions, add when condition to "fallback_state"
```

### 4. Multi-Field Conditions (Informational)

**Condition**: A transition has multi-field `when` condition.

**Informational message** (not an error, but documented in output):
```
note: state "analyze_results": transition "deploy" has multi-field when condition {decision: proceed, priority: high}
  → mutual exclusivity with other transitions cannot be statically verified
  → ensure your template logic prevents both this transition and others from being satisfied simultaneously
  → (this is a template author responsibility, not validated by the compiler)
```

## Edge Cases and Design Decisions

### Edge Case 1: Null/Empty Values

**Question**: Is `when: {decision: ""}` (empty string value) allowed?

**Decision**: Yes. Empty string is a valid evidence value. The compiler accepts it. Validation happens at submission time (if the `accepts` schema forbids empty, the agent-submitted evidence is rejected).

### Edge Case 2: Field Declared but Not Used in `when`

**Question**: If `accepts` declares field `decision` but no transition uses it in `when`, is that an error?

**Decision**: No. The field is part of the evidence schema agents can submit, but not required by any transition. This is valid (the state might have a single unconditional transition, or `decision` might be used for audit/logging, not routing).

### Edge Case 3: Field Used in `when` but Not Declared in `accepts`

**Question**: Can a transition test `when: {unknown_field: value}` if `accepts` doesn't declare it?

**Decision**: Error. Every field in a `when` condition must be declared in the state's `accepts` block. This is validated at compile time to catch typos.

**Error**:
```
error: state "analyze_results": transition "deploy" references unknown field "unknown_field" in when condition
  → field must be declared in the accepts block for this state
  → available fields: decision, rationale
```

### Edge Case 4: Transitions with No `when` and No `accepts`

**Question**: What if a state has no `accepts` block and a transition has no `when`?

**Decision**: This is valid. The state is either:
- Terminal (no outgoing transitions), or
- Auto-advances when gates pass (gates: field_not_empty, field_equals, command), or
- A sink state (no transitions defined)

No mutual exclusivity check needed.

### Edge Case 5: Circular Validation

**Question**: Can mutual exclusivity check be fooled by circular transitions?

Decision**: No. The compiler checks mutual exclusivity per state, not across paths. A state can have a transition back to itself (`when: {retry: true}` going back to the same state) — it's still mutually exclusive with other transitions from that state.

### Edge Case 6: Enum Values Not Exhaustive

**Question**: If `accepts` declares `decision: {type: enum, values: [proceed, escalate]}` but only one transition tests `proceed`, what happens?

**Decision**: Valid. Not all enum values need to be used. If evidence `decision: something_else` is submitted (not in the enum), it's rejected at submission time. If only `proceed` is tested, `escalate` leads nowhere — but that's a template design issue, not a compilation error.

### Edge Case 7: Type Mismatches in `when`

**Question**: If `accepts` declares `decision: {type: enum, values: [proceed, escalate]}` but a transition tests `when: {decision: 123}` (integer, not string)?

**Decision**: Compile-time validation converts values to strings for comparison. If the source YAML has `when: {decision: 123}`, it's serialized as `"123"`. Enum values are strings. They don't match. This causes a runtime issue (evidence never matches), but it's allowed at compile time. Consider a linter warning (out of scope for compiler).

## No Transitions Scenarios

### Scenario 1: State with No Outgoing Transitions

```yaml
states:
  current:
    # no transitions
```

**Behavior**: No mutual exclusivity check needed. State must be terminal or configured elsewhere.

### Scenario 2: State with Single Transition

```yaml
states:
  current:
    transitions:
      - target: next
        when: {decision: proceed}
```

**Behavior**: No mutual exclusivity check needed (only one transition). Valid.

### Scenario 3: Terminal State

```yaml
states:
  done:
    terminal: true
    transitions: []
```

**Behavior**: No mutual exclusivity check. Terminal states don't advance.

## Compiler Error vs. Warning vs. Note

### Errors (Reject Template)
- Duplicate `when` values for the same field
- `when` condition references undeclared field
- Empty `when` condition on a transition (when: {})

### Warnings (Template Loads, but Alert Author)
- Mixing conditional and unconditional transitions from same state
- Multi-field `when` conditions detected (author responsibility for exclusivity)

### Notes (Informational, No Action)
- Multi-field conditions found

## Test Cases for Implementation

1. **Single-field disjoint**: `{decision: proceed}` vs `{decision: escalate}` → Valid, mutually exclusive
2. **Single-field duplicate**: `{decision: proceed}` vs `{decision: proceed}` → Error
3. **Multi-field**: `{decision: proceed, priority: high}` and `{decision: proceed, priority: low}` → Warning, not validated
4. **Mixed single/multi**: `{decision: proceed}` and `{decision: proceed, priority: high}` → Warning
5. **Missing field in some transitions**: Transition A has `when`, Transition B doesn't → Warning
6. **Empty when**: `when: {}` → Error
7. **Field not in accepts**: `when: {unknown: value}` → Error
8. **No when conditions**: All transitions omit `when` → Valid
9. **Single transition**: Only one outgoing transition → Valid
10. **Three transitions, pairwise exclusive**: `{decision: a}`, `{decision: b}`, `{decision: c}` → Valid
11. **Three transitions, one duplicate**: `{decision: a}`, `{decision: b}`, `{decision: a}` → Error

## Algorithm Pseudocode

```
fn validate_mutual_exclusivity(state: TemplateState) -> Result<(), Error> {
  transitions_with_when := [t for t in state.transitions if t.when is not null]
  
  if transitions_with_when.len() <= 1:
    return Ok(())  // 0 or 1 transition with when: no conflict possible
  
  single_field_transitions := [
    t for t in transitions_with_when if t.when.keys().len() == 1
  ]
  
  // Check for duplicate values on same field
  for field_name in field_names_in_conditions(single_field_transitions):
    values_for_field := [t.when[field_name] for t in single_field_transitions if field_name in t.when]
    seen := set()
    for (transition, value) in pairs(values_for_field):
      if value in seen:
        return Err(format!(
          "state {}: transitions {} and {} both match when {}={}",
          state.name,
          transition.target,
          other_transition.target,
          field_name,
          value
        ))
      seen.insert(value)
  
  // Warn about multi-field conditions
  multi_field := [t for t in transitions_with_when if t.when.keys().len() > 1]
  if multi_field.len() > 0:
    warn!("Multi-field when conditions on state {}: not statically validated", state.name)
  
  // Warn about mixed single/multi
  if single_field_transitions.len() > 0 and multi_field.len() > 0:
    warn!("State {} has both single-field and multi-field when conditions", state.name)
  
  Ok(())
}
```

## Summary

Mutual exclusivity validation in koto's template format v2 compiler:

1. **Single-field focus**: Detects conflicts only when two transitions from the same state test the same field with overlapping (or identical) values
2. **Simple algorithm**: Group transitions by field name, check for duplicate values within each group
3. **Multi-field deferral**: Cannot statically verify exclusivity when conditions span multiple fields; compiler warns but does not reject
4. **Error taxonomy**: Errors for non-deterministic templates (duplicate values), warnings for unverifiable cases, notes for informational messages
5. **Edge cases handled**: Empty conditions, undeclared fields, unconditional transitions, enum values, terminal states

The compiler's job is to catch the most common mistakes (duplicate values on same field) while being honest about what it cannot verify (multi-field logical relationships). Clear error messages guide template authors to fix issues.

## Implementation Notes for Tactial Sub-Design (#47)

- Transition `when` is a Map<String, String> or serde_json::Value
- Field names are validated against state's `accepts` block at compile time
- Value comparison is string-based (enum values are strings in YAML)
- Mutual exclusivity check runs once per state after all transitions are parsed
- Error messages should name the specific transitions and field that conflict
- Multi-field conditions generate compiler warnings, not errors
- The compiler must reject obviously non-deterministic templates (same field, same value)

