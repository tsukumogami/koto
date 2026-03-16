# Test Plan: Template Evidence Routing

Generated from: docs/plans/PLAN-template-evidence-routing.md
Issues covered: 5

---

## Scenario 1: Structured transition compilation
**ID**: scenario-1
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with `transitions: [{target: done}]` (structured syntax, no `when`)
- `cargo test` (unit + compiler tests)
- `koto template compile <template-path>`
**Expected**: Template compiles without error. Compiled JSON contains `transitions` as an array of objects with `target` field. `koto template compile` outputs the cache path and exits 0.
**Status**: pending

---

## Scenario 2: Flat transition syntax rejected after migration
**ID**: scenario-2
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template using old flat syntax: `transitions: [done]`
- `koto template compile <template-path>`
**Expected**: Compilation fails with a deserialization error because `transitions` now expects objects, not bare strings. Exit code is non-zero with a JSON error message.
**Status**: pending

---

## Scenario 3: Accepts block compiles and validates
**ID**: scenario-3
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with an `accepts` block declaring an enum field with `type: enum`, `values: [proceed, escalate]`, `required: true`
- `koto template compile <template-path>`
- Read the compiled JSON and inspect the state's `accepts` field
**Expected**: Compiled JSON includes `accepts` with the declared field schema. Field has `field_type: "enum"`, `required: true`, and `values: ["proceed", "escalate"]`.
**Status**: pending

---

## Scenario 4: When conditions compile and route transitions
**ID**: scenario-4
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with two transitions from the same state, each with mutually exclusive `when` conditions on a shared enum field (e.g., `when: {decision: proceed}` and `when: {decision: escalate}`)
- `koto template compile <template-path>`
**Expected**: Compilation succeeds. Compiled JSON has two transitions with `target` and `when` fields correctly populated.
**Status**: pending

---

## Scenario 5: Mutual exclusivity violation rejected
**ID**: scenario-5
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with two transitions from the same state where `when` conditions share no fields (e.g., `when: {decision: proceed}` and `when: {priority: high}`)
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error indicating the transitions are not provably mutually exclusive. Exit code is non-zero.
**Status**: pending

---

## Scenario 6: Duplicate when conditions rejected
**ID**: scenario-6
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with two transitions that have identical `when` conditions (e.g., both `when: {decision: proceed}`)
- `koto template compile <template-path>`
**Expected**: Compilation fails because the transitions are not mutually exclusive (all shared fields have the same value). Exit code is non-zero.
**Status**: pending

---

## Scenario 7: When condition references undeclared accepts field
**ID**: scenario-7
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template where a transition's `when` references a field not in the state's `accepts` block
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error about the `when` field not being declared in `accepts`. Exit code is non-zero.
**Status**: pending

---

## Scenario 8: When condition without accepts block rejected
**ID**: scenario-8
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template where a state has no `accepts` block but a transition uses `when`
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error indicating `when` conditions require an `accepts` block. Exit code is non-zero.
**Status**: pending

---

## Scenario 9: Empty when block rejected
**ID**: scenario-9
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template where a transition has `when: {}` (empty map)
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error about empty `when` blocks. Exit code is non-zero.
**Status**: pending

---

## Scenario 10: When value for enum field not in values list
**ID**: scenario-10
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with `accepts: {decision: {type: enum, values: [proceed, escalate]}}` and `when: {decision: invalid_value}`
- `koto template compile <template-path>`
**Expected**: Compilation fails because `invalid_value` is not in the enum's `values` list. Exit code is non-zero.
**Status**: pending

---

## Scenario 11: Non-scalar when values rejected
**ID**: scenario-11
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template where a `when` condition value is an array (e.g., `when: {decision: [a, b]}`)
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error about non-scalar values in `when` conditions. Exit code is non-zero.
**Status**: pending

---

## Scenario 12: Invalid field_type rejected
**ID**: scenario-12
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with `accepts: {decision: {type: invalid_type}}`
- `koto template compile <template-path>`
**Expected**: Compilation fails because `invalid_type` is not one of: enum, string, number, boolean. Exit code is non-zero.
**Status**: pending

---

## Scenario 13: Enum field without values list rejected
**ID**: scenario-13
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with `accepts: {decision: {type: enum}}` (no `values` list)
- `koto template compile <template-path>`
**Expected**: Compilation fails because enum-typed fields must have a non-empty `values` list. Exit code is non-zero.
**Status**: pending

---

## Scenario 14: field_not_empty gate rejected with migration hint
**ID**: scenario-14
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with a gate of `type: field_not_empty`
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error that mentions `accepts`/`when` as the replacement. Exit code is non-zero.
**Status**: pending

