---
status: Proposed
spawned_from:
  issue: 68
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-shirabe-work-on-template.md
problem: |
  During long-running judgment states like implementation and analysis, agents make
  non-obvious choices — assumptions about API behavior, tradeoff decisions, approach
  pivots — that are currently buried in reasoning traces. koto has no way to accept
  structured records of these decisions mid-state without triggering the advancement
  loop. Evidence submission always resolves transitions, so agents can only record
  decisions at the moment they're ready to leave the state. By then, the decisions
  are reconstructed from memory rather than captured as they happen.
decision: |
  TBD
rationale: |
  TBD
---

# DESIGN: mid-state decision capture

## Status

Proposed

## Context and Problem Statement

koto's evidence submission model is tightly coupled to state advancement. When an agent
calls `koto transition --with-data`, the engine validates the evidence, appends an
`evidence_submitted` event, and immediately runs the advancement loop (gate checks,
transition resolution). There's no way to record structured data mid-state without
risking an unintended transition.

This matters for judgment states where agents work for extended periods — writing code,
researching approaches, creating plans. During this work, agents make decisions that
shape the outcome: "the API doesn't support batch operations, so I'll iterate instead,"
"this test framework doesn't support mocking at this level, I'll use integration tests,"
"the design says X but the code suggests Y, going with Y." These decisions are currently
invisible to anyone reviewing the work — they're in the agent's reasoning, not in koto's
event log.

The parent design (DESIGN-shirabe-work-on-template) identifies this as a cross-cutting
engine concern: the `implementation` and `analysis` states in the work-on template both
need decision capture, and any future template with long-running judgment states will
benefit from the same mechanism.

The specific requirements from the parent design:
- Agents submit decision records mid-state without triggering transitions
- Records include at minimum: `choice`, `rationale`, `alternatives_considered`
- koto persists decisions in the event log and surfaces them to the user
- Templates can optionally require decision capture before allowing completion
- The mechanism is compatible with the existing evidence submission flow

## Decision Drivers

- **Decoupled from advancement**: submitting a decision must not trigger the advancement
  loop — the agent stays in the current state
- **Structured and queryable**: decisions are typed records in the event log, not freeform
  text — they can be filtered, counted, and displayed
- **Minimal engine surface**: the change should be small and targeted — one new event type
  or CLI flag, not a new subsystem
- **Backwards compatible**: existing templates and workflows must work unchanged
- **Surfaceable**: accumulated decisions must be retrievable — via `koto next`,
  `koto query`, a dedicated flag, or a new subcommand
- **Rewind-safe**: rewinding past a state discards its decisions, consistent with how
  evidence events work today
