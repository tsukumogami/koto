# /prd Scope: override-gate-rationale

## Problem Statement

When agents override gates in koto (submitting evidence on a gate-failed state), the override happens implicitly and the reasoning is lost. The agent can separately call `koto decisions record`, but nothing connects the override to the rationale -- they're two independent operations. This makes it impossible to answer "why was this gate overridden?" from the event log, blocking future visualization, audit, and redo capabilities.

## Initial Scope

### In Scope
- Making overrides explicit, first-class events in the JSONL event log
- Requiring mandatory rationale when overriding a gate
- Capturing gate failure context (which gate, why it failed) alongside the override
- Cross-epoch queryability for override events (not limited to current state epoch)
- CLI interface for submitting rationale at evidence submission time

### Out of Scope
- Visualization UI for override audit trails
- Redo/rewind capability triggered by disagreement with override rationale
- Evidence verification by koto (polling CI, parsing files, embedded validation calls)
- `required_when` conditional validation as a general template schema feature
- Changes to the advance loop's gate evaluation logic itself

## Research Leads

1. **What is the complete set of override scenarios?**: The exploration identified implicit override via evidence on gate-failed states. Are there other override patterns (e.g., `--to` directed transitions bypassing gates, action skip via evidence presence)?
2. **What acceptance criteria does the visualization consumer need?**: The north star is session visualization showing all overrides with rationale. What query shapes and data completeness does that require from the persistence layer?
3. **How should the CLI accept rationale?**: Evidence is submitted via `koto next --with-data`. How does rationale travel alongside it -- a separate flag, embedded in the evidence JSON, or a different mechanism?
4. **What edge cases exist in gate override detection?**: Multiple gates on a state, partial gate failure, gates that pass but with warnings -- do these produce override events?

## Coverage Notes

The exploration established strong consensus on the direction (first-class override events, mandatory rationale, not reusing the decisions subsystem) but didn't fully investigate:
- The exact boundary between "override" and "normal evidence submission" when koto doesn't yet verify evidence independently
- Whether `--to` directed transitions (which bypass gates entirely) should also be captured as override-like events
- The interaction between override events and the existing `DefaultActionExecuted` event (which already captures action skips)

## Decisions from Exploration

- Override should be a first-class event, not implicit: all research and external patterns converge on this; implicit override via evidence presence is an outlier among workflow engines
- Existing decisions subsystem should not be overloaded for override capture: agent-initiated deliberation and engine-detected gate bypass are distinct concepts with different query needs
- Rationale should be mandatory on override: consistent with decisions subsystem requiring rationale, and aligned with audit/compliance patterns
- Visualization/redo scoped out of current work: persist the data now, build on it later
- Conditional validation (required_when) scoped out as general template feature: handle override rationale at the engine level, not as schema evolution
