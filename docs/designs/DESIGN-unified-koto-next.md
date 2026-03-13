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

## Considered Options

### Decision: Where does the auto-advancement loop live?

**Context:** koto next must chain through multiple state transitions in a single call,
stopping only when blocked. This loop requires visiting-state tracking, stopping-condition
evaluation, integration invocation, and evidence lifecycle management. The question is
which package owns this orchestration logic.

**Chosen: Controller-owned advancement loop (Approach A).**

The controller already holds the workflow's config context — delegation rules, template
path, integration configuration. Advancement is an orchestration concern, not a
transaction concern; the engine's job is to execute one correct transition atomically,
not to decide when to stop. Keeping orchestration in the controller preserves a minimal
engine package that is easy to reason about and test in isolation. The `IntegrationRunner`
interface injected into the controller follows the same pattern already established for
delegate checking in the delegation design. No new packages are introduced, and the loop
is unit-testable by injecting mock engine and integration runner implementations.

*Alternative rejected: Engine-owned advancement (Approach B).* The engine gains an
`Advance()` loop alongside `transitionOne()`, making it the single source of truth for
advancement. The appeal is that "how advancement works" is answerable by reading one
package. The cost is significant: the engine grows from ~700 to ~1100 lines and gains
awareness of integrations, config, and agent-facing concepts — concerns it currently has
no knowledge of. Mixing transaction logic (atomically commit one state change) with
orchestration logic (decide when to stop chaining) in the same package makes each harder
to navigate and the package harder to maintain. The engine package size concern is a
long-term maintainability risk; the controller approach avoids it without sacrificing
testability.

*Alternative rejected: New workflow orchestrator layer (Approach C).* A new `pkg/workflow`
package provides maximum architectural clarity — three explicit layers (engine, workflow,
controller) enforced at the package import level. The engine stays a pure single-transition
executor, and future capabilities extend `Workflow`, not the controller. The trade-off is
real: ~300-400 lines of new infrastructure for logic that fits naturally in the controller.
If the controller becomes a thin wrapper around `Workflow`, it loses its rationale as a
distinct type, and callers might as well use `Workflow` directly. The architectural clarity
benefit doesn't justify the added indirection at this stage — the controller already serves
as the orchestration layer between CLI and engine, and the mandatory engine and template
changes are identical regardless of approach.

## Decision Outcome

The controller owns the auto-advancement loop. `Engine.Transition()` is extended to support
`TransitionDecl` (per-transition gate declarations) and evidence clearing on commit, but
executes exactly one transition per call. The controller gains `Advance(opts AdvanceOpts)`
which loops — calling `Engine.Transition()`, evaluating stopping conditions, and invoking
an injected `IntegrationRunner` when a processing integration stops the chain. The CLI
adds `--with-data` and `--to` flags routed through the controller.

Key properties:
- Engine remains a single-transition executor with an extended type model
- Controller owns the advancement loop and integration invocation
- `IntegrationRunner` interface injected into the controller; no subprocess side effects
  in test paths
- Each transition is independently committed; crash mid-chain recovers from last committed
  state
- Visited-state set (per `Advance()` call) prevents infinite loops on cyclic templates
- Evidence is cleared atomically with each transition commit and archived to history
- No new packages introduced; changes are contained to existing packages

