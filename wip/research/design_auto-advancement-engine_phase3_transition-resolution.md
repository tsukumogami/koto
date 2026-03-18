# Phase 3 Research: Transition Resolution

## Questions Investigated
- How should unconditional transition selection work when there are multiple transitions?
- How should evidence matching against `when` conditions work?
- What does the template compiler already guarantee about transition validity?
- How should the engine handle the case where evidence matches zero transitions vs multiple?
- Where should this logic live -- inside the engine loop, or as a standalone function the engine calls?

## Findings

### 1. Unconditional Transition Selection

The `Transition` struct in `src/template/types.rs:47-52` has an optional `when` field:

```rust
pub struct Transition {
    pub target: String,
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}
```

A transition with `when: None` is unconditional. The template compiler (`validate_evidence_routing` in `src/template/types.rs:178-288`) enforces mutual exclusivity only for *conditional* transitions (those with `when: Some(...)`). It does not validate or constrain unconditional transitions at all.

This means the compiler currently allows a state to have multiple unconditional transitions. There is no existing code that selects among them. The design doc's data flow (`DESIGN-unified-koto-next.md` lines 462-463) says:

> if no accepts and gates pass: append transitioned event -> fsync -> continue

This implies there should be exactly one unconditional transition for auto-advance states. The engine should treat **multiple unconditional transitions as an error** -- not first-wins. The compiler could also be extended to reject this case, but defensive runtime checking is still needed since the engine must not silently pick an arbitrary path.

For states with an `accepts` block, a mix of conditional and unconditional transitions is possible. The test `accepts_with_unconditional_transitions` (line 773) shows the compiler accepts a state with an `accepts` block and only unconditional transitions. An unconditional transition in a state that has an `accepts` block should be treated as a **default/fallback** route -- taken when no conditional transition matches. Multiple unconditional transitions on an accepts-bearing state should also be an error.

### 2. Evidence Matching Against `when` Conditions

Evidence values are `serde_json::Value` (stored in `HashMap<String, serde_json::Value>` per `EventPayload::EvidenceSubmitted`). The `when` conditions are `BTreeMap<String, serde_json::Value>`. Both use the same JSON value type.

**Matching semantics should be exact JSON value equality** (`serde_json::Value` implements `PartialEq`). No type coercion is needed because:

- The `validate_evidence` function in `src/engine/evidence.rs:45-98` already enforces that submitted evidence matches the declared field types (string must be string, number must be number, etc.)
- The compiler validates that `when` values are JSON scalars (line 223: arrays and objects rejected) and that enum `when` values appear in the allowed values list (line 232-245)
- Both sides go through schema validation against the same `FieldSchema`, so types will already match at comparison time

For multi-field `when` conditions, the matching rule should be: a transition matches if **all** fields in its `when` block match the corresponding evidence values. Fields in the evidence not mentioned in `when` are ignored. This is conjunction (AND) semantics -- consistent with how the mutual exclusivity checker works (it looks for at least one shared field with a differing value to prove disjointness).

Evidence is derived by `derive_evidence` in `src/engine/persistence.rs:235-265`, which returns `Vec<&Event>` -- potentially multiple `evidence_submitted` events in the current epoch. The matching function needs to **merge** evidence from all events in the epoch into a single field map before comparing against `when` conditions. Later submissions for the same field should override earlier ones (last-write-wins within the epoch).

### 3. What the Template Compiler Already Guarantees

The `validate` method on `CompiledTemplate` (lines 94-175) and `validate_evidence_routing` (lines 178-288) guarantee:

1. **Transition targets exist**: every `transition.target` references a declared state (line 125-130)
2. **when blocks are non-empty**: empty `when` maps are rejected (line 196-201)
3. **when requires accepts**: a transition with `when` conditions requires the state to have an `accepts` block (line 204-209)
4. **when fields reference accepts fields**: every field in a `when` block must be declared in `accepts` (line 215-219)
5. **when values are JSON scalars**: arrays and objects are rejected (line 222-229)
6. **Enum values are valid**: `when` values for enum fields must appear in the `values` list (line 232-245)
7. **Pairwise mutual exclusivity**: conditional transitions must share at least one field with differing values (line 250-285)
8. **Field types are valid**: accepts fields must be one of `enum`, `string`, `number`, `boolean` (line 156-158)

The compiler does **not** guarantee:
- That there is at most one unconditional transition per state
- That conditional transitions are exhaustive (cover all possible evidence combinations)
- That multi-field conditions are truly mutually exclusive (documented limitation at design doc line 303-308)
- That a state with only unconditional transitions has exactly one

