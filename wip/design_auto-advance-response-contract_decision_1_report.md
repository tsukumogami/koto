<!-- decision:start id="advance-until-stop-double-call" status="assumed" -->
### Decision: Eliminating the double-call pattern in advance_until_stop()

**Context**

The engine's `advance_until_stop()` loops through states with unconditional transitions, stopping when it hits evidence requirements, gate failures, terminal states, integrations, or cycle detection. When the engine stops at a state with conditional transitions but no `accepts` block, it returns `StopReason::EvidenceRequired`. The CLI then maps this to an `EvidenceRequired` response with empty expects -- a signal that means nothing to the caller. The caller has to call `koto next` again to get the engine to re-evaluate, producing the double-call pattern.

The root cause is that `EvidenceRequired` conflates two semantically different situations: "this state needs agent input and here's what it accepts" vs. "this state has conditionals that didn't match and no way to submit evidence." The engine reports both the same way.

**Assumptions**
- The `accepts` field on `TemplateState` is the canonical indicator of whether a state expects agent-provided evidence. The engine can use `accepts.is_some()` as a reliable signal without understanding the accepts schema itself.
- The constraint "fix belongs in the engine layer" means the primary behavioral change goes in `advance_until_stop`, but downstream CLI changes to handle the new signal are expected and acceptable.
- Library consumers who match on `StopReason` will need to handle a new variant, but this is an acceptable cost since the enum is already extended when new features land (e.g., `ActionRequiresConfirmation`, `SignalReceived`).

**Chosen: Engine-layer accepts awareness with new StopReason variant (Option B)**

Add an `accepts.is_some()` check in the `advance_until_stop` loop at the point where `resolve_transition()` returns `NeedsEvidence`. When accepts is `None`, instead of returning `StopReason::EvidenceRequired`, return a new variant -- something like `StopReason::Stuck` or `StopReason::UnresolvableTransition` -- that tells callers "the engine can't advance and the state doesn't accept input."

Concretely, in `advance.rs` after line 338:

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

The CLI maps `UnresolvableTransition` to a response that clearly communicates the state is stuck rather than waiting for input. This eliminates the synthetic empty-expects response and gives callers an unambiguous signal.

**Rationale**

Option B is the only alternative that satisfies the constraint of fixing this in the engine layer while giving library consumers the same benefit the CLI gets. The engine already accesses `template_state.gates` and `template_state.integration` for its stopping decisions; adding `accepts` is consistent with existing patterns, not a new kind of coupling.

Options A and C both place the fix in the CLI, which means library consumers calling `advance_until_stop` directly still face the double-call problem. They also introduce cycle-safety risks by resetting the visited set between re-invocations.

The new `StopReason` variant is a breaking change for exhaustive matches, but this is consistent with how the enum has grown (`ActionRequiresConfirmation` and `SignalReceived` were both additions). Consumers already need to handle new variants as the engine evolves.

**Alternatives Considered**

- **Option A: Post-loop continuation (CLI-layer loop)**. After `advance_until_stop` returns EvidenceRequired with empty expects, the CLI re-invokes the engine. Rejected because it violates the "engine layer" constraint, doesn't help library consumers, and resets cycle detection between calls (creating an infinite-loop risk for templates with A -> B -> A patterns where B has no accepts).

- **Option C: Re-dispatch loop in handle_next**. Functionally equivalent to Option A -- the CLI detects `advanced: true` + empty expects and re-invokes the engine. Same problems: CLI-layer fix, no library consumer benefit, cycle-safety risk. The "strict error" sub-variant (treat no-accepts + NeedsEvidence as a dead-end error) has merit as a template validation check but doesn't eliminate the double-call for existing valid templates.

**Consequences**

- `StopReason` gains a new variant. Library consumers updating to the new version must add a match arm.
- The engine now reads `template_state.accepts` during the advance loop. This is a new dependency but a lightweight one (just checking `is_some()`).
- The CLI's `handle_next` mapping for `EvidenceRequired` simplifies: it no longer needs the "no accepts block" fallback path that synthesizes empty expects. The empty-expects `EvidenceRequired` response type may be removable entirely.
- Templates with conditional transitions but no accepts block and no unconditional fallback will produce a clear "stuck" response instead of a misleading "evidence required" response. Template authors get better error signals.
<!-- decision:end -->
