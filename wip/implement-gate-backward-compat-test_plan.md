# Test Plan: Gate backward compatibility

Generated from: docs/plans/PLAN-gate-backward-compat.md
Issues covered: 3

---

## Infrastructure scenarios

These scenarios validate compile-time correctness and unit-level behavior. They are automatable and run in CI via `cargo test`.

---

## Scenario 1: validate() accepts strict parameter — strict mode errors on legacy-gate template

**ID**: scenario-1
**Category**: infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto -- template::types::tests::strict_mode_rejects_legacy_gate`
**Expected**: Unit test passes. `validate(strict=true)` returns `Err` containing the state name ("verify" or equivalent), the gate name, and a hint mentioning `--allow-legacy-gates` when a state has gates but no `gates.*` when-clause references.
**Status**: pending

---

## Scenario 2: validate() permissive mode warns and returns Ok on legacy-gate template

**ID**: scenario-2
**Category**: infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto -- template::types::tests::permissive_mode_warns_legacy_gate`
**Expected**: Unit test passes. `validate(strict=false)` returns `Ok(())` and writes a warning to stderr containing the state name and gate name with "(legacy behavior)" for the same template that strict mode rejects.
**Status**: pending

---

## Scenario 3: validate() passes for a no-gate template in both strict and permissive modes

**ID**: scenario-3
**Category**: infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto -- template::types::tests::no_gate_template_passes_both_modes`
**Expected**: Unit test passes. `validate(strict=true)` and `validate(strict=false)` both return `Ok(())` for a template with no gates defined.
**Status**: pending

---

## Scenario 4: D4 reachability warnings suppressed in permissive mode

**ID**: scenario-4
**Category**: infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto -- template::types::tests::d4_suppressed_in_permissive_mode`
**Expected**: Unit test passes. `validate_gate_reachability()` returns `Ok(())` early when `strict=false` without iterating gate schema fields, producing no per-field warnings to stderr.
**Status**: pending

---

## Scenario 5: compile() passes strict parameter through to validate()

**ID**: scenario-5
**Category**: infrastructure
**Testable after**: Issue 1
**Commands**:
- `cargo test -p koto -- template::compile::tests`
**Expected**: All compile-layer unit tests pass. `compile(strict=true)` propagates the `Err` from `validate()` when the template has legacy gates; `compile(strict=false)` returns `Ok`.
**Status**: pending

---

## Scenario 6: advance loop — legacy state evidence map contains no gates.* key

**ID**: scenario-6
**Category**: infrastructure
**Testable after**: Issue 3
**Commands**:
- `cargo test -p koto -- engine::advance::tests::legacy_state_no_gates_evidence`
**Expected**: Unit test passes. When a state has gates but no `gates.*` when-clause references (`has_gates_routing = false`), the merged evidence map passed to `resolve_transition` contains no key named "gates". Gate results are still computed (available for events and `blocking_conditions`), but not injected into the resolver map.
**Status**: pending

---

## Scenario 7: advance loop — structured-mode state evidence map contains gates.* key

**ID**: scenario-7
**Category**: infrastructure
**Testable after**: Issue 3
**Commands**:
- `cargo test -p koto -- engine::advance::tests::structured_state_gates_evidence_present`
**Expected**: Unit test passes. When a state has gates and at least one `gates.*` when-clause reference (`has_gates_routing = true`), the merged evidence map passed to `resolve_transition` contains the "gates" key with gate output, preserving existing behavior.
**Status**: pending

---

## Scenario 8: has_gates_routing initialized before gate evaluation loop (compile check)

**ID**: scenario-8
**Category**: infrastructure
**Testable after**: Issue 3
**Commands**:
- `cargo build`
- `cargo test`
**Expected**: Project compiles without errors. The `has_gates_routing` boolean is in scope at the evidence merge step regardless of whether `any_failed` is true, meaning the guard `if !gate_evidence_map.is_empty() && has_gates_routing` compiles correctly.
**Status**: pending

---

## Scenario 9: --allow-legacy-gates flag present on koto template compile

