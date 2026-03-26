---
status: Proposed
problem: |
  When koto's engine auto-advances through states, the CLI returns a response
  with `advanced: true` that tells callers "something moved" but not whether
  they need to act. Every caller works around this by immediately calling
  `koto next` again. The double-call is mechanical overhead with no decision
  value. The engine's stopping conditions and the response contract both need
  updating to eliminate the pattern and provide useful observability instead.
decision: |
  Extend the engine's advance_until_stop() to distinguish "state needs agent
  input" (has accepts block) from "state has unresolvable transitions" (no
  accepts). Add a new StopReason::UnresolvableTransition variant for the
  latter. Add transition_count: usize to AdvanceResult and all NextResponse
  variants. Keep advanced: bool unchanged for backward compat, deprecate it
  in docs.
rationale: |
  The engine already owns the advancement loop. Checking accepts.is_some()
  is consistent with how it already checks gates and integrations. A new
  StopReason variant gives both CLI and library consumers an unambiguous
  signal. transition_count uses the existing counter (zero instrumentation
  cost) and provides the observability that advanced: bool can't.
---

# DESIGN: Auto-advance and response contract evolution

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
4. Observability belongs in the event log, not in every response.
   `transition_count` is enough as lightweight response metadata.

## Decision drivers

- **Eliminate the double-call.** Every koto caller implements the same
  workaround. The fix should make `koto next` return an actionable response
  in a single call.
- **Preserve state machine integrity.** Transitions must be recorded, evidence
  epochs must stay clean, gates must execute. The event log is the source of
  truth.
- **Keep responses lean.** `koto next` answers "what should I do?" Rich
  observability ("what happened along the way?") belongs in `koto state`
  or the event log. Mirrors git status vs git log.
- **Backward compatibility.** Adding fields is fine. Removing or changing
  field semantics is a breaking change. `advanced: bool` stays in the
  response but gets deprecated as a decision signal.
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
- The behavioral fix and response contract evolution are independent;
  behavioral fix can proceed first
- `advanced` field: keep for backward compat, deprecate as decision signal

### Round 3
- Response stays lean: `transition_count` for lightweight observability,
  not `passed_through`
