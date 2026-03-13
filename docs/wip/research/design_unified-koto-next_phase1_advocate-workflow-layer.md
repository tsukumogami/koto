# Advocate: New Workflow Orchestrator Layer

## Approach Description

Introduce a new `pkg/workflow` package that owns the advancement loop, evidence lifecycle, and
integration invocation. The engine remains a pure single-transition executor — it accepts one
transition at a time and knows nothing about chaining. The controller is refactored to delegate
to the new `Workflow` type rather than calling the engine directly. The `Workflow` type composes
the engine and an injectable `IntegrationRunner` interface, manages the visited-state set, clears
evidence on each transition commit, and returns when a stopping condition is reached. Three clear
layers: engine (atomic state transitions), workflow (orchestration loop), controller (request
handling and output formatting).

## Investigation

**Current engine.go:** The engine has a clean transaction model — `Transition()` validates,
evaluates gates, builds a history entry, and persists atomically. It is already well-isolated
with no CLI or HTTP dependencies. The `MachineState.Gates map[string]*GateDecl` flat map must
change to per-transition regardless of approach — this is unavoidable.

**Current controller.go:** `Next()` (~40 lines) loads state, resolves delegation from tags and
config, builds a `Directive` struct. The controller already acts as an orchestration layer
between the CLI and the engine. Under this approach it becomes thinner, delegating advancement
to `pkg/workflow.Workflow`.

**What changes under this approach:**
- New `pkg/workflow/workflow.go`: `Workflow` struct wraps `*engine.Engine` and `IntegrationRunner`.
  `Workflow.Next(opts WorkflowOpts) (*WorkflowResult, error)` runs the loop.
- `IntegrationRunner` interface defined in `pkg/workflow`; implementations live in `internal/` or
  are injected from the CLI layer.
- Engine changes are the same as other approaches: `TransitionDecl`, per-state evidence clearing,
  per-transition gate evaluation.
- `Controller.Next()` becomes a thin wrapper: construct `Workflow`, call `Workflow.Next()`,
  format result as `Directive`.
- CLI gains `--with-data` and `--to` flags; passes them as `WorkflowOpts` fields.

**New package footprint:** ~300-400 lines in `pkg/workflow/`, plus the same engine and template
changes required by all approaches.

## Strengths

- **Maximum architectural clarity**: the three-layer separation (engine → workflow → controller)
  is explicit in the package structure. A reader navigating the codebase immediately knows where
  to find the advancement loop.
- **Engine purity preserved and enforced**: the engine package boundary prevents future creep of
  orchestration logic into the transaction layer. The package import graph enforces this.
- **Workflow type is independently testable**: unit tests for `Workflow.Next()` inject a mock
  engine and mock integration runner with no subprocess side effects. The test surface is cleanly
  separated from engine transaction tests.
- **Future extensibility**: a new capability (e.g., approval gates, async integrations) gets a
  method on `Workflow`, not a flag in the controller or a new engine feature. The growth vector
  is clear.
- **Controller stays presentation-focused**: formatting, config loading, and output shape stay in
  the controller; the controller doesn't accumulate orchestration complexity over time.

## Weaknesses

- **New package for moderate scope**: introducing `pkg/workflow` adds a layer of indirection for
  logic that could live in the controller. The question of what belongs in `workflow` vs.
  `controller` may not always be obvious to contributors.
- **Same mandatory changes still required**: `TransitionDecl`, per-state evidence clearing, and
  format version bump are required regardless of approach. The new package doesn't reduce the
  scope of engine and template changes.
- **Controller becomes a passthrough**: if the controller becomes a thin wrapper around
  `Workflow`, it may lose its rationale as a distinct type. Callers might as well use `Workflow`
  directly.
- **Slightly more files to touch**: a new package means new files, new tests, and a new entry in
  the import graph — modestly more overhead than extending the controller.

## Deal-Breaker Risks

- **None identified** for the core approach. The risk that the `pkg/workflow` abstraction becomes
  underused (controller passthrough) is real but manageable — if the controller adds no value, it
  can be collapsed later without breaking the engine.

## Implementation Complexity

- Files to modify: `pkg/engine/types.go`, `pkg/engine/engine.go`, `pkg/template/` (schema +
  compilation), `pkg/controller/controller.go`, `cmd/koto/main.go`, engine tests, controller tests
- New infrastructure: `pkg/workflow/` package (~300-400 lines), `IntegrationRunner` interface
- Estimated scope: **Medium-Large** — same depth of change as controller-loop approach, plus new
  package overhead

## Summary

The workflow orchestrator layer is the cleanest long-term architecture: it makes the separation
between transaction execution (engine) and orchestration (workflow) explicit at the package level,
keeps the controller as a thin presentation layer, and provides a natural growth vector for future
capabilities. The trade-off is a new package whose footprint may not justify the indirection at
this stage — the mandatory engine and template changes are the same regardless, and the controller
already serves as an orchestration point. No deal-breaker risks; the primary question is whether
the architectural clarity of a new package outweighs the added complexity.
