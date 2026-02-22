# Review: Issue #5 - pragmatic focus

**Issue**: #5 feat(engine): add rewind, cancel, and query methods
**Files changed**: `pkg/engine/engine.go`, `pkg/engine/engine_test.go`
**Commit**: 48e9dcd

## Findings

### 1. Machine() deep copy is speculative generality (Advisory)

**File**: `pkg/engine/engine.go:239-254`

```go
func (e *Engine) Machine() *Machine {
    states := make(map[string]*MachineState, len(e.machine.States))
    for name, ms := range e.machine.States {
        transitions := make([]string, len(ms.Transitions))
        copy(transitions, ms.Transitions)
        states[name] = &MachineState{
            Transitions: transitions,
            Terminal:    ms.Terminal,
        }
    }
    return &Machine{
        Name:         e.machine.Name,
        InitialState: e.machine.InitialState,
        States:       states,
    }
}
```

The `Machine()` method is called once in `controller.go:37` to check whether the current state is terminal. The controller only reads -- it never mutates the returned machine. The deep copy allocates a new map, new slices, and new MachineState pointers every call. A simpler approach: either expose a read-only accessor for the specific info the controller needs (e.g., `IsTerminal(state string) bool`), or return the pointer directly since all callers are read-only.

However, this was explicitly added as reviewer feedback on issue #4 ("defensive Machine copy"), so it's a conscious design choice for library safety. The cost is small and bounded. **Advisory.**

### 2. Rewind implementation is correct and minimal (No finding)

The `Rewind()` method at `engine.go:144-195` matches the design doc exactly:
- Validates target exists in machine
- Rejects terminal targets
- Allows initial state without history check
- Checks history for visited states
- Preserves full history (appends rewind entry)
- Persists atomically

No over-engineering. No unnecessary abstractions.

### 3. Cancel implementation is correct and minimal (No finding)

`Cancel()` at `engine.go:199-201` is a one-liner delegating to `os.Remove`. Correct.

### 4. Copy-safe query methods are correct (No finding)

`Variables()` at line 210, `History()` at line 219, `Snapshot()` at line 225 all return proper copies. Tests verify mutation independence. This is the right approach for a library API.

### 5. No scope creep detected

The commit touches only the two files listed. No CLI changes, no new packages, no doc changes beyond what the issue requires.

### 6. Test coverage is thorough (No finding)

11 new tests cover:
- Rewind to previously visited state
- Rewind to initial state (never in history as "to")
- Rewind from terminal state (recovery)
- Rewind to never-visited state (error)
- Rewind to terminal state (error)
- Rewind to unknown state (error)
- Rewind persistence verification
- Rewind history preservation
- Cancel success
- Cancel failure (nonexistent file)
- Snapshot copy safety

All align with test plan scenarios 10-16.

## Summary

No blocking findings. The implementation is clean, correct, and matches the design doc. The only observation is that `Machine()` does a deep copy on every call when all current callers are read-only, but this was a deliberate design choice from the #4 review cycle and the cost is negligible.