---

## Scenario 15: field_equals gate rejected with migration hint
**ID**: scenario-15
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with a gate of `type: field_equals`
- `koto template compile <template-path>`
**Expected**: Compilation fails with an error that mentions `accepts`/`when` as the replacement. Exit code is non-zero.
**Status**: pending

---

## Scenario 16: Command gate still works alongside accepts/when
**ID**: scenario-16
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with a state that has both a command gate and an `accepts`/`when` block
- `koto template compile <template-path>`
**Expected**: Compilation succeeds. Both the gate and the `accepts`/`when` constructs appear in the compiled JSON.
**Status**: pending

---

## Scenario 17: Integration field passes through compilation
**ID**: scenario-17
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template with `integration: delegate_review` on a state
- `koto template compile <template-path>`
- Read the compiled JSON
**Expected**: Compiled JSON has `integration: "delegate_review"` on the corresponding state. No validation error for integration name.
**Status**: pending

---

## Scenario 18: koto next preserves flat transition output
**ID**: scenario-18
**Testable after**: Issue 1, Issue 2, Issue 3
**Category**: infrastructure
**Commands**:
- Write a template with structured transitions (including `when` conditions)
- `koto init wf --template <template-path>`
- `koto next wf`
**Expected**: Output JSON has `transitions` as a flat array of target name strings (e.g., `["deploy", "escalate_review"]`), not structured objects. No `accepts`, `when`, or `integration` fields appear in the output.
**Status**: pending

---

## Scenario 19: Unconditional transitions alongside accepts
**ID**: scenario-19
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Write a template where a state has `accepts` but some transitions have no `when` condition (unconditional fallback)
- `koto template compile <template-path>`
**Expected**: Compilation succeeds. The unconditional transition has no `when` field in the compiled JSON.
**Status**: pending

---

## Scenario 20: hello-koto plugin template compiles after migration
**ID**: scenario-20
**Testable after**: Issue 2, Issue 5
**Category**: infrastructure
**Commands**:
- `koto template compile plugins/koto-skills/skills/hello-koto/hello-koto.md`
**Expected**: Template compiles without error. Transitions in compiled JSON use structured format with `target` field.
**Status**: pending

---

## Scenario 21: Full evidence routing workflow end-to-end
**ID**: scenario-21
**Testable after**: Issue 1, Issue 2, Issue 3, Issue 4
**Category**: use-case
**Commands**:
- Write the review-workflow template from the design doc (with `accepts`, `when`, `integration`, command gates)
- `koto template compile review-workflow.md` -- verify compilation
- `koto init review-wf --template review-workflow.md` -- start workflow
- `koto next review-wf` -- verify output has flat transitions `["deploy", "escalate_review"]`, directive text, state name, and no evidence routing internals
**Expected**: The full lifecycle works: compilation validates mutual exclusivity and field references, initialization creates a state file, and `koto next` returns the directive with flat transition targets. The template is self-describing (accepts block captures what evidence the agent should submit) even though the CLI doesn't expose it yet (deferred to issue 48).
**Status**: pending

---

## Scenario 22: Existing test suite passes after fixture migration
**ID**: scenario-22
**Testable after**: Issue 1, Issue 2, Issue 3, Issue 4
**Category**: infrastructure
**Commands**:
- `cargo test`
**Expected**: All existing unit tests and integration tests pass after all fixtures have been migrated to structured transition syntax. Zero regressions.
**Status**: pending

---

## Scenario 23: Compiled JSON round-trips with new fields
**ID**: scenario-23
**Testable after**: Issue 1, Issue 2
**Category**: infrastructure
**Commands**:
- Compile a template with `accepts`, `when`, and `integration`
- Serialize the compiled template to JSON
- Deserialize the JSON back to `CompiledTemplate`
- Compare the two
**Expected**: Round-trip produces identical structures. `accepts`, `when`, and `integration` fields survive serialization and deserialization.
**Status**: pending

---

## Scenario 24: Multi-field when conditions with AND semantics
**ID**: scenario-24
**Testable after**: Issue 1, Issue 2
**Category**: use-case
**Commands**:
- Write a template with a state that accepts two fields (`decision` and `priority`) and has transitions with multi-field `when` conditions: `when: {decision: proceed, priority: high}` vs `when: {decision: proceed, priority: low}` vs `when: {decision: escalate}`
- `koto template compile <template-path>`
**Expected**: Compilation succeeds. The first two transitions are exclusive on `priority`. The third is exclusive from both on `decision`. This validates the pairwise exclusivity algorithm handles multi-field AND conditions correctly.
**Status**: pending
