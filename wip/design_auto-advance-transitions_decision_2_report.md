<!-- decision:start id="skip-if-test-strategy" status="assumed" -->
### Decision: skip_if Test Coverage Plan

**Context**

`skip_if` is a new predicate on `TemplateState` that auto-advances deterministic state transitions within a single `advance_until_stop()` invocation. When conditions match, the engine injects synthetic evidence and calls `resolve_transition()` normally, writing a `Transitioned` event with `condition_type: "skip_if"`. The feature touches five source areas: template schema and compile-time validation (`src/template/`), the advance loop and stop-reason logic (`src/engine/advance.rs`), the event schema (`src/engine/types.rs`), and state derivation from the log (`src/engine/persistence.rs`).

Three test layers are available:

- **Gherkin functional tests** (`test/functional/features/`): human-readable end-to-end scenarios backed by `.feature` files and template fixtures. Best for communicating user-facing behavior to template authors and contributors.
- **Rust integration tests** (`tests/integration_test.rs`): full binary invocations in temp directories. Can read raw `.state.jsonl` files and assert on JSON field values. Best for log-format verification, multi-step command chaining, and edge-case behavior that is hard to express in Gherkin steps.
- **Unit tests** (`src/` modules with `#[cfg(test)]`): call internal functions directly. Best for pure Rust logic like compile-time validation and condition evaluation.

The user explicitly requested thorough testing. Two known coverage gaps also exist in the current test suite (cycle detection and chain-limit triggering), and `skip_if` is the first feature to exercise both paths.

**Assumptions**

- `skip_if` is not yet implemented. All eleven scenarios describe behavior to be written, not regression coverage of existing code.
- The synthetic `Transitioned` event gains a `skip_if_matched` field (the condition values that fired) in addition to `condition_type: "skip_if"`. This is consistent with the design decision recorded in `explore_auto-advance-transitions_decisions.md`.
- Compile-time validation errors (E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS, W-SKIP-GATE-ABSENT from Decision 1) are pure Rust functions with no binary or filesystem dependency, making unit tests both sufficient and fast.
- The existing Gherkin step definitions support `koto init --var NAME=VALUE` (confirmed by `var-substitution.feature`) and JSON field assertions on `koto next` output. No new step vocabulary is required for scenarios 1-3, 8-9.
- Integration tests can read JSONL state files using `std::fs::read_to_string` + line-by-line `serde_json::from_str`, as done implicitly in the existing `advance.rs` unit tests. No new test helpers are needed.
- Scenario 6 (cycle prevention) and scenario 11 (chain limit) cover gaps in existing coverage. They belong in integration tests alongside the existing `auto_advance_*` tests rather than Gherkin, because cycle and chain-limit scenarios are engine-internals stories rather than template-author stories.

**Chosen: Option A — Gherkin-first with selective integration tests**

Most scenarios go to Gherkin functional tests; scenarios requiring log-format inspection, cycle detection, or chain-limit verification go to Rust integration tests; compile-time validation logic goes to unit tests.

**Rationale**

Option A matches the existing project pattern: Gherkin files serve as living documentation for template authors (`gate-with-evidence-fallback.feature`, `var-substitution.feature`, `structured-gate-output.feature` all follow this pattern), while integration tests handle scenarios that require either raw JSONL inspection or complex multi-command chaining that Gherkin cannot express cleanly.

Option B (integration-test-first) would cluster most coverage in `integration_test.rs`, which already has 3,600+ lines. Adding 7-8 new integration test functions for behaviors that are readable in Gherkin would reduce discoverability for contributors. The Gherkin tests also run through the same binary as integration tests, so coverage depth is identical.

Option C (all three layers for all scenarios) would triple the test-writing effort for scenarios like "single skip_if fires" that need only a Gherkin test. The user's explicit request for thoroughness is satisfied by covering all eleven scenarios, not by testing every scenario at every layer. The compile-time validation unit tests in Option A already provide the unit-layer coverage that is genuinely valuable.

**Test Plan**

