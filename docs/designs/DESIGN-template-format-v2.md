---
status: Proposed
problem: |
  koto's template format v1 uses flat transition lists (transitions: [target1, target2])
  with no way to express evidence-driven routing or processing integrations. As the
  event log format (#46) adds typed events like evidence_submitted, the template format
  must declare what evidence each state accepts, how submitted values route to outgoing
  transitions, and which states invoke external processing tools. Without these
  declarations, agents can't know what data to submit, and the advancement engine can't
  route transitions based on evidence values.
decision: |
  Replace FormatVersion=1 with FormatVersion=2. Add three constructs to the template
  format: accepts blocks (per-state evidence field schema with types and required flags),
  when conditions on transitions (field-value equality matching for routing), and an
  integration field (string tag for processing tool routing). The compiler validates
  mutual exclusivity of single-field when conditions at compile time and rejects
  non-deterministic templates. Field gates (field_not_empty, field_equals) are forbidden
  on states with accepts blocks to keep the two control models cleanly separated.
rationale: |
  Extending v1's flat transition list would require agents to carry out-of-band knowledge
  about what evidence to submit and which transitions to target. The accepts/when model
  makes templates self-describing: the compiled JSON contains everything the CLI needs to
  generate an expects field telling agents exactly what to submit. Forbidding field gates
  on accepts states eliminates a semantic overlap that would create ambiguous evaluation
  order. Command gates remain allowed since they check environmental conditions, not
  agent-submitted evidence.
---

# DESIGN: Template Format v2

## Status

Proposed

## Upstream Design Reference

Strategic design: `docs/designs/DESIGN-unified-koto-next.md` (status: Planned)

This tactical design implements Phase 2 (Template Format v2) from the strategic design.
Relevant sections: Template Format, Sub-Design Boundaries, Implementation Approach Phase 2.

## Context and Problem Statement

koto's template format v1 defines workflows as states with flat transition lists
and koto-verifiable gates. A state declares its outgoing transitions as a list of
target state names (`transitions: [deploy, escalate_review]`) and optional gates
that block advancement until conditions are met.

This works for linear workflows where states have a single outgoing path. But the
event-sourced state model from #46 introduces `evidence_submitted` events where
agents provide structured data, and the advancement engine needs to route to
different transitions based on that data. The v1 format has no way to express:

- What fields an agent should submit at a given state (evidence schema)
- Which transition fires when specific evidence values are provided (conditional routing)
- Which states should invoke external processing tools before accepting evidence

The strategic design (`DESIGN-unified-koto-next.md`) defines the high-level shape:
`accepts` blocks for evidence schema, `when` conditions for routing, and
`integration` tags for processing tools. This tactical design specifies the exact
YAML syntax, compiled JSON schema, compiler validation rules, and Rust types.

koto has no released users, so this is a clean format break with no migration
concerns.

## Decision Drivers

- **Self-describing templates**: The compiled JSON must contain enough information
  for `koto next` to generate an `expects` field telling agents what to submit,
  without the CLI needing to understand template-specific logic
- **Compile-time safety**: Non-deterministic templates (where two transitions could
  fire for the same evidence) must be caught by the compiler, not at runtime
- **Clean separation of concerns**: Gates check environmental conditions (CI passed,
  file exists). Evidence routing checks agent-submitted data. These shouldn't overlap.
- **Minimal complexity**: Only add what's needed for the advancement engine. No
  operator extensibility, no complex condition DSL, no multi-field validation
- **Existing gate compatibility**: Command gates remain useful for environmental
  checks and shouldn't be removed

