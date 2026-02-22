# Pragmatic Review: Issue #13 (compiled template format with JSON parsing)

## Review Scope

Files reviewed (in the issue's diff):
- `pkg/template/compiled.go` (new)
- `pkg/template/compiled_test.go` (new)
- `pkg/engine/types.go` (modified: added `DeclaredVars`, `Gates`, `GateDecl`)
- `pkg/engine/engine.go` (modified: `Machine()` deep copy updated for gates/declaredvars)

## Findings

### 1. ADVISORY: GateDecl struct duplicated between template and engine packages

`pkg/template/compiled.go:42-48` defines `GateDecl` with identical fields to `pkg/engine/types.go:52-58`. The `BuildMachine()` method manually copies field-by-field between them.

These are in different packages with different purposes (one is JSON-serializable compiled format, the other is the engine's in-memory representation), and the design doc explicitly calls for this separation. The duplication is intentional and bounded -- it prevents the engine from depending on the template package or carrying JSON tags. Not blocking, but worth noting as a maintenance cost if gate fields change.

### 2. ADVISORY: `field_equals` gate with `value: ""` is rejected

`pkg/template/compiled.go:99-101` rejects `field_equals` when `gd.Value == ""`. This means you can't write a gate that asserts a field equals the empty string. The design doc's validation table says "missing required field" for this case, which aligns with the implementation. But `field_equals` with an explicit `"value": ""` in JSON is arguably "present but empty" rather than "missing."

This is a design subtlety, not a code bug. The current behavior matches the design doc's validation table. If checking "field equals empty string" becomes needed, `field_not_empty` with negation or a new gate type would be cleaner. Not blocking.

### 3. No findings on over-engineering

The implementation is lean. `ParseJSON` is a single function with straightforward validation. `BuildMachine` does a direct conversion. No unnecessary abstractions, no speculative generality, no unused configuration options. The `GateDecl` on `MachineState` is needed for issue #16 (gate evaluation) and #17 (command gates), not speculative -- both are in the same milestone.

The deep copy extension in `engine.go:Machine()` for gates and `DeclaredVars` is the minimum needed to maintain the existing immutability contract.

### 4. Test coverage is complete and proportionate

All 13 validation rules from the design doc have corresponding test cases. Tests use table-driven patterns consistent with the existing `engine_test.go` style. The deep copy tests (`TestEngineMachineDeepCopy_IncludesGates`, `TestEngineMachineDeepCopy_NilGatesAndDeclaredVars`) verify the security-relevant immutability contract for the new fields.

The round-trip test (`TestParseJSON_RoundTrip`) and format-version-zero test (`TestParseJSON_FormatVersionZero`) are practical edge cases, not over-testing.

## Summary

No blocking findings. The implementation is the simplest correct approach for the issue requirements. Two advisory observations: the duplicated `GateDecl` struct is intentional per the design's package separation, and `field_equals` with empty string value is rejected (matches the design doc). No unnecessary abstractions, no speculative features, no scope creep.
