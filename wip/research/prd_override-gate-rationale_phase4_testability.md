# Testability Review

## Verdict: PASS

All ten acceptance criteria are concrete, verifiable, and cover both happy paths and key edge cases. A test plan can be written directly from the AC without consulting the requirements section.

## Untestable Criteria

None. Every criterion specifies observable behavior with clear inputs, outputs, and verification methods:

- AC1 (missing rationale -> exit code 2): Testable via CLI invocation, check exit code.
- AC2 (rationale present -> two events emitted): Testable by inspecting JSONL log after invocation.
- AC3 (event field completeness): Testable by parsing the event JSON and asserting field presence.
- AC4 (non-blocked state, no rationale required): Testable via CLI invocation on a passing-gates state.
- AC5 (overrides list returns JSON): Testable via `koto overrides list` output parsing.
- AC6 (override survives rewind): Testable via override -> advance -> rewind -> query sequence.
- AC7 (re-override after rewind): Testable by counting override events after rewind + re-submit.
- AC8 (backward compat): Testable by running existing workflows without rationale flag.
- AC9 (partial gate failure, 3 gates / 2 fail): Testable with a purpose-built template and assertion on failed gate list.
- AC10 (rationale on non-blocked is no-op): Testable via CLI invocation, verify no override event and no error.

## Missing Test Coverage

1. **Empty or whitespace-only rationale string.** R2 says "mandatory rationale" and Known Limitations mentions "non-empty string," but no AC verifies that `--rationale ""` or `--rationale "   "` is rejected. Should add: "Submitting evidence with an empty or whitespace-only `--rationale` returns a validation error."

2. **Event ordering (R11).** The requirements specify strict ordering (EvidenceSubmitted before GateOverrideRecorded with sequential sequence numbers), but no AC verifies this. Should add: "In the JSONL log, `EvidenceSubmitted` precedes `GateOverrideRecorded` with strictly increasing sequence numbers within the same invocation."

3. **`derive_overrides` function directly.** AC5 tests the CLI surface (`koto overrides list`), but R6 specifies a `derive_overrides` library function. No AC covers the programmatic API. If this is intended to be a public Go API, it deserves a unit-level criterion or at least acknowledgment that the CLI test implicitly covers it.

4. **Override event emitted only on successful transition (D4).** The decisions section states override events are only emitted when evidence actually resolves a transition. No AC tests the negative case: submitting evidence with rationale that doesn't match any transition should NOT produce a `GateOverrideRecorded` event.

5. **Multiple sequential overrides without rewind.** AC7 covers re-override after rewind, but there's no AC for the scenario where an agent overrides gate A, advances to B (also gate-blocked), and overrides gate B. The list query should return both.

## Summary

The acceptance criteria are well-written and directly testable. Each one specifies a concrete input, action, and observable output. The main gaps are around boundary validation (empty rationale), event ordering guarantees, and a few negative-path scenarios documented in the requirements and decisions sections but not reflected in the AC. Adding 3-4 criteria would close these gaps.
