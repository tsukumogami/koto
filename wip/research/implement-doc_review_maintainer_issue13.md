# Maintainer Review: Issue #13 (Compiled Template Format with JSON Parsing)

## Review Focus: Clarity, Readability, Duplication

## Files Reviewed

- `pkg/template/compiled.go` (new)
- `pkg/template/compiled_test.go` (new)
- `pkg/engine/types.go` (modified: added `GateDecl`, `DeclaredVars`, `Gates`)
- `pkg/engine/engine.go` (modified: `Machine()` deep copy updated for gates and declared vars)

## Findings

### Finding 1: GateDecl godoc says "before entering" but design says "before leaving"

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/types.go:50-52`
**Severity:** Blocking

```go
// GateDecl represents a gate declaration on a machine state. Gates are
// preconditions that must be satisfied before entering the state.
type GateDecl struct {
```

The design doc is explicit: "Gates are exit conditions: all gates on a state must pass before leaving that state." The godoc says the opposite -- "before entering the state." The next developer implementing gate evaluation in issue #16 will read this comment and wire up gate checks at the wrong point. They'll check gates when entering a state instead of when leaving it (i.e., during the transition out). This inverts the evaluation timing: the `assess` state's `task_defined` gate would be checked when entering `assess` (where evidence doesn't exist yet) instead of when leaving `assess` (where evidence has been accumulated).

**Suggestion:** Change to `"Gates are exit conditions that must be satisfied before leaving this state."`

### Finding 2: Duplicate GateDecl type conversion pattern

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:120-132` and `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:258-269`
**Severity:** Advisory

The field-by-field copy from `template.GateDecl` to `engine.GateDecl` in `BuildMachine()` is identical in shape to the deep copy in `engine.Machine()`. Both do:

```go
gates[gn] = &engine.GateDecl{
    Type:    gd.Type,
    Field:   gd.Field,
    Value:   gd.Value,
    Command: gd.Command,
    Timeout: gd.Timeout,
}
```

These are different source types (`template.GateDecl` vs `engine.GateDecl`), so they can't literally share code without an interface. But if a new field is added to `engine.GateDecl` (e.g., a `Description` field in a future gate enhancement), a developer updating one copy-site would need to find and update the other. The test coverage is strong enough to catch this (the round-trip and deep copy tests would fail if a field were missing), so this is advisory rather than blocking.

### Finding 3: Gate type strings are bare literals in two packages

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:91-107`
**Severity:** Advisory

The gate type strings `"field_not_empty"`, `"field_equals"`, and `"command"` appear as bare string literals in the `ParseJSON` validation switch. These same strings will reappear in at least two more places: the gate evaluator (issue #16) and the command gate executor (issue #17). A typo in any one location (e.g., `"field_not_empty"` vs `"field_notempty"`) would silently break gate matching.

The design doc's validation table uses these exact strings, and test assertions pin them. But once gate evaluation lands, the literal duplication across packages becomes a maintenance concern. Consider defining constants in `pkg/engine/` (where `GateDecl` lives) before issues #16-#17 are implemented. This is advisory now because only one consumer exists today.

### Finding 4: Test names and organization are clear and well-structured

The test file at `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled_test.go` is well organized:

- `TestParseJSON_ValidTemplate` -- happy path with full assertions
- `TestParseJSON_MissingRequiredFields` -- table-driven, one case per field
- `TestParseJSON_StateMachineIntegrity` -- structural validation
- `TestParseJSON_GateValidation` -- gate-specific rules
- `TestBuildMachine_WithGates` -- conversion fidelity
- `TestEngineMachineDeepCopy_IncludesGates` -- defensive copy verification
- `TestParseJSON_RoundTrip` -- JSON stability
- `TestParseJSON_FormatVersionZero` -- zero-value edge case

Test names match what they assert. The `validCompiledJSON` constant as a shared fixture is a good pattern. The deep-copy tests proactively verify that the new `Gates` and `DeclaredVars` fields can't be tampered with through returned copies, consistent with the existing codebase pattern.

### Finding 5: `compiled.go` validation order matches the design doc table

The validation checks in `ParseJSON` at `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:59-110` follow the same order as the design doc's validation rules table. This makes it easy to audit completeness by reading the design doc and code side by side. The error messages match the design doc's specified format strings exactly. The 11 validation rules from the design doc's table are all present (the "Invalid JSON" case is handled by `json.Unmarshal` returning an error). Clean work.

## Summary

The implementation is well-structured and readable. Types, functions, and tests are named clearly and accurately describe what they do. The code is concise, uses no external dependencies, and follows the existing codebase patterns closely.

One finding is blocking: the `GateDecl` godoc comment says gates are checked "before entering" a state, but the design specifies they're exit conditions checked "before leaving" a state. This will mislead whoever implements gate evaluation in issue #16.

Two advisory findings: gate type strings should eventually become named constants (before issues #16-#17 add more consumers), and the GateDecl field-by-field copy exists in two places (mitigated by good test coverage).
