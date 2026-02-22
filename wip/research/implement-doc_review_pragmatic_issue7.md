# Pragmatic Review: Issue #7 - Template Parsing and Interpolation

**Files reviewed:**
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go`
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go`

## Findings

### 1. [Blocking] Single-caller abstraction: `isTransitionsLine`

`template.go:347` -- `isTransitionsLine()` wraps a single `strings.HasPrefix` call and is called exactly once at line 328. Adds a function declaration and doc comment for something that reads just as clearly inline. Inline `strings.HasPrefix(trimmed, "**Transitions**:")` at the call site and delete lines 346-349.

### 2. [Blocking] Custom `contains`/`containsSubstring` in tests reimplements `strings.Contains`

`template_test.go:622-633` -- Hand-rolled `contains` and `containsSubstring` do exactly what `strings.Contains` does. The `contains` function has a needlessly complex condition (`len(s) >= len(substr) && (s == substr || len(s) > 0 && containsSubstring(s, substr))`) when the stdlib handles all edge cases. Replace all calls with `strings.Contains` and delete both functions. `strings` is not currently imported in the test file but should be.

### 3. [Advisory] `parseHeader` error return is dead

`template.go:231,284` -- `parseHeader` returns `err` as its last value but the function never assigns to `err` or returns a non-nil error. Every path returns `nil` for the error. The error return is speculative -- if the intent is to validate required fields in the future, it should be added when that validation exists. Currently it's dead weight that makes the caller handle an impossible error at line 77-78. Not blocking because the unused error path is small and doesn't mask real failures.

### 4. [Advisory] `machineName` fallback to "unnamed" at line 116-119

`template.go:116-119` -- No test exercises the `name: ""` or missing-name path that produces `machineName = "unnamed"`. If this fallback matters, test it. If it doesn't matter, remove it and let the name be empty. Either way, untested code is a minor smell. Not blocking because the fallback is 3 lines and inert.