### 4. Evidence Matches Zero vs Multiple Transitions

**Zero matches**: When evidence has been submitted but no conditional transition's `when` block matches, the engine should **stop and report the state as requiring evidence**. This is the normal case where the agent has submitted partial or non-routing evidence. The state still needs more or different evidence to advance. If there is an unconditional transition as a fallback, it should be taken.

**Multiple matches**: The compiler's pairwise mutual exclusivity check (for single-field conditions) should make this impossible for well-formed templates. However, multi-field conditions can theoretically produce overlapping matches. The engine should treat **multiple conditional matches as an error** -- a determinism violation that should surface clearly rather than silently picking a winner. This is a runtime safety net for the compiler's documented limitation.

**No transitions at all**: A non-terminal state with zero transitions is structurally unusual. The engine should treat this as an error (dead-end state).

### 5. Where This Logic Should Live

This logic should be a **standalone function** that the engine loop calls, not inline in the loop. Reasons:

- `dispatch_next` in `src/cli/next.rs` is already a pure function that classifies state without doing I/O. Transition resolution is a similar pure classification: given a template state, its transitions, and current evidence, determine which transition (if any) fires.
- The advancement loop needs transition resolution at step "evaluate which transition's when conditions match current evidence" in the design doc data flow. Isolating it makes the loop body cleaner and the resolution logic independently testable.
- The function signature would be something like:

```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
) -> TransitionResolution
```

Where `TransitionResolution` is an enum:
- `Resolved(String)` -- target state name
- `NeedsEvidence` -- has conditional transitions but none match
- `Ambiguous(Vec<String>)` -- multiple matches (error)
- `NoTransitions` -- dead-end (error)

This function belongs in `src/engine/advance.rs` alongside the advancement loop, since it's internal to the engine's advancement logic and not needed by the CLI dispatcher.

## Implications for Design

1. **Evidence merging is required before matching**: `derive_evidence` returns a list of events. The advancement engine needs a step to flatten these into a single `BTreeMap<String, Value>` (last-write-wins) before calling `resolve_transition`.

2. **The fallback case needs definition**: The design doc says states with no `accepts` block and gates passing are auto-advanced. But what about states with an `accepts` block, conditional transitions that don't match, AND an unconditional fallback transition? The engine should take the unconditional transition as a default route. This means the resolution function must distinguish "no match and no fallback" from "no match but has fallback."

3. **Compiler strengthening is optional but advisable**: Adding a validation rule that states with no `accepts` block have at most one unconditional transition would catch template errors at compile time rather than runtime. This is a small change to `validate_evidence_routing`.

4. **The `condition_type` field on `transitioned` events needs values for each resolution path**: Auto-advance (unconditional) should use `"auto"`, evidence-matched should use `"evidence"`, and fallback-unconditional should use `"auto"` or a new `"default"` value. This affects the event log's audit trail.

## Surprises

1. **The compiler does not validate unconditional transition counts.** A state can have three unconditional transitions with no `when` blocks, and the compiler will accept it silently. The engine must handle this defensively.

2. **`dispatch_next` already handles the fallback case** -- its branch at line 98-111 returns `EvidenceRequired` with empty expects for states with no accepts, no integration, and no gates blocking. This is labeled "auto-advance candidate" in the comment. The advancement loop will need to recognize this response type and resolve the unconditional transition rather than stopping.

3. **Evidence is multi-event, not single-event.** The epoch model allows multiple `evidence_submitted` events for the same state. The design doc doesn't specify whether later submissions replace or merge with earlier ones at the field level. The resolution function needs a clear merging strategy. Last-write-wins per field is the natural choice.

4. **The mutual exclusivity checker has a documented gap for multi-field conditions** where transitions test different fields. Evidence like `{decision: "proceed", priority: "high"}` could match a transition with `when: {decision: "proceed"}` AND a transition with `when: {priority: "high"}`. The engine must handle this case even though the compiler can't prevent it.

## Summary

The template compiler guarantees transition targets exist, `when` values are valid scalars, and conditional transitions are pairwise exclusive on shared fields -- but does not constrain unconditional transition counts or multi-field overlap. Evidence matching should use exact `serde_json::Value` equality with conjunction semantics (all `when` fields must match), and evidence from multiple events in the current epoch must be merged (last-write-wins) before comparison. The resolution logic should be a standalone pure function in `src/engine/advance.rs` returning an enum that distinguishes "resolved target," "needs more evidence," and "ambiguous/error" cases, with unconditional transitions serving as fallback routes when no conditional transition matches.
