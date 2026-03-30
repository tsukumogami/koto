# Lead: What behavioral invariants does the test suite encode that aren't in any doc?

## Findings

### 1. Cycle Detection Semantics (unit test only)

The `cycle_detection` test in `src/engine/advance.rs` (line 980) pins down a subtle invariant: the **starting state is NOT added to the visited set**. This means a -> b -> a is legitimate (the engine re-visits the starting state), but a -> b -> a -> b triggers CycleDetected on the second visit to b.

The comment at line 187 explains: "The starting state was already arrived at before this invocation, so re-visiting it (e.g., in a review -> implement -> review loop) is legitimate."

This behavior is **not documented in any design doc or guide**. The DESIGN-auto-advancement-engine.md mentions "visited-state set for cycle detection" and the pseudocode shows `if visited[target]: return CycleDetected`, but never explains the starting-state exemption. The PLAN mentions "cycle detection" as a checklist item without specifying this edge case.

**Impact**: A caller building a template with intentional review loops (review -> implement -> review) would not know this works unless they tested it or read the source.

### 2. Evidence Clearing on Auto-Advance (unit test only, partially documented)

The `auto_advance_clears_evidence_for_new_states` test (line 1234) proves that evidence is scoped to the initial state only. When auto-advancing from state A (with evidence) to state B, state B starts with empty evidence. This means conditional transitions on B won't match even if the evidence would have satisfied them.

The PLAN mentions this as a checklist item ("States reached by auto-advance start with empty evidence (fresh epoch)"), and one design doc (`DESIGN-shirabe-work-on-template.md`) alludes to it. But no guide or CLI usage doc explains this to callers.

### 3. UnresolvableTransition vs EvidenceRequired (unit test, partially documented)

The `auto_advance_clears_evidence_for_new_states` test also demonstrates that when a state has conditional transitions but **no accepts block**, the engine returns `StopReason::UnresolvableTransition` (not `EvidenceRequired`). This distinction is documented in the post-implementation notes of DESIGN-unified-koto-next.md (lines 672-687) but **not in any guide or CLI usage doc**.

The cli-usage.md guide shows the five response variants but does not mention UnresolvableTransition at all. Callers encountering exit code 2 from this case would have to guess what went wrong.

### 4. Chain Limit of 100 (unit test, documented in design)

The `chain_limit_reached` test (line 1078) builds 102 linearly chaining states and confirms the engine stops at exactly MAX_CHAIN_LENGTH (100) with `StopReason::ChainLimitReached`. The DESIGN-auto-advancement-engine.md documents this (line 380-382) and the constant is defined at line 14 of advance.rs.

**However**: no guide or CLI doc tells callers what JSON output or exit code ChainLimitReached produces. The mapping from StopReason to NextResponse for this case is undocumented.

### 5. Signal Handling (unit test, documented in design)

The `signal_received_stops_loop` test (line 1173) confirms that when the shutdown AtomicBool is set before the loop starts, the engine returns immediately with the original state, `advanced: false`, and `StopReason::SignalReceived`. Documented in the design but not in any user-facing guide.

### 6. ActionRequiresConfirmation (unit test, documented in design)

The `action_requires_confirmation_stops_loop` test (line 1411) proves:
- The engine stops without advancing (`advanced: false`)
- The stop reason carries the state name, exit code, and stdout/stderr
- It stops **before** gate evaluation and transition resolution

DESIGN-default-action-execution.md documents this variant, but cli-usage.md doesn't show what the JSON output looks like for this case.

### 7. Gate-with-Evidence Fallback Mechanism (unit + functional test, partially documented)

The `gate_failed_skips_unconditional_fallback` unit test (line 670) and the functional `gate-with-evidence-fallback.feature` pin down a critical interaction:

- When gates fail on a state that has an `accepts` block, the engine does NOT return GateBlocked.
- Instead, it falls through to transition resolution with `gate_failed=true`.
- The `gate_failed` flag prevents unconditional transitions from firing, forcing the agent to submit evidence.
- If evidence matches a conditional transition, it resolves normally even with `gate_failed=true`.

The functional test shows the three-step flow: (1) gate passes -> auto-advance, (2) gate fails -> returns expects block, (3) gate fails then evidence submitted -> advances.

The `resolve_transition` doc-comment at line 371 explains the algorithm, but **no guide documents this fallback mechanism for template authors or callers**.

### 8. Action Execution Order Relative to Gates (unit tests only)

Two tests establish the order:
- `action_skipped_continues_to_gate_evaluation` (line 1466): action Skipped, then gates evaluated (and can block)
- `action_executed_continues_to_gate_evaluation` (line 1528): action Executed, then gates pass, then transition resolves

This confirms the documented order (action -> gates -> transitions) but the tests also prove:
- ActionResult::Executed does NOT stop the loop (continues to gates)
- ActionResult::Skipped does NOT stop the loop (continues to gates)
- Only ActionResult::RequiresConfirmation stops the loop

No guide documents these semantics.

