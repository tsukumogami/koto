# Architect Review: Issue #8 - feat(discover): implement state file discovery

**Commit**: 5388b5c on branch `docs/koto-engine`
**Files**: `pkg/discover/discover.go` (new), `pkg/discover/discover_test.go` (new)

## Architecture Alignment

### Design Doc Conformance

The implementation matches the design doc (DESIGN-koto-engine.md) Discover API specification exactly:

- `Workflow` struct fields and JSON tags match the spec at lines 511-517 of the design doc
- `Find(dir string) ([]Workflow, error)` signature matches the spec at line 521
- File pattern `koto-*.state.json` matches the naming convention specified in the design

### Dependency Direction: Correct

The design doc specifies: "discover imports only engine (to read state file headers)."

The actual implementation imports **zero** koto packages -- only standard library (`encoding/json`, `errors`, `fmt`, `os`, `path/filepath`). This is stricter than the design requires, achieved by defining a local `stateHeader` struct for minimal JSON unmarshaling instead of importing `engine.State` or `engine.WorkflowMeta`.

This is the right call. The design doc says discover imports engine, but the implementation shows that's unnecessary. The discover package only needs to read four fields from a JSON file. Importing engine to get the `WorkflowMeta` type would create a coupling that doesn't pay for itself -- discover doesn't need `Machine`, `Engine`, `TransitionError`, or any engine behavior. The local `stateHeader` struct reads exactly what it needs and nothing more.

### Decoupling Assessment

The discover package is properly decoupled:

1. **No upward imports**: Does not import controller, template, or cmd packages
2. **No lateral imports**: Does not import engine (even though the design permits it)
3. **No external dependencies**: Standard library only
4. **Self-contained types**: The `Workflow` return type is defined within the package, not borrowed from engine

### Structural Fit

The package follows the same patterns established by the existing `pkg/engine/`, `pkg/template/`, and `pkg/controller/` packages:

- Package doc comment explaining purpose
- Exported types with JSON tags
- Unexported helper types (`stateHeader`, `workflowHeader`)
- Standard library only (matching go.mod which has zero dependencies)
- Table-driven test style consistent with `engine_test.go` and `template_test.go`

### Error Handling Pattern

The `errors.Join` pattern for partial results (return valid workflows + accumulated errors) is a clean approach. It matches the design doc's requirement that discovery continues scanning when individual files fail. This is the correct behavior for a discovery function -- one corrupted file shouldn't prevent finding the others.

## Findings

### Finding 1: stateHeader duplicates engine.WorkflowMeta field names (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover.go`, lines 29-38

The `stateHeader` and `workflowHeader` structs duplicate the JSON field names from `engine.State` and `engine.WorkflowMeta`. If the engine's JSON schema ever changes (e.g., renaming `template_path` or adding a required field), the discover package's header structs would need a coordinated update.

However, this duplication is the consequence of a deliberate decoupling choice. The discover package reads raw JSON -- it doesn't need or want a dependency on the engine package. The duplicated field names are the JSON wire format, which is the actual contract. Both packages reference the same schema (the state file JSON format), not each other.

This is an advisory-level observation, not a blocking concern. The four duplicated field names are stable (defined in the design doc's state file schema), and the test suite (`TestFind_SingleFile`) validates the mapping. If schema drift becomes a real risk, a shared schema validation test in the integration test suite (#10) would catch it without coupling the packages.

**Severity**: Advisory

### Finding 2: No `schema_version` validation (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/discover/discover.go`, lines 62-66

The `stateHeader` struct doesn't read or check the `schema_version` field. If a future koto version changes the state file schema (moving `workflow.name` to a different location, for instance), `Find` would silently return empty or incorrect metadata from files with the new schema version.

For Phase 1 this is fine -- there's only one schema version. When schema version 2 is introduced, the discover package should read `schema_version` and handle the difference. This is an advisory note for that future change, not a blocking concern now.

**Severity**: Advisory

## Summary

The discover package fits the architecture cleanly. It occupies the correct position in the dependency graph (leaf package, no koto imports), implements the exact API surface from the design doc, and follows the coding patterns established by the other three packages. The decision to avoid importing engine is an improvement over the design doc's stated import direction -- it results in a simpler, more decoupled package.

No blocking findings.
