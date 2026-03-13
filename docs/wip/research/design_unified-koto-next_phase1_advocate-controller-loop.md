# Advocate: Controller-Owned Advancement Loop

## Approach Description

The engine remains a single-transition executor. `Engine.Transition()` is extended to support
`TransitionDecl` (per-transition conditions) and evidence clearing on commit, but it executes
exactly one transition per call and returns. The controller gains an advancement loop that calls
`Engine.Transition()` repeatedly until a stopping condition (unsatisfied gate, processing
integration, terminal state, or visited-state cycle) is reached. Integration invocation (delegate
CLIs) lives in the controller behind an injected interface. The CLI gains `--with-data` and `--to`
flags that route to different controller entry points.

## Investigation

**Current engine.go:** `Transition()` (lines 136-243) does one transition: validates target,
evaluates gates via `evaluateGates()`, builds a history entry, merges evidence into state,
increments version, and persists atomically. Gate types: `field_not_empty`, `field_equals`,
`command`. Gates are on `MachineState.Gates map[string]*GateDecl` — flat map, no per-transition
structure. `Transitions []string` is the current field shape.

**Current controller.go:** `Next()` (lines 51-90) is ~40 lines. It loads state, finds the
current machine state, resolves any delegation info from tags + config, builds a `Directive`
struct, and returns. No looping. It calls `engine.Next()` for the directive text, not
`engine.Transition()`.

**What changes under this approach:**
- `MachineState.Transitions` changes from `[]string` to `[]TransitionDecl{Target string, Gates map[string]*GateDecl}`. Breaking change — template format version bump required.
- `Engine.Transition()` gains per-transition gate evaluation: evaluate shared gates first, then the target transition's gates. Evidence is cleared from the state map on commit (archived to history entry).
- New `Controller.Advance(opts AdvanceOpts) (*AdvanceResult, error)` method replaces the current thin `Next()` + separate transition call. It loops: call engine, check stopping condition, repeat.
- Visited-state set is a `map[string]bool` local to each `Advance()` call.
- `IntegrationRunner` interface injected into controller; called when the stopping state has a processing integration configured.
- CLI `cmdNext` gains flag parsing for `--with-data` (reads JSON file, passes as evidence) and `--to` (passes as directed target, bypasses gate eval).

**Existing patterns that support this:**
- `DelegateChecker` interface in the delegation design shows the codebase already uses injection for subprocess availability checks — the same pattern extends to `IntegrationRunner`.
- `evaluateCommandGate()` (lines 603-649) already runs subprocesses with timeout. The controller loop calling this via the engine is the same pattern as the existing gate evaluation.
- Test structure (engine_test.go) tests `Transition()` in isolation with table-driven cases — the approach keeps this testability intact since the engine API surface barely changes.

## Strengths

- **Engine API stays minimal**: `Transition()` signature changes to accept `TransitionOptions` but the semantics are unchanged — one call, one transition. Existing callers (tests, CLI) require minimal update.
- **Loop logic is testable in isolation**: `Controller.Advance()` can be unit-tested by injecting a mock engine and mock integration runner. No subprocess side effects in the test path.
- **Natural home for side effects**: The controller already holds config (delegation rules, template path). Integration invocation belongs with config, not deep in the engine.
- **Incremental implementation**: Can ship `--with-data` and the engine evidence-scoping changes before the full advancement loop. The features compose independently.
- **Matches existing architecture direction**: The delegation design (issue #41) already shows the controller owning delegate config resolution. Advancement fits the same layer.

## Weaknesses

- **Controller grows significantly**: `Next()` (40 lines) becomes `Advance()` (~150-200 lines) plus helper methods for stopping-condition evaluation, visited-set management, and integration invocation. The controller package takes on more responsibility.
- **Engine-controller boundary requires care**: Gate evaluation happens in the engine; stopping-condition evaluation (is this a processing integration? is this visited?) happens in the controller. Two places need to understand state semantics.
- **TransitionDecl migration is mandatory**: Every call site that currently reads `ms.Transitions []string` must handle `[]TransitionDecl`. This affects template compilation, engine, controller, and tests.

## Deal-Breaker Risks

- **None identified.** The approach is consistent with the existing architecture direction, avoids introducing new packages, and the breaking template format change is acknowledged and accepted in the PRD (no production users, format version bump planned).

## Implementation Complexity

- Files to modify: `pkg/engine/types.go`, `pkg/engine/engine.go`, `pkg/controller/controller.go`, `pkg/template/` (schema + compilation), `cmd/koto/main.go`, engine and controller tests
- New infrastructure: `IntegrationRunner` interface (new, in `pkg/controller` or `pkg/engine`); no new packages
- Estimated scope: **Medium** — the changes are deep (engine data model) but contained within existing packages

## Summary

The controller-owned advancement loop fits naturally into the existing architecture: the engine
stays a single-transition executor with an extended type model, the controller gains the loop and
integration ownership consistent with its current config-holding role, and the CLI adds two flags.
The approach is testable at every layer via interface injection, avoids introducing new packages,
and the primary cost — the `TransitionDecl` type migration — is unavoidable regardless of which
approach is chosen. No deal-breaker risks identified.
