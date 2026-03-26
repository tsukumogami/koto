# Lead: What invariants would auto-advance need to preserve?

## Findings

### State Machine Execution Order

The advancement loop in `src/engine/advance.rs` follows a deterministic sequence for each state:

1. **Signal check** — shutdown flag (can interrupt at any point)
2. **Chain limit check** — max 100 transitions per invocation
3. **Terminal state check** — stop if state is marked terminal
4. **Integration check** — stop if state declares an integration
5. **Action execution** — run default_action if present; continues to gates regardless of result
6. **Gate evaluation** — all gates evaluated without short-circuit
7. **Transition resolution** — match evidence or fall back to unconditional

Each phase has specific stopping conditions and side-effects. The key invariant: **all phases before transition always execute, and their results are recorded in the state file.**

### Side-Effects That Would Be Lost Without Auto-Advance

#### Actions Execute and Are Recorded

- If a state has a `default_action`, it is executed during the loop (line 258-285 in advance.rs)
- The action result is **always appended as a `DefaultActionExecuted` event** (line 1093-1100 in cli/mod.rs), regardless of whether the action blocks or allows continuation
- This happens before gates are evaluated
- Evidence submission skips actions (line 1043-1044), but action *execution* still creates an event
- **Implication**: If auto-advance skips pausing at a state, the caller never triggers that `koto next` invocation, and no `DefaultActionExecuted` event is recorded for any intermediate state

#### Gates Are Evaluated and Results Recorded

- Gate evaluation (line 289-309 in advance.rs) runs all gates without short-circuit
- Gate results influence transition resolution: if gates fail and an `accepts` block exists, the engine falls through to evidence requirement
- The gate evaluation is injected as a closure from the CLI (line 1020-1032 in cli/mod.rs)
- **Gate execution itself is NOT directly persisted as an event** — only the consequences are (indirectly through GateBlocked stops or transitions)
- However, if a gate failure blocks advancement, the pause happens and the caller can inspect it
- **Implication**: Auto-advance changes the observable behavior: currently, callers *see* that gates were evaluated (by observing the GateBlocked response). With auto-advance, a gated state with no accepts block would auto-advance silently, and the gate evaluation would be invisible to callers

#### Integration Availability Check

