# Clarity Review

## Verdict: PASS
The PRD is unusually precise for a contract specification -- requirements are concrete, acceptance criteria are binary, and the decision tree is unambiguous. A few minor ambiguities exist but none that would cause two developers to build meaningfully different things.

## Ambiguities Found

1. **R4, bullet 3 (action: "execute" with integration)**: "install the runner or use `--to` to skip" -> What does "install the runner" mean in practice? Is this something koto does, or is it a manual step the caller must take outside koto? A caller implementor would need to guess what "install" entails. -> Clarify whether this is an out-of-band manual action or a koto command, and whether there's a specific mechanism to detect which runner is missing.

2. **R4, bullet 2 (action: "confirm")**: "review the result, submit evidence via `--with-data` if `expects` is present, otherwise call `koto next` again" -> What does "review the result" mean for an AI agent? Is this a human-in-the-loop step, or should the agent parse `action_output` and make a programmatic decision? The word "review" is subjective. -> Clarify what the expected programmatic behavior is: does the agent always approve, or does it evaluate `action_output` against some criteria?

3. **R5, error table**: "Template/structural error" and "Infrastructure error" both map to exit code 3. -> If a caller dispatches on exit code alone (which R5 implies they should), these two categories are indistinguishable. The caller action column says the same thing ("Report to user"). If they're truly identical from the caller's perspective, consider collapsing them into one category in the table to avoid confusion. -> State explicitly whether callers should distinguish these via the `code` string field, or whether exit code 3 is a single "not your problem" bucket.

4. **R6**: "A populated array means gates failed but evidence can override" -> Does "evidence can override" mean submitting any valid evidence will bypass the gate, or that the evidence is evaluated against the gate conditions specifically? The word "override" is ambiguous about the mechanism. -> Clarify whether evidence submission causes gates to be re-evaluated, or whether evidence acceptance unconditionally advances past the gate.

5. **R9 (SignalReceived)**: "The response degrades to whichever shape fits the state the engine stopped at" -> The word "degrades" is imprecise. Does this mean the response is a fully valid response for the stopped-at state, or a partial/degraded response? The next sentence says "the response is valid" which contradicts "degrades." -> Replace "degrades" with "resolves to" or "falls back to" and confirm the response is complete and valid, not partial.

6. **R4, bullet 6 (passthrough state)**: "action: `execute` with no `expects`, no `blocking_conditions`, no `integration` -> Passthrough state. Call `koto next` again." -> Is this actually reachable? If auto-advancement handles passthrough states internally, callers should never see this. If it IS reachable (e.g., after `--to`), say so explicitly. If it's a defensive fallback, say that. -> Clarify under what conditions a caller would actually receive this response shape.

7. **Known limitations, bullet 4**: "The `name` field can contain either a clean integration name or an error message" -> This is a type-level ambiguity the PRD acknowledges but doesn't resolve. Two developers might handle this differently (one might parse the name field for error patterns, another might ignore it). -> Consider adding a field like `integration.error` to carry failure details, or at minimum specify how callers should detect which kind of value `name` contains.

8. **Acceptance criteria, bullet 11**: "Calling `koto next` twice on a gate-blocked state (without fixing gates) returns `advanced: false` on the second call" -> What does the FIRST call return for `advanced`? If the first call also returns `advanced: false`, this criterion is trivially true and doesn't test idempotency. If the first call returns `advanced: true` (because auto-advancement moved to this state), the criterion tests something meaningful. -> Specify the expected `advanced` value for both the first and second calls.

## Suggested Improvements

1. **Add a concrete JSON example for each response shape**: R1 lists shapes in a table but never shows actual JSON. Two developers could agree on the table and still disagree on nesting, field ordering, or whether `expects` is a sibling of `action` or nested under `directive`. A single canonical example per shape would eliminate structural ambiguity.

2. **Define the passthrough response shape explicitly in R1**: R4 bullet 6 describes a "passthrough" case that doesn't appear in the R1 table. Either add it as a seventh shape or explain why it's excluded (auto-advancement handles it internally and callers never see it).

3. **Clarify "caller" throughout**: The PRD uses "caller" to mean both AI agents and human CLI users. The decision tree in R4 is clearly agent-oriented, but some language (like "review the result") implies human judgment. Pick one primary audience or note where behavior differs.

## Summary

This is a strong contract specification. The response shape catalog, error code taxonomy, and decision tree are concrete enough that two developers would build compatible implementations. The eight ambiguities found are mostly edge cases and terminology issues rather than structural gaps. The most impactful improvement would be adding canonical JSON examples for each response shape -- this single addition would resolve most remaining interpretation risk.
