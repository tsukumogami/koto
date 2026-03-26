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

## Round 2

### Key Insights

- The agent-vs-engine distinction doesn't matter for any real caller. All consumers treat `advanced: true` as mechanical retry. The event log already provides full disambiguation. Adding `advanced_by` would expand the contract for zero behavioral gain. (agent-vs-engine)
- `advanced` becomes redundant post-auto-advance. Response variants are self-describing: EvidenceRequired, GateBlocked, Terminal each tell callers exactly what to do. (lifecycle, response-contract)
- The behavioral fix and response contract are independent concerns. Auto-advance can proceed without touching the response. Contract can be extended later with observability metadata. (response-contract)
- `advanced` should be kept for backward compat but deprecated as a decision signal. 22 integration test assertions depend on it. (lifecycle)

### Tensions

None remaining from round 1. All three leads converge: the semantic distinction is operationally irrelevant.

### Gaps

- Whether `passed_through: Vec<String>` or `transition_count: usize` is the better observability extension

### Decisions

- The agent-vs-engine semantic distinction is not worth encoding in the CLI response (event log handles it)
- The behavioral fix (auto-advance) and response contract evolution are independent; behavioral fix can proceed first
- `advanced` field: keep for backward compat, deprecate as decision signal

### User Focus

User wants to explore observability options: `passed_through` vs `transition_count` before crystallizing.

## Accumulated Understanding

Issue #89 is directionally correct. The behavioral fix (extend `advance_until_stop()` to keep looping until evidence is required) is safe, fits the architecture, and breaks no invariants. The `advanced` field's semantic overload is a separate, independent concern -- it should be deprecated as a decision signal and kept for backward compatibility.

The remaining question is how to provide observability for auto-advanced transitions in the response contract, which is needed to meet #89's acceptance criterion: "Response includes indication that advanced phase(s) were passed through."
