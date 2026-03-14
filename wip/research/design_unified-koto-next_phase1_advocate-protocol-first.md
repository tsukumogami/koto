# Advocate: Protocol-First Design

## Approach Description

The agent contract — what `koto next` outputs and accepts — is the primary design artifact.
The JSON output schema is finalized first. The state model is then designed to store exactly
what the protocol needs to produce that output. The template format is designed to declare
exactly what the protocol schema requires. Everything flows from the agent contract downward.

**Example `koto next` JSON output (proposed):**

```json
{
  "state": "analyze_results",
  "action": "execute",
  "directive": "Review the test output and determine whether to proceed or escalate.",
  "advanced": true,
  "expects": {
    "submit_with": "--with-data",
    "fields": {
      "decision": {
        "type": "enum",
        "values": ["proceed", "escalate"],
        "required": true
      },
      "rationale": {
        "type": "string",
        "required": true
      }
    },
    "options": [
      { "transition": "deploy", "when": { "decision": "proceed" } },
      { "transition": "escalate_review", "when": { "decision": "escalate" } }
    ]
  },
  "error": null
}
```

The `expects` field is the contract. Its presence signals that the agent must submit evidence.
Its `fields` describe what to submit. Its `options` describe which transition each submission
triggers. An agent reading only this output knows exactly what to do next.

## Investigation

The current CLI output is unstructured text or minimal JSON with only `action`, `state`,
`directive`, and `message` fields. There is no `expects`, no `advanced` flag, no error code
structure, no transition options. The `Directive` struct in `pkg/controller` is flat.

Under protocol-first, the JSON schema is defined up front as a formal contract. The state
model then needs to store: current state, evidence (scoped per state, cleared on exit),
version counter, history entries with archived evidence. The template format needs to
declare: per-transition conditions (the `when` clauses) and evidence field requirements
(the `fields` schema). The `expects` derivation is a compile-time computation on the
template's declared transitions and evidence requirements.

This creates a clear sequencing: (1) finalize the JSON contract, (2) design the state model
to support it, (3) design the template format to produce the declarations the contract needs.

The main coordination requirement is that the JSON contract is locked before sub-designs
begin. Once it's stable, the three sub-designs have clear, non-circular dependencies.

## Strengths

- **Contract clarity from day one**: the agent-facing JSON schema is the first artifact
  produced; every other system is designed to satisfy it rather than being discovered later
- **Sub-design dependencies are explicit and non-circular**: CLI contract → state model →
  template format, each building on the previous
- **Self-describing output is the design goal, not a side effect**: the protocol is designed
  around what agents need, ensuring `expects` is complete and correct by construction
- **Easy to validate correctness**: the JSON contract can be reviewed by anyone (agents,
  workflow authors, operators) independently of implementation; integration tests validate
  the contract as a black box
- **Branching is naturally expressed**: the `expects.options` field captures which
  transitions are available and what evidence triggers each, without agents needing state
  names

## Weaknesses

- **Protocol must be right before implementation starts**: if the JSON schema turns out to
  be wrong or incomplete mid-implementation, the sub-designs built on it must be reworked;
  the design-first approach amplifies the cost of early mistakes
- **Three systems remain tightly coupled**: the protocol creates a hard dependency chain;
  changes to what the agent contract needs propagate to state model and template format
- **`expects` derivation logic lives somewhere**: computing the `expects` field from
  template declarations requires a component that reads the compiled template and formats it
  into the output schema — this is new infrastructure that doesn't yet exist

## Deal-Breaker Risks

- **Protocol instability during sub-design**: if the JSON contract isn't finalized before
  tactical designs begin, the sub-designs thrash trying to keep up. This is manageable
  by treating the contract spec as an accepted artifact before any implementation begins —
  which this strategic design should accomplish.
- **None technical**: the protocol-first approach is sound. The execution risk is
  organizational, not architectural.

## Sub-Design Boundaries

1. **CLI contract design**: the `koto next` JSON output schema — `expects`, `advanced`,
   error codes, integration output field, directed transition semantics
2. **State model design**: what koto persists — evidence scoping, history structure,
   state file versioning, atomic write guarantees
3. **Template format design**: what developers declare — per-transition conditions,
   evidence field requirements, integration declarations; compilation pipeline
4. **Auto-advancement engine design**: the loop, stopping conditions, cycle detection,
   integration invocation; implements the contract agreed in (1)

## Implementation Complexity

- Scope: **Medium**
- CLI: new JSON output struct, `expects` derivation from compiled template, error code
  mapping, `advanced` flag, integration output field
- State model: per-state evidence scoping (clear on exit, archive to history), version
  bump for state file schema
- Template format: per-transition condition declarations, evidence field declarations;
  format version bump

## Summary

Protocol-first makes the agent contract the primary design artifact, forcing clarity about
what state must be persisted and what templates must declare before implementation begins.
The JSON output schema becomes the acceptance test for all three sub-systems. The main
execution risk is not technical but organizational: the contract must be finalized before
sub-designs begin, or they will thrash. That's a solvable problem — and solving it is
exactly what this strategic design should do.
