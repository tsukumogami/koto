# Maintainer Review: Issue #4 (Walking Skeleton)

Review focus: maintainability (clarity, readability, duplication, naming)

## Findings

### 1. `Machine()` returns mutable internal pointer -- invisible side effect trap

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:175-177`
**Severity**: Blocking

```go
func (e *Engine) Machine() *Machine {
    return e.machine
}
```

The engine carefully returns copies from `Variables()`, `History()`, and `Snapshot()` to protect internal state. But `Machine()` returns the raw internal pointer. A caller (like the controller at `controller.go:37-38`) can mutate the machine's `States` map, `Transitions` slices, or `Terminal` flags, and the engine will silently use the corrupted definition on the next `Transition()` call.

The next developer will see the copy pattern on `Variables()` and `History()`, assume the engine protects all its internals, and won't think twice about modifying the returned Machine.

The controller already reads `machine.States[current]` -- today it's read-only, but nothing prevents a future caller from doing `machine.States["done"].Terminal = false` and breaking the engine's transition validation.

**Suggestion**: Either return a deep copy (consistent with the other accessors), or document that the returned pointer is shared and must not be modified. A deep copy is safer since this is a public API:

```go
func (e *Engine) Machine() *Machine {
    states := make(map[string]*MachineState, len(e.machine.States))
    for k, v := range e.machine.States {
        ts := make([]string, len(v.Transitions))
        copy(ts, v.Transitions)
        states[k] = &MachineState{Transitions: ts, Terminal: v.Terminal}
    }
    return &Machine{
        Name:         e.machine.Name,
        InitialState: e.machine.InitialState,
        States:       states,
    }
}
```

---

### 2. CLI arg parsing silently ignores missing flag values

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:53-71`
**Severity**: Blocking

```go
case "--name":
    if i+1 < len(args) {
        i++
        name = args[i]
    }
```

When the user writes `koto init --name` with no value after the flag, the code silently skips it and `name` stays empty. Then on line 73, the error says `"--name is required"`. The next developer debugging a user complaint ("I passed --name and it said it's required!") will be confused because the error message implies the flag was absent, not that the value was missing.

The same pattern repeats for `--template`, `--state-dir`, and `--state` across all three command functions.

**Suggestion**: Return a specific error when the flag exists but lacks a value:

```go
case "--name":
    if i+1 >= len(args) {
        return fmt.Errorf("--name requires a value")
    }
    i++
    name = args[i]
```

---

### 3. `HistoryEntry.Type` uses bare strings without constants

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/types.go:32`, `engine.go:133`
**Severity**: Advisory

The string `"transition"` appears as a literal in `engine.go:133` and in test assertions at `engine_test.go:203`. When issue #5 adds `"rewind"` type entries, the same pattern will spread further. The comment on the field says `// "transition" or "rewind"` -- this is documentation that will need manual updating as types are added.

Defining constants now would prevent a misspelling from silently corrupting history entries:

```go
const (
    HistoryTransition = "transition"
    HistoryRewind     = "rewind"
)
```

This isn't blocking because the walking skeleton only uses one type, but it's a trap waiting for issue #5.

---

### 4. Divergent `testMachine()` definitions across packages

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine_test.go:12-28` and `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller_test.go:10-23`
**Severity**: Advisory

Both test packages define a `testMachine()` helper, but they return different machines:
- Engine tests: `start -> middle -> done` (3 states)
- Controller tests: `start -> done` (2 states)

The CLI has a third variant: `stubMachine()` with `ready -> running -> done`.

All three are intentionally different (the controller tests don't need a middle state, and the CLI stub uses different names to be clearly distinct). But the next developer copying a test from one package to another will grab the local `testMachine()` without realizing the definitions differ.

Not blocking since each is self-contained in its own test file and the differences don't affect correctness. Adding a one-line comment like `// Two-state machine (no intermediate state)` to the controller's version would prevent confusion.

---

### 5. `printError` and `printTransitionError` output to stdout, not stderr

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:236-251`
**Severity**: Advisory

Both error output functions use `fmt.Println` (stdout). The design doc says agent-facing commands return structured JSON, so errors on stdout is arguably correct for machine parsing. But the next developer might expect errors on stderr (the Unix convention), and a human running `koto transition foo --state bar 2>/dev/null` would still see errors because they're on stdout.

The design doc is explicit about JSON output for agents, so stdout is intentional. A comment above `printError` noting this is deliberate (`// Errors go to stdout as structured JSON for agent consumption`) would prevent a "helpful" fix that breaks agent parsing.

---

### 6. Controller's `New()` signature doesn't match the design doc

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go:27`
**Severity**: Advisory

The design doc specifies `New(eng *engine.Engine, tmpl *template.Template) (*Controller, error)`, but the implementation is `New(eng *engine.Engine) *Controller`. The godoc comment explains this is intentional for the skeleton:

```go
// In this skeleton, template hash verification is skipped. Full hash
// verification will be added in issue #6.
```

This is well-documented and the right call for a walking skeleton. No action needed now, but issue #9 (remaining CLI subcommands) should wire in the template parameter. The comment makes this clear.

---

### 7. `MarshalJSON` on `TransitionError` is a no-op

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/errors.go:21-24`
**Severity**: Advisory

```go
func (e *TransitionError) MarshalJSON() ([]byte, error) {
    type alias TransitionError
    return json.Marshal((*alias)(e))
}
```

This method does exactly what the default `json.Marshal` would do. The type alias trick is used when you need to break a recursive `MarshalJSON` call, but `TransitionError` doesn't have a recursive case -- it's just struct fields with `json` tags.

The next developer will look at this method and think "there must be custom marshaling logic here" and spend time trying to understand what it does differently. It does nothing differently.

Remove the method entirely, or add a comment explaining it's a forward-compatible hook for when the JSON shape needs to diverge from the struct shape (e.g., when the `error` wrapper envelope is added). If the intent is to produce the `{"error": {...}}` envelope, that's actually done in `main.go:247` -- making this method doubly confusing.

---

## Overall Assessment

The code is clean, well-organized, and closely follows the design doc. File organization is logical: types in `types.go`, errors in `errors.go`, core logic in `engine.go`. The godoc comments are thorough and accurate. The copy-on-read pattern for `Variables()` and `History()` is good defensive practice, well-tested with dedicated mutation tests.

The two blocking findings are both about traps for the next developer: `Machine()` breaks the copy-on-read contract established by the other accessors, and the silent flag-value swallowing will produce misleading errors. Both are straightforward to fix.

Test quality is solid. Names accurately describe what they test. Each test exercises one behavior. The `TestVariables_ReturnsCopy` and `TestHistory_ReturnsCopy` tests are particularly good -- they document the defensive copy contract, so anyone modifying those methods will know the expectation.
