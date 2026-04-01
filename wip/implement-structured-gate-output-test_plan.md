# Test Plan: Structured gate output

Generated from: docs/plans/PLAN-structured-gate-output.md
Issues covered: 5

---

## Scenario 1: GateOutcome and StructuredGateResult serialization round-trip

**ID**: scenario-1
**Category**: Infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto gate::tests -- --nocapture` (unit test added as part of Issue 1)
**Expected**: `GateOutcome` variants (Passed, Failed, TimedOut, Error) and `StructuredGateResult` serialize to JSON and deserialize back without loss. `serde_json::to_value` and `serde_json::from_value` round-trip successfully for each variant.
**Status**: passed (2026-04-01)

---

## Scenario 2: Command gate produces structured output on success

**ID**: scenario-2
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::passing_gate -- --nocapture`
**Expected**: `evaluate_command_gate` for `exit 0` returns `StructuredGateResult { outcome: GateOutcome::Passed, output: {"exit_code": 0, "error": ""} }`. The `output` field is a `serde_json::Value::Object` with both keys present.
**Status**: passed (2026-04-01)

---

## Scenario 3: Command gate produces structured output on failure

**ID**: scenario-3
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::failing_gate -- --nocapture`
**Expected**: `evaluate_command_gate` for `exit 42` returns `StructuredGateResult { outcome: GateOutcome::Failed, output: {"exit_code": 42, "error": ""} }`. The `error` field is an empty string, not absent.
**Status**: passed (2026-04-01)

---

## Scenario 4: Command gate produces structured output on timeout

**ID**: scenario-4
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::timed_out_gate -- --nocapture`
**Expected**: `evaluate_command_gate` for a long-running command with a 1s timeout returns `StructuredGateResult { outcome: GateOutcome::TimedOut, output: {"exit_code": -1, "error": "timed_out"} }`. Schema shape is identical to the success case.
**Status**: passed (2026-04-01)

---

## Scenario 5: Command gate produces structured output on spawn error

**ID**: scenario-5
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::error_gate -- --nocapture` (new test for spawn error case)
**Expected**: `evaluate_command_gate` for a command that errors at spawn (not shell 127) returns `StructuredGateResult { outcome: GateOutcome::Error, output: {"exit_code": -1, "error": "<message>"} }` where `error` is non-empty.
**Status**: passed (2026-04-01)

---

## Scenario 6: Context-exists gate produces structured output

**ID**: scenario-6
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::context_exists_gate -- --nocapture`
**Expected**: `evaluate_context_exists_gate` returns `{"exists": true, "error": ""}` with `outcome: Passed` when key is present, and `{"exists": false, "error": ""}` with `outcome: Failed` when key is absent. The `error` field is always present.
**Status**: passed (2026-04-01)

---

## Scenario 7: Context-matches gate produces structured output

**ID**: scenario-7
**Category**: Infrastructure
**Testable after**: Issue 2
**Commands**:
- `cargo test -p koto gate::tests::context_matches_gate -- --nocapture`
**Expected**: `evaluate_context_matches_gate` returns `{"matches": true, "error": ""}` with `outcome: Passed` when the pattern matches, and `{"matches": false, "error": ""}` with `outcome: Failed` when it does not. An invalid regex returns `outcome: Error` with a non-empty `error` field.
**Status**: passed (2026-04-01)

---

## Scenario 8: resolve_value traverses nested dot-paths

**ID**: scenario-8
**Category**: Infrastructure
**Testable after**: Issue 3
**Commands**:
- `cargo test -p koto engine::advance::tests::resolve_value -- --nocapture`
**Expected**: `resolve_value(root, "a.b.c")` returns `Some(&value)` for `{"a": {"b": {"c": 42}}}`. `resolve_value(root, "flat_key")` returns the same result as `root.get("flat_key")`. `resolve_value` returns `None` for missing path segments, empty string paths, and non-object intermediates (e.g., path through a string value).
**Status**: passed (2026-04-01)

---

## Scenario 9: resolve_transition uses dot-path traversal for gates.* conditions

