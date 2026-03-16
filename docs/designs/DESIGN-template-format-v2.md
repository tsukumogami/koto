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
  integration field (string tag for processing tool routing). Remove field gates
  (field_not_empty, field_equals) entirely since accepts/when replaces them. Only command
  gates survive. The compiler validates mutual exclusivity of single-field when conditions
  at compile time and rejects non-deterministic templates.
rationale: |
  In v2's event-sourced model, evidence only enters through koto next --with-data and is
  scoped to the current state via the epoch boundary rule. Field gates that check
  agent-submitted evidence are redundant with accepts/when, which handles the same use
  cases more expressively. Removing them yields two orthogonal concepts (command gates
  for environment, accepts/when for agent evidence) with no overlap or interaction rules
  to define. koto has no users, so this is a clean break.
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

## Considered Options

### Decision: How to handle the overlap between field gates and accepts/when

v1 has three gate types: `field_not_empty`, `field_equals`, and `command`. The first
two check agent-submitted evidence (does a field exist? does it equal a value?). The
new `accepts`/`when` system also checks agent-submitted evidence, but with more
expressiveness (typed schemas, conditional routing, mutual exclusivity validation).

In v2's event-sourced model, evidence only enters the system through explicit agent
submission (`koto next --with-data`), and it's scoped to the current state by the
epoch boundary rule. This means field gates and `accepts`/`when` operate on the
same data through different mechanisms. The question is whether to keep both, restrict
their interaction, or remove the redundant one.

#### Chosen: Unified Model (remove field gates)

Remove `field_not_empty` and `field_equals` gate types entirely from v2. Only
`command` gates survive. Everything field gates expressed is now expressed through
`accepts`/`when`:

- `field_not_empty: decision` becomes `accepts: {decision: {type: string, required: true}}`
- `field_equals: decision = proceed` becomes `when: {decision: proceed}` on a transition

This leaves two orthogonal concepts in the template format:
- **Command gates**: check the environment (CI passed, file exists). Koto evaluates
  these without agent involvement.
- **Accepts/when**: handle agent-submitted evidence. Agents submit data via
  `--with-data`, and `when` conditions route to the matching transition.

No interaction rules are needed because the two concepts don't overlap. A state can
have command gates (environmental prerequisites) and `accepts`/`when` (evidence
routing) without ambiguity: gates evaluate first, then `when` conditions match
against submitted evidence.

koto has no users, so removing field gates is a clean break with no migration cost.

#### Alternatives Considered

**Strict Separation**: Keep field gates but forbid them on states with `accepts`
blocks (compiler error). This eliminates the overlap ambiguity but keeps dead code
around. If field gates only work on states without `accepts`, they're checking
evidence on states that have no evidence schema, which is contradictory in the v2
model where evidence is scoped to the current state.

**Coexistence with Precedence**: Allow field gates and `accepts` on the same state,
with gates evaluating first as prerequisites. Rejected because it creates a complex
mental model (two evaluation phases for the same data), semantic ambiguities (field
required by gate but optional in accepts?), and degrades the self-describing
principle (agents see an `expects` field they can't submit to while gates block).

## Decision Outcome

### Summary

Template format v2 replaces the flat `transitions: [target1, target2]` list with
three new constructs and removes field gates.

Each state can declare an `accepts` block: a map of field names to schemas. Each
field has a `type` (enum, string), a `required` flag, and for enums a `values` list
of allowed values. This block is the source of truth for what evidence an agent
should submit at this state.

Transitions change from plain strings to structured objects. Each transition has a
`target` state and an optional `when` condition: a map of field names to expected
values. When an agent submits evidence via `--with-data`, the advancement engine
matches the submitted values against each transition's `when` conditions and routes
to the first match. The compiler validates that single-field `when` conditions are
mutually exclusive (disjoint values on the same field) and rejects non-deterministic
templates. Multi-field conditions can't be statically verified and are the template
author's responsibility.

States can declare an `integration` field: a string tag naming a processing tool.
The compiler stores it verbatim. The integration runner (#49) resolves the tag to an
actual command at runtime through project configuration. Missing config is not a
compile-time error.

`field_not_empty` and `field_equals` gate types are removed. `command` gates remain
unchanged. The `format_version` field bumps from 1 to 2.

### Rationale

The unified model produces the simplest design because it follows from the
event-sourced architecture. In v1, evidence was a mutable map that gates could
inspect at any time. In v2, evidence enters through typed events and is scoped by
the epoch boundary. Field gates were checking the same data that `accepts`/`when`
now handles with better expressiveness and compile-time validation. Keeping both
would require defining interaction rules for no practical benefit. Removing field
gates yields a net code reduction and fewer concepts for template authors.

