---
status: Proposed
problem: |
  When koto's engine auto-advances through states, the CLI returns a response
  with `advanced: true` that tells callers "something moved" but not whether
  they need to act. Every caller works around this by immediately calling
  `koto next` again. The double-call is mechanical overhead with no decision
  value. The engine's stopping conditions need updating to eliminate the
  pattern.
decision: |
  Extend the engine's advance_until_stop() to distinguish "state needs agent
  input" (has accepts block) from "state has unresolvable transitions" (no
  accepts). Add a new StopReason::UnresolvableTransition variant for the
  latter. No response contract changes needed -- response variants are
  already self-describing.
rationale: |
  The engine already owns the advancement loop. Checking accepts.is_some()
  is consistent with how it already checks gates and integrations. A new
  StopReason variant gives both CLI and library consumers an unambiguous
  signal. The response contract stays unchanged because callers already use
  the response variant (EvidenceRequired, GateBlocked, Terminal) to decide
  what to do -- adding transition_count would be noise no caller acts on.
---

# DESIGN: Auto-advance engine loop fix

## Status

Proposed

## Context and problem statement

Issue #89 asked koto to auto-advance past "advanced phases." Exploration
revealed that "advanced phases" don't exist -- `advanced: true` is a CLI
response flag meaning "at least one transition occurred in this invocation,"
not a template-level concept. The flag was introduced on March 17 to report
agent-initiated changes. Auto-advancement was added March 18 and overloaded
the same field for engine-initiated transitions. The semantic collision was
never discussed in design docs.

The engine already auto-advances via `advance_until_stop()` in
`src/engine/advance.rs`. It loops through unconditional transitions, evaluates
gates at every step, and stops at evidence requirements, gate failures,
terminal states, integrations, or cycle detection. But the CLI handler returns
after a single engine invocation, forcing callers to re-invoke `koto next` to
classify the new state. The work-on skill's execution loop codifies this as
step 2: "If `action: execute` with `advanced: true`, run `koto next` again."

Three rounds of exploration confirmed:

1. The double-call is emergent, not designed. No invariants are at risk.
2. The agent-vs-engine distinction in `advanced` doesn't matter for any
   real caller. The event log already provides full disambiguation.
3. Response variants (EvidenceRequired, GateBlocked, Terminal) are
   self-describing. Callers don't need `advanced` to decide what to do.
4. Observability belongs in the event log, not in every response. No
   caller acts on transition metadata -- the response variant is enough.

## Decision drivers

- **Eliminate the double-call.** Every koto caller implements the same
  workaround. The fix should make `koto next` return an actionable response
  in a single call.
- **Preserve state machine integrity.** Transitions must be recorded, evidence
  epochs must stay clean, gates must execute. The event log is the source of
  truth.
- **Keep responses lean.** `koto next` answers "what should I do?"
  Observability belongs in the event log or `koto state`, not in every
  response. Response variants are already self-describing.
- **Backward compatibility.** `advanced: bool` stays unchanged in the
  response. No response contract changes.
- **Single layer owns the behavior.** The engine already manages the
  advancement loop. Extending its stopping conditions is cleaner than adding
  a second loop in the CLI handler.

## Decisions already made

These choices were settled during exploration and should be treated as
constraints, not reopened.

### Round 1
- Issue #89 fits koto's philosophy: the double-call is emergent overhead,
  not intentional design
- No state machine invariants at risk: transitions recorded, evidence epochs
  clean, gates still execute
- The fix belongs in the engine layer (`advance_until_stop`), not the CLI or
  caller convention

### Round 2
- Agent-vs-engine semantic distinction: not worth encoding in the CLI
  response (event log handles it)
- `advanced` field: keep unchanged, no response contract changes needed
- Response variants are already self-describing for callers

### Round 3
- No response-level observability metadata needed. No caller acts on
  transition counts or state lists -- the response variant tells callers
  what to do
