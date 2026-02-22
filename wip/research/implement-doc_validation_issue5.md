# Validation Report: Issue #5

**Issue**: feat(engine): add rewind, cancel, and query methods
**Date**: 2026-02-22
**Branch**: docs/koto-engine
**Commit**: 48e9dcd feat(#5): add Rewind, Cancel, and copy-safe query methods

## Environment

- Go module: github.com/tsukumogami/koto
- Platform: Linux 6.17.0-14-generic
- Validation method: Go unit tests in pkg/engine/

## Scenario Results

### scenario-10: Rewind to previously visited state succeeds
**Status**: PASSED
**Validated by**: `TestRewind_ToPreviouslyVisitedState`
**Details**:
- Machine: start -> research -> implementing -> review -> done/escalated
- Transitions: start -> research -> implementing -> review
- Rewind to "research" succeeds
- CurrentState() == "research" after rewind
- Version incremented to 5 (init=1, 3 transitions, 1 rewind)
- History has 4 entries (3 transitions + 1 rewind)
- Last history entry: `{from: "review", to: "research", type: "rewind"}`
- State file persisted correctly (verified by `TestRewind_PersistsToFile`)

### scenario-11: Rewind to initial state succeeds even without prior visit in history
**Status**: PASSED
**Validated by**: `TestRewind_ToInitialState_NoHistory`
**Details**:
- Transitions: start -> research
- Rewind to "start" (initial state) succeeds
- "start" never appears as a `to` field in history -- it's valid because it's the initial state
- CurrentState() == "start" after rewind
- Engine code (line 165): `if target != e.machine.InitialState` skips history check for initial state

### scenario-12: Rewind from terminal state succeeds (error recovery)
**Status**: PASSED
**Validated by**: `TestRewind_FromTerminalState`
**Details**:
- Advances to terminal state "escalated" (start -> research -> implementing -> review -> escalated)
- Rewind to "implementing" succeeds
- CurrentState() == "implementing" after rewind
- Engine does NOT check if current state is terminal before allowing rewind -- only the target matters
- This is the intended recovery path per the design

### scenario-13: Rewind to unvisited non-initial state fails
**Status**: PASSED
**Validated by**: `TestRewind_ToNeverVisitedState`
**Details**:
- Only advances to "research"
- Rewind to "implementing" (never visited, not initial) fails
- Error: `*TransitionError` with code "rewind_failed"
- Message: `cannot rewind to "implementing": state has never been visited`
- Also verified: `TestRewind_ToUnknownState` checks rewind to a state not even in the machine definition

### scenario-14: Rewind to terminal state fails
**Status**: PASSED
**Validated by**: `TestRewind_ToTerminalState`
**Details**:
- Advances through all states to "done" (terminal)
- Rewinds to "review" (valid, non-terminal, previously visited)
- Attempts rewind to "done" (terminal) -- fails
- Error: `*TransitionError` with code "rewind_failed"
- Message: `cannot rewind to "done": target is a terminal state`
- Engine checks `ms.Terminal` on the target state (line 155-162)

### scenario-15: Cancel removes the state file
**Status**: PASSED
**Validated by**: `TestCancel_RemovesStateFile`, `TestCancel_ReturnsErrorOnFailure`
**Details**:
- After Cancel(), `os.Stat(path)` returns `os.IsNotExist(err) == true`
- Cancel() calls `os.Remove(e.path)` -- only removes the single state file
- Other files in the directory are unaffected (os.Remove by contract only removes the named file)
- Cancel on a nonexistent file returns an error (validated by `TestCancel_ReturnsErrorOnFailure`)

### scenario-16: Query methods return independent copies
**Status**: PASSED
**Validated by**: `TestVariables_ReturnsCopy`, `TestHistory_ReturnsCopy`, `TestSnapshot_ReturnsCopy`, `TestMachine_ReturnsCopy`
**Details**:
- **Variables()**: Mutating `vars["KEY"] = "modified"` does not affect `eng.Variables()["KEY"]`
- **History()**: Mutating `hist[0].From = "tampered"` does not affect `eng.History()[0].From`
- **Snapshot()**: Mutating `snap.Variables`, `snap.History`, and `snap.CurrentState` does not affect engine internals
- **Machine()**: Mutating `m.States["done"].Terminal`, `m.States["start"].Transitions[0]`, and adding `m.States["injected"]` does not affect `eng.Machine()`
- All four return defensive copies using `make()` + manual copy or `copy()` builtin

Note: scenario-16 references `-run TestCopySafety` but the actual test names are `TestVariables_ReturnsCopy`, `TestHistory_ReturnsCopy`, `TestSnapshot_ReturnsCopy`. The functionality tested is identical.

## Additional Validation

- `go build ./...` exits 0 with no output
- `go vet ./...` exits 0 with no output
- `go test ./...` passes all packages (cmd/koto, internal/buildinfo, pkg/controller, pkg/engine)
- Walking skeleton tests from issue #4 continue to pass without modification
- `TestRewind_HistoryPreserved` confirms history is appended-to (not truncated) on rewind

## Summary

All 7 scenarios passed. The engine's Rewind, Cancel, and query-copy-safety implementations match the design's acceptance criteria. The rewind semantics correctly handle:
1. Previously visited states (history lookup)
2. Initial state bypass (always valid)
3. Terminal state recovery (rewind FROM terminal allowed)
4. Invalid targets (unvisited states rejected)
5. Terminal targets (rejected to prevent stuck workflows)
6. History preservation (append, not truncate)
