# Exploration Findings: advanced-auto-advance

## Core Question

Issue #89 asks koto to auto-advance past `advanced: true` phases instead of requiring callers to double-call `koto next`. Does this fit koto's philosophy and architecture?

## Round 1

### Key Insights

- `advanced: true` is a response flag ("at least one transition occurred"), not a template-level phase type. Templates never define `advanced: true`. (engine-semantics, template-usage)
- The double-call pattern is emergent, not designed. The `advanced` field was introduced for agent-initiated changes; auto-advancement overloaded it one day later for engine-initiated transitions. (design-intent)
- The engine already auto-advances via `advance_until_stop()`. The double-call exists because the CLI returns after the engine's loop completes, requiring callers to re-invoke to classify the new state. (engine-semantics, architectural-layer)
- No state machine invariants would be lost by collapsing the calls. Transitions are recorded, evidence epochs are clean, gates still execute. (state-invariants)
- The only production consumer (work-on skill) treats `advanced: true` as mechanical retry with zero inspection. (template-usage)

### Decisions

- Issue #89's demand fits koto's philosophy: the double-call is emergent overhead, not intentional design
- No invariants are at risk from the proposed change

## Round 2

### Key Insights

- The agent-vs-engine distinction doesn't matter for any real caller. All consumers treat `advanced: true` as mechanical retry. The event log already provides full disambiguation. (agent-vs-engine)
- `advanced` becomes redundant post-auto-advance. Response variants are self-describing. (lifecycle, response-contract)
- The behavioral fix and response contract are independent concerns. (response-contract)

### Decisions

- The agent-vs-engine semantic distinction is not worth encoding in the CLI response
- The behavioral fix and response contract evolution are independent; behavioral fix can proceed first
- `advanced` field: keep for backward compat, deprecate as decision signal

## Round 3

### Key Insights

- The response should stay lean; observability belongs in the event log. Mirrors git/kubectl/docker pattern. (response-vs-eventlog)
- No production caller would consume `passed_through`. Use cases are hypothetical. (use-cases)
- `transition_count` is nearly free (existing counter); `passed_through` has real cost (Vec + N string clones). (field-comparison)
- `passed_through` wins on extensibility; `transition_count` wins on cost, simplicity, and architectural fit. (field-comparison)

### Decisions

- Response stays lean: `transition_count` for lightweight observability awareness
- Rich observability via deferred `koto state` command (already designed, post-#49)
- `passed_through` not needed in the response -- event log + koto state handle it

## Decision: Crystallize

## Accumulated Understanding

Issue #89 is directionally correct but misframes the problem. There are no "advanced phases" in templates -- `advanced` is a CLI response flag that became semantically overloaded when auto-advancement was added. The behavioral fix (extend `advance_until_stop()` to keep looping until evidence is required) is safe, fits the architecture, and breaks no invariants.

The fix involves two independent concerns:
1. **Behavioral**: Extend the engine's loop to continue past states where the caller has no work to do. This eliminates the double-call pattern.
2. **Response contract**: Add `transition_count: usize` for lightweight observability. Deprecate `advanced` as a decision signal (keep for backward compat). Rich observability deferred to `koto state` command.

The existing issue #89 captures the intent but needs refinement -- it assumes "advanced phases" are a template concept and proposes auto-advancing "past" them. The real fix is in the engine's stopping conditions and the CLI response semantics.
