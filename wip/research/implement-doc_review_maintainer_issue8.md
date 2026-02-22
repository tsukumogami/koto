# Maintainer Review: Issue #8 -- feat(discover): implement state file discovery

**Reviewer**: maintainer-reviewer
**Files reviewed**: `pkg/discover/discover.go`, `pkg/discover/discover_test.go`
**Commit**: 5388b5c on `docs/koto-engine`

## Overall Assessment

This is clean, well-scoped code. The package does one thing, the function signature is honest about what it returns, and the doc comments match the behavior. The test suite covers the important cases including partial-success semantics. A next developer picking this up would understand it quickly.

Two findings below, one blocking.

---

## Finding 1: Divergent type definitions for state file JSON structure (Blocking)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover.go`, lines 29-38

The `stateHeader` and `workflowHeader` structs duplicate the JSON field names from `engine.State` and `engine.WorkflowMeta` (`pkg/engine/types.go`, lines 9-24). The JSON tags must stay in sync: if `engine.WorkflowMeta` renames `template_path` to `source_path`, `discover` silently returns empty strings for that field. There is no compile-time or test-time coupling to catch the drift.

The design doc (line 221) says discover should import engine. The current implementation chose not to -- likely to keep the package dependency-free. That's a reasonable trade-off, but the consequence is an implicit contract: these two struct definitions must agree on the JSON shape. Nothing in the codebase enforces it.

**What the next developer gets wrong**: They rename a JSON tag in `engine.WorkflowMeta`, run `go test ./pkg/engine/...` (passes), run `go test ./pkg/discover/...` (passes, because the test helper `writeStateFile` uses hardcoded strings that match the old names), and ship a broken `koto workflows` command.

**Suggestion**: Add a comment on `stateHeader` explicitly noting the coupling:

```go
// stateHeader mirrors the JSON shape of engine.State but only the fields
// needed for discovery. If engine.State or engine.WorkflowMeta JSON tags
// change, this struct must be updated to match. See pkg/engine/types.go.
```

Better yet, add a test in `discover_test.go` that marshals an `engine.State` and unmarshals it into a `stateHeader`, asserting the fields round-trip correctly. This turns the implicit contract into an enforced one without adding an import to the production code.

---

## Finding 2: Test helper `writeStateFile` builds JSON from hardcoded strings, not engine types (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover_test.go`, lines 12-39

The test helper constructs state file JSON using `map[string]interface{}` with string keys like `"current_state"`, `"template_path"`, etc. These are duplicated from the engine's JSON tags. If the engine schema changes, these tests keep passing against stale fixture data -- they test that `discover` can parse the old format, not the actual format the engine produces.

This compounds Finding 1: not only does the production code have duplicated JSON knowledge, but the tests do too, so there's no safety net.

**What the next developer gets wrong**: The tests all pass, giving false confidence that discover works with the current engine format.

**Suggestion**: In the test file, import `engine` and construct a real `engine.State` struct, marshal it, and write it to disk. This makes the test fixtures track the actual schema. Since this is test code, the added import doesn't affect the production dependency graph.

```go
import "github.com/tsukumogami/koto/pkg/engine"

func writeStateFile(t *testing.T, dir, name, currentState string) string {
    t.Helper()
    state := engine.State{
        SchemaVersion: 1,
        Workflow: engine.WorkflowMeta{
            Name:         name,
            TemplateHash: "sha256:abc123",
            TemplatePath: "/tmp/template.md",
            CreatedAt:    "2026-02-22T12:00:00Z",
        },
        Version:      1,
        CurrentState: currentState,
        Variables:    map[string]string{},
        History:      []engine.HistoryEntry{},
    }
    // ... marshal and write
}
```

---

## Not Flagged (code that reads well)

- `Find` returning `[]Workflow{}`/non-nil on empty is documented and tested -- the nil-vs-empty distinction is explicit.
- Partial results + aggregated error via `errors.Join` is a good pattern. The doc comment on `Find` explains the contract clearly.
- The `stateHeader` approach (minimal unmarshal) is the right call for a discovery function that may scan many files.
- Test names match their assertions. `TestFind_CorruptedFile` tests corrupted files. `TestFind_NonMatchingFilesIgnored` tests non-matching files. No lies here.
- The `TestFind_MultipleFiles` test uses a map for order-independent assertions, which is correct since `filepath.Glob` order is platform-dependent.

## Summary

| Severity | Count |
|----------|-------|
| Blocking | 1 |
| Advisory | 1 |

The blocking finding is about the implicit coupling between `discover`'s `stateHeader` and `engine.State` JSON shapes. A schema change in engine will silently break discover with no test catching it. The fix is small: either a cross-package round-trip test or at minimum a comment pointing to the coupled type.
