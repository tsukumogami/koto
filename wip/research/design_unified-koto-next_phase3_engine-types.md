# Phase 3 Research: Engine Type Model Changes

## Questions Investigated

- What does the current `MachineState` struct look like? What fields need to change?
- What does `Transition()` currently do with gates? How does `evaluateGates()` use `MachineState.Gates`?
- How is evidence currently stored in the state? How is the history entry built?
- What would `TransitionDecl` need to contain?
- Where is `persist()` called? Can evidence clearing be included in the same write?
- Are there any callers of `Transition()` beyond engine tests and controller?

## Findings

### Current `MachineState` struct (`pkg/engine/types.go`)

`MachineState.Transitions` is a simple `[]string` — just a list of valid target state names.
Gates are stored separately in a flat `map[string]*GateDecl` at the state level. All gates
in this map are evaluated (AND logic) before any outgoing transition.

### Evidence model

Evidence is accumulated globally in `State.Evidence map[string]string`. In `evaluateGates()`,
transition-supplied evidence (injected via `WithEvidence` option) is merged into `State.Evidence`
at evaluation time — but not committed there. The evidence is recorded in `HistoryEntry.Evidence`
only for the current transition. `State.Evidence` is never cleared; it accumulates across the
entire workflow lifetime.

### `Transition()` mechanics (`pkg/engine/engine.go:136-243`)

- Validates target is in `ms.Transitions []string`
- Calls `evaluateGates()` with the state's flat gate map
- Builds a `HistoryEntry` with current evidence snapshot
- Increments `State.Version`
- Calls `persist()` — single write of the entire `State` struct to disk
- `persist()` is the atomic boundary; all state mutation happens before this call

### `evaluateGates()` (`pkg/engine/engine.go:554`)

Iterates the gate map and returns on first failure (simple AND logic). Gate types: `field_not_empty`,
`field_equals`, `command`. Already well-formed for extension.

### `GateDecl` struct

Already exists with: `Type`, `Field`, `Value`, `Command` fields. Well-suited for reuse in
per-transition gate declarations without modification.

### `deepCopyState()` (`pkg/engine/engine.go:526-531`)

Correctly handles copying the Evidence map. Clearing becomes a simple
`e.state.Evidence = make(map[string]string)` before the `persist()` call.

### Callers of `Transition()`

- `cmd/koto/main.go:275` — calls plain `Transition(target)`, no options
- `pkg/controller/controller_test.go` — plain calls
- Engine tests (`engine_test.go`) — use `WithEvidence(...)` option, extensive table-driven coverage

`TransitionOption` functional options already exist; the `WithEvidence` option pattern works
as-is for the new design.

## Implications for Design

**`TransitionDecl` struct** (new): `Target string`, `Gates map[string]*GateDecl`. Shared
state-level gates remain on `MachineState.Gates` and are merged with per-transition gates
during `evaluateGates()` — two separate iterations (shared AND, then per-transition AND).

**Evidence clearing**: Add atomically within `Transition()` before `persist()`:
1. Archive current `State.Evidence` to the `HistoryEntry.Evidence`
2. Set `e.state.Evidence = make(map[string]string)`
3. Call `persist()` — one write, evidence is now cleared

**`Transition()` signature change**: Minimal. `target string` resolves to a matching
`TransitionDecl` in the new `[]TransitionDecl` slice to access per-transition gates.
The `TransitionOption` functional options pattern is unchanged. A `WithDirected bool`
option could be added for directed transitions (bypasses gate evaluation, records
`directed: true` in history).

**`evaluateGates()` update**: Two-phase — evaluate shared gates first (fail fast), then
per-transition gates. Both phases use the same AND-fail-fast logic.

**Format version bump**: Required. `MachineState.Transitions []string` → `[]TransitionDecl`
is a breaking schema change. The format version field must be incremented and validation
added for old-format detection.

## Surprises

1. Evidence is recorded in `HistoryEntry.Evidence` per-transition already — making the
   clearing design straightforward: the archive step is already there, only the clearing
   step is missing.
2. `GateDecl` needs no changes for per-transition use — it's already the right shape.
3. `deepCopyState()` makes clearing cheap: reset the reference, let GC collect the old map.
4. Only one external caller (`cmd/koto/main.go`) of `Transition()` — minimal blast radius
   for signature changes.

## Summary

`MachineState.Transitions []string` becomes `[]TransitionDecl{Target, Gates}` with shared
gates remaining on `MachineState.Gates`. Evidence clearing is a 2-line addition before
`persist()` — archive to `HistoryEntry.Evidence`, reset `State.Evidence`. The `GateDecl`
type is already the right shape for per-transition use. External caller blast radius is
minimal (one CLI callsite, one controller test file, engine tests use stable option pattern).