**ID**: scenario-9
**Category**: Infrastructure
**Testable after**: Issue 3
**Commands**:
- `cargo test -p koto engine::advance::tests::resolve_transition -- --nocapture`
**Expected**: `resolve_transition` called with a `serde_json::Value::Object` containing `{"gates": {"ci_check": {"exit_code": 0, "error": ""}}}` matches a transition `when: { gates.ci_check.exit_code: 0 }`. Flat evidence keys (`"mode": "issue_backed"`) still match their `when` conditions without dot-path traversal overhead.
**Status**: passed (2026-04-01)

---

## Scenario 10: Advance loop injects gate output into evidence map

**ID**: scenario-10
**Category**: Infrastructure
**Testable after**: Issues 2, 3, 4
**Commands**:
- `cargo test -p koto engine::advance::tests -- --nocapture`
**Expected**: After gate evaluation, the merged evidence map passed to `resolve_transition` has shape `{"gates": {"gate_name": {...gate_output...}}, ...agent_evidence...}`. Gate data is nested under `"gates"`, not flattened. Agent evidence keys appear at the top level. Gate output appears after agent evidence merge, so engine data takes precedence on key collision.
**Status**: passed (2026-04-01)

---

## Scenario 11: any_failed derived from GateOutcome, not legacy GateResult

**ID**: scenario-11
**Category**: Infrastructure
**Testable after**: Issues 2, 4
**Commands**:
- `cargo test -p koto engine::advance::tests::gate_pass_fail_from_outcome -- --nocapture`
**Expected**: A gate with `outcome: GateOutcome::Passed` does not contribute to `any_failed`. Gates with `Failed`, `TimedOut`, or `Error` outcomes do. The `gate_failed` boolean passed to `resolve_transition` correctly reflects whether any gate did not pass.
**Status**: passed (2026-04-01)

---

## Scenario 12: blocking_conditions_from_gates includes structured output

**ID**: scenario-12
**Category**: Infrastructure
**Testable after**: Issues 2, 5
**Commands**:
- `cargo test -p koto cli::next_types::tests::blocking_conditions -- --nocapture`
**Expected**: `blocking_conditions_from_gates` called with `StructuredGateResult` values produces `BlockingCondition` entries that include the gate name and the full structured `output` field (e.g., `{"exit_code": 1, "error": ""}`). Passed gates are excluded. The output field is not omitted or summarized.
**Status**: pending

---

## Scenario 13: Gate-blocked koto next response contains structured output in blocking_conditions

**ID**: scenario-13
**Category**: Use-case
**Testable after**: Issues 2, 4, 5
**Commands**:
- Use the `simple-gates` fixture template (or a new structured-gates fixture)
- `koto init test-wf --template .koto/templates/structured-gates.md`
- `koto next test-wf`
**Expected**: When `koto next` is called on a state with a failing command gate, the JSON response has `action: "gate_blocked"` and `blocking_conditions` is a non-empty array. Each entry includes `gate` (the gate name) and `output` as a JSON object with `exit_code` and `error` fields. The `output` field is not a string -- it is a nested JSON object.

Example expected shape:
```json
{
  "action": "gate_blocked",
  "state": "check",
  "blocking_conditions": [
    {"gate": "ci_check", "output": {"exit_code": 1, "error": ""}}
  ]
}
```
**Status**: pending

---

## Scenario 14: Gate passes and auto-advances based on gates.* when clause

**ID**: scenario-14
**Category**: Use-case
**Testable after**: Issues 2, 3, 4, 5
**Commands**:
- Create a template with a command gate and transitions routing on `gates.<name>.exit_code`
- `koto init test-wf --template .koto/templates/structured-routing.md`
- Set up conditions so the gate passes (exit code 0)
- `koto next test-wf`
**Expected**: When a command gate passes (`exit_code: 0`) and the template has `when: { gates.ci_check.exit_code: 0 }` targeting a next state, `koto next` returns `action: "done"` with `advanced: true` and the workflow moves to the target state. No agent evidence is needed -- gate output alone drives routing.

