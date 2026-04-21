# Crystallize Decision: auto-advance-transitions

## Chosen Type

Design Doc

## Rationale

What to build is clear (a `skip_if` predicate on template states that auto-advances deterministic transitions). How to build it surfaced three architectural decisions during exploration, each with evaluated alternatives:

1. Synthetic event format: `Transitioned` with `condition_type: "skip_if"` vs. two other options
2. Context-exists in v1: defer (use gate workaround) vs. thread ContextStore
3. Transition target selection: synthetic-evidence injection vs. require unconditional fallback

These decisions need to live in a permanent document before the wip/ branch closes. Beyond the decisions, the implementation touches four distinct areas (template schema, engine advance loop, event schema, compile-time validation) with specific ordering constraints that benefit from explicit design specification. The discovery phase also surfaced a non-obvious constraint: `has_gates_routing` detection must be extended to include skip_if references, or the context-exists workaround silently breaks.

## Signal Evidence

### Signals Present

- **What to build is clear, but how to build it is not**: The issue described skip_if at a high level; exploration determined the insertion point, condition types, event format, chaining mechanism, and target selection strategy -- none of which were specified before.
- **Technical decisions were made between approaches**: Three decision reports produced (d1, d2, d3), each with evaluated alternatives and rationale.
- **Multiple viable implementation paths explored**: Event format had 3 alternatives; transition target had 3 alternatives; context-exists had 2 alternatives.
- **Architectural decisions were made that should be on record**: `Transitioned.condition_type = "skip_if"`, deferred ContextStore threading, synthetic-evidence injection for target selection -- all need permanent documentation.
- **Core question was "how should we build this?"**: Yes, throughout.

### Anti-Signals Checked

- **What to build is still unclear**: Not present. The feature scope is well-defined.
- **No meaningful technical risk or trade-offs**: Not present. Three genuine trade-off decisions made.
- **Problem is operational, not architectural**: Not present. Pure engine/schema change.

## Alternatives Considered

- **Plan**: Ranked second (2 signals, 0 anti-signals). The implementation is understood well enough to sequence. Loses to Design Doc on the tiebreaker: no design doc exists yet, so the technical decisions haven't been captured in a permanent artifact that a Plan could reference.
- **Decision Record**: 2 signals but 1 anti-signal (multiple interrelated decisions need a design doc, not separate records). Demoted below Design Doc.
- **No Artifact**: Demoted. Architectural decisions were made during exploration that must survive the branch.

## Deferred Types

None applicable.