### 9. `advanced` Field Edge Cases (integration tests)

Several integration tests pin down `advanced` field behavior:

| Scenario | `advanced` | Documented? |
|----------|-----------|-------------|
| Terminal state, no transitions made | `false` | No guide |
| Auto-advance through unconditional chain | `true` | PLAN mentions it |
| Evidence submission triggers advance | `true` | No guide |
| Directed transition via --to | `true` | No guide |
| Gate blocked, no advance | not tested explicitly | -- |
| Evidence required, no advance | `false` (implicit in test) | No guide |

The integration test `next_on_terminal_state_returns_done` (line 1068) explicitly asserts `advanced: false` with comment "(no event appended)" -- the only test that directly explains what `advanced` means in a specific case.

### 10. Concurrent Access / Flock (integration test only)

`concurrent_next_fails_with_lock_contention` (line 1556) proves:
- Holding an exclusive flock on the state file causes `koto next` to fail
- Exit code is 2
- Error code is `precondition_failed`
- Error message contains "already running"

The design mentions "advisory flock" but no guide documents this behavior or explains what callers should do when they encounter it.

### 11. Decisions Cleared After Rewind (functional test only)

The `rewind.feature` scenario "Decisions cleared after rewind" pins down that rewinding to a previous state clears all decisions recorded in the epoch that was rewound. This is **not documented in any design doc or guide**.

### 12. `--with-data` and `--to` Mutual Exclusivity (integration test only)

`next_with_data_and_to_mutually_exclusive` (line 975) proves these flags can't be combined, with exit code 2 and error code `precondition_failed`. The error-codes.md reference doc likely covers the error code, but the constraint itself isn't in any guide.

### 13. Payload Size Limit of 1MB (integration test, partially tested)

`next_with_data_rejects_oversized_payload` (line 1018) attempts to test a 1MB limit but acknowledges the OS kernel's MAX_ARG_STRLEN prevents actual testing via CLI. The test proves a 100KB payload passes the size check. The 1MB limit exists in code but is effectively untestable via CLI.

## Implications

1. **The `advanced` field contract is the biggest documentation gap.** The post-implementation note in DESIGN-unified-koto-next.md (#89) acknowledges the semantic overload problem but doesn't prescribe a fix. A PRD should define exactly what `advanced` means and what callers should use instead (the response variant / action field).

2. **The gate-with-evidence-fallback mechanism is a behavioral cliff.** Template authors who add both `gates` and `accepts` to a state are opting into a complex interaction that's only discoverable through the source code or failing tests.

3. **Stop reasons that aren't mapped to CLI output are invisible to callers.** ChainLimitReached, CycleDetected, SignalReceived, and ActionRequiresConfirmation all have well-tested engine behavior but no documented JSON output shape in the cli-usage guide.

4. **The starting-state exemption for cycle detection is load-bearing.** Templates with review loops depend on this, but there's no documentation explaining it works or why.

## Surprises

1. **The `advanced` field semantic overload was already identified as a problem** (#89 post-implementation note) but never resolved. The note says "the response variant is the authoritative signal" -- this should be the central message of the PRD.

2. **UnresolvableTransition maps to exit code 2** but isn't mentioned in cli-usage.md at all. This means callers can get an error response that doesn't match any documented variant.

3. **The `auto_advance_clears_evidence_for_new_states` test reveals a deliberate design choice** that forces multi-state evidence chains to be impossible. Each auto-advanced state starts fresh. This prevents a common misunderstanding where callers might expect evidence to propagate.

4. **Decisions are cleared on rewind** -- a behavior tested only in the functional feature file, not in any unit test or design doc. This has significant implications for agents that record decisions and then encounter a rewind.

## Open Questions

1. Should the PRD recommend deprecating the `advanced` field, or redefine its semantics more precisely? The post-implementation note suggests the response variant is authoritative, but `advanced` is still present in every response.

2. How should ChainLimitReached, CycleDetected, and SignalReceived be surfaced in CLI output? They're engine stop reasons with no documented NextResponse mapping in the guides.

3. Is the starting-state exemption for cycle detection intentional design or an implementation artifact? Should templates explicitly declare which cycles are allowed?

4. Should the gate-with-evidence-fallback mechanism be documented as a first-class template pattern, or should it be discouraged?

5. What is the expected caller behavior when receiving UnresolvableTransition? The only recourse appears to be fixing the template.

## Summary

The test suite encodes at least 13 behavioral invariants that are either completely undocumented or only partially captured in design docs but missing from user-facing guides -- most critically, the starting-state exemption for cycle detection, evidence clearing on auto-advance, the gate-with-evidence-fallback interaction, and the `advanced` field's actual semantics across all stop reasons. The biggest implication for the PRD is that the `advanced` field semantic overload was already identified as a known problem (#89) but never resolved, and the design docs already state that the response variant (not `advanced`) is the authoritative signal for caller behavior. The key open question is whether the PRD should deprecate `advanced` outright or redefine it with precise, per-variant documentation that eliminates the current ambiguity.
