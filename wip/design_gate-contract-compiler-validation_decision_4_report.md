# Decision 4: Reachability Check Scope and Implementation

**Question:** How should the reachability check be scoped and implemented to satisfy PRD R9?

**Status:** DECIDED

---

## Decision

**Chosen: Scope A (pure-gate transitions only) + Implementation i (inline resolver call)**

---

## Analysis

### The core problem

The reachability check must not flag states as dead ends when the workflow simply requires agent evidence in addition to gates. The `review` state example in the problem statement is the canonical case: applying lint override defaults to `gates.lint` but providing no `decision` evidence correctly produces `NeedsEvidence`, not a dead end. Flagging this as an error would be wrong.

### Scope evaluation

**Option A (pure-gate transitions only)** aligns with what the check can actually prove. A state has a pure-gate transition when at least one `when` clause references only `gates.*` fields — no agent evidence keys. For such transitions, gate override defaults are the only input that matters. If all pure-gate transitions fail under those defaults, the workflow genuinely has no path forward without further gate configuration. The check is sound: it only fires when the conclusion is certain.

States where every transition requires agent evidence (all `when` clauses contain non-`gates.*` fields) are exempt. This is correct because the agent can always unblock the state by submitting the right evidence. The compiler cannot know what evidence the agent will provide at runtime, so claiming a dead end would be a false positive.

**Option B (all gated states)** would generate false positives for any state with mixed transitions. The existing `validate_evidence_routing` code in `types.rs` already separates gate fields (`gates.*`) from agent fields — the infrastructure for Option A already exists. Forcing the resolver to operate on incomplete evidence and interpreting `NeedsEvidence` as a dead end misrepresents the semantics of the engine.

**Option C (skip)** leaves R9 unimplemented. The other validations (D2 type checks, D3 field references) don't substitute for reachability. Skip is only justified if implementation complexity is prohibitive, which it isn't.

### Implementation evaluation

**Option i (inline resolver call)** is the right choice because `resolve_transition` is already `pub` in `advance.rs` and handles all value types correctly. Building a `serde_json::Value` evidence map of gate override defaults and calling `resolve_transition` with `gate_failed: false` is a small, well-contained operation. The result `NeedsEvidence` or `NoTransitions` means dead end; `Resolved` or `Ambiguous` means at least one transition fires.

This approach reuses tested logic rather than duplicating it. It handles numeric exit codes, string comparisons, and booleans without special cases — precisely because the PRD caveat about "best-effort for non-enum fields" was written assuming symbolic analysis. The inline approach makes the check more powerful at no extra cost.

**Option ii (analytical pattern matching)** duplicates comparison logic that already exists in `resolve_value`. Any divergence between the analytical walker and the actual resolver creates a class of bugs where the compiler says "OK" but the engine disagrees at runtime, or vice versa. Duplication without clear benefit is the wrong trade-off.

### Algorithm

For each state with at least one gate:

1. Collect all `gates.*`-only transitions (transitions whose `when` clause contains no agent evidence keys). If none exist, skip the state — it's exempt.
2. Build evidence: `{"gates": {gate_name: override_default_or_builtin_default, ...}}` for every gate in the state.
3. Call `resolve_transition(state, &evidence, false)`.
4. If the result is `NeedsEvidence` or `NoTransitions`, emit a hard error. `Resolved` or `Ambiguous` both mean at least one transition fires — the check passes.

The check only fires on states where it can produce a sound result. Mixed-transition states are exempt by construction.

### Interaction with existing validators

D2 already validates that `override_default` values match the gate's declared type. D3 validates that `when` field references are well-formed. The reachability check (R9) builds on those: it can assume override defaults are type-correct when it runs.

---

## Rejected Options

**Option B (all gated states with false positives):** Produces incorrect errors for any state with mixed transitions. The existing code already distinguishes gate fields from agent fields; ignoring that distinction here would be inconsistent with the rest of the validator.

**Option C (skip reachability):** Leaves R9 unimplemented and relies on weaker checks that don't substitute for it. The implementation cost for A+i is low given existing infrastructure.

**Option ii (analytical pattern matching):** Duplicates logic from `resolve_value` / `resolve_transition`, creating a risk of subtle divergence between compiler and engine behavior.
