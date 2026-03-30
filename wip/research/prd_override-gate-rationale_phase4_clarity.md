# Clarity Review

## Verdict: PASS

This PRD is well-structured with specific, testable requirements and binary acceptance criteria. The ambiguities found are minor and unlikely to cause divergent implementations.

## Ambiguities Found

1. **R2 "validation error"**: "Evidence submission without rationale on a gate-blocked state is rejected with a validation error." The acceptance criteria pin this to exit code 2, but the requirement itself doesn't specify the error code, error message format, or whether the error is returned before or after any side effects (e.g., is evidence written to the log before rejection?). -> Two implementers might disagree on whether a partial write occurs. -> Clarify: "Rejected before any events are emitted. No EvidenceSubmitted event is written. CLI exits with code 2 and a message indicating rationale is required."

2. **R5 "--rationale on non-blocked state"**: "It's ignored (or optional) when the state isn't gate-blocked." The parenthetical "(or optional)" introduces uncertainty about behavior. The acceptance criteria later say it's "accepted without error (no-op, doesn't produce override event)" which resolves this, but the requirement text itself is ambiguous about whether the flag is silently ignored or stored somewhere. -> Clarify: Remove the "(or optional)" parenthetical from R5 and state definitively: "The flag is silently ignored when the state isn't gate-blocked."

3. **R3 "gate result details"**: The event includes "result details" for failed gates. The acceptance criteria specify "exit code/timeout/error" but R3 just says "result details." -> An implementer reading only R3 might include different fields than one reading the acceptance criteria. -> Clarify R3 to enumerate the specific fields: "gate result details (exit code, timeout status, error message)."

4. **R6 "derive_overrides function (or equivalent)"**: The "(or equivalent)" hedge leaves the API surface undefined. -> Two implementers might expose this as a method on different types, or as a standalone function, or as a query parameter. -> Clarify whether this must be a `derive_overrides` function on the persistence layer (following existing `derive_*` pattern) or whether a different approach is acceptable, and if so, what constraints apply.

5. **R7 "koto overrides list (or equivalent subcommand)"**: Same hedge as R6. The command name isn't pinned. -> Two implementers might name it `koto query --overrides`, `koto overrides`, `koto overrides list`, etc. -> Pick a command name. The acceptance criteria already use `koto overrides list`, so pin that in R7.

6. **R1 "evidence resolves a transition"**: This phrase assumes the reader knows what "resolving a transition" means in the engine. If evidence is submitted but doesn't match any transition's requirements, is that a failed override attempt or just a failed evidence submission? -> The "Decisions and trade-offs" section (D4) clarifies this, but R1 itself could be misread. -> Add a brief parenthetical: "resolves a transition (i.e., the evidence satisfies at least one outgoing transition's requirements, causing the workflow to advance)."

7. **"non-empty string" rationale validation (Known Limitations)**: The known limitations section says "The engine requires a non-empty string" but no requirement specifies this validation rule. Is whitespace-only considered non-empty? -> Clarify in R2: "Rationale must be a non-empty, non-whitespace-only string."

8. **R11 event ordering**: "Evidence first, override second" is clear, but doesn't address where `TransitionCompleted` (or equivalent state-change event) falls in the sequence. If three events are emitted in one invocation, the relative ordering of all three matters for consumers. -> Clarify the full event sequence for an override invocation.

## Suggested Improvements

1. **Add an event schema example**: Include a sample `GateOverrideRecorded` JSON payload in the PRD. This eliminates field-naming ambiguity and gives implementers a concrete target. Even a "representative, not normative" example would reduce interpretation variance.

2. **Specify error message format for R2**: The acceptance criteria test exit code 2, but agents and skill authors will parse or display the error message. Specifying the message format (or at least the key information it must contain) prevents fragile string matching later.

3. **Clarify the relationship between R1 and R4**: R1 says the engine emits the event; R4 says the event is self-contained. These are related but could be merged or cross-referenced more explicitly to avoid the reader needing to synthesize them.

4. **Pin the "(or equivalent)" hedges in R6 and R7**: The acceptance criteria already use specific names (`derive_overrides`, `koto overrides list`). Promote those to the requirements so the two sections don't conflict.

## Summary

The PRD is above average in specificity. Requirements are enumerated, acceptance criteria are binary and testable, and the out-of-scope section is thorough. The main ambiguities are soft hedges ("or equivalent", "or optional") that the acceptance criteria already resolve -- these should be tightened in the requirement text for consistency. The lack of a sample event schema is the most likely source of implementation divergence.