This validates the core user story from the PRD (Example 1): gate output feeds into transition routing automatically.
**Status**: pending

---

## Scenario 15: Gate fails and routes to a different state based on gates.* exit_code

**ID**: scenario-15
**Category**: Use-case
**Testable after**: Issues 2, 3, 4, 5
**Commands**:
- Same structured-routing fixture template as scenario-14
- Configure the gate command to exit 1
- `koto next test-wf`
**Expected**: When a command gate fails (`exit_code: 1`) and the template has `when: { gates.ci_check.exit_code: 1 }` targeting a "fix" state, `koto next` automatically routes to the "fix" state and returns `action: "done"` with `advanced: true`. No manual override or evidence submission is required -- the gate's structured output drives the route selection.

This validates the automatic routing-on-failure path from PRD Example 1.
**Status**: pending

---

## Scenario 16: Gate output and agent evidence coexist in when clause matching

**ID**: scenario-16
**Category**: Use-case
**Testable after**: Issues 2, 3, 4, 5
**Commands**:
- Create a template with a command gate and an `accepts` block (both gate output and agent evidence used in the same `when` clause)
- `koto init test-wf --template .koto/templates/mixed-routing.md`
- Gate passes (exit 0)
- `koto next test-wf --with-data '{"decision": "approve"}'`
**Expected**: A transition `when: { gates.lint.exit_code: 0, decision: approve }` resolves when both the gate output and agent evidence match. If either does not match, the transition is not selected. This validates PRD Example 2: gate data and agent evidence coexist in the same resolver call.
**Status**: pending

---

## Scenario 17: Backward compatibility -- existing templates without gates.* when clauses

**ID**: scenario-17
**Category**: Use-case
**Testable after**: Issues 2, 3, 4, 5
**Commands**:
- `koto init test-wf --template .koto/templates/simple-gates.md` (existing fixture)
- Run the existing `gate-with-evidence-fallback.feature` scenarios
**Expected**: All three scenarios in `gate-with-evidence-fallback.feature` continue to pass without modification:
1. Gate passes and auto-advances (no `gates.*` in when clause -- uses unconditional transition)
2. Gate fails and evidence is required
3. Gate fails then evidence advances

The existing `simple-gates.md` template uses `when: { status: completed }` (flat agent evidence, no `gates.*` key). These flat conditions must work identically after the dot-path resolver is added.
**Status**: pending

---

## Scenario 18: Context-exists gate blocked response with structured output

**ID**: scenario-18
**Category**: Use-case
**Testable after**: Issues 2, 4, 5
**Commands**:
- Create a template with a `context-exists` gate
- `koto init test-wf --template .koto/templates/context-gate.md`
- Do not set the required context key
- `koto next test-wf`
**Expected**: The `koto next` response includes `blocking_conditions` with an entry whose `output` field is `{"exists": false, "error": ""}`. The `action` is `"gate_blocked"`. The structured output confirms the gate type's schema (exists + error) rather than a generic status string.
**Status**: pending

---

## Notes

**Fixture template needed for scenarios 13-16, 18**: A new template (e.g., `structured-gates.md`) that uses `gates.*` dot-paths in `when` clauses does not exist yet. The implementer must add this fixture as part of Issue 5's acceptance criteria ("New template fixture demonstrating structured gate output routing"). Scenarios 13-18 are blocked on this fixture being created alongside the implementation.

**Step vocabulary**: The functional test steps already support `the JSON output has field "<dotted.path>"` and `the JSON output field "<dotted.path>" equals "<value>"` (dotted path navigation is implemented in `getJSONField`). New `.feature` files for scenarios 13-18 can use these steps to assert `blocking_conditions.0.output.exit_code` style paths.

**Automatable scenarios**: 1-12 (unit tests in Rust, runnable via `cargo test`), 17 (existing `.feature` scenarios in CI).

**Environment-dependent scenarios**: 13-16, 18 require the new fixture template and new `.feature` files to be committed alongside the implementation. They run in CI via `make test-functional` once the template fixture exists.
