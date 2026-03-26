# Lead: What does `advanced: true` mean in koto's engine?

## Findings

### Definition: `advanced` as a Flag, Not a Phase Type

The `advanced` field in `NextResponse` is a **boolean flag indicating whether at least one state transition occurred during the invocation**, not a property of the state itself. 

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/engine/advance.rs` line 85:
```rust
/// True if at least one transition was made.
pub advanced: bool,
```

The flag is set to `true` in the `AdvanceResult` struct when `transition_count` increments (line 334 in advance.rs). It remains `false` if the engine stops at the initial state without moving.

### How `advanced: true` Arises in Practice

The `advanced` flag becomes `true` when one of these occurs:

1. **Evidence submission triggers auto-advancement** (line 1132 in cli/mod.rs):
   - Caller invokes `koto next --with-data <json>` 
   - Evidence is validated and an `evidence_submitted` event is appended
   - The `advanced_until_stop()` engine runs and performs ≥1 transition(s) before hitting a stop reason
   - Response includes `"advanced": true` alongside the stop reason

2. **Directed transition** (line 1132 in cli/mod.rs):
   - Caller invokes `koto next --to <target>`
   - A `directed_transition` event is appended
   - If that transition chains into auto-advanceable states, `advanced` is set based on the chain result

3. **Auto-advance loop with no explicit evidence** (line 1132 in cli/mod.rs):
   - A state has an unconditional transition (no `when` condition)
   - No gates are failing
   - The engine transitions automatically, setting `advanced: true`
   - **This is NOT a separate "advanced phase" -- it's just the consequence of auto-advancing**

The key insight: **there is no "advanced phase" concept in the template or state machine**. The `advanced` flag is metadata about what the engine did in the current invocation, not a state property.

### The Double-Call Pattern: What's Really Happening

When callers observe `"advanced": true` and re-call `koto next`, they are:
1. Getting back that "I moved at least one state"
2. Immediately re-running the dispatcher to see what the engine wants now that it has advanced

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/cli/next.rs` lines 21-25:
```rust
/// 4. Accepts block exists -> `EvidenceRequired`
// 5. Fallback: `EvidenceRequired` with empty expects (auto-advance candidate)
///
/// The `advanced` flag is set by the caller (true when an event was appended
/// before dispatching). This function never does I/O.
```

The dispatcher (`dispatch_next`) doesn't perform transitions—it **classifies** the current state. An `advanced: true` response means "I just moved one or more states via the engine" and the second call re-classifies the new state to determine what to do next.

### Engine Architecture: The Advancement Loop

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/engine/advance.rs` (lines 158-357):

The `advance_until_stop()` function implements a loop that:
1. Checks shutdown signal
2. Checks chain limit (max 100 transitions per call)
3. Checks if current state is terminal
4. Checks if state declares an integration
5. **Evaluates gates** (gates still block auto-advancement)
6. **Resolves transitions** (match evidence against `when` conditions; fall through to unconditional if no match and gates haven't failed)
7. **Appends `transitioned` event** with `condition_type: "auto"` for each successful transition
8. **Clears evidence for next iteration** (line 336: `current_evidence = BTreeMap::new()`)

The loop returns `AdvanceResult { final_state, advanced, stop_reason }` where `advanced: true` if ≥1 transitions occurred.

### Stop Reasons Define Where Auto-Advance Halts

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/engine/advance.rs` (lines 49-77):

The advancement loop stops and returns when it hits:
- `Terminal` -- workflow complete
- `GateBlocked` -- ≥1 gate failed; no further transitions
- `EvidenceRequired` -- conditional transitions exist but no evidence matches; no unconditional fallback
- `Integration` -- state declares an integration runner; output returned
- `IntegrationUnavailable` -- integration declared but no runner configured
- `CycleDetected` -- attempted transition to a state already visited in this chain
- `ChainLimitReached` -- safety limit of 100 transitions per invocation
- `ActionRequiresConfirmation` -- default action executed but requires user confirmation before advancing
- `SignalReceived` -- SIGTERM/SIGINT during loop

Each stop reason maps to a response variant. **The dispatcher only classifies; the engine performs the actual transitions.**

