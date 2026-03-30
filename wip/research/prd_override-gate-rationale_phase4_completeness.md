# Completeness Review

## Verdict: PASS

The PRD is implementable as-is. The issues below are clarifications that would reduce ambiguity, not blockers.

## Issues Found

1. **No AC for the `derive_overrides` function (R6)**: R6 specifies a `derive_overrides` function following the `derive_*` pattern in persistence.rs. There is no acceptance criterion that directly tests this function's behavior (e.g., returns events across epochs, excludes non-override events, handles empty logs). The AC for `koto overrides list` covers the user-facing surface, but the library function contract is untested. Add an AC like: "`derive_overrides` returns all `GateOverrideRecorded` events regardless of epoch boundaries, including events from states that were later rewound past."

2. **R3/R4 don't specify gate result detail fields**: The PRD says the event includes "gate result details (exit code/timeout/error)" but doesn't define the schema shape. An implementer must decide whether this is a nested object per gate, a flat list, or something else. The existing `GateResult` type in the codebase likely dictates this, but the PRD should reference it or specify the structure. Suggest adding: "Gate failure details use the same structure as the `gate_results` field in the `GateBlocked` CLI response."

3. **Event serialization format not specified**: The `EventPayload` enum uses `#[serde(untagged)]` deserialization. Adding a new variant to an untagged enum requires careful field design to avoid ambiguity with existing variants. The PRD should note that `GateOverrideRecorded` needs a field combination that is unique across all variants, or suggest switching to tagged serialization. This is an implementation risk an implementer could miss.

4. **R11 says "evidence first, override second" but no AC verifies ordering**: The AC checks that both events are emitted, but no criterion verifies their relative ordering. Add: "In the JSONL log, `EvidenceSubmitted` has a lower sequence number than `GateOverrideRecorded` for the same `koto next` invocation."

5. **No AC for rationale non-empty validation**: R2 and the Known Limitations mention the engine requires a non-empty string, but no AC tests that `--rationale ""` (empty string) is rejected. The AC only tests the absence of the flag. Add: "Submitting `--rationale ''` (empty string) on a gate-blocked state returns a validation error."

## Suggested Improvements

1. **Specify the `koto overrides list` output schema**: R7 says "formatted as JSON" but doesn't specify the shape. Even a rough sketch (array of override event objects, each containing the fields from R4) would help implementers align with future visualization consumers.

2. **Clarify interaction with `--to` when state is gate-blocked**: The PRD scopes out `--to` tracking, but doesn't say what happens if an agent uses `--to` to leave a gate-blocked state. Does that bypass the rationale requirement? The current behavior (directed transitions skip gates) should be explicitly acknowledged as unchanged by this PRD.

3. **Add a user story for the programmatic consumer**: The user stories cover skill authors, human reviewers, template authors, and visualization. Missing: "As a CI/automation consumer, I want to parse override events from the JSONL log so I can enforce override policies (e.g., fail a pipeline if too many overrides occur)." This would validate that the event schema is machine-parseable, not just human-queryable.

4. **Mention the `serde(untagged)` constraint in Decisions**: D1 discusses why a dedicated event type was chosen but doesn't mention the practical serialization constraint. Since `EventPayload` is untagged, this is a real design consideration that should be documented.

## Summary

The PRD covers the core problem thoroughly and the requirements are specific enough for implementation. The main gaps are around precise schema definitions (gate result details, output format, event field uniqueness for untagged serde) and a few acceptance criteria that don't fully cover their corresponding requirements -- particularly `derive_overrides` (R6), event ordering (R11), and empty-rationale rejection. None of these are show-stoppers; they're clarifications that prevent an implementer from making reasonable but wrong assumptions.
