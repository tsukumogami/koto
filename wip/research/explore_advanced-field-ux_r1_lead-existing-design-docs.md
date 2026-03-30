# Lead: What do existing design docs already specify about auto-advancement behavior?

## Findings

### 1. DESIGN-unified-koto-next.md (strategic design)

**What it specifies about auto-advancement:**
- Defines the advancement loop pseudocode in the Data Flow section: visited-set cycle detection, terminal/integration/gate/accepts stopping conditions, and the fsync-per-event append pattern.
- Specifies that states with no `accepts` block, no `when` conditions, and passing gates are auto-advanced through.
- Lists the six event types including `transitioned` with `condition_type: "auto"` for engine-initiated transitions.

**`advanced` field semantics:**
- Introduces `advanced: bool` in the CLI Output Schema section. The field appears in all four response shapes (execute with evidence, gate-blocked, integration, terminal).
- Does NOT define its semantics in the original text. However, a post-implementation note (added later, referencing #89) acknowledges the semantic overload: "The `advanced: bool` field was designed to report agent-initiated changes ('true when an event was appended before dispatching'). When the auto-advancement engine was added, the field was overloaded to also report engine-initiated transitions."
- The post-implementation note explicitly states: "The response variant (EvidenceRequired, GateBlocked, Terminal) is the authoritative signal for what the caller should do -- not the `advanced` field."

**Response shapes:**
- Shows five JSON examples: evidence-required, gate-blocked, integration available, integration unavailable, terminal.
- Does NOT document what callers should DO with each shape. The shapes are presented as output examples, not as a caller contract.

**Edge cases:**
- Mentions cycle detection (visited-set) and signal handling in the Phase 4 deliverables but does not specify behavior details (those are deferred to the tactical design).
- Does NOT specify chain limits.

**What's missing:**
- No formal definition of what `advanced` means for callers.
- No caller decision tree ("when you see this shape, do this").
- No specification of the `UnresolvableTransition` stop reason (added post-hoc in #89 note).

### 2. DESIGN-auto-advancement-engine.md (tactical design)

**What it specifies about auto-advancement:**
- Full loop specification: `advance_until_stop()` signature with I/O closures, per-iteration order (shutdown check, cycle detection, terminal, integration re-invocation prevention, integration invocation, gate evaluation, transition resolution, event append).
- `StopReason` enum: `Terminal`, `GateBlocked`, `EvidenceRequired`, `Integration`, `IntegrationUnavailable`, `CycleDetected`, `ChainLimitReached`, `SignalReceived`.
- `AdvanceResult` struct: `final_state: String`, `advanced: bool`, `stop_reason: StopReason`.
- `TransitionResolution` enum: `Resolved`, `NeedsEvidence`, `Ambiguous`, `NoTransitions`.
- Chain limit of 100 transitions per invocation.
- Evidence merging: last-write-wins within epoch.
- Re-invocation prevention for integrations.
- Signal check granularity: between iterations, not within gate evaluations.
- Cancellation detection: pre-loop check for `WorkflowCancelled` event.

**`advanced` field semantics:**
- `AdvanceResult.advanced` is defined as: "true if any transitions were made."
- The functional test scenario shows `advanced: true` in the response when auto-advancement occurred (plan -> implement -> verify chain).
- Does NOT define what callers should do when `advanced` is true vs. false.

**Response shapes:**
- Shows two example responses in the functional test scenario (initial auto-advance, evidence submission triggering another chain).
- Specifies `StopReason` -> `NextResponse` mapping as a handler responsibility but does NOT document the mapping rules.

**Edge cases covered:**
- Cycle detection: `CycleDetected { state }`.
- Chain limit: `ChainLimitReached` after 100 transitions.
- Signal handling: `SignalReceived` when `AtomicBool` is set.
- Concurrent access: advisory flock with non-blocking error.
- Cancellation: pre-loop check.
- Re-invocation prevention for integrations.

**What's missing:**
- No caller-facing contract. The design specifies engine internals but not what callers see or should do.
- No mapping from `StopReason` to the specific JSON response shapes callers receive.
- No mention of `UnresolvableTransition` (this was added in #89 after implementation).

### 3. DESIGN-koto-cli-output-contract.md (tactical design)

**What it specifies about auto-advancement:**
- Explicitly out of scope: "auto-advancement loop, integration runner, `koto cancel`, and signal handling are deferred to #49."
- Covers single-step dispatch only. The `dispatch_next()` function classifies one state into a response variant.

**`advanced` field semantics:**
- `advanced` appears in all five `NextResponse` variants.
- Issue 4 acceptance criteria: "`advanced` field is populated by the caller (true when an event was appended before dispatching)."
- This is the pre-auto-advancement definition. Once the auto-advancement engine was integrated, the "event was appended before dispatching" became ambiguous -- did the agent append it (`--with-data`) or did the engine append it (auto-advance)?

**Response shapes:**
- Field presence table documents exactly which fields appear in which variant (EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable, Terminal).
- Custom serialization rules: "no" = absent from JSON, "null" = present with null value.
- This is the most complete specification of response shapes.

**Edge cases:**
- Error codes: `gate_blocked`, `invalid_submission`, `precondition_failed`, `integration_unavailable`, `terminal_state`, `workflow_not_initialized`.
- Exit code mapping: 0 success, 1 transient, 2 caller error, 3 config error.
- Stale submission handling: validates against current state, no client-side state assertion.

**What's missing:**
- No documentation of what callers should do with each response variant.
- No mention of auto-advancement response shapes (since auto-advancement was out of scope).

### 4. PLAN-auto-advancement-engine.md

- Implementation plan, not behavioral spec. Lists acceptance criteria for each issue.
- Issue 3 acceptance criteria: "`advanced` field in `NextResponse` reflects whether transitions were made." This is consistent with the design but adds no additional semantic clarity.
- Does not add behavioral specification beyond what the design covers.

### 5. PLAN-koto-cli-output-contract.md

- Implementation plan. Lists acceptance criteria matching the design.
- Issue 4: "`advanced` field is populated by the caller (true when an event was appended before dispatching)."
- No additional behavioral specification.

### 6. Post-implementation note in DESIGN-unified-koto-next.md (the #89 fix)

This is the most relevant existing documentation of the problem:
- Acknowledges that `advanced` was overloaded when auto-advancement was added.
- States that the response variant is the authoritative signal, not `advanced`.
- Documents `StopReason::UnresolvableTransition` as a fix for states with conditional transitions but no `accepts` block.
- This note is buried in the strategic design doc's post-implementation section and is easy to miss.

## Implications

**The existing docs specify the engine's internal behavior thoroughly but never specify the caller-facing contract.** There is no document that answers: "When `koto next` returns shape X, what should the caller do?" The designs specify what the engine produces (StopReason, NextResponse variants, JSON shapes) but not how callers should interpret those outputs.

This gap is the root cause of issue #102. The `advanced` field was defined internally ("true if transitions were made") but its caller-facing meaning was never pinned down. When the auto-advancement engine started setting it to true for engine-initiated transitions, callers had no specification to consult and fell back on the English meaning of "advanced" -- which is ambiguous.

A PRD for the auto-advancement behavioral contract needs to cover:

1. **Caller decision tree**: For each response variant, what should the caller do?
2. **`advanced` field semantics**: Rename or redefine so callers can distinguish "new phase entered" from "same phase, no progress."
3. **Complete response shape catalog**: Enumerate every shape callers can see, including edge cases like `UnresolvableTransition`.
4. **StopReason -> NextResponse mapping**: Currently undocumented; lives only in code.

## Surprises

1. **The `advanced` field was originally designed for a different purpose.** It was meant to signal "an event was appended before dispatching" (i.e., `--with-data` was processed). The auto-advancement engine repurposed it to mean "any transitions were made." The post-implementation note in DESIGN-unified-koto-next.md acknowledges this but doesn't fix it -- it just says "use the response variant instead."

2. **`UnresolvableTransition` is documented only in a post-implementation note.** This stop reason was added in #89 but never got its own design treatment. It's a sixth response shape that callers can encounter but it's not in the field presence table or the response type definitions in DESIGN-koto-cli-output-contract.md.

3. **No design doc specifies the StopReason -> NextResponse mapping.** The auto-advancement design returns `StopReason`; the CLI output contract design returns `NextResponse`. The translation between them is described as "a single match expression" but the actual mapping rules are only in code, not in any design doc.

4. **The strategic design's post-implementation notes are the closest thing to a behavioral contract.** They were added after the fact as errata, not as authoritative spec. They're the only place that says "the response variant is the authoritative signal, not `advanced`."

## Open Questions

1. Should the PRD propose renaming `advanced` or adding a new field? The post-implementation note suggests the response variant is sufficient, but issue #102 shows callers still rely on `advanced`.

2. Should `UnresolvableTransition` be treated as a new response variant in the field presence table, or is it an error code? The current implementation maps it to exit code 2 (`precondition_failed`), suggesting it's an error, but it represents a reachable state in well-formed-ish templates.

3. How many distinct response shapes can callers actually see today? The CLI output contract design lists five, but auto-advancement and #89 fixes may have added more. A code audit would confirm.

4. Should the caller decision tree be part of the output contract design or a separate document? It's a different audience (callers vs. implementers).

## Summary

Existing design docs thoroughly specify auto-advancement engine internals (loop mechanics, stopping conditions, edge cases) but never define the caller-facing behavioral contract -- no document says "when you see response shape X, do Y." The `advanced` field's semantics drifted from "an event was appended" to "transitions were made" without updating any spec, and the only acknowledgment is a post-implementation note in the strategic design that says callers should ignore `advanced` and use the response variant instead. The biggest open question is whether the PRD should rename/redefine `advanced` or formalize the post-implementation guidance that response variants are the authoritative signal.
