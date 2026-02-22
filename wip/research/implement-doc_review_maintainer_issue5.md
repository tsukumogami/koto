# Maintainer Review: Issue #5 (feat(engine): add rewind, cancel, and query methods)

## Review Focus: Maintainability (clarity, readability, duplication)

### Files in scope
- `pkg/engine/engine.go` (Rewind, Cancel, Snapshot, Machine methods)
- `pkg/engine/engine_test.go` (11 new test functions)

---

## Finding 1: Magic strings for history entry types

**File**: `pkg/engine/engine.go:134` and `pkg/engine/engine.go:191`
**Severity**: Advisory

The strings `"transition"` and `"rewind"` appear as literal values when constructing `HistoryEntry` structs:

```go
// engine.go:134
Type: "transition",

// engine.go:191
Type: "rewind",
```

And in tests:

```go
// engine_test.go:204
if snap.History[0].Type != "transition" {

// engine_test.go:580
if last.Type != "rewind" {

// engine_test.go:763
if snap.History[1].Type != "rewind" {

// engine_test.go:871
{"start", "research", "transition"},
{"research", "implementing", "transition"},
{"implementing", "research", "rewind"},
```

The `HistoryEntry.Type` field comment says `// "transition" or "rewind"`, which is good documentation. With only two values and all usage within one package, named constants would be a marginal improvement. That said, when issue #6 adds structured error serialization, these strings will likely also appear in error path tests. If a third type is ever added (the design doc mentions evidence gates could affect history entries), the grep surface grows. Consider defining:

```go
const (
    HistoryTypeTransition = "transition"
    HistoryTypeRewind     = "rewind"
)
```

This is advisory -- the current codebase is small enough that a typo would be caught by existing tests.

---

## Finding 2: Rewind and Transition share structural duplication

**File**: `pkg/engine/engine.go:98-138` (Transition) and `pkg/engine/engine.go:144-195` (Rewind)
**Severity**: Advisory

Both methods follow the same commit-and-persist pattern:

```go
// Transition (lines 128-137)
e.state.CurrentState = target
e.state.Version++
e.state.History = append(e.state.History, HistoryEntry{
    From:      current,
    To:        target,
    Timestamp: time.Now().UTC().Format(time.RFC3339),
    Type:      "transition",
})
return e.persist()

// Rewind (lines 184-194)
e.state.CurrentState = target
e.state.Version++
e.state.History = append(e.state.History, HistoryEntry{
    From:      from,
    To:        target,
    Timestamp: time.Now().UTC().Format(time.RFC3339),
    Type:      "rewind",
})
return e.persist()
```

The only difference is the `Type` string. This isn't dangerous today -- the duplication is in a single file, the methods are short, and the differences are obvious. But if issue #6 adds version conflict detection (re-read before write, compare versions), the persist logic will need to change in both places identically. A small private helper like `commitAndPersist(from, to, entryType string) error` would remove that risk. Advisory because the methods are adjacent and readable; a next developer would see both.

---

## Finding 3: Test names accurately describe behavior

**Severity**: No issue

All 11 new test functions have names that match their assertions:
- `TestRewind_ToPreviouslyVisitedState` -- tests exactly that
- `TestRewind_ToInitialState_NoHistory` -- tests the special case for initial state
- `TestRewind_FromTerminalState` -- tests the recovery path
- `TestRewind_ToNeverVisitedState` -- tests the error case
- `TestRewind_ToTerminalState` -- tests the error case
- `TestRewind_ToUnknownState` -- tests the error case
- `TestRewind_PersistsToFile` -- round-trips through Load
- `TestRewind_HistoryPreserved` -- validates history isn't truncated
- `TestCancel_RemovesStateFile` -- checks file deletion
- `TestCancel_ReturnsErrorOnFailure` -- checks error on missing file
- `TestSnapshot_ReturnsCopy` -- validates copy independence

No test name lies detected.

---

## Finding 4: rewindMachine helper is well-documented

**Severity**: No issue

The `rewindMachine()` helper at `engine_test.go:505-535` includes an ASCII diagram of the state machine topology in its doc comment:

```go
// rewindMachine returns a machine with multiple non-terminal states for
// testing rewind scenarios:
// start -> research -> implementing -> review -> done (terminal)
//      \-> escalated (terminal)
```

This makes the test machine structure immediately visible without reading the struct literal. Good practice for test fixtures with non-trivial topology.

---

## Finding 5: Cancel returns raw os.Remove error, not a TransitionError

**File**: `pkg/engine/engine.go:199-201`
**Severity**: Advisory

```go
func (e *Engine) Cancel() error {
    return os.Remove(e.path)
}
```

Every other engine method that can fail returns a `*TransitionError` with a structured code. `Cancel` returns a raw `*os.PathError` from `os.Remove`. The next developer looking at the CLI's error handling in `main.go:41-47`:

```go
if te, ok := err.(*engine.TransitionError); ok {
    printTransitionError(te)
} else {
    printError("internal_error", err.Error())
}
```

...will see that Cancel errors fall through to the `internal_error` path, which works but produces a generic error code instead of something like `"cancel_failed"`. This is consistent with the current issue's scope (CLI wiring for Cancel comes in issue #9), but the next person wiring Cancel in the CLI might not realize they need special handling.

The design doc's error codes list (`terminal_state`, `invalid_transition`, `unknown_state`, `template_mismatch`, `version_conflict`, `rewind_failed`) does not include a Cancel error code, so this is intentional. Advisory because the raw error is adequate and the CLI will handle it in issue #9.

---

## Finding 6: Rewind godoc comment is thorough

**Severity**: No issue

The `Rewind` method godoc at `engine.go:140-143` covers all four behavioral rules:
1. Target must have been visited (appear in history as "to" field)
2. Or be the machine's initial state (always valid)
3. Rewinding TO a terminal state is not allowed
4. Rewinding FROM a terminal state is allowed (recovery path)

A next developer reading this doc would form the correct mental model of the method's behavior.

---

## Finding 7: TestCancel_ReturnsErrorOnFailure constructs Engine directly

**File**: `pkg/engine/engine_test.go:792-794`
**Severity**: Advisory

```go
eng := &Engine{
    path: path,
}
```

This test bypasses `Init`/`Load` to create an Engine with only the `path` field set. It works because `Cancel` only uses `e.path`, but it creates an implicit contract: the test knows which fields `Cancel` accesses. If Cancel later needs the state (e.g., to log which workflow was cancelled), this test would silently pass while the real call path panics.

This is minor -- the test is explicitly testing the "file doesn't exist" error path, and using `Init` would mean the file would exist (defeating the test's purpose). A comment like `// Directly construct an Engine with a nonexistent path; Cancel only needs e.path` would make the intent clear to the next reader.

---

## Summary Assessment

The implementation is clean and well-organized. The `Rewind` method's validation logic follows a clear sequence (unknown state, terminal check, history check with initial-state exception) that matches the design doc. Test coverage is thorough, with each rewind scenario tested independently and a dedicated `TestRewind_HistoryPreserved` test that validates the design doc's "history is preserved, not truncated" decision.

No blocking findings. The code is readable, the names match the behavior, and the tests document the intended semantics clearly. The advisory items are minor improvements -- constants for history types, a shared commit helper to reduce duplication risk when issue #6 modifies persist logic, and a small comment on the direct Engine construction in the Cancel error test.