- Rich observability via deferred `koto state` command (already designed,
  post-#49)
- `passed_through` not needed in the response -- event log + `koto state`
  handle it

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
            transition_count,
            stop_reason: StopReason::EvidenceRequired,
        });
    } else {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            transition_count,
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

### Decision 2: How to integrate transition_count into the response

The `advanced: bool` field is set once per invocation when any transition
occurs. After auto-advance, it's almost always `true`, making it a
low-signal indicator. The engine already tracks `transition_count` as a
local variable -- it just doesn't expose it. Consumers need a precise
signal for observability: "nothing happened" (0) vs "one step" (1) vs
"auto-advanced through several states" (5).

Key assumptions:
- No external consumer depends on `advanced` meaning specifically "event
  appended" vs "transition happened" -- it's treated as a rough "did
  something change" indicator
- JSON parsers ignore unknown fields (standard forward-compatibility)

#### Chosen: Add transition_count to AdvanceResult and all NextResponse variants

Add `transition_count: usize` to `AdvanceResult`. Add `transition_count: u64`
to all six `NextResponse` variants. Serialize it alongside `advanced` in
the JSON output. Document `transition_count > 0` as the preferred way to
check for transitions. Deprecate `advanced` in docs only -- it keeps its
current semantics.

#### Alternatives considered

**Align advanced = transition_count > 0**: Semantically clean but changes
`advanced` behavior for directed transitions (currently hardcoded `true`).
This is a subtle breaking change. The aesthetic benefit doesn't justify
the risk.

**Add to AdvanceResult only, not NextResponse**: Minimal code change but
provides zero consumer value. The whole point is observability in the CLI
response. Consumers who need the count would have to read the event log,
defeating the purpose.

## Decision outcome

**Chosen: engine-layer accepts awareness + transition_count in response**

### Summary

The engine's `advance_until_stop()` gains one new check: when
`resolve_transition()` returns `NeedsEvidence`, the engine looks at
whether the current state has an `accepts` block. If it does, the engine
stops with `EvidenceRequired` as before -- the state genuinely needs agent
input. If it doesn't, the engine stops with a new
`StopReason::UnresolvableTransition` that tells callers "this state is
stuck, not waiting for you."

The `AdvanceResult` struct gains a `transition_count: usize` field that
counts transitions made in the current invocation. This already exists as
a local variable in the loop; it just moves into the struct. All six
`NextResponse` variants gain a matching `transition_count: u64` field in
their JSON output.

The `advanced: bool` field stays exactly as-is. No semantic changes, no
removal. New documentation and examples reference `transition_count`
instead, and `advanced` is noted as legacy. Consumers can migrate at their
own pace.

For callers, the double-call workaround goes away entirely. The response
variant (EvidenceRequired, GateBlocked, Terminal) already tells callers
what to do. `transition_count` tells them how much happened on the way.
No second call needed.

### Rationale

These two changes reinforce each other. The engine-layer fix eliminates the
ambiguous response that caused the double-call. The transition_count field
replaces the information `advanced: bool` tried to provide with something
precise. Neither change alone is sufficient: without the engine fix, callers
still double-call; without transition_count, the observability acceptance
criterion from #89 isn't met.

Both changes are minimal. The engine check is one `if` branch. The
transition_count field uses an existing counter. The total code change is
mechanical: ~18 sites in next_types.rs for the new field, one branch in
advance.rs for the new stop reason, and test updates.

## Solution architecture

### Overview

Two changes to the engine and CLI layers, with no template changes needed.

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

**AdvanceResult struct**: Add `transition_count`:

```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub advanced: bool,
    pub transition_count: usize,
    pub stop_reason: StopReason,
}
```

**advance_until_stop() loop**: In the `NeedsEvidence` branch, check
`template_state.accepts.is_some()` to decide between `EvidenceRequired`
and `UnresolvableTransition`. Thread `transition_count` through all return
sites (~11 locations).

### CLI changes

**NextResponse variants** (src/cli/next_types.rs): Add `transition_count: u64`
to all six variants. Update `with_substituted_directive` to pass it through.
Update the custom `Serialize` impl to write the field.

**handle_next** (src/cli/mod.rs): Extract `transition_count` from
`AdvanceResult` and pass it into each `NextResponse` construction. Map
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
so aligning it is deferred. A follow-up issue can address consistency if
needed.

### Data flow

```
Template state
    |
    v
advance_until_stop()
    |-- checks accepts.is_some() at NeedsEvidence branch
    |-- returns AdvanceResult { transition_count, stop_reason }
    v
handle_next()
    |-- maps StopReason to NextResponse
    |-- threads transition_count into all variants
    v
JSON response to caller
    { "action": "execute", "transition_count": 3, "advanced": true, ... }
```

## Implementation approach

### Phase 1: Engine changes

Add `UnresolvableTransition` to `StopReason`. Add `transition_count` to
`AdvanceResult`. Update `advance_until_stop()` to check accepts and thread
the count. Update engine unit tests.

Deliverables:
- src/engine/advance.rs (StopReason, AdvanceResult, loop logic)
- Engine unit tests updated

### Phase 2: CLI response contract

Add `transition_count` to all `NextResponse` variants and serialization.
Map `UnresolvableTransition` in `handle_next`. Simplify or remove the
empty-expects fallback in `dispatch_next`.

Deliverables:
- src/cli/next_types.rs (NextResponse enum, Serialize impl)
- src/cli/next.rs (dispatch_next simplification)
- src/cli/mod.rs (handle_next mapping)

### Phase 3: Tests and docs

Update integration tests to assert on `transition_count`. Update CLI usage
guide and README to reference `transition_count` and deprecate `advanced`
as a decision signal.

Deliverables:
- tests/integration_test.rs (updated assertions)
- docs/guides/cli-usage.md (transition_count docs, advanced deprecation note)
- README.md (updated response examples)

## Security considerations

This change doesn't introduce new attack surface. No new user input
parsing, network calls, file permissions, or dependencies. The engine's
existing safety mechanisms (cycle detection, chain limit of 100, signal
handling) are unchanged.

The `transition_count` field exposes operational metadata (number of state
transitions per invocation) in the JSON response. This information is
already available in the event log. It reveals workflow structure
(how many states were traversed) but not user data or credentials.

## Consequences

### Positive

- Eliminates the double-call pattern for all callers (CLI and library)
- Gives callers a precise observability signal (transition_count) instead
  of an ambiguous boolean
- Improves template error signals: states with unresolvable transitions
  get a distinct stop reason instead of a fake "evidence required"
- Backward compatible: existing callers continue working unchanged

### Negative

- `StopReason` gains a variant, breaking exhaustive matches for library
  consumers. This is a compile-time error, not a runtime surprise.
- `advanced` becomes legacy debt in the response contract. Removing it
  later requires a major version boundary.
- ~18 mechanical code sites to update in next_types.rs for the new field.

### Mitigations

- The new StopReason variant follows the existing pattern
  (ActionRequiresConfirmation, SignalReceived were similar additions).
  Library consumers already handle enum growth.
- `advanced` costs nothing to keep. It can be removed at a natural version
  boundary without urgency.
- The mechanical changes are straightforward; no complex logic involved.
