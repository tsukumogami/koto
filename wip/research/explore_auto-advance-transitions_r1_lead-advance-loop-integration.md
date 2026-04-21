# Lead: Advance loop integration point for skip_if

## Findings

### Loop Structure and Control Flow (lines 208-532)
The `advance_until_stop()` function implements a single infinite loop with 7 decision points executed sequentially per iteration:

1. **Shutdown check** (lines 210-216): Checks atomic flag, returns if signal received
2. **Chain limit check** (lines 219-225): Returns if transition_count >= 100 (MAX_CHAIN_LENGTH)
3. **Terminal state check** (lines 237-242): Returns if state marked terminal
4. **Integration check** (lines 246-276): Returns if state declares an integration
5. **Action execution** (lines 280-307): Runs default_action if present, returns only if requires confirmation
6. **Gate evaluation** (lines 309-431): Evaluates gates, synthesizes gate output into evidence, may block or fall through
7. **Transition resolution** (lines 433-531): Matches evidence against transition conditions, decides to advance or block

After transition resolution, the loop either:
- **Continues** (implicit): When `TransitionResolution::Resolved(target)` matches (lines 470-493)
  - Appends a synthetic `Transitioned` event with condition_type="auto"
  - Adds target to `visited` set
  - Updates `state` variable
  - Increments `transition_count`
  - Clears `current_evidence` (fresh epoch)
  - Loop repeats from top (implicit continue)
- **Returns** (exit): When any other TransitionResolution variant or stopping condition matches

### Gate Evidence Synthesis (lines 309-431)
Gates are processed in three sub-steps:
1. **Override handling** (lines 320-362): For each gate, check for active GateOverrideRecorded events. If override exists, inject override value into gate_evidence_map without calling evaluate_gates.
2. **Non-overridden gate evaluation** (lines 364-386): Call evaluate_gates(), emit GateEvaluated events, inject output into gate_evidence_map.
3. **Failure handling** (lines 393-430): Check if any gate failed. If `accepts.is_none()` and `!has_gates_routing`, return GateBlocked immediately. Otherwise, set `gates_failed=true` and fall through to transition resolution.

Gate output is injected into the resolver evidence map (lines 457-462) ONLY when:
- `has_gates_routing` is true (at least one transition references a gates.* key)
- The gate_evidence_map is non-empty
This preserves backward compatibility with legacy boolean gates.

### Cycle Detection (line 472)
After transition resolution returns `Resolved(target)`:
```rust
if visited.contains(&target) {
    return Ok(AdvanceResult {
        final_state: state,
        advanced,
        stop_reason: StopReason::CycleDetected { state: target },
    });
}
```

The `visited` set tracks states entered during THIS invocation of advance_until_stop() (initialized empty at line 186). The starting state is EXPLICITLY NOT added to visited (comment lines 203-206), allowing legitimate re-visitation in review→implement→review loops.

Cycle detection uses a simple set membership check. For skip_if states that auto-advance deterministically, a sequence like A→skip_if1→B→skip_if2→C would be:
1. Iteration 1: state=A, resolve→B, visited={B}, state=B, continue loop
2. Iteration 2: state=B, resolve→C, visited={B,C}, state=C, continue loop
3. Iteration 3: state=C, resolve→..., no cycle until we try to revisit B or C

The mechanism prevents infinite loops naturally; if skip_if states form a cycle (A→skip1→B→skip2→A), it will be caught when trying to re-enter A.

### Transition Resolution Algorithm (lines 580-661)
resolve_transition() receives:
- `template_state`: The TemplateState with transitions list
- `evidence_value`: Merged JSON object (agent evidence + synthesized gate output)
- `gate_failed`: Boolean flag indicating whether any gate failed
- `variables`: Template variables for vars.* when-clause matching

Algorithm:
1. Iterate transitions, classify as conditional (when: Some) or unconditional (when: None)
2. For conditional transitions, evaluate ALL when-clause fields match (exact JSON equality, with dot-path support for nested access)
3. Return TransitionResolution variants:
   - `Resolved(target)`: Exactly one conditional matched, OR no conditionals matched but unconditional exists AND gate_failed=false
   - `NeedsEvidence`: Conditional exist but none match, OR no unconditional fallback and has_conditional=true
   - `Ambiguous(targets)`: Multiple conditionals matched (template bug)
   - `NoTransitions`: No transitions at all

The `gate_failed` flag prevents unconditional fallbacks from firing when gates fail (lines 644-651). This ensures states with both gates and accepts blocks require evidence when gates fail, rather than auto-advancing.

### Current Auto-advance Behavior
States auto-advance (continue the loop) when:
1. No gates fail OR (gates fail but accepts.is_some() and no gates.* conditions match)
2. resolve_transition() returns Resolved(target)
3. target not in visited set
4. condition_type recorded as "auto"

This works for states with:
- No transitions (dead end) → error
- Unconditional transition + no accepts + no gates → Resolved(fallback), advances
- Unconditional transition + accepts + gates pass → Resolved(fallback), advances
- Unconditional transition + accepts + gates fail → NeedsEvidence, blocks (requires agent override/recovery)

## Implications

