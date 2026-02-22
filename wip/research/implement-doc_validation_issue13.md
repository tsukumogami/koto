# Validation Results: Issue #13 (Compiled Template Format with JSON Parsing)

**Date**: 2026-02-22
**Issue**: #13 feat(template): define compiled template format with JSON parsing
**Scenarios validated**: scenario-1 through scenario-8
**Overall result**: ALL PASSED (8/8)

---

## Scenario 1: ParseJSON accepts valid compiled template
**Status**: PASSED

Created a fully-populated JSON template with all fields: format_version, name, version, description, initial_state, variables (with required/default), states (with directives, transitions, terminal flags, and all three gate types: field_not_empty, field_equals, command with timeout). ParseJSON returned a valid CompiledTemplate with all fields correctly populated. No error returned.

**Verified fields**:
- FormatVersion = 1
- Name = "deploy-workflow"
- Version = "1.5"
- Description = "Deployment pipeline"
- InitialState = "plan"
- 2 variables (TARGET_ENV required, DRY_RUN with default)
- 4 states with correct directives, transitions, terminal flags
- Gates: field_not_empty, field_equals, and command (with timeout=30) all parsed correctly

---

## Scenario 2: ParseJSON rejects invalid JSON
**Status**: PASSED

Called `ParseJSON([]byte("{not valid json"))`. Returned an error containing "invalid character" (from encoding/json). Error type is `*json.SyntaxError`. No validation error was produced -- the JSON parse error surfaces first, as expected.

---

## Scenario 3: ParseJSON rejects unsupported format_version
**Status**: PASSED

Created valid JSON with `format_version: 99`. ParseJSON returned error with exact message: `"unsupported format version: 99"`. Matches the acceptance criteria format string exactly.

---

## Scenario 4: ParseJSON rejects missing required fields
**Status**: PASSED

Three sub-cases tested:
- Missing `name`: error = `"missing required field: name"` (exact match)
- Missing `version`: error = `"missing required field: version"` (exact match)
- Missing `initial_state`: error = `"missing required field: initial_state"` (exact match)

Each error message matches the format `"missing required field: <field>"` specified in the acceptance criteria.

---

## Scenario 5: ParseJSON validates state machine integrity
**Status**: PASSED

Four sub-cases tested:
- `initial_state` references non-existent state: error = `initial_state "nonexistent" is not a declared state` (exact match)
- Empty states map: error = `template has no states` (exact match)
- Transition targets non-existent state: error = `state "start" references undefined transition target "phantom"` (exact match)
- State has empty directive: error = `state "start" has empty directive` (exact match)

All error messages match the format strings in the acceptance criteria.

---

## Scenario 6: ParseJSON validates gate declarations
**Status**: PASSED

Three sub-cases tested:
- Unknown gate type "unknown_type": error = `state "s" gate "g1": unknown type "unknown_type"` (exact match)
- field_not_empty missing field property: error = `state "s" gate "g1": missing required field "field"` (exact match)
- command gate with empty command: error = `state "s" gate "g1": command must not be empty` (exact match)

All error messages match the format strings in the acceptance criteria.

---

## Scenario 7: CompiledTemplate builds engine.Machine with gates
**Status**: PASSED

Parsed a compiled JSON with gates on states, then called BuildMachine(). Verified:
- Machine.Name, InitialState correctly set
- Machine.States has correct count (3 states)
- MachineState.Gates populated correctly:
  - assess state: 2 gates (field_not_empty with field=TASK, field_equals with field=PRIORITY value=high)
  - plan state: 1 gate (command with command="make test", timeout=120)
  - done state: 0 gates (terminal state)
- Terminal flags correctly set on done state
- Transitions correctly set on all states
- Machine.DeclaredVars contains both declared variable names (TASK, PRIORITY)

---

## Scenario 8: Engine.Machine() deep copy includes gates
**Status**: PASSED

Created an engine with a machine that has gates on states. Called Engine.Machine() to get copy1. Mutated copy1 extensively:
- Changed gate field and type values
- Added a new gate ("new_gate")
- Added a new DeclaredVars key ("INJECTED_VAR")
- Deleted an existing DeclaredVars key ("CONTEXT")
- Appended to transitions

Called Engine.Machine() again to get copy2. Verified:
- copy2 gates have original values (Field="CONTEXT", Type="field_not_empty")
- Injected gate does not exist in copy2
- Gate count is original (2)
- DeclaredVars has original content (CONTEXT=true, no INJECTED_VAR)
- DeclaredVars count is original (1)
- Transitions have original content ([finish])
- Command gate values are original (Command="echo ok", Timeout=10)

Deep copy is fully independent. Mutations to one copy do not affect subsequent copies.

---

## Additional Verification

Ran the issue's own structural validation script. All checks passed:
- CompiledTemplate, GateDecl, StateDecl, VariableDecl structs exist
- ParseJSON function exists
- Gates field exists on engine.MachineState
- JSON Schema file exists at pkg/template/compiled-template.schema.json
- No external dependencies added (only github.com/tsukumogami/koto internal import)
- All existing unit tests pass (template: 33 tests, engine: 37 tests)
