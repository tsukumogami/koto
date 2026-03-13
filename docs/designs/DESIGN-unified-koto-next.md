---
status: Proposed
upstream: docs/prds/PRD-unified-koto-next.md
problem: |
  To be completed after approach selection.
decision: |
  To be completed after approach selection.
rationale: |
  To be completed after approach selection.
---

# DESIGN: Unified koto next Command

## Status

Proposed

## Context and Problem Statement

koto's current architecture treats state reading and state advancement as separate
operations. Agents call `koto next` to read the current directive, then call
`koto transition <target>` to advance — and must know which to call and when. As new
capabilities are added (evidence submission, delegation, per-transition conditions),
this model breaks down: the set of valid operations at any point grows, agents must track
it themselves, and each new capability adds a new command.

Three systems need to change together to fix this. The **CLI contract** — what `koto next`
accepts and returns — must become self-describing so agents never need out-of-band
knowledge. The **state model** — what koto persists between calls — must support per-state
evidence scoping so evidence doesn't contaminate branching or looping workflows. The
**workflow definition format** — how developers author koto templates — must support
per-transition conditions so workflows can branch based on what agents submit without
agents naming target states.

These systems are interdependent: the CLI contract depends on what state is stored, and
state storage depends on what templates can declare. But each is large enough to warrant
its own tactical design. This document makes the three unifying high-level decisions and
defines the constraints each tactical design must satisfy.

## Decision Drivers

- **Agent contract stability**: the CLI surface (`koto next` input/output schema) must
  stay constant as capabilities are added; tactical sub-designs must not require flag or
  schema changes visible to agents
- **Self-describing output**: an agent that has never seen the workflow template must be
  able to determine its next action from the `koto next` response alone
- **Breaking change scope**: both state file format and template format changes are
  breaking; the design must make clear what migration is required and by whom
- **Sub-design independence**: tactical designs for CLI contract, state model, and
  template format must be implementable in sequence without circular dependencies
- **Evidence correctness**: per-state evidence scoping is required for correctness with
  directed transitions and looping workflows — the design must specify the scoping model,
  not defer it
- **Template authoring ergonomics**: workflow developers need a template format that's
  readable and writable without understanding koto internals
- **Testability**: every system boundary (CLI output, state persistence, template
  compilation) must be independently testable

