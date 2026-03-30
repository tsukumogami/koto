# Completeness Review

## Verdict: PASS
The PRD is thorough and implementable with minor gaps that don't block implementation.

## Issues Found

1. **Missing "Passthrough" response shape in R1 catalog**: R4 (decision tree) references a case where `action: "execute"` has no `expects`, no `blocking_conditions`, and no `integration` -- a passthrough state. But R1's response shape table doesn't list this as a named variant. An implementer would need to infer that this is a valid response shape not covered by any row in the table. Add a "Passthrough" row to R1 or explicitly state that the six shapes are exhaustive and the passthrough case falls under one of them (likely a bare `execute` with only a directive).

2. **No AC for R7/R8 (`--to` behavior beyond gate/auto-advancement)**: R7 specifies five behaviors for `--to` (validate target, append event, return shape, no auto-advancement, no gate evaluation). AC only covers "no auto-advancement or gate evaluation" (one bullet). The other three (validate target returns error on illegal target, append directed_transition event, return target state's shape) have no corresponding AC. Add ACs for illegal `--to` target returning `precondition_failed` exit 2, and for the directed_transition event appearing in the workflow log.

3. **No AC for R9 (SignalReceived transparency)**: R9 specifies that SIGTERM/SIGINT produces a normal-looking response for the stopped-at state. No acceptance criterion tests this. Add an AC like: "When SIGTERM interrupts an advancement chain, the response is a valid shape for the state the engine stopped at, with no interruption indicator."

4. **No AC for R3 (`advanced` consistency across invocation modes)**: R3 defines `advanced` semantics per code path. The only AC touching `advanced` per-mode is the `--to` case (AC 12). There's no AC for `--with-data` setting `advanced: true` when evidence triggers a transition, or for bare `koto next` setting `advanced: false` when no transition occurs. Add ACs for each invocation mode.

5. **No AC for R10 (backward compatibility)**: R10 states changes are additive and existing callers continue to work. No AC verifies this. Consider adding: "Callers that don't inspect `blocking_conditions` or new error codes receive no regressions in existing response shapes."

6. **`gate_blocked` and `integration_unavailable` error codes at exit 1 conflict with R1 success shapes**: R1 lists GateBlocked and IntegrationUnavailable as success responses (exit 0). R5 lists `gate_blocked` (exit 1) and `integration_unavailable` (exit 1) as error codes. The PRD doesn't explain when the same condition produces a success shape vs. an error code. Clarify: are these for different code paths (e.g., `gate_blocked` error for the dispatch path vs. GateBlocked success shape for the advancement loop)? The R5 table hints at this with "(from dispatch path)" but the distinction needs a sentence of explanation.

7. **StopReason -> NextResponse mapping omitted**: The scope document lists "StopReason -> NextResponse mapping" as in-scope, but the PRD doesn't include an explicit mapping table. The information is spread across R1, R4, and R6. An implementer familiar with the engine's StopReason enum would benefit from a direct mapping. This is a scope coverage gap, though the information is inferrable.

8. **No JSON examples**: R1 says response shapes should be cataloged with "exact JSON structure" (Goal 1). The PRD describes field presence rules in prose but provides no example JSON payloads. An implementer could still build from the field descriptions, but the goal as stated isn't fully met.

## Suggested Improvements

1. **Add a "Response shape examples" appendix**: Even abbreviated JSON snippets for each of the six shapes would eliminate ambiguity about field nesting, null vs. absent, and array vs. object distinctions. This directly serves Goal 1.

2. **Clarify the passthrough case in R1**: Either add a seventh row to the table or add a note that a bare `execute` response with only `directive` (no `expects`, `blocking_conditions`, or `integration`) is the passthrough case referenced in R4.

3. **Expand ACs to cover R3, R7, R9, and R10**: These requirements are well-specified but lack binary pass/fail acceptance criteria. The current 12 ACs cover R1, R2, R4, R5, R6, R8, and R11. Adding 4-5 more ACs would close the gap.

4. **Add a one-sentence explanation for the exit-0 vs. exit-1 duality of gate_blocked/integration_unavailable**: A parenthetical in R5 noting "GateBlocked as a success shape means the advancement loop stopped at a gate; `gate_blocked` as an error means the dispatch path hit a gate before entering the loop" would prevent confusion.

## Summary

The PRD is well-structured and addresses all research leads from the scope document. An implementer could build the contract from this spec with minimal guesswork. The main gap is that 4 of 11 requirements (R3, R7, R9, R10) lack corresponding acceptance criteria, and the exit-0/exit-1 duality for gate_blocked and integration_unavailable needs a clarifying sentence. These are completeness issues, not correctness issues -- the requirements themselves are sound.