**ID**: scenario-9
**Category**: infrastructure
**Testable after**: Issue 2
**Commands**:
- `koto template compile --help`
**Expected**: Help output lists `--allow-legacy-gates` as an available flag. A TODO comment referencing the shirabe migration appears in the source at the flag's definition site.
**Status**: pending

---

## Scenario 10: Full test suite passes after all changes

**ID**: scenario-10
**Category**: infrastructure
**Testable after**: Issues 1, 2, 3
**Commands**:
- `cargo test`
- `go test ./... -v` (from `test/functional/`)
**Expected**: All unit and functional tests pass. No regressions in existing gate behavior (structured-gate-output, gate-with-evidence-fallback, mixed-gate-routing scenarios).
**Status**: pending

---

## Use-case scenarios

These scenarios validate the feature from a user's perspective with real binary invocations. They are automatable via Gherkin in `test/functional/features/` and run in CI.

---

## Scenario 11: koto init with a legacy-gate template exits 0 and emits warning to stderr

**ID**: scenario-11
**Category**: use-case (automatable)
**Testable after**: Issues 1, 2
**Fixture required**: `test/functional/fixtures/templates/legacy-gates.md` — a template with a state that has a gate but no `gates.*` when-clause references (only plain `accepts`-based transitions, matching the known shirabe work-on template pattern)

**Proposed fixture** (`legacy-gates.md`):
```yaml
---
name: legacy-gates
version: "1.0"
description: Template using legacy boolean gate behavior
initial_state: verify

states:
  verify:
    gates:
      ci_check:
        type: command
        command: "true"
    accepts:
      status:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: complete
        when:
          status: done
      - target: complete
  complete:
    terminal: true
---

## verify

Legacy gate: boolean pass/block only. No gates.* routing.

## complete

Done.
```

**Proposed Gherkin** (`test/functional/features/legacy-gate-backward-compat.feature`):
```gherkin
Feature: Legacy gate backward compatibility

  Scenario: koto init with a legacy-gate template exits 0 and emits warning
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto init test-wf --template .koto/templates/legacy-gates.md"
    Then the exit code is 0
    And the error output contains "legacy behavior"
```

**Expected**: `koto init` exits 0. stderr contains a warning like `warning: state "verify" gate "ci_check" has no gates.* routing (legacy behavior)`. No D4 per-field warnings appear. The workflow state file is created and the initial state is reachable.
**Status**: pending

---

## Scenario 12: koto template compile on a legacy-gate template without flag exits nonzero with actionable error

**ID**: scenario-12
**Category**: use-case (automatable)
**Testable after**: Issues 1, 2
**Fixture required**: `legacy-gates.md` (same as scenario 11)

**Proposed Gherkin** (add to `legacy-gate-backward-compat.feature`):
```gherkin
  Scenario: koto template compile on a legacy-gate template without flag exits nonzero
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto template compile .koto/templates/legacy-gates.md"
    Then the exit code is not 0
    And the output contains "gates.* routing"
    And the output contains "--allow-legacy-gates"
```

**Expected**: `koto template compile` exits nonzero. The error message names the state ("verify") and gate ("ci_check"), states that no `gates.*` routing is present, and tells the user to add a `when` clause referencing `gates.ci_check.*` or pass `--allow-legacy-gates`.
**Status**: pending

---

## Scenario 13: koto template compile --allow-legacy-gates on a legacy-gate template exits 0 cleanly

**ID**: scenario-13
**Category**: use-case (automatable)
**Testable after**: Issues 1, 2
**Fixture required**: `legacy-gates.md` (same as scenario 11)

**Proposed Gherkin** (add to `legacy-gate-backward-compat.feature`):
```gherkin
  Scenario: koto template compile --allow-legacy-gates exits 0 with no error output
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto template compile --allow-legacy-gates .koto/templates/legacy-gates.md"
    Then the exit code is 0
```

**Expected**: `koto template compile --allow-legacy-gates` exits 0. No error output. No D4 per-field warnings. The template compiles successfully.
**Status**: pending

---

## Scenario 14: Legacy state at runtime — gates pass/block but gates.* keys absent from resolver evidence