### Gate Logic for Advanced Phases Still Executes (Confirmed)

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/engine/advance.rs` (lines 288-309):

Gates are evaluated **inside the loop on every iteration**, not just before the initial transition. If any gate fails:
- A `GateBlocked` stop reason is returned immediately
- **Auto-advancement stops**
- The response includes the blocking conditions

Gate failures also affect transition resolution (line 380-420): if a gate fails and the state has an `accepts` block, the engine does NOT fall through to the unconditional transition—it requires evidence instead. This preserves evidence-as-recovery semantics.

### Evidence Epoch Boundaries

From `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/koto/src/engine/advance.rs` (lines 335-336):

After each transition during auto-advance:
```rust
// Fresh epoch: auto-advanced states have no evidence
current_evidence = BTreeMap::new();
```

This means:
- Evidence submitted in epoch N only affects the first state transition
- Auto-advanced states (those in the same invocation) see empty evidence
- The next evidence submission begins a new epoch
- **This is a deliberate design choice to prevent evidence from "leaking" through multiple states**

## Implications

### For Issue #89 (Auto-Advance Past `advanced: true`)

The issue request is asking: **"Why does the engine stop at all? Why not continue chaining?"**

The current answer is: **The CLI handler (dispatch_next) is a classifier, not an executor.** The separation of concerns means:
- The engine loop performs transitions and returns a stop reason
- The CLI handler maps the stop reason to a response
- The caller interprets the response and decides to re-call or not

For the issue to be resolved, the engine would need to continue looping until it reaches a state where the caller would have "nothing to do"—i.e., a state requiring evidence, blocked by gates, or terminal. **This is actually already implemented in the engine via `advance_until_stop()`.**

The real discovery: **The double-call pattern exists because the CLI handler stops and returns the response after the first advancement chain.** The fix would be architectural: have the CLI handler continue looping within a single `koto next` invocation until hitting a state where evidence is required (stop reason = `EvidenceRequired` with non-empty `expects` or other blocking conditions).

### For State Machine Integrity

The `advanced` flag does not represent a phase in the template or state machine. It's metadata from the engine layer. The state machine itself is defined by:
1. State definitions (name, directive, accepts, gates, transitions)
2. Transition conditions (when fields)
3. Stop conditions (terminal, gates, evidence required)

Auto-advancement does not introduce new state machine concepts—it just chains multiple transitions in a loop. The integrity constraints (cycle detection, gate evaluation per-iteration, evidence epoch boundaries) are already implemented.

### For the Work-On Skill Workaround

The skill's pattern of "if `advanced: true`, run `koto next` again" is a **caller-level workaround for the CLI's single-dispatch design**, not a fundamental requirement of the state machine. If the CLI were refactored to loop internally, the workaround would no longer be needed.

## Surprises

### `advanced` is Not About State Type; It's About What Happened This Call

The term "advanced" is semantically ambiguous in the codebase. The exploration assumed it might indicate "this is a state type that auto-advances" or "this state has auto-advance enabled." Instead, it's a **boolean outcome**: "I made transitions during this invocation."

### Gate Evaluation Happens Every Iteration, Not Just at Barriers

The discovery that gates are re-evaluated on every auto-advanced state (lines 289-309 in advance.rs) is significant. This means:
- A gate can block progression even during an auto-advance chain
- Multiple gate failures are possible in one invocation
- Gates are not "markers" for manual stopping—they're active constraints in the loop

### Evidence Epochs Are Scoped Per Invocation, Not Per State

The clearing of evidence after each transition (line 336) was surprising. This prevents evidence from "carrying forward" through multiple auto-advanced states, which is a deliberate safety mechanism but not immediately obvious from the design docs.

## Open Questions

1. **Why does the CLI handler stop and return after the engine completes one advancement chain?** The engine is capable of continuing, but the handler maps the result and returns JSON. Is this deliberate separation of concerns, or a layering artifact?

2. **Should the fix be in the engine (auto-continue looping) or the CLI handler (keep looping in dispatch)?** The design docs favor engine-layer logic, but the CLI handler could also loop internally until hitting a state that blocks.

3. **What does "advance" mean semantically in the skill's context?** If the double-call is a workaround, does auto-advance remove decision value (the issue's claim) or is there observability/audit benefit to seeing each intermediate state?

4. **Are there templates or use cases where stopping at each auto-advance state provides value?** If agents always immediately re-call, the double-call is pure overhead. But if some callers inspect intermediate states, auto-advance changes behavior.

5. **Should `advanced: true` be renamed to something clearer?** Names like `transitioned: true` or `chained: true` might reduce confusion.

## Summary

The `advanced: true` field in koto's response is a **boolean flag indicating whether the engine made at least one state transition during the invocation**, not a state type or phase marker. The advancement loop (`advance_until_stop()`) in the engine automates chaining through states with unconditional transitions, evaluating gates at every step and stopping at terminal states, evidence-required boundaries, gate failures, or cycle detection. The "double-call pattern" exists because the CLI handler's dispatcher classifies states without performing transitions itself—when the engine advances, the handler returns the result and the caller re-invokes to classify the new state. The issue's complaint about mechanical overhead is valid for the CLI interaction model, but the engine already supports continuous auto-advancement; the fix belongs in the handler layer (loop internally until hitting a real stopping condition) rather than the engine.

