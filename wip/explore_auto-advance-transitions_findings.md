# Exploration Findings: auto-advance-transitions

## Core Question

How should koto implement a `skip_if` predicate that auto-advances deterministic state transitions without requiring agent evidence, writes a synthetic event to preserve resume-awareness, and chains consecutive auto-advancing states within a single advance loop turn?

## Round 1

### Key Insights

- **Insertion point is well-defined** (advance-loop-integration): skip_if evaluates at step 7 of `advance_until_stop()`, after gate synthesis completes (L431) and before `resolve_transition()` is called (L464). The loop already continues implicitly when a transition resolves; no restructuring is needed to support chaining.

- **Chaining is free** (advance-loop-integration): The loop already iterates over consecutive auto-advancing states. Multiple skip_if states fire in sequence within a single `advance_until_stop()` invocation. `MAX_CHAIN_LENGTH = 100` bounds it; the `visited` set prevents cycles. Both mechanisms are already in place and apply to skip_if automatically.

- **Template vars and evidence fields are natively in scope** (condition-types): `workflow_variables: HashMap<String, String>` is extracted at loop start (L193-201) and passed to `resolve_transition()`. The `vars.NAME: {is_set: bool}` matcher already works. Evidence field matching reuses `resolve_transition()` logic -- no new evaluation code needed for those two condition types.

- **Context-exists requires a workaround, not a blocker** (condition-types): `ContextStore` is only available inside the gate-evaluator closure, not in `advance_until_stop()` directly. Template authors need a `context-exists` gate + skip_if referencing its output (`gates.NAME.exists: true`). This is mildly verbose but functional with zero architectural change.

- **Gate output synthesis has a conditional trigger** (condition-types, template-schema -- SURPRISE): Gate output is only injected into the merged evidence map when `has_gates_routing` is true, meaning at least one *transition* references a `gates.*` key. A skip_if that references `gates.*` but has only unconditional transitions won't see gate output today. The implementation must extend the `has_gates_routing` check to include skip_if references.

- **`Transitioned` with `condition_type: "skip_if"` is the right event format** (decision d1): The existing `Transitioned` event is semantically correct (a state change happened). Adding `"skip_if"` as a new `condition_type` value costs one string value and an optional `skip_if_matched` metadata field. No changes to state-derivation, epoch-scoping, or CLI output enumeration logic. Adding to `EvidenceSubmitted` would be semantically wrong ("submitted" implies agent action). A new `AutoAdvanced` event type would be cleaner semantically but requires 4+ touch-points in persistence and CLI code.

- **Transition target uses existing resolution logic** (decision d3): When skip_if fires, inject the condition key-value pairs as synthetic evidence and call `resolve_transition()` normally. For states with unconditional fallbacks, this naturally selects them. For states with conditional transitions (like `plan_validation`), the skip_if condition values (`verdict: proceed`) match the correct `when` clause. Compile-time validation enforces that exactly one transition matches -- no ambiguity.

- **Existing test coverage is solid but has gaps** (test-coverage): `auto_advance_reaches_verify_from_plan()` and `evidence_triggers_auto_advance_chain()` cover chaining. Cycle detection and `MAX_CHAIN_LENGTH` are **implemented but untested** -- a surprising gap that skip_if tests should close alongside the new feature scenarios.

### Tensions

- **Verbosity of context-exists workaround vs. threading ContextStore**: Authors needing context-key skip_if conditions must add a `context-exists` gate declaration. This is two YAML stanzas instead of one. Resolved: workaround accepted for v1. The implementation must ensure gate output synthesis fires when skip_if (not just transitions) references `gates.*` keys; without that fix, the workaround itself won't work.

- **Synthetic evidence injection vs. event log clarity**: Injecting skip_if conditions as synthetic evidence into `resolve_transition()` selects the right transition, but that synthetic evidence is NOT written as `EvidenceSubmitted`. Only the `Transitioned` event with `skip_if_matched` is written. A resuming agent sees the outcome (which state was reached and why) but not the synthetic fields in the merged-evidence view. This is acceptable because the `Transitioned.skip_if_matched` carries the same information.

### Gaps

- **Gate output synthesis extension**: The `has_gates_routing` check (L402-410 in advance.rs) must be extended to scan skip_if condition keys for `gates.*` references, not just transition `when` clauses. This is an implementation detail discovered during research that wasn't in the original issue.

- **Compile-time validation**: The template compiler must add a new validation step for skip_if states: exactly one transition must be reachable when skip_if fires (either unconditional fallback exists, or exactly one conditional transition matches the skip_if values). No compile-time validation for this exists today.

- **Cycle detection and chain-limit tests are absent**: These will be partially addressed by skip_if tests but warrant dedicated coverage regardless of this feature.

### Decisions

- **Synthetic event format**: Use `Transitioned` event with `condition_type: "skip_if"` and optional `skip_if_matched` metadata field. Rationale: minimal schema change, reuses existing state-derivation logic, semantically correct.

- **Context-exists in v1**: Defer direct context-key predicates. Require `context-exists` gate + skip_if referencing gate output. Rationale: workaround is functional; ContextStore threading adds scope without enabling new capabilities.

- **Transition target selection**: Re-run `resolve_transition()` with skip_if condition values injected as synthetic evidence. Compile-time validation enforces exactly-one-match. Rationale: reuses existing resolution logic, avoids redundant unconditional-fallback-only constraint, cleanly handles `plan_validation` pattern.

### User Focus

Operating in --auto mode. Decisions made autonomously based on research evidence. The reporter's suggestion to treat `skip_if` as the primary primitive (over `auto_advance`) is accepted -- more general, covers all three condition types. The reporter's assertion that chaining is required for 80% of the value is confirmed: the advance loop already supports it, making chaining essentially free to implement.

## Accumulated Understanding

The implementation surface is well-understood. The skip_if predicate is an optional `BTreeMap<String, Value>` added to `TemplateState`, using the same syntax as transition `when` clauses. It evaluates after gate synthesis but before transition resolution in `advance_until_stop()`. When conditions are satisfied, skip_if injects its condition values as synthetic evidence, calls `resolve_transition()` normally to select a target, appends a `Transitioned` event with `condition_type: "skip_if"` and `skip_if_matched` metadata, and continues the loop (chaining naturally).

The main implementation constraints are:
1. Extend `has_gates_routing` detection to include skip_if `gates.*` references
2. Add compile-time validation for skip_if transition coverage
3. Add `skip_if: Option<BTreeMap<String, Value>>` to `TemplateState` and `SourceState`
4. Extend `Transitioned` event with optional `skip_if_matched` field
5. Insert skip_if evaluation block in `advance_until_stop()` after L431

The reporter's concern about chaining delivering 80% of the value is addressed: chaining is free from the engine's perspective. The feature is ready for a design doc that captures the full implementation plan, template schema, event schema changes, and test scenarios.

## Decision: Crystallize
