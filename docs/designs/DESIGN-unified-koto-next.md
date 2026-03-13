---
status: Proposed
upstream: docs/prds/PRD-unified-koto-next.md
problem: |
  koto's state evolution is split across multiple commands (koto next, koto transition,
  and the planned koto delegate run), each valid only at specific workflow points. The
  engine's gate model is state-level only, with no support for per-transition conditions
  needed for evidence-based branching. Evidence is globally accumulated across the
  workflow lifetime, which breaks when transitions skip states or when workflows loop.
  The CLI has no flags for evidence submission or human-directed overrides. There is no
  auto-advancement loop — the controller returns the current directive without chaining
  through states with satisfied conditions.
decision: |
  Restructure koto's command interface, engine, and state model around a single
  koto next command that auto-advances until blocked, accepts evidence via --with-data,
  and supports human-directed transitions via --to. Per-transition conditions replace
  the flat state-level gate map. Evidence becomes per-state scoped, cleared and archived
  on each transition. Auto-advancement chains through states using a visited-set to
  prevent cycles, stopping at unsatisfied conditions, processing integrations, or
  terminal states.
rationale: |
  A single command surface eliminates ordering errors and keeps the agent contract
  stable as new capabilities are added. Per-state evidence scoping is required for
  correctness with directed transitions and looping workflows — global evidence breaks
  when states are skipped or re-entered. Per-transition conditions are the minimum
  change needed to support evidence-based branching without agents naming target states.
  Independent transaction-per-transition atomicity is simpler than multi-state
  transactions and recovers safely from crashes by re-evaluating from the last committed
  state.
---

# DESIGN: Unified koto next Command

## Status

Proposed

## Context and Problem Statement

koto's current architecture separates state reading (`koto next`) from state advancement
(`koto transition`) into two commands that the agent must call in the right order. This
creates correctness risks — nothing prevents an agent from calling `koto transition` when
the engine expects an evidence submission — and the surface will grow further as new
capabilities like delegation are added.

The engine's data model compounds the problem. Gates live on `MachineState` as a flat
map, evaluated with AND logic before any transition. There is no structure for attaching
conditions to individual outgoing transitions, which is required for evidence-based
branching where the agent's submission determines which branch to take. Evidence is stored
in a single global map that accumulates across the entire workflow; this breaks when a
directed transition skips a state that would have collected evidence, and when looping
workflows re-enter branching states carrying stale values from a prior iteration.

The controller has no auto-advancement loop. `Next()` returns the current state's directive
without chaining through states whose conditions are already satisfied. This forces agents
to call `koto next` and `koto transition` in an alternating loop rather than letting koto
advance autonomously through states it can verify itself.

Three systems need to change together: the CLI (new flags, removed subcommand), the engine
(per-transition gate model, per-state evidence scoping, advancement loop with cycle
detection), and the template format (transitions change from a string list to structured
declarations with per-transition conditions).

## Decision Drivers

- **Correctness of evidence-based branching**: per-transition conditions require structural
  changes to `MachineState.Transitions` — the field must become a typed slice, not a string
  slice, to carry per-transition gate declarations
- **Evidence isolation**: global evidence breaks with directed transitions and looping
  workflows; the fix (clear evidence on transition) must be atomic with the transition commit
- **Cycle safety in auto-advancement**: the advancement loop needs visited-state tracking
  scoped to the current call to prevent infinite loops on cyclic templates
- **Crash recovery**: each transition must be independently committed so the workflow
  recovers to a valid state after a mid-chain crash; no multi-transition atomic transactions
- **Template format migration**: `transitions: []string` → `transitions: []TransitionDecl`
  is a breaking change; a format version bump is required
- **CLI surface stability**: all new capabilities go through `koto next` flags, not new
  subcommands; `koto transition` is removed
- **Testability**: auto-advancement and gate evaluation must remain unit-testable without
  subprocess side effects; processing integrations are injected via interface
