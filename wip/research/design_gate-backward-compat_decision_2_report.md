<!-- decision:start id="gate-legacy-evidence-exclusion" status="confirmed" -->
### Decision: Gate output exclusion from legacy-mode resolver evidence

**Context**

The advance loop in `src/engine/advance.rs` evaluates gates and builds a `gate_evidence_map`. It currently merges this map under the `"gates"` key into the resolver evidence for every state that has gates, regardless of whether any `when` clause references `gates.*` fields.

For legacy states — those where no `when` clause references `gates.*` — this data is never used. The pure-legacy path (no `accepts` block) returns `GateBlocked` before the merge is constructed. The legacy-with-accepts path reaches the resolver but no condition matches on `gates.*`, so the injected output is inert.

PRD R10 acceptance criterion states: "Gates on states where no `when` clause references `gates.*` fields produce no structured output (legacy boolean behavior)." The DESIGN-gate-backward-compat.md decision driver states explicitly: "gate output should not enter the resolver's evidence map for legacy states."

**Assumptions**

None. The code path and PRD requirements are unambiguous.

**Chosen: Exclude gate output from merged evidence for legacy states**

For states where `has_gates_routing` is false (no `when` clause references `gates.*`), gate output is not inserted into the merged evidence map passed to `resolve_transition`. Gate output is still collected and used for `GateEvaluated` events and `blocking_conditions` in `StopReason::GateBlocked` — neither of those uses the merged evidence map. The `"gates"` key simply does not appear in the resolver's evidence for legacy states.

The implementation guard uses `has_gates_routing`, a flag already computed earlier in the same gate evaluation block (lines 395-403 of advance.rs). No new logic is needed; it's a matter of applying the existing flag to control whether gate output enters the evidence merge.

**Rationale**

Both alternatives produce identical observable behavior. The PRD and design doc unambiguously require Option A. Choosing Option B (keep current behavior) leaves the code in a state that contradicts the acceptance criterion and creates a confusing invariant — the engine injects data into the evidence map that it knows can never be matched. Option A is a minimal, safe change that makes the implementation match the specification. The risk is negligible: the `has_gates_routing` check is already in place, GateEvaluated events and blocking_conditions are unaffected, and the change touches only the evidence merge step.

**Alternatives Considered**

- **Keep current behavior (inject but ignore)**: Gate output continues to appear in the merged evidence for all states. Functionally identical for all observable behaviors, but contradicts the PRD R10 acceptance criterion and the design doc decision driver. Rejected because it accumulates specification drift without any offsetting benefit.

**Consequences**

- The advance loop for legacy states no longer populates the `"gates"` key in the resolver evidence. For the pure-legacy early-return path, this is already the case (the merge never happens); the only visible change is for legacy-with-accepts states.
- GateEvaluated events and `StopReason::GateBlocked` contents are unchanged.
- The code now matches the PRD acceptance criterion precisely.
- When the last legacy template migrates to structured routing, removing the `has_gates_routing` guard becomes a contained, obvious cleanup.
<!-- decision:end -->
