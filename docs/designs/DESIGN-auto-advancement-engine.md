# DESIGN: Auto-Advancement Engine

## Status

Proposed

## Upstream Design Reference

Parent: `docs/designs/DESIGN-unified-koto-next.md` (Phase 4: Auto-advancement engine)

Relevant sections: Solution Architecture > Data Flow (advancement loop pseudocode),
Sub-Design Boundaries (scope definition), Security Considerations (signal handling,
integration invocation).

## Context and Problem Statement

`koto next` currently evaluates one state and returns. If the current state has no
`accepts` block, passing gates, and an unconditional transition, the agent gets back
a response that says "you can advance" but doesn't actually advance. The agent must
call `koto next --to <target>` manually for every intermediate state, turning what
should be automatic chaining into a tedious back-and-forth.

The strategic design specifies an advancement loop that chains through states until
hitting a stopping condition (terminal, gate blocked, evidence required, integration,
or cycle). This design covers that loop, plus the integration runner that the
strategic design deferred, signal handling for clean shutdown, and `koto cancel` for
workflow abandonment.

The existing codebase has solid foundations: event types are defined, the gate
evaluator has process group isolation, evidence validation works, and the pure
`dispatch_next` function classifies states correctly. What's missing is the loop
that ties them together.

## Decision Drivers

- **Correctness over speed**: the loop must never corrupt the event log, even on
  SIGTERM mid-chain
- **Pure function preservation**: `dispatch_next` stays pure (no I/O); the loop
  lives in the handler layer
- **Reuse existing infrastructure**: gate evaluator, evidence validation, and
  persistence layer are battle-tested; don't rewrite them
- **Integration runner must degrade gracefully**: a missing or misconfigured
  integration returns `IntegrationUnavailable`, not a crash
- **Cycle detection must be simple**: visited-state set per invocation; no need
  for graph analysis
