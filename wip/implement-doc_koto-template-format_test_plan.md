# Test Plan: koto Template Format Specification

Generated from: docs/designs/DESIGN-koto-template-format.md
Issues covered: 5
Total scenarios: 20

## Scenario Checklist

- [x] scenario-1
- [x] scenario-2
- [x] scenario-3
- [x] scenario-4
- [x] scenario-5
- [x] scenario-6
- [x] scenario-7
- [x] scenario-8
- [x] scenario-9
- [x] scenario-10
- [x] scenario-11
- [x] scenario-12
- [x] scenario-13
- [ ] scenario-14
- [ ] scenario-15
- [ ] scenario-16
- [ ] scenario-17
- [ ] scenario-18
- [ ] scenario-19
- [ ] scenario-20

---

## Scenario 1: ParseJSON accepts valid compiled template
**ID**: scenario-1
**Testable after**: #13
**Commands**:
- Create a JSON file with all required fields (format_version, name, version, initial_state, states with directives and transitions, terminal states, variables, gates)
- Call `template.ParseJSON(jsonBytes)`
**Expected**: Returns a valid `CompiledTemplate` with all fields populated. No error returned.
**Status**: passed (2026-02-22)

---

## Scenario 2: ParseJSON rejects invalid JSON
**ID**: scenario-2
**Testable after**: #13
**Commands**:
- Call `template.ParseJSON([]byte("{not valid json"))`
**Expected**: Returns an error containing JSON parse error information.
**Status**: passed (2026-02-22)

---

## Scenario 3: ParseJSON rejects unsupported format_version
**ID**: scenario-3
**Testable after**: #13
**Commands**:
- Create valid JSON with `format_version: 99`
- Call `template.ParseJSON(jsonBytes)`
**Expected**: Returns error with message `"unsupported format version: 99"`.
**Status**: passed (2026-02-22)

---

## Scenario 4: ParseJSON rejects missing required fields
**ID**: scenario-4
**Testable after**: #13
**Commands**:
- Create JSON missing `name` field, call `ParseJSON`
- Create JSON missing `version` field, call `ParseJSON`
- Create JSON missing `initial_state` field, call `ParseJSON`
**Expected**: Each returns error with message `"missing required field: <field>"` for the respective field.
**Status**: passed (2026-02-22)

---

## Scenario 5: ParseJSON validates state machine integrity
**ID**: scenario-5
**Testable after**: #13
**Commands**:
- Create JSON where `initial_state` references a non-existent state, call `ParseJSON`
- Create JSON with empty states map, call `ParseJSON`
- Create JSON where a transition targets a non-existent state, call `ParseJSON`
- Create JSON where a state has an empty directive, call `ParseJSON`
**Expected**: Returns errors: `"initial_state %q is not a declared state"`, `"template has no states"`, `"state %q references undefined transition target %q"`, `"state %q has empty directive"` respectively.
**Status**: passed (2026-02-22)

---

## Scenario 6: ParseJSON validates gate declarations
**ID**: scenario-6
**Testable after**: #13
**Commands**:
- Create JSON with a gate of type `"unknown_type"`, call `ParseJSON`
- Create JSON with a `field_not_empty` gate missing the `field` property, call `ParseJSON`
- Create JSON with a `command` gate with empty `command` string, call `ParseJSON`
**Expected**: Returns errors: `"state %q gate %q: unknown type %q"`, `"state %q gate %q: missing required field %q"`, `"state %q gate %q: command must not be empty"` respectively.
**Status**: passed (2026-02-22)

---

## Scenario 7: CompiledTemplate builds engine.Machine with gates
**ID**: scenario-7
**Testable after**: #13
**Commands**:
- Parse a valid compiled JSON with gates on states
- Build `engine.Machine` from the parsed `CompiledTemplate`
- Inspect the resulting `Machine.States` for gates populated on `MachineState`
**Expected**: `MachineState.Gates` contains the gate declarations from the compiled template. Terminal flags and transitions are correctly set. `Machine.DeclaredVars` (or equivalent) contains declared variable names.
**Status**: passed (2026-02-22)

---

## Scenario 8: Engine.Machine() deep copy includes gates
**ID**: scenario-8
**Testable after**: #13
**Commands**:
- Create an engine with a machine that has gates on states
- Call `Engine.Machine()` to get a deep copy
- Mutate the returned copy's gates
- Call `Engine.Machine()` again
**Expected**: Second call returns unmodified gates, proving the deep copy is independent.
**Status**: passed (2026-02-22)

---

## Scenario 9: Source format compiler produces valid CompiledTemplate
**ID**: scenario-9
**Testable after**: #13, #14
**Commands**:
- Create a `.md` template source with YAML frontmatter (name, version, initial_state, variables, states with transitions/gates) and markdown body with `## heading` sections
- Call `compile.Compile(sourceBytes)`
- Validate the result by passing it through `template.ParseJSON`
**Expected**: Compiler returns a valid `CompiledTemplate`. The compiled output passes all ParseJSON validation. Directives contain the markdown content from the body sections, with leading/trailing whitespace trimmed. `format_version` is 1.
**Status**: passed (2026-02-22)

---

## Scenario 10: Compiler uses declared states for heading resolution
**ID**: scenario-10
**Testable after**: #13, #14
**Commands**:
- Create a source template where a state's directive body contains `### Decision Criteria` (a subheading that is NOT a declared state)
- Compile it
**Expected**: The `### Decision Criteria` heading is treated as directive content, not a state boundary. The compiled output includes it within the state's directive text.
**Status**: passed (2026-02-22)

