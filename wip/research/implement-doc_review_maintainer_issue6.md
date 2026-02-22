# Maintainer Review: Issue #6 -- Version Conflict Detection and TransitionError JSON Serialization

## Summary

The code is well-structured overall. Error constants are clear, the `TransitionError` type is straightforward, and test coverage is thorough. Two issues deserve attention -- one blocking, one advisory.

## Findings

### 1. In-memory state corrupted on persist failure (Blocking)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go`, lines 128-137 (Transition) and 184-194 (Rewind)

Both `Transition()` and `Rewind()` mutate the in-memory state (CurrentState, Version, History) *before* calling `persist()`. If `persist()` fails -- either from a version conflict or a disk I/O error -- the Engine's in-memory state no longer matches what's on disk. The caller gets an error, but the Engine object is now poisoned:

```go
// engine.go:128-137
e.state.CurrentState = target
e.state.Version++
e.state.History = append(e.state.History, HistoryEntry{...})

return e.persist()  // If this fails, in-memory state is already changed
```

The next developer will think: "I got a version_conflict error, let me re-read and retry." But the Engine they're holding has already been mutated. Calling `eng.CurrentState()` returns the *target* state even though the transition never persisted. Calling `eng.Snapshot()` returns a version number that doesn't exist on disk. A retry call to `Transition()` will compute the wrong `expectedVersion` (it was already incremented), so even if the conflict is resolved, the next persist will either succeed with a skipped version number or fail with a spurious conflict.

This is the classic "mutate then persist" bug. Either:
- Save the prior state and restore it on error (rollback pattern), or
- Build the new state in a local variable and only assign to `e.state` after `persist()` succeeds

The second option is cleaner and requires minimal change.

### 2. "not_implemented" error code is not a defined constant (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, line 208

```go
func cmdStub(name string) error {
    return &engine.TransitionError{
        Code:    "not_implemented",
        Message: fmt.Sprintf("%s is not yet implemented", name),
    }
}
```

The six error codes in `errors.go` are well-defined constants, but the CLI introduces a seventh code as a raw string. The next developer seeing the constants in `errors.go` will assume that's the complete set of codes that can appear in JSON output. They won't find `"not_implemented"` there.

This is advisory rather than blocking because `cmdStub` is explicitly temporary (issue #9 will replace it) and the string only appears in one place. But if anyone writes a client that switches on error codes, they'll miss this one.

Suggestion: Add `ErrNotImplemented = "not_implemented"` to the constants in `errors.go`, or use `fmt.Errorf` instead of `TransitionError` for non-domain errors (since "not implemented" is not really a transition error).

### 3. Magic strings for HistoryEntry.Type (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go`, lines 134 and 191

The strings `"transition"` and `"rewind"` are used as `HistoryEntry.Type` values in `Transition()` and `Rewind()` respectively. These aren't defined as constants anywhere. The comment on the struct field in `types.go:31` documents them (`// "transition" or "rewind"`), but if a third type is added (e.g., `"reset"`), the next developer has to find all comparison sites by searching strings.

Currently there are no code paths that *compare* these values -- they're write-only from the engine's perspective, and consumers treat them as opaque strings. The tests assert exact values, which provides a safety net. So this is advisory: the risk is low today, but the pattern of defining some domain strings as constants (`ErrTerminalState` etc.) and leaving others as inline strings creates a split convention that will confuse the next contributor.

Suggestion: Define `HistoryTypeTransition = "transition"` and `HistoryTypeRewind = "rewind"` constants alongside the error code constants, or in `types.go` near the struct definition.

### 4. Directive action strings follow the same pattern (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go`, lines 19, 66, 73

The `Directive.Action` field uses `"execute"` and `"done"` as inline strings with only a comment documenting the valid values. Same split-convention observation as finding #3. Lower priority because the controller package is smaller and the values are used in only one function.

## What's Clear

- The error constant naming (`ErrTerminalState`, `ErrInvalidTransition`, etc.) is consistent and self-documenting.
- The `TransitionError` struct with `omitempty` tags gives clean JSON for both full and minimal errors -- the test at `TestTransitionError_JSONOmitempty` documents this contract well.
- The version conflict detection logic in `persist()` is easy to follow: compute expected version as `current - 1`, skip for Init (version 1), re-read and compare.
- The `checkVersionConflict` function correctly uses a minimal struct to unmarshal only the version field, avoiding coupling to the full State schema.
- Template hash verification in `Controller.New()` is a clean separation -- the engine doesn't know about templates, the controller does.
- Test names accurately describe what they test.