**ID**: scenario-14
**Category**: use-case (automatable)
**Testable after**: Issues 1, 2, 3
**Fixture required**: `legacy-gates.md` with the gate command set to `true` (always passes)

**Proposed Gherkin** (add to `legacy-gate-backward-compat.feature`):
```gherkin
  Scenario: Legacy state gate passes and auto-advances without gates.* evidence
    Given a clean koto environment
    And the template "legacy-gates" exists
    And I run "koto init test-wf --template .koto/templates/legacy-gates.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "complete"
    And the JSON output field "advanced" is true
```

**Expected**: When the legacy gate passes, the advance loop auto-advances to the terminal state. The JSON response does not contain `gates.*` keys in any evidence or condition fields — gate output was not injected into the resolver evidence map. `GateEvaluated` events still fire normally.

**Note**: The absence of `gates.*` in resolver evidence is covered by the unit test in scenario 6. This functional scenario validates the end-to-end advance behavior is unaffected for passing gates.
**Status**: pending

---

## Scenario 15: Structured-mode state — gates.* keys present in resolver evidence as before

**ID**: scenario-15
**Category**: use-case (automatable)
**Testable after**: Issues 1, 2, 3
**Fixture required**: `structured-routing.md` (already exists in `test/functional/fixtures/templates/`)

**Verification**: Existing `structured-gate-output.feature` scenarios (scenarios 14 and 15 in that file) cover this path. Re-running them after Issue 3 confirms that structured routing is unaffected by the `has_gates_routing` guard.

**Commands**:
- Run the existing functional suite: `make test-functional` or `go test ./...` from `test/functional/`

**Expected**: All existing structured-gate-output and structured-routing scenarios continue to pass. `gates.*` evidence keys are present in the resolver map for states with `gates.*` when-clause references, as before the engine change.
**Status**: pending

---

## Fixture requirements

The following new fixture is required for scenarios 11–14:

| Fixture | Path | Purpose |
|---------|------|---------|
| `legacy-gates.md` | `test/functional/fixtures/templates/legacy-gates.md` | Template with a gate state using only `accepts`-based routing (no `gates.*` when-clause references) |

The fixture must be created as part of Issue 2's implementation. Its gate command should be `true` (always exits 0) so that scenario 14 can exercise the auto-advance path without external file dependencies.

---

## Scenario summary

| ID | Description | Category | Testable after |
|----|-------------|----------|----------------|
| scenario-1 | `validate(strict=true)` errors on legacy-gate template | infrastructure | Issue 1 |
| scenario-2 | `validate(strict=false)` warns and returns Ok | infrastructure | Issue 1 |
| scenario-3 | No-gate template passes in both modes | infrastructure | Issue 1 |
| scenario-4 | D4 warnings suppressed in permissive mode | infrastructure | Issue 1 |
| scenario-5 | `compile()` passes strict parameter through | infrastructure | Issue 1 |
| scenario-6 | Legacy state evidence map has no `gates.*` key | infrastructure | Issue 3 |
| scenario-7 | Structured state evidence map has `gates.*` key | infrastructure | Issue 3 |
| scenario-8 | `has_gates_routing` hoisted, compiles cleanly | infrastructure | Issue 3 |
| scenario-9 | `--allow-legacy-gates` flag present in help output | infrastructure | Issue 2 |
| scenario-10 | Full test suite passes with no regressions | infrastructure | Issues 1, 2, 3 |
| scenario-11 | `koto init` legacy-gate template exits 0, warns stderr | use-case | Issues 1, 2 |
| scenario-12 | `koto template compile` legacy-gate without flag exits nonzero | use-case | Issues 1, 2 |
| scenario-13 | `koto template compile --allow-legacy-gates` exits 0 cleanly | use-case | Issues 1, 2 |
| scenario-14 | Legacy state runtime: gate passes, no `gates.*` in evidence | use-case | Issues 1, 2, 3 |
| scenario-15 | Structured state runtime: `gates.*` evidence present as before | use-case | Issues 1, 2, 3 |