---

## Scenario 11: Compiler emits warning for heading collision
**ID**: scenario-11
**Testable after**: #13, #14
**Commands**:
- Create a source template where state `assess`'s directive body contains `## plan` and `plan` is also a declared state
- Compile it
**Expected**: Compiler emits a warning: `"state %q directive contains ## heading matching state %q; is this intentional?"`. Compilation still succeeds and the heading is treated as directive content of `assess`, not the start of `plan`.
**Status**: passed (2026-02-22)

---

## Scenario 12: Compiler fails when declared state has no matching heading
**ID**: scenario-12
**Testable after**: #13, #14
**Commands**:
- Create a source template that declares a state `verify` in frontmatter but has no `## verify` heading in the markdown body
- Compile it
**Expected**: Compilation fails with a clear error indicating that the declared state has no matching section in the body.
**Status**: passed (2026-02-22)

---

## Scenario 13: Compiler produces deterministic output
**ID**: scenario-13
**Testable after**: #13, #14
**Commands**:
- Compile the same source template twice
- Compare the serialized JSON bytes from both compilations
- Compute SHA-256 hash of both outputs
**Expected**: Both compilations produce byte-identical JSON output and identical SHA-256 hashes. JSON keys are sorted.
**Status**: passed (2026-02-22)

---

## Scenario 14: Evidence accumulates across transitions and persists across rewind
**ID**: scenario-14
**Testable after**: #15
**Commands**:
- Init an engine with a multi-state machine
- Call `Transition("state2", WithEvidence(map[string]string{"key1": "val1"}))`
- Call `Transition("state3", WithEvidence(map[string]string{"key2": "val2"}))`
- Verify `Engine.Evidence()` contains both key1 and key2
- Call `Rewind("state2")`
- Verify `Engine.Evidence()` still contains both key1 and key2
**Expected**: Evidence merges across transitions (append/overwrite). Evidence persists unchanged after rewind. History entries record evidence supplied per transition.
**Status**: pending

---

## Scenario 15: Schema version backward compatibility
**ID**: scenario-15
**Testable after**: #15
**Commands**:
- Create a v1 state file (schema_version: 1, no Evidence field) on disk
- Call `engine.Load` with the v1 file
- Verify the engine initializes with an empty Evidence map
- Init a new engine and verify it creates schema_version 2
**Expected**: `Load` accepts v1 files and initializes empty evidence. New files are created with schema_version 2.
**Status**: pending

---

## Scenario 16: field_not_empty gate blocks transition when evidence is missing
**ID**: scenario-16
**Testable after**: #13, #15, #16
**Commands**:
- Create a machine with a `field_not_empty` gate on state "assess" requiring field "TASK"
- Init engine at state "assess"
- Call `Transition("plan")` without supplying evidence
- Call `Transition("plan", WithEvidence(map[string]string{"TASK": ""}))`
- Call `Transition("plan", WithEvidence(map[string]string{"TASK": "build feature"}))`
**Expected**: First two transitions fail with `gate_failed` error. Third transition succeeds because "TASK" is non-empty.
**Status**: pending

---

## Scenario 17: field_equals gate blocks transition when value doesn't match
**ID**: scenario-17
**Testable after**: #13, #15, #16
**Commands**:
- Create a machine with a `field_equals` gate requiring field "status" to equal "approved"
- Call `Transition` with evidence `{"status": "pending"}`
- Call `Transition` with evidence `{"status": "approved"}`
**Expected**: First transition fails with `gate_failed`. Second transition succeeds.
**Status**: pending

---

## Scenario 18: Namespace collision rejects evidence shadowing declared variables
**ID**: scenario-18
**Testable after**: #13, #15, #16
**Commands**:
- Create a machine with `DeclaredVars` including "TASK"
- Call `Transition("plan", WithEvidence(map[string]string{"TASK": "value"}))`
**Expected**: Transition fails with error `"evidence key \"TASK\" shadows declared variable"` before any gate evaluation runs.
**Status**: pending

---

## Scenario 19: Command gate executes shell command and blocks on failure
**ID**: scenario-19
**Testable after**: #13, #15, #16, #17
**Commands**:
- Create a machine with a `command` gate: `command: "exit 1"`
- Call `Transition` to leave that state
- Change the gate to `command: "exit 0"` and retry
**Expected**: First transition fails with `gate_failed` indicating a command gate failure. Second transition succeeds. CWD is git repo root (or process CWD if not in git repo). Command stdout/stderr are not captured in the error.
**Status**: pending

---

## Scenario 20: Command gate enforces timeout and does not interpolate variables
**ID**: scenario-20
**Testable after**: #13, #15, #16, #17
**Environment**: manual (timeout behavior may vary by system load)
**Commands**:
- Create a machine with a `command` gate: `command: "sleep 60"`, `timeout: 1`
- Call `Transition` to leave that state
- Create a machine with a `command` gate: `command: "echo {{TASK}}"` where TASK is a declared variable with value "hello"
- Call `Transition` with that gate
- Inspect the actual command that was executed
**Expected**: Timed-out command fails the gate with a timeout indication in the error message. The `{{TASK}}` placeholder in the command string is NOT expanded -- it is passed literally to `sh -c`. This is a security boundary: command strings are never interpolated.
**Status**: pending

---
