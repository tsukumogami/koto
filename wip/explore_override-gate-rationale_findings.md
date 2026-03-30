# Exploration Findings: override-gate-rationale

## Core Question

How should koto capture gate overrides as first-class auditable events with rationale, so they're queryable for visualization and eventually actionable for redo?

## Round 1

### Key Insights

- **Override needs a first-class event type.** All external patterns (GitHub Actions, SOX compliance, Temporal) treat gate bypass as a distinct auditable event. Koto's current implicit override (evidence presence on gate-failed state) is an outlier. A dedicated event enables direct querying ("show all overrides") without correlating multiple event types. *(event-representation, external-patterns)*
- **Gate failure context must travel with the override.** The `BTreeMap<String, GateResult>` is already available at the override detection point in `advance_until_stop`. Preserving which gate failed and why enables future visualization of "what was overridden" alongside "why the agent overrode it." *(data-shape)*
- **The decisions subsystem is the wrong home for override rationale.** `DecisionRecorded` is agent-initiated and epoch-scoped. Override events are engine-detected and need cross-epoch queryability. Merging them conflates deliberation with bypass justification. *(decisions-interaction)*
- **Cross-epoch query access is required.** `koto decisions list` is epoch-scoped. Visualization needs "all overrides in this session." No query API exists today beyond raw JSONL parsing. *(query-patterns)*
- **Demand is validated.** Maintainer-filed with three concrete use cases, downstream workflows depend on override behavior, gate-with-evidence-fallback and decisions subsystem already provide partial infrastructure. *(adversarial-demand)*

### Tensions

- **Event representation choice.** Dedicated `GateOverrideRecorded` (most explicit, adds schema) vs. extended `EvidenceSubmitted` (simpler, mixes concerns) vs. auto-generated `DecisionRecorded` (reuses structure, conflates concepts). Evidence favors a dedicated type for queryability and forward compatibility with redo.
- **Rationale: required vs. optional.** Mandatory adds friction to every override but ensures complete audit trail. The decisions subsystem already requires rationale -- consistency argues for mandatory here too.

### Gaps

- Two research agents (data-shape, decisions-interaction) returned summaries but didn't persist full findings files
- No investigation into `--to` directed transitions and whether they constitute a different kind of override

### Decisions

- Override should be a first-class event, not implicit
- Existing decisions subsystem should not be overloaded for this
- Rationale mandatory on override
- Visualization/redo scoped out of current work (persist data only)
- Conditional validation (required_when) scoped out as general template feature

### User Focus

User confirmed findings are convergent and sufficient. No additional gaps identified. Ready to decide on artifact type.

## Accumulated Understanding

Gate overrides in koto are currently invisible in the event log. The override mechanism is implicit: submitting evidence on a gate-failed state bypasses the gate, but no event records the bypass or the agent's reasoning. The decisions subsystem exists but is the wrong fit -- it's agent-initiated, epoch-scoped, and conceptually distinct from engine-detected gate bypass.

The solution should introduce a dedicated override event type that captures: which gate failed, what evidence bypassed it, and why (mandatory rationale). This event needs to be queryable across epochs for session visualization. The gate result data is already available at the detection point in `advance_until_stop`.

Infrastructure is partially in place: gate-with-evidence-fallback handles the runtime override path, and the JSONL event system supports new event types without schema migration. The main design work is: (1) defining the override event shape, (2) determining where in the advance loop to emit it, (3) adding a cross-epoch query surface, and (4) defining how the CLI accepts rationale at evidence submission time.

## Decision: Crystallize
