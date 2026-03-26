# Architecture Review: Auto-advance and Response Contract Evolution

## Design Document

`docs/designs/DESIGN-auto-advance-response-contract.md`

## Source Files Reviewed

- `src/engine/advance.rs` -- StopReason, AdvanceResult, advance_until_stop, resolve_transition
- `src/cli/next_types.rs` -- NextResponse enum, custom Serialize impl, derive_expects
- `src/cli/next.rs` -- dispatch_next (pure dispatcher)
- `src/cli/mod.rs` -- handle_next (CLI handler, StopReason-to-NextResponse mapping)

---

## Question 1: Is the architecture clear enough to implement?

**Yes, with one gap.**

The design precisely identifies the two code changes (new StopReason variant, transition_count field) and names the exact files, functions, and insertion points. The engine-side change is a single `if` branch in the `NeedsEvidence` arm of `advance_until_stop()` at line 338 of advance.rs. The data flow diagram is accurate.

**Gap: What CLI response does UnresolvableTransition produce?**

The design says "the CLI maps UnresolvableTransition to a clear error response" but doesn't specify which `NextErrorCode` or exit code. Looking at `handle_next` (mod.rs:1129-1306), every `StopReason` maps to either a `NextResponse` variant or a `NextError` with a specific code. The implementer needs to decide:

- Is `UnresolvableTransition` a `NextError` with `PreconditionFailed` (exit 2, caller error)?
- Or a new `NextErrorCode::UnresolvableTransition` variant?

The answer matters for callers. A state with only conditional transitions and no accepts block is arguably a template authoring error (the template should have an accepts block or an unconditional fallback). That points to `PreconditionFailed` with exit code 2. The design should specify this.

---

## Question 2: Are there missing components or interfaces?

### dispatch_next dual-path problem

The design mentions simplifying the fallback in `dispatch_next` (next.rs:104-117). But `dispatch_next` is a second classification path that duplicates logic the engine now owns. After this change:

- `handle_next` uses `advance_until_stop` -> maps `StopReason` to `NextResponse` (lines 1129-1291)
- `handle_next --to` uses `dispatch_next` -> classifies template state directly (line 842)

The `--to` (directed transition) path still calls `dispatch_next`, which doesn't go through the engine's advancement loop. This is by design (directed transitions are single-shot). But `dispatch_next` still synthesizes empty expects for states with no accepts block (next.rs:108-117) -- the exact pattern the design eliminates from the engine path.

**Finding: The design should explicitly state whether `dispatch_next` also needs updating.** If a directed transition lands on a state with no accepts and only conditional transitions, `dispatch_next` will return `EvidenceRequired` with empty expects -- the same misleading signal. The engine fix doesn't cover this path.

Severity: **Advisory.** The `--to` path is used for explicit agent-directed transitions, not auto-advancement. The double-call pattern doesn't apply there because the agent explicitly chose the target. But the misleading empty-expects response is still wrong. Should be addressed, doesn't have to block.

### with_substituted_directive passthrough

Adding `transition_count` to all six `NextResponse` variants means `with_substituted_directive` (next_types.rs:67-135) needs to thread it through all six match arms. The design counts "~18 sites" but doesn't mention this specific function. It's mechanical but should be in the implementation checklist.

---

## Question 3: Are the implementation phases correctly sequenced?

**Yes.** The sequencing is sound:

- **Phase 1 (engine)** is independent. `AdvanceResult` gains `transition_count` and the new `StopReason` variant. All existing engine tests break at compile time (exhaustive matches), forcing updates. No CLI changes needed to compile -- the engine is consumed by the CLI but not the reverse.

- **Phase 2 (CLI)** depends on Phase 1. It maps the new `StopReason::UnresolvableTransition` and threads `transition_count` into responses. The dependency direction is correct: CLI imports engine, never the reverse.

- **Phase 3 (tests/docs)** depends on both. Integration tests exercise the full CLI, so they go last.

One adjustment: the design says Phase 1 deliverables include "engine unit tests updated." The advance.rs tests will need updating in Phase 1 because the `AdvanceResult` struct change will break them at compile time. This is already implicit but worth calling out -- Phase 1 is not shippable without those test updates.

---

## Question 4: Are there simpler alternatives we overlooked?

### Alternative: Don't add a new StopReason -- just continue the loop

When `resolve_transition` returns `NeedsEvidence` and `accepts.is_none()`, the engine could treat this as a dead-end and return `AdvanceError::DeadEndState` instead of a new `StopReason`. The error path already exists (advance.rs:351-354) and `handle_next` already maps `AdvanceError` to `NextError::PreconditionFailed` (mod.rs:1297-1305).

This avoids adding a variant to `StopReason` (which is a library-facing enum). The question is whether "has conditional transitions but no accepts" is semantically different from "has no transitions at all" (`NoTransitions`). They're different template bugs with different fixes, so a distinct signal is justified. The design's approach is the right one.

### Alternative: Make transition_count a response-level field only, not per-variant

Instead of adding `transition_count` to all six `NextResponse` variants (18 sites), add it as a wrapper:

```rust
pub struct NextOutput {
    pub response: NextResponse,
    pub transition_count: u64,
}
```

Serialize as a flat JSON by implementing custom `Serialize` on `NextOutput` that merges the inner response fields with `transition_count`. This reduces the mechanical change from 18 sites to 1 struct + 1 serialize impl.

Tradeoff: the custom serialization gets more complex (serialize inner, then inject field). But `NextResponse` already has a custom `Serialize` impl, so this could be a straightforward wrapper. Worth considering -- it keeps `transition_count` out of the enum entirely, which is cleaner since it's metadata about the invocation, not about the response classification.

---

## Structural Findings

### 1. Consistent with engine-owns-advancement pattern -- no issues

The design correctly extends `advance_until_stop` rather than adding logic to the CLI layer. The dependency direction (CLI -> engine) is preserved. The new `StopReason` variant follows the existing pattern (`ActionRequiresConfirmation`, `SignalReceived` were similar additions). No architectural violations.

### 2. dispatch_next divergence (Advisory)

`src/cli/next.rs:104-117` -- The `dispatch_next` function still synthesizes empty expects for no-accepts states. After this change, the engine path will return `UnresolvableTransition` for the same situation, but the `--to` directed-transition path will still return the misleading `EvidenceRequired` with empty expects. Two code paths produce different responses for the same template condition. Not blocking because `--to` is an explicit agent action (no double-call), but the design should note whether to align `dispatch_next` or leave it.

### 3. Response contract: transition_count placement (Advisory)

Adding `transition_count` to all six variants is mechanically correct but structurally noisy. The field is invocation metadata, not classification-specific. A wrapper struct (`NextOutput { response, transition_count }`) would keep the classification enum clean and reduce the mechanical change surface. This is a design preference, not a structural violation.

### 4. Missing specification: UnresolvableTransition CLI mapping (Advisory)

The design doesn't specify the exit code or error code for `UnresolvableTransition`. The implementer will need to choose between reusing `PreconditionFailed` (exit 2) or adding a new `NextErrorCode` variant. This should be decided in the design to avoid ad-hoc choices during implementation.

---

## Summary

The design fits the existing architecture cleanly. The engine-layer fix is the right place for the change, the dependency direction is correct, and the phasing is sound. No blocking findings.

Three advisory items to address before implementation:
1. Specify the CLI error mapping for `UnresolvableTransition`
2. Decide whether `dispatch_next` (the `--to` path) gets the same fix
3. Consider a wrapper struct for `transition_count` to reduce mechanical churn