### Insertion Point for skip_if Evaluation
skip_if must be evaluated AFTER gate evaluation but BEFORE transition resolution. The precise insertion point is within step 7, after line 431 (after gate handling completes and before calling resolve_transition at line 464).

The skip_if evaluation would:
1. Check if template_state.skip_if exists and evaluates to true (similar to how when-clauses evaluate conditions)
2. If true, immediately synthesize a SkipTransitioned event and transition to the state's only/default transition target (or next state in a sequence)
3. If false, proceed normally to transition resolution

This placement ensures:
- Gates have already been synthesized into evidence (needed for skip_if conditions to reference gate output)
- Deterministic auto-advancement happens before requiring evidence
- Synthetic events are written to preserve resume-awareness
- Chaining occurs naturally because skip_if evaluation triggers another loop iteration

### Loop Structure for Chaining
The loop structure ALREADY supports chaining naturally via implicit continue. When skip_if auto-advances:
1. Append SkipTransitioned event (synthetic, condition_type="skip_if")
2. Update state to transition target
3. Increment transition_count
4. Clear current_evidence
5. Loop implicitly continues from line 208

No loop restructuring needed. The existing continue-via-fallthrough mechanism (lines 470-493) proves the pattern works. Multiple consecutive skip_if states would iterate normally:
- Iteration 1: state=A, skip_if=true, auto-advance to B
- Iteration 2: state=B, skip_if=true, auto-advance to C
- Iteration 3: state=C, skip_if=false, requires evidence or has accepts

### Cycle Detection Effectiveness
The visited set mechanism prevents infinite auto-advance chains naturally. For a template bug where skip_if creates a cycle:
- Iteration 1: state=A, advance to B, visited={B}
- Iteration 2: state=B, advance to C, visited={B,C}
- Iteration 3: state=C, advance to A, but A in visited? NO (A was starting state, not added)
- Iteration 4: state=A, advance to B, but B in visited={B,C}? YES, return CycleDetected

**Issue**: The cycle detection has a subtle gap for starting-state cycles. If the template is A→skip→B→skip→A, the starting state A is not in visited, so the first re-entry to A won't be caught. Only the SECOND re-entry to B would be caught. This is unlikely in practice but represents a theoretical edge case.

Recommendation: Consider adding the starting state to visited after the first transition, or track all encountered states regardless of whether they're starting or mid-loop.

## Surprises

1. **Gate synthesis is unconditional**: Even when gates fail, their output is synthesized into the evidence map (lines 349, 368). This enables states with gates to route on gate output via gates.* when-clauses even when gates fail. The "no gates.* references" check (has_gates_routing) prevents this for legacy boolean gates. Very elegant backward-compatibility mechanism.

2. **Evidence is cleared per-iteration**: After each auto-advance, current_evidence is set to BTreeMap::new() (line 493). This means skip_if states CANNOT inherit evidence from the previous state. Each auto-advanced state starts with blank evidence for its transitions. This is intentional (comment "Fresh epoch") and isolates state transitions.

3. **Transitioned events have condition_type**: The condition_type field is set to "auto" (line 484) when synthesized. This allows callers to distinguish agent-submitted transitions from engine auto-advances. For skip_if, a new type like "skip_if" would be needed, or it could reuse "auto".

4. **No skip_if concept exists yet**: The TemplateState struct (types.rs) has no skip_if field. Would need to add this to the type definition and template compilation.

## Open Questions

1. **skip_if evaluation scope**: Should skip_if be a boolean expression (like when-clauses) that can reference evidence/gates/variables? Or a simpler predicate? The exploration scope mentions "deterministic state transitions without requiring agent evidence" but doesn't specify the expression language.

2. **Synthetic event type**: What event type and payload structure should skip_if use? The codebase uses Transitioned with condition_type="auto". For skip_if, should this be:
   - Transitioned with condition_type="skip_if"?
   - A new event type SkipTransitioned (would require EventPayload variant)?
   - Something else?

3. **skip_if vs. unconditional transition**: How does skip_if differ from an unconditional transition semantically? Is skip_if meant to replace the need for unconditional fallbacks in some scenarios? Or is it orthogonal (a state can have skip_if AND transitions)?

4. **Writing synthetic events**: The exploration context mentions "writes a synthetic event to the log to preserve resume-awareness." Need clarification: should skip_if evaluation write the event before checking gates, or after? Current gate synthesis happens but no event is written until transition resolution.

5. **Starting state exclusion rationale**: Why is the starting state explicitly excluded from visited (lines 203-206)? The comment says it was "already arrived at before this invocation," but review→implement→review workflows would re-enter the same state within a single invocation if that state is explicitly revisited. This seems correct but deserves confirmation.

## Summary

The advance loop's step 7 (transition resolution, lines 433-531) is where skip_if evaluation should insert—specifically after gate synthesis completes (line 431) and before resolve_transition() is called (line 464). A skip_if check would deterministically auto-advance by synthesizing a SkipTransitioned event and continuing the loop, with no restructuring needed because the loop already supports implicit continue via fallthrough. The existing cycle detection via the visited set prevents infinite chains naturally, though starting-state re-entry has a theoretical gap if a template creates a cycle involving the starting state itself.