- Rich observability via deferred `koto state` command (already designed,
  post-#49) and the event log
- Issue #89's observability acceptance criterion was written based on a
  misunderstanding of the abstraction; the behavioral fix is what matters

## Considered options

### Decision 1: How to extend advance_until_stop()

The engine's `advance_until_stop()` stops with `StopReason::EvidenceRequired`
whenever conditional transitions exist but no evidence matches and there's no
unconditional fallback. This conflates two situations: "this state has an
`accepts` block and genuinely needs agent input" vs "this state has
conditionals that didn't match and no way for the agent to submit evidence."
The second case produces an `EvidenceRequired` response with empty expects --
a meaningless signal that triggers the double-call.

Key assumptions:
- `accepts.is_some()` on `TemplateState` is the canonical indicator that a
  state expects agent-provided evidence
- A new `StopReason` variant is acceptable; the enum already grows with
  features (`ActionRequiresConfirmation`, `SignalReceived`)

#### Chosen: Engine-layer accepts awareness with new StopReason variant

Add an `accepts.is_some()` check in the advance loop where
`resolve_transition()` returns `NeedsEvidence`. When accepts is `None`,
return `StopReason::UnresolvableTransition` instead of
`StopReason::EvidenceRequired`. In `advance.rs`, after the existing
`NeedsEvidence` handling:

```rust
TransitionResolution::NeedsEvidence => {
    if template_state.accepts.is_some() {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,

            stop_reason: StopReason::EvidenceRequired,
        });
    } else {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,

            stop_reason: StopReason::UnresolvableTransition,
        });
    }
}
```

The CLI maps `UnresolvableTransition` to a clear error response, not a
fake "evidence required" with empty expects. This eliminates the double-call
at its source and gives library consumers the same benefit.

#### Alternatives considered

**Post-loop continuation (CLI-layer loop)**: After `advance_until_stop`
returns EvidenceRequired with empty expects, the CLI re-invokes the engine.
Rejected because it violates the engine-layer constraint, doesn't help
library consumers, and resets cycle detection between calls.

**Re-dispatch loop in handle_next**: Functionally equivalent to the
CLI-layer loop with the same problems. The "strict error" sub-variant
(treat no-accepts + NeedsEvidence as a dead-end) has merit as a template
validation signal, which the chosen approach provides via the new
StopReason variant.

### Decision 2: Response contract changes (rejected)

Exploration rounds 2 and 3 investigated adding `transition_count` or
`passed_through` to the response contract. Both were rejected on review:
no caller acts on transition metadata. The response variant
(EvidenceRequired, GateBlocked, Terminal) already tells callers what to
do. Adding observability fields to every response is noise when the event
log provides the same information with more detail.

The `advanced: bool` field stays unchanged. It's semantically overloaded
but harmless -- callers who check it will stop needing to after the
engine fix eliminates the ambiguous response.

## Decision outcome

**Chosen: engine-layer accepts awareness, no response contract changes**

### Summary

The engine's `advance_until_stop()` gains one new check: when
`resolve_transition()` returns `NeedsEvidence`, the engine looks at
whether the current state has an `accepts` block. If it does, the engine
stops with `EvidenceRequired` as before -- the state genuinely needs agent
input. If it doesn't, the engine stops with a new
`StopReason::UnresolvableTransition` that tells callers "this state is
stuck, not waiting for you."

No response contract changes. The `advanced: bool` field stays as-is.
`NextResponse` variants stay as-is. The response variant already tells
callers what to do -- EvidenceRequired means "submit evidence,"
GateBlocked means "fix gates," Terminal means "done." The double-call
workaround in callers can be removed because the ambiguous
EvidenceRequired-with-empty-expects response no longer occurs.

### Rationale

The engine fix is the only change needed. The double-call existed because
`EvidenceRequired` fired for states that don't accept evidence, producing
a meaningless response callers had to re-query. With
`UnresolvableTransition` handling that case, every response is actionable
on the first call.

We investigated adding `transition_count` and `passed_through` to the
response but dropped both: no current or foreseeable caller branches on
transition metadata. The event log already records every transition with
full detail. Adding response fields no one reads is complexity for
complexity's sake.

## Solution architecture

### Overview

One engine change, one CLI mapping update. No template or response
contract changes.

### Engine changes (src/engine/advance.rs)

**StopReason enum**: Add `UnresolvableTransition` variant:

```rust
pub enum StopReason {
    // ... existing variants ...
    /// Conditional transitions exist but no evidence matches, and the state
    /// has no accepts block -- the agent can't submit evidence to resolve this.
    UnresolvableTransition,
}
```

**advance_until_stop() loop**: In the `NeedsEvidence` branch, check
`template_state.accepts.is_some()` to decide between `EvidenceRequired`
and `UnresolvableTransition`:

```rust
TransitionResolution::NeedsEvidence => {
    if template_state.accepts.is_some() {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            stop_reason: StopReason::EvidenceRequired,
        });
    } else {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            stop_reason: StopReason::UnresolvableTransition,
        });
    }
}
```

### CLI changes

**handle_next** (src/cli/mod.rs): Map
`StopReason::UnresolvableTransition` to exit code 2 (precondition failed),
consistent with how other "can't proceed" conditions are handled. The error
response should include the state name and a message explaining that the
state has conditional transitions but no accepts block.

**dispatch_next** (src/cli/next.rs): The fallback path (lines 104-117) that
synthesizes empty expects for states with no accepts block can be removed or
simplified -- the engine now handles this case.

**Out of scope: `--to` directed transition path.** The `--to` flag calls
`dispatch_next` directly (mod.rs:842), which still produces the old
empty-expects response for no-accepts states. This doesn't trigger the
double-call pattern (directed transitions are explicit, not auto-advanced),
so aligning it is deferred.

### Data flow

```
Template state
    |
    v
advance_until_stop()
    |-- checks accepts.is_some() at NeedsEvidence branch
    |-- returns AdvanceResult { stop_reason }
    v
handle_next()
    |-- maps StopReason to NextResponse (unchanged variants)
    |-- UnresolvableTransition -> exit code 2
    v
JSON response to caller
    { "action": "execute", "advanced": true, "expects": {...}, ... }
    (no ambiguous empty-expects response)
```

## Implementation approach

### Phase 1: Engine changes

Add `UnresolvableTransition` to `StopReason`. Update `advance_until_stop()`
to check `accepts.is_some()` in the `NeedsEvidence` branch. Update engine
unit tests.

Deliverables:
- src/engine/advance.rs (StopReason, NeedsEvidence branch)
- Engine unit tests updated

### Phase 2: CLI mapping

Map `UnresolvableTransition` in `handle_next`. Simplify or remove the
empty-expects fallback in `dispatch_next`. Update integration tests.

Deliverables:
- src/cli/mod.rs (handle_next mapping)
- src/cli/next.rs (dispatch_next simplification)
- tests/integration_test.rs (updated assertions)

### Phase 3: Docs

Update CLI usage guide to note that the empty-expects response no longer
occurs. Note that callers can remove the "if advanced, call again"
workaround.

Deliverables:
- docs/guides/cli-usage.md (response behavior update)

## Security considerations

This change doesn't introduce new attack surface. No new user input
parsing, network calls, file permissions, or dependencies. The engine's
existing safety mechanisms (cycle detection, chain limit of 100, signal
handling) are unchanged. No new data is exposed in responses.

## Consequences

### Positive

- Eliminates the double-call pattern for all callers (CLI and library)
- Improves template error signals: states with unresolvable transitions
  get a distinct stop reason instead of a fake "evidence required"
- Backward compatible: no response contract changes, existing callers
  continue working unchanged
- Small change: one `if` branch in the engine, one CLI mapping

### Negative

- `StopReason` gains a variant, breaking exhaustive matches for library
  consumers. This is a compile-time error, not a runtime surprise.

### Mitigations

- The new StopReason variant follows the existing pattern
  (ActionRequiresConfirmation, SignalReceived were similar additions).
  Library consumers already handle enum growth.
