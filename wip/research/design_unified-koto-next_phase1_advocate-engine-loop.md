# Advocate: Engine-Owned Advancement

## Approach Description

The engine gains an `Advance()` method that chains through multiple transitions internally until
a stopping condition is reached, then returns the final directive. All state machine orchestration
— gate evaluation, per-state evidence clearing, visited-state tracking, stopping condition logic,
and integration invocation via an injected interface — lives in the engine package. The controller
and CLI become thin wrappers: the controller constructs an `AdvanceOptions` struct and calls
`Engine.Advance()`; the CLI parses `--with-data` and `--to` flags and delegates immediately. The
engine is the single source of truth for how advancement works.

## Investigation

**Current engine.go:** `Transition()` (lines 136-243) is already the most complex function in the
codebase at ~100 lines. It handles: target validation, gate evaluation (including command gates
with subprocess invocation), history entry construction, evidence merging, version increment, and
atomic persist. Adding an `Advance()` loop on top of this makes the engine package the heaviest in
the repo.

**Concrete changes:**

1. **Template format** — `MachineState.Transitions []string` → `[]TransitionDecl{Target, Gates}`.
   Same breaking change as all approaches. Format version bump required.

2. **Engine.Advance(opts AdvanceOpts) (*AdvanceResult, error)** — new primary method. Contains:
   visited-state `map[string]bool`, stopping-condition check, loop body calling internal
   `transitionOne()`, integration runner interface call when stopping at a processing integration.

3. **Engine.transitionOne()** — extracted from current `Transition()`: validate target, evaluate
   shared gates, evaluate per-transition gates, clear evidence, build history entry, persist.

4. **IntegrationRunner interface** — injected at engine construction time (or as an `Advance()`
   option). The engine calls it when a stopping state has a configured processing integration.

5. **Evidence lifecycle** — per-state clearing happens inside `transitionOne()` atomically with
   the persist call. Evidence for the departing state is archived to the history entry; the new
   state's evidence map starts empty.

6. **Visited-state tracking** — `map[string]bool` local to each `Advance()` call. If the next
   target is already in the map, stop and return the current state's directive with an
   `advanced: false` cycle-halted flag.

7. **Controller** becomes ~20 lines: load engine, construct `AdvanceOpts`, call `Advance()`,
   format `*AdvanceResult` as JSON output. Nearly all logic removed.

**Engine package size after change:** Currently ~700 lines. After: ~1000-1100 lines.

## Strengths

- **Single-package ownership**: "how advancement works" is answerable by reading one package.
  No need to trace through controller to understand stopping conditions.
- **Atomic evidence clearing**: evidence clearing happens inside the same `persist()` call as the
  state transition — no separate write, no window where evidence exists but state hasn't advanced.
- **Controller becomes trivially simple**: a ~20 line controller is easier to reason about and
  test than a ~200 line controller owning a loop.
- **Integration injection at construction**: `IntegrationRunner` is injected when the engine is
  created, making the test setup identical to the current `DelegateChecker` pattern already in the
  design.
- **Testability of loop logic**: `Engine.Advance()` can be tested with a mock `IntegrationRunner`
  and in-memory state files. The loop, visited-set, and stopping conditions are unit-testable
  without controller involvement.
- **Consistent growth vector**: future capabilities (approval gates, async integrations) extend
  `Advance()` or add new gate types — the growth always lands in the engine, not distributed
  across packages.

## Weaknesses

- **Engine package becomes very large**: at ~1100 lines, the engine is significantly larger than
  any other package in the codebase. Combining atomic transaction logic with orchestration loop
  logic in one package makes each harder to navigate.
- **Separation of concerns blurred**: the engine currently has no knowledge of integrations,
  config, or agent-facing concepts. `IntegrationRunner` injection brings external-system awareness
  into the engine's responsibility surface.
- **Harder to swap orchestration independently**: if the advancement loop needs a different
  strategy (e.g., async advancement, event-driven advancement), it requires modifying the engine
  rather than replacing the orchestration layer.
- **`Transition()` API becomes internal-only**: the current `Transition()` method, which callers
  use directly (e.g., in tests), would be replaced by `Advance()`. Callers that want single-step
  transitions for testing need a new `transitionOne()` exported path or test via `Advance()` with
  options that limit to one step.

## Deal-Breaker Risks

- **None identified** that are unique to this approach. The `IntegrationRunner` injection is the
  most novel element, but it follows the existing `DelegateChecker` pattern exactly. The engine
  size concern is a maintainability risk, not a correctness one.

## Implementation Complexity

- Files to modify: `pkg/engine/types.go`, `pkg/engine/engine.go` (significant), `pkg/template/`
  (schema + compilation), `pkg/controller/controller.go` (simplified), `cmd/koto/main.go`,
  engine tests (updated), controller tests (simplified)
- New infrastructure: `IntegrationRunner` interface in `pkg/engine`; no new packages
- Estimated scope: **Medium** — same engine and template changes as other approaches; controller
  simplification partially offsets the engine growth

## Summary

Engine-owned advancement consolidates all orchestration in the package that already owns state
machine semantics, making the "how" of advancement readable in one place and keeping the
controller trivially thin. The cost is a larger, more complex engine package that mixes
transaction logic with loop orchestration. No deal-breaker risks; the primary trade-off is
maintainability: a single large package vs. a clean layer boundary.
