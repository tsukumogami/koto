# Exploration Findings: advanced-auto-advance

## Core Question

Issue #89 asks koto to auto-advance past `advanced: true` phases instead of requiring callers to double-call `koto next`. Does this fit koto's philosophy and architecture?

## Round 1

### Key Insights

- `advanced: true` is a response flag ("at least one transition occurred"), not a template-level phase type. Templates never define `advanced: true`. (engine-semantics, template-usage)
- The double-call pattern is emergent, not designed. The `advanced` field was introduced for agent-initiated changes; auto-advancement overloaded it one day later for engine-initiated transitions. The semantic collision was never discussed. (design-intent)
- The engine already auto-advances via `advance_until_stop()`. The double-call exists because the CLI returns after the engine's loop completes, requiring callers to re-invoke to classify the new state. (engine-semantics, architectural-layer)
- No state machine invariants would be lost by collapsing the calls. Transitions are recorded, evidence epochs are clean, gates still execute. (state-invariants)
- The only production consumer (work-on skill) treats `advanced: true` as mechanical retry with zero inspection. (template-usage)

### Tensions

- **Semantic clarity vs. behavioral fix.** The design-intent lead argues the root cause is that `advanced` conflates "agent caused this" with "state changed" and proposes disambiguating the field. Other leads argue the fix is behavioral: keep looping until hitting a state where the caller needs to act. Both are valid and not mutually exclusive, but they solve different problems.
- **Engine vs. CLI layer.** Architectural-layer recommends engine (Option A). Engine-semantics suggests CLI handler. No external library consumers exist to validate the distinction.

### Gaps

- The `advanced` field's future is unclear if auto-advance eliminates most cases where it's `true`
- No investigation of whether the semantic distinction (agent vs engine initiated) matters for any real caller scenario

### Decisions

- Issue #89's demand fits koto's philosophy: the double-call is emergent overhead, not intentional design
- No invariants are at risk from the proposed change

### User Focus

User wants to explore the semantic question: should `advanced` be disambiguated (agent-initiated vs engine-initiated) separately from the behavioral auto-advance fix?

## Accumulated Understanding

Issue #89 is directionally correct but built on a misunderstanding -- there are no "advanced phases" in templates. The `advanced` flag is a CLI response field that became semantically overloaded when auto-advancement was added. The behavioral fix (keep looping until evidence is required) is safe and fits the architecture. The open question is whether the `advanced` field itself needs redesigning, and whether that's a prerequisite for, consequence of, or independent concern from the auto-advance behavior change.
