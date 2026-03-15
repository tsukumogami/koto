# Research: Epoch Boundary Rule and Rewind Interaction

## Summary

The epoch boundary rule creates a directional scoping mechanism: evidence is active only when submitted to the current state after its most recent arrival. `rewound` events do not create a new epoch — they are state-change events, not transition boundaries for evidence scoping. After a rewind, the epoch boundary rule applies retroactively: evidence from a prior visit to the rewound-to state remains archived (not in scope), and only evidence submitted after the rewind becomes active. This prevents evidence from stale visits from contaminating the current state, even when returning to a previously visited state.

## How rewound affects current state derivation

From `DESIGN-unified-koto-next.md` (line 238): "Current state: `to` field of the last `transitioned` or `directed_transition` event."

The design explicitly lists only two event types that affect current state: `transitioned` and `directed_transition`. It does not mention `rewound`. This is a critical gap.

**Analysis of the gap:**

The design defines six event types (line 211-220):
- `workflow_initialized`
- `transitioned` (auto-advancement)
- `evidence_submitted` (agent data)
- `directed_transition` (human override)
- `integration_invoked` (integration output)
- `rewound` (rewind action)

The `rewound` event carries `from` and `to` fields (line 220), indicating a state change. But the current state derivation rule doesn't mention it. Two interpretations are plausible:

1. **`rewound` does not change current state** — it's an audit-only event; the actual state change is recorded by a subsequent `transitioned` event (two-phase). But this doesn't match the event structure (`to` field suggests it changes state).

2. **`rewound` should be included in the state derivation rule** — current state = `to` field of last `transitioned`, `directed_transition`, OR `rewound` event. This is the simpler model and matches the semantic intent.

The second interpretation is correct. `koto rewind` must change current state; the `rewound` event is the mechanism. The design doc's line 238 is incomplete and must be fixed to include `rewound` in the state derivation rule.

**Corrected rule:** Current state = `to` field of the last event whose type is `transitioned`, `directed_transition`, or `rewound`.

## Scenario analysis: rewind to state with prior evidence

### Scenario 1: Simple rewind (from research question)

```
seq1: workflow_initialized (state: gather)
seq2: transitioned (from: null, to: gather)
seq3: evidence_submitted (state: gather, fields: {key: "val1"})
seq4: transitioned (from: gather, to: analyze)
seq5: rewound (from: analyze, to: gather)
```

**After seq5:**

