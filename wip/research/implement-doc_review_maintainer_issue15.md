# Maintainer Review: Issue #15 (Evidence Support with Transition Options)

## Review Scope

Files changed: `pkg/engine/types.go`, `pkg/engine/engine.go`, `pkg/engine/engine_test.go`, `pkg/controller/controller.go`, `pkg/controller/controller_test.go`

## Findings

### 1. Stale comment on `deepCopyState` -- shallow copy of HistoryEntry.Evidence

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:460-483`
**Severity**: Blocking

The godoc on `deepCopyState` says "the copy shares no references with the original." This is now false. `HistoryEntry` gained an `Evidence map[string]string` field in this issue, but the history copy at line 481-483 uses Go's built-in `copy()`, which copies structs by value -- meaning the `Evidence` map inside each `HistoryEntry` is a shared reference, not a deep copy.

```go
// deepCopyState returns a deep copy of a State value, duplicating the
// History slice and Variables map so the copy shares no references with
// the original.
func deepCopyState(s State) State {
    // ...
    if s.History != nil {
        cp.History = make([]HistoryEntry, len(s.History))
        copy(cp.History, s.History)  // <-- shallow: Evidence maps are shared
    }
    return cp
}
```

The current code doesn't trigger a bug because existing history entries' Evidence maps are never mutated after appending. But the next developer reading the godoc will trust "shares no references" and make changes accordingly -- for example, adding a feature that modifies historical evidence entries after a rollback, which would corrupt the saved `prev` state.

The same issue exists in `History()` at line 296-300, which also uses `copy()` and returns HistoryEntry structs with shared Evidence maps.

**Fix**: Either deep-copy each HistoryEntry's Evidence map in both `deepCopyState` and `History()`, or amend the godoc to document the exception. Deep copy is the safer choice since both functions promise independence from internal state.

```go
if s.History != nil {
    cp.History = make([]HistoryEntry, len(s.History))
    for i, h := range s.History {
        cp.History[i] = h
        if h.Evidence != nil {
            cp.History[i].Evidence = make(map[string]string, len(h.Evidence))
            for k, v := range h.Evidence {
                cp.History[i].Evidence[k] = v
            }
        }
    }
}
```

### 2. Controller merges evidence into variables without comment about design boundary

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go:76-80`
**Severity**: Advisory

```go
// Build interpolation context: variable defaults, init-time
// vars, then evidence (last wins on key collision).
ctx := c.eng.Variables()
for k, v := range c.eng.Evidence() {
    ctx[k] = v
}
```

The comment says "last wins on key collision," which correctly describes the behavior. But the design doc specifies a namespace collision rule where evidence keys must NOT shadow declared variable names (rejected with error). That enforcement is scoped to issue #16. The next developer looking at this code before #16 lands will see evidence deliberately overwriting variables and think that's the intended final behavior.

A one-line comment noting that #16 will add shadow rejection at the engine layer would prevent confusion:

```go
// Build interpolation context: variable defaults, init-time
// vars, then evidence (last wins on key collision).
// Note: issue #16 adds shadow rejection at the engine layer
// so evidence keys can't collide with declared variable names.
ctx := c.eng.Variables()
```

### 3. `TestEvidence_OmitemptyWhenEmpty` asserts nothing

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine_test.go:1956-1981`
**Severity**: Advisory

This test reads the persisted file and unmarshals it, but then has no assertions. It ends with a comment explaining why the behavior is expected, but the test doesn't actually verify anything. A test with no assertions always passes. If the test name says "OmitemptyWhenEmpty," the next developer expects it to verify omitempty behavior.

Either add an assertion (e.g., check that the `evidence` key is present and is `{}`) or convert it to a comment in another test. A test that documents behavior via comments but asserts nothing is misleading -- it looks like coverage but isn't.

### 4. Test names consistently clear and behavior-focused

The evidence test names follow a clear pattern: `TestTransition_WithEvidence_MergesIntoState`, `TestRewind_DoesNotModifyEvidence`, `TestTransition_WithEvidence_StateRestoredOnPersistFailure`. Each name accurately describes what the test verifies. The test for backward compatibility (`TestTransitionOption_ZeroOpts_BackwardCompat`) is well named and documents the API contract.

### 5. `WithEvidence` called with nil map -- defensive but undocumented

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:40-43`
**Severity**: Advisory

```go
func WithEvidence(evidence map[string]string) TransitionOption {
    return func(cfg *transitionConfig) {
        cfg.evidence = evidence
    }
}
```

If someone calls `WithEvidence(nil)`, the `len(cfg.evidence) > 0` guards in `Transition()` protect against nil dereference, so it works correctly. But the godoc doesn't mention this. If a caller passes `WithEvidence(nil)` expecting it to clear evidence, nothing happens (evidence map becomes nil, len check skips the merge, existing evidence is untouched). This is probably fine since clearing evidence is not a documented use case, but a brief note in the godoc ("nil maps are treated as empty") would prevent questions.

### 6. `Load` backward compat only handles schema_version 1

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:101-107`
**Severity**: Advisory

```go
if state.SchemaVersion == 1 {
    if state.Evidence == nil {
        state.Evidence = map[string]string{}
    }
}
```

There's no rejection for unknown schema versions (e.g., 3, 0, or negative values). The design doc says `Load` accepts v1 and v2, implying other versions should be rejected. A state file with `schema_version: 3` from a future koto version would load silently. This is arguably a pragmatic concern rather than maintainability, but the lack of a version check means the next developer adding schema_version 3 won't find an obvious place to add migration logic -- they'd need to add a new conditional rather than extending an existing version switch.

## Overall Assessment

The implementation is clean and well-structured. The functional options pattern for `TransitionOption` is idiomatic Go. Evidence storage, accumulation, and persistence across rewind all follow the design doc. Tests are thorough and accurately named.

The blocking finding is the shallow copy of `HistoryEntry.Evidence` in `deepCopyState` and `History()`. The godoc explicitly promises no shared references, but that promise is now broken for Evidence maps within history entries. While it doesn't cause a bug today, it will mislead the next developer who trusts the documented contract.
