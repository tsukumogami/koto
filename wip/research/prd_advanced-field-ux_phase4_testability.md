# Testability Review

## Verdict: PASS

The acceptance criteria are specific, binary, and mechanically verifiable. A tester could write a complete test plan from the AC alone.

## Untestable Criteria

None of the 12 acceptance criteria are untestable. Every criterion specifies a concrete, observable condition (field presence, exit code value, JSON structure, behavioral outcome) that can be verified with automated tests.

## Missing Test Coverage

1. **R4 caller decision tree -- passthrough state (no expects, no blocking_conditions, no integration)**: The AC covers the six response shapes and several edge cases, but there is no AC verifying the passthrough behavior (action "execute" with none of the distinguishing fields). A caller hitting this path has no AC to confirm the expected response shape or that calling `koto next` again advances correctly.

2. **R5 error exit codes -- per-failure mapping**: AC5-7 verify that `template_error`, `persistence_error`, and `concurrent_access` error codes exist, but no AC verifies the full exit-code-to-failure mapping table. For example, there is no AC confirming that `precondition_failed` returns exit 2, or that `invalid_submission` returns exit 2 for evidence validation failures. Only three of eleven error codes have explicit AC.

3. **R9 SignalReceived transparency**: No AC covers signal handling behavior. The PRD states the response degrades gracefully to the stopped-at state's shape, but there is no criterion to verify this. Testing signal delivery is harder but not impossible (send SIGTERM during a multi-step chain, verify the response is valid for the interrupted state).

4. **R10 backward compatibility**: No AC verifies that callers ignoring `blocking_conditions` and new error codes continue to work. This could be tested by running an older caller integration test against the new output.

5. **R11 structured error format migration**: AC10 says "all error responses use the structured NextError format," but no AC specifies what happens to existing unstructured error paths. A test would need to enumerate all error-producing code paths and verify none emit the old `{"error": "<string>", "command": "next"}` shape.

6. **R7 `--to` validation**: AC9 covers no auto-advancement and no gate evaluation, but no AC verifies that `--to` with an invalid target returns the correct error code (`precondition_failed`, exit 2) or that the directed_transition event is appended to the workflow log.

7. **R3 `advanced` consistency for `--with-data`**: AC12 tests `--to` returning `advanced: true`, and AC11 tests repeated gate-blocked returning `advanced: false`, but no AC explicitly tests `--with-data` setting `advanced: true` when evidence submission triggers a transition.

8. **R6 empty vs. populated `blocking_conditions`**: AC8 verifies `blocking_conditions` exists on `EvidenceRequired`, but no AC distinguishes between the empty-array (normal evidence request) and populated-array (gate-failure override) cases. Both paths need separate test cases.

## Summary

The PRD's acceptance criteria are well-written and mechanically testable -- none require subjective judgment or are too vague to verify. The gap is coverage breadth: 12 criteria cover the most important behaviors, but the PRD has 11 requirements and the AC concentrate on R1-R3, R5-R6, and R8. Signal handling (R9), backward compatibility (R10), full error code mapping (R5 detail), and several edge cases within covered requirements lack explicit AC. Adding 5-6 more criteria would close the coverage gap without inflating the list unreasonably.