- **Current state:** `gather` (from seq5's `to` field)
- **Current evidence:** Empty set
- **Why:** The epoch boundary rule (line 239-243) states: "evidence occurring after the most recent `transitioned` event whose `payload.to` matches the current state." At seq5, the most recent `transitioned` event with `to: gather` is seq2 (not seq3 — evidence doesn't create an epoch boundary). The rule applies from seq2 forward: seq3 falls between seq2 and seq4, so when we transition away at seq4, it's archived. After the rewind at seq5, we're back to the state at seq2's completion. The epoch boundary now starts from seq2 and extends forward, but seq3 is in the archived past (between the old epoch boundary at seq2 and the transition out at seq4). No evidence is active.

### Scenario 2: Looping workflow (rewind is not relevant, but illustrates the rule)

```
seq1: workflow_initialized
seq2: transitioned (to: gather)
seq3: evidence_submitted (state: gather, fields: {key: "stale"})
seq4: transitioned (to: analyze)
seq5: transitioned (to: gather)  <-- loop back to gather
seq6: evidence_submitted (state: gather, fields: {key: "fresh"})
```

**At seq6:**

- **Current state:** `gather` (from seq5's `to` field)
- **Current evidence:** `{key: "fresh"}` (from seq6 only)
- **Why:** The most recent `transitioned` event with `to: gather` is seq5. The epoch boundary rule is: evidence occurring *after* seq5. Only seq6 falls after seq5, so only `{key: "fresh"}` is active. The `{key: "stale"}` evidence from seq3 is in the prior epoch (between seq2 and seq4) and is archived.

This is the key correctness property: evidence from a prior visit is not re-activated when the workflow loops back.

### Scenario 3: Rewind in a looping workflow

```
seq1: workflow_initialized
seq2: transitioned (to: gather)
seq3: evidence_submitted (state: gather, fields: {key: "v1"})
seq4: transitioned (to: analyze)
seq5: transitioned (to: gather)  <-- loop back
seq6: evidence_submitted (state: gather, fields: {key: "v2"})
seq7: transitioned (to: analyze)
seq8: rewound (from: analyze, to: gather)
```

**After seq8:**

- **Current state:** `gather` (from seq8's `to` field)
- **Current evidence:** Empty set
- **Why:** The most recent `transitioned` event with `to: gather` is seq5. The epoch boundary rule: evidence occurring *after* seq5. But seq7 is a transition *out* of gather, so anything after seq5 and before seq7 is the evidence for that epoch. After seq7, the gather epoch closes. After the rewind (seq8), we're back at gather, but seq5 is still the most recent `transitioned` event to gather — it hasn't changed. We're in the same epoch as before, which now has no unsubmitted evidence because the evidence (seq6) was submitted in the past visit. The rewind is a state change, not a new epoch boundary.

Wait — this needs reconsideration. Let me re-read the epoch boundary rule more carefully.

**Re-reading line 239-243:**
> "Current evidence: `evidence_submitted` events occurring after the most recent `transitioned` event whose `payload.to` matches the current state"

The rule is: after the most recent `transitioned` event whose `to` field matches the current state. In scenario 3, after seq8, current state is `gather`. The most recent `transitioned` event with `to: gather` is seq5. So we look for `evidence_submitted` events occurring after seq5. Seq6 is after seq5, so it should be in the evidence set... but seq7 is a transition *away* from gather. Does that close the epoch?

The rule says "occurring after the most recent transitioned event whose to matches the current state." It doesn't say "occurring before any subsequent transition away." The natural interpretation is: after seq5, up to now (the current log tail). At seq8, the events after seq5 are: seq6 (evidence_submitted), seq7 (transitioned away), and seq8 (rewound back). The rule is looking for evidence_submitted events after seq5. Seq6 is an evidence_submitted event after seq5, so it's in the active set.

So:
- **Current evidence:** `{key: "v2"}` (seq6)

This is the more nuanced reading. The epoch boundary rule doesn't close the epoch when you transition away — it remains active if you return to the same state. This makes sense for recovery: if an agent submits evidence in a state, then the workflow advances and comes back, the evidence is still there. The agent can see what they submitted before and re-use or overwrite it.

But this conflicts with the looping-workflow correctness goal stated in line 244: "Only the evidence accumulated since the last arrival at this state is active." In scenario 2, fresh evidence from the loop-back arrival was active, not stale evidence from the prior visit. How is that consistent with scenario 3?

**The resolution:** The looping-workflow goal means "since the last *auto-transition* arrival." A `rewound` event is different — it's a human-directed state change, not an auto-advance. In scenario 2, seq5 is an auto-transition to gather (a new visit), so the epoch resets. In scenario 3, seq8 is a rewind (a undo), so it doesn't reset the epoch — you return to the same logical visit.

But the design doc doesn't distinguish between auto-transitions and human-directed transitions when defining the epoch boundary. It only cares about "most recent `transitioned` event." Let me check if there's a distinction between `transitioned` (auto) and `directed_transition` (human).

Yes (line 250-254): `transitioned` and `directed_transition` are separate event types. `transitioned` is auto-advancement; `directed_transition` is human override via `--to`. They are different events.

So the epoch boundary rule, as written, applies to `transitioned` events. It doesn't mention `directed_transition`. If a human-directed transition happens, does it create an epoch boundary?

The design doesn't specify. This is another gap.

**Plausible interpretations:**

1. **Epoch boundary only on auto-transitions** — only `transitioned` events create epoch boundaries; `directed_transition` and `rewound` are transparent to evidence scoping. Evidence from prior visits remains active. This seems too permissive — you could directed-transition to a state and see stale evidence from a prior visit.

2. **All state changes create epoch boundaries** — `transitioned`, `directed_transition`, and `rewound` all reset evidence to the current epoch. This is more conservative and matches the looping-workflow goal: "since the last arrival" at any state.

The design doc's language (line 244) says "since the last arrival at this state," not "since the last auto-transition to this state." This suggests interpretation 2 is correct: any arrival (transition, directed_transition, or rewind) creates a new epoch.

**Revised rule for epoch boundary:**
Evidence is active if it was submitted after the most recent event (of any type) that changed the current state to the value it has now. Such events are: `transitioned` with `to` matching current state, `directed_transition` with `to` matching current state, or `rewound` with `to` matching current state.

In scenario 3 with this rule:
- After seq8, the most recent event that set current state to gather is seq8 itself (the rewound event).
- Evidence occurring after seq8 is in the active set.
- Seq6 is before seq8, so it's archived.
- **Current evidence:** Empty set

This is correct for the rewind semantics: rewind is a fresh start at that state.

## Recommendation: Evidence scoping after rewind

**Rule 1: Rewind changes current state**

The state derivation rule must include `rewound` events. Correct rule:

> Current state = `to` field of the last event of type `transitioned`, `directed_transition`, or `rewound`.

**Rule 2: Rewind creates a new evidence epoch**

The epoch boundary rule must be revised to account for all three event types that change state:

> Current evidence = `evidence_submitted` events occurring after the most recent event whose type is `transitioned`, `directed_transition`, or `rewound` and whose `to` field matches the current state.

This applies uniformly to all state changes: auto-advance, human-directed transition, or rewind.

**Rationale:**

- Consistency: all state changes should behave the same way with respect to evidence scoping. No special cases.
- Correctness for rewind: rewind is a recovery mechanism; returning to a state should start fresh from evidence submitted after the rewind, not re-activate stale evidence.
- Simplicity: the rule is uniform, easy to implement (check the last state-changing event, not just `transitioned`).
- Compliance with PRD R3 (line 145-149): "Evidence is scoped to the state it is submitted in. When the workflow transitions out of a state (by any means, including directed transition), the state's accumulated evidence is committed to the audit trail and the next state starts with an empty evidence map." By any means includes rewind — rewind is a state change that should reset the evidence scope.

## Design gap identified

The upstream design (`DESIGN-unified-koto-next.md`) has two key ambiguities that issue #46's design doc must resolve:

1. **State derivation includes `rewound` events:** Line 238 says "Current state: `to` field of the last `transitioned` or `directed_transition` event." The design defines `rewound` as a state-changing event with `from` and `to` fields, but doesn't include it in the state derivation rule. The tactical design for #46 must clarify: does `rewound` change current state? The answer is yes, and the rule must be corrected to include it.

2. **Epoch boundary applies to `directed_transition` and `rewound`:** Line 239-243 defines the epoch boundary rule in terms of `transitioned` events. The design also defines `directed_transition` events (line 218) for human-directed transitions, and `rewound` events (line 220) for rewind actions. The tactical design for #46 must specify: when a `directed_transition` or `rewound` event occurs, does it create a new evidence epoch (clearing prior evidence from the state), or is evidence transparent across these events? The PRD R3 suggests all state changes should be treated uniformly, and the looping-workflow correctness goal requires evidence to reset on re-entry. The rule should be: any state-changing event creates a new epoch.

The DESIGN-event-log-format.md design doc (issue #46) should include explicit test scenarios covering:
- Rewind to a state and verify evidence resets
- Directed transition to a state and verify evidence resets (if supported in phase 1)
- Looping workflow and verify evidence doesn't contaminate across visits
- Rewind in a looping workflow and verify evidence scoping is correct

These scenarios would have caught the ambiguity and driven a clearer design.