- States with an `integration` field are checked (line 224-255 in advance.rs)
- Currently, integration runners are unavailable (deferred to #49), so states with `integration` always stop with `IntegrationUnavailable`
- If auto-advance were implemented, this check would still happen, but the stop would be lost
- **Implication**: The caller never sees that integration was declared (not recorded in events)

#### Evidence Epoch Boundaries

- After each transition, evidence is cleared (line 336 in advance.rs: `current_evidence = BTreeMap::new()`)
- This preserves the property that evidence submitted at state A doesn't influence transitions at state B
- Auto-advancing doesn't break this: the loop correctly clears evidence at each boundary
- **Invariant preserved**: Evidence epochs remain clean

#### Transition Events Persist

- Each transition, auto or conditional, generates a `Transitioned` event (line 324-329 in advance.rs)
- Events include `from`, `to`, and `condition_type` ("auto" or "gate" based on what triggered it)
- Whether called once or twice, the same transitions are recorded
- **Invariant preserved**: Transition audit trail is identical

### Gate Logic for Advanced Phases Still Executes

The phrasing "gate logic for advanced phases still executes" refers to this dynamic:

1. A state with `advanced: true` from the response (when stop_reason is EvidenceRequired with empty expects) signals "this state can auto-advance"
2. Callers currently work around this by calling `koto next` again (without --with-data)
3. On the second call, the loop runs again from that state, gates are re-evaluated, and if they still pass, the state auto-advances to the next

**If auto-advance is implemented in the engine:**
- The same gate evaluation happens (gates still execute, side-effects preserved)
- But it happens in the *same invocation* rather than requiring a second call
- The `advanced: true` response would never be emitted; instead, the response would reflect the final state after auto-advancing

### State File Atomicity

Each event is appended with `sync_data()` (line 31 in persistence.rs), ensuring atomic writes. The sequence number is auto-assigned per-state (line 44-48 in persistence.rs: `next_seq = read_last_seq + 1`).

With auto-advance collapsing two calls into one:
- The same transitions are recorded in the same order (e.g., state A → B → C)
- The state file format is unchanged
- **Invariant preserved**: The state file remains a deterministic replay of the workflow

### Current "advanced: true" Semantics

From `src/cli/next.rs` (line 24): "The `advanced` flag is set by the caller (true when an event was appended before dispatching)."

In `src/cli/mod.rs` (line 1132): `let advanced = advance_result.advanced;` — this reflects whether `advance_until_stop` returned `advanced=true`, which is set to true on line 333 of advance.rs when a transition happens.

**Current behavior:**
- Call 1: State A auto-transitions to B. Response: `{ "action": "execute", "state": "B", "advanced": true, "expects": {} }`
- Caller checks: `advanced == true` and `expects.fields` is empty → calls `koto next` again
- Call 2: State B either transitions further or stops with a blocking condition

**With auto-advance in engine:**
- Single call completes all auto-advances
- Response would be the final state after all auto-transitions
- `advanced` would still accurately reflect whether any transition happened

## Implications

### What Auto-Advance Preserves

1. **Transition audit trail** — All transitions are still recorded with `condition_type`
2. **Evidence boundaries** — Evidence epochs remain clean; no evidence leakage between states
3. **Gate execution** — Gates are still evaluated (though results are no longer observable via a pause)
4. **Action execution** — Actions still run and are recorded (though all within a single call)
5. **State file format** — JSONL with sequential events remains unchanged
6. **Determinism** — The final state reached is the same whether via double-call or auto-advance

### What Auto-Advance Loses (Observable Side-Effects)

1. **Caller visibility into gate evaluation results** — Currently, a GateBlocked response shows what gates failed. With auto-advance, silent gate passes are invisible
2. **Caller pause at each action** — A caller can currently inspect action output at each state. With auto-advance, actions run silently between user-visible states
3. **Intermediate state inspection** — Currently, callers can observe each state in a chain (via the advanced flag). Auto-advance collapses this to final state only

### What Auto-Advance Cannot Break (Invariants)

1. **Seq monotonicity** — Events maintain strict `seq = prev_seq + 1` ordering
2. **State machine validity** — Transitions only occur to valid template states
3. **Gate+accepts interaction** — When gates fail on a state with accepts, evidence is required (this is enforced in resolve_transition with the gate_failed flag)
4. **Cycle detection** — The visited set still tracks auto-advanced-through states
5. **Terminal state finality** — Terminal states still stop the loop immediately

## Surprises

**Strong Evidence for Safe Auto-Advance:**

1. **Gates are already non-persisted** — Gate results aren't recorded as events; only their *effects* are (via GateBlocked stops or transitions). Auto-advancing doesn't change the evaluation, only the visibility.

2. **Evidence isolation is built-in** — The line `current_evidence = BTreeMap::new()` (line 336) happens *after* a transition is recorded. This is deliberately placed to ensure fresh epochs. Auto-advance doesn't interact with this.

3. **The double-call pattern is purely mechanical** — There's no state that gets "forgotten" by skipping the pause. The second call simply re-evaluates gates and transitions from the same state. If gates pass identically, the same transition occurs. This is pure redundancy.

4. **Action execution is transparent to transitions** — Actions execute but don't block transitions (unless `requires_confirmation: true`). Their results are recorded but don't affect routing. Auto-advancing doesn't lose information; it just collapses the timing.

5. **The "advanced: true" signal was a workaround** — The scope document explicitly states: "The skill's execution loop has an explicit workaround: 'if advanced: true, run koto next again.'" This confirms the double-call is not an intentional design pause, but a mechanical loop-until-done pattern.

## Open Questions

1. **Should intermediate state events still be recorded?** Auto-advance within a single call means no pauses, so no intermediate `koto next` invocations. Should the engine artificially insert "state transition observed" events for debugging? Currently, only `Transitioned` events are recorded; intermediate states without transitions aren't marked. This is existing behavior, not new.

2. **How should the CLI signal auto-advancement completion?** Currently, the `advanced` flag signals "something happened; call again." With auto-advance, `advanced` would accurately reflect "at least one transition occurred," but the response would be the *final* state. Is this confusing to callers? Should there be an `auto_advanced` field listing intermediate states?

3. **Should `advanced: true` responses still exist?** With auto-advance in the engine, a response would only have `advanced: true` if the final state has no accepts, gates, or transitions (genuinely stuck). This is rare. Should callers expect the meaning of `advanced: true` to change (from "call me again" to "I hit an unexpected dead-end")?

4. **Backward compatibility** — Any library consumers calling the engine directly (not through the CLI) would see identical results if they re-call the advance function with the same parameters. But CLI callers expecting the double-call workaround would need to adapt.

## Summary

Auto-advance would collapse two sequential calls into one without losing state machine invariants: transitions are recorded, evidence boundaries are clean, gates still execute, and the state file format is unchanged. The main losses are *observability* (callers no longer pause to inspect intermediate states) and *explicit action recording* (actions run silently between user-visible states), but neither affects correctness. The double-call pattern is purely mechanical with no decision value — callers don't inspect the intermediate state, they just re-invoke with the same parameters.