| # | Scenario | Test Type | File | Rationale |
|---|----------|-----------|------|-----------|
| 1 | Single skip_if fires | Gherkin | `test/functional/features/skip-if.feature` | Happy-path user story; readable fixture serves as template-author documentation |
| 2 | Consecutive skip_if states chain in one loop turn | Gherkin (behavior) + Integration (chain assertion) | `skip-if.feature` + `integration_test.rs` | Gherkin verifies the final stopped state; integration test asserts `advanced: true` and counts intermediate `Transitioned` events in the JSONL log |
| 3 | skip_if condition unmet — falls through to evidence blocking | Gherkin | `skip-if.feature` | Simple negative-path scenario; Gherkin steps already support asserting `action: evidence_required` |
| 4 | Resume after skip_if — log contains correct `condition_type` and `skip_if_matched` | Integration | `tests/integration_test.rs` | Requires reading raw `.state.jsonl` to verify field presence and values; only integration tests can do this |
| 5 | skip_if + gates — skip_if references gate output via `gates.NAME.exists: true` | Gherkin | `skip-if.feature` (new scenario in same file) | Gate-backed skip_if is a template-author-facing pattern; fixture template with a `context-exists` gate illustrates the workaround documented in Decision 1 |
| 6 | Cycle prevention — skip_if creating a cycle triggers `CycleDetected` | Integration | `tests/integration_test.rs` | Fills existing coverage gap; `StopReason::CycleDetected` is an engine-internal stop reason, not surfaced as a named action in the CLI JSON output, so Gherkin cannot assert it directly |
| 7 | skip_if + conditional transitions — correct branch selected based on injected synthetic evidence | Gherkin | `skip-if.feature` | Tests multi-branch routing from a single skip_if; expressible via existing JSON field assertions on the stopped state |
| 8 | skip_if + accepts — skip_if fires skips evidence; skip_if unmet prompts evidence normally | Gherkin | `skip-if.feature` | Two-scenario block in the same feature; confirms the interaction between skip_if and the `accepts` block without needing JSONL inspection |
| 9 | skip_if with vars condition — `vars.SHARED_BRANCH: {is_set: true}` fires when var set, doesn't fire when absent | Gherkin | `skip-if.feature` | Reuses `--var` init flag already exercised in `var-substitution.feature`; two scenarios (with-var / without-var) are clean Gherkin |
| 10 | Compile-time validation — E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS each produce compile error | Unit | `src/template/compile.rs` (extend existing `mod tests`) | Pure Rust logic with no binary or filesystem dependency; the existing `compile.rs` test module already tests 20+ validation rules inline; error text assertions belong here |
| 11 | Chain limit with skip_if — skip_if chains don't bypass `MAX_CHAIN_LENGTH = 100` | Integration | `tests/integration_test.rs` | Fills existing coverage gap; requires constructing a 101-state template programmatically (not feasible in a Gherkin fixture); asserts `StopReason::ChainLimitReached` via CLI output |

**New files and additions needed:**

- `test/functional/features/skip-if.feature` — new feature file (scenarios 1, 3, 5, 7, 8, 9, and partial 2)
- `test/functional/fixtures/templates/skip-if-chain.md` — fixture template: A→B→C chain, A and B have firing skip_if, C is evidence-required
- `test/functional/fixtures/templates/skip-if-gate.md` — fixture template: state with `context-exists` gate and skip_if referencing `gates.NAME.exists: true`
- `test/functional/fixtures/templates/skip-if-vars.md` — fixture template: state with `vars.SHARED_BRANCH: {is_set: true}` skip_if condition
- `tests/integration_test.rs` — add four new test functions: `skip_if_log_records_condition_type_and_matched_values`, `skip_if_cycle_detection`, `skip_if_chain_triggers_limit`, and additional assertions in the chaining scenario
- `src/template/compile.rs` (mod tests) — add three test functions: `skip_if_on_terminal_state_is_error`, `skip_if_with_no_transitions_is_error`, `skip_if_ambiguous_routing_is_error`

**Alternatives Considered**

- **Option B (integration-test-first)**: Most scenarios move to `integration_test.rs`, with Gherkin only for scenarios 1, 3, and 8. Rejected because it increases `integration_test.rs` size without improving coverage depth, reduces documentation value for template authors, and goes against the project's established pattern of using Gherkin for user-facing behavior.

- **Option C (all three layers)**: Unit + integration + Gherkin for all eleven scenarios. Rejected because it multiplies test-writing cost by 2-3x for scenarios that are fully covered by a single layer. "Thorough" means every scenario is tested, not that every scenario is tested three times. The compile-time validation scenarios are genuinely unit-test territory; the log-format scenarios are genuinely integration-test territory; the user-facing behavior scenarios are genuinely Gherkin territory. Blurring those lines adds maintenance burden without adding confidence.

**Consequences**

- One new `.feature` file with 8-10 scenarios covers the user-facing behavior that template authors need to understand.
- Three fixture templates are added to `test/functional/fixtures/templates/`. Each is self-contained and minimal (under 40 lines).
- Four new integration test functions in `integration_test.rs` cover log-format verification and the two pre-existing coverage gaps (cycle detection and chain limit) that skip_if happens to exercise.
- Three unit test functions in `compile.rs` cover the new compile-time validation rules.
- Total new test code: approximately 200-250 lines across the three layers, which is proportionate for a feature this size.
- The chain-limit integration test requires generating a template with 101 states programmatically; a helper function produces the string inline, following the pattern of `template_auto_advance_4state()` in the existing test suite.
<!-- decision:end -->
