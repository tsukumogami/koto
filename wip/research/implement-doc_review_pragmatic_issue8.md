# Pragmatic Review: Issue #8 - State File Discovery

**Files reviewed:**
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover.go`
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover_test.go`

## Summary

Clean, minimal package. 78 lines of implementation, one exported function, no unnecessary abstractions. The implementation is simpler than the design prescribed (avoids importing `engine` by defining its own `stateHeader` struct for JSON parsing, which is the right call). No blocking findings.

## Findings

### 1. [Advisory] `TestFind_ReadsOnlyMinimalFields` tests stdlib behavior

`discover_test.go:212-266` -- This test creates a state file with extra fields (history, variables) and verifies `Find` still works. But `json.Unmarshal` into a struct that lacks those fields ignores them by default -- that's standard Go behavior, not application logic. The test is functionally equivalent to `TestFind_SingleFile` with a larger input file. Not blocking because extra test coverage is inert.

### 2. [Advisory] `TestWorkflow_JSONTags` is a contract test for struct tags

`discover_test.go:268-296` -- Tests that `json.Marshal` produces specific key names. This guards against accidental tag renames, which is marginally useful as a contract lock. But struct tags are compile-time declarations -- if someone changes them, they're already modifying the struct definition. Not blocking because the test is small and serves as documentation of the expected JSON shape.
