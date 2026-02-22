# Architect Review: Issue #13 (Compiled Template Format with JSON Parsing)

## Files Changed

- `pkg/template/compiled.go` (new)
- `pkg/template/compiled_test.go` (new)
- `pkg/engine/types.go` (modified: added `Gates` to `MachineState`, added `GateDecl`, added `DeclaredVars` to `Machine`)
- `pkg/engine/engine.go` (modified: `Machine()` deep copy updated for `Gates` and `DeclaredVars`)

## Findings

### Finding 1: Duplicate GateDecl types -- parallel pattern introduction

**Severity**: blocking

**Files**: `pkg/template/compiled.go:42-48` and `pkg/engine/types.go:52-58`

The implementation defines two structurally identical `GateDecl` types:

- `template.GateDecl` (compiled.go:42) -- JSON-serializable, used in `CompiledTemplate`
- `engine.GateDecl` (types.go:52) -- non-JSON, used in `Machine`/`MachineState`

Both have the exact same five fields: `Type`, `Field`, `Value`, `Command`, `Timeout`. The `BuildMachine()` method (compiled.go:120-131) manually copies field-by-field between them.

This is a parallel pattern. Two types for the same concept with a manual translation layer between them. When a new gate field is added (and the design anticipates this -- new gate types are an explicit extension point), both types must be updated and the copy logic must be kept in sync. Future contributors will encounter two `GateDecl` types and won't know which to extend.

The design doc defines `GateDecl` once, in the compiled template types section. It does not show a separate engine-level gate type. The intent is that compiled template types flow into the engine's machine.

**Recommendation**: Use `engine.GateDecl` with JSON tags as the single definition. The template package already imports `engine`, so this doesn't create a new dependency. `StateDecl.Gates` would become `map[string]*engine.GateDecl`. This eliminates the translation code in `BuildMachine()` and the duplicate maintenance surface.

Alternatively, if the intent is to keep the compiled format's types self-contained (avoiding JSON tags on engine types), add a brief comment documenting the intentional duplication and the sync contract. But given the engine already uses the same field names and the template package already imports engine, using one type is cleaner.

### Finding 2: Design alignment -- types match the design doc specification

**Severity**: not a finding (positive)

The `CompiledTemplate`, `VariableDecl`, `StateDecl`, and `GateDecl` types match the design doc's Go types section exactly (DESIGN-koto-template-format.md lines 436-466). JSON tags match the compiled JSON example. Validation rules in `ParseJSON()` cover all 11 checks from the design doc's validation rules table (lines 470-484). The `format_version` field correctly uses its own versioning track separate from the state file's `schema_version`.

### Finding 3: Design alignment -- BuildMachine correctly populates engine types

**Severity**: not a finding (positive)

`BuildMachine()` correctly:
- Maps `StateDecl` to `engine.MachineState` including `Gates`, `Transitions`, and `Terminal`
- Populates `DeclaredVars` from the variable declarations (needed for namespace collision rejection in #16)
- Deep-copies transition slices (prevents aliasing between compiled template and machine)
- Returns nil maps when no gates/variables exist (avoiding empty-map allocation)

### Finding 4: Engine deep copy correctly extended for new fields

**Severity**: not a finding (positive)

`Engine.Machine()` in engine.go was correctly updated to deep-copy `Gates` (pointer-value copy per entry) and `DeclaredVars`. The test `TestEngineMachineDeepCopy_IncludesGates` verifies mutation isolation, and `TestEngineMachineDeepCopy_NilGatesAndDeclaredVars` covers the nil case. This prevents the gate injection attack surface identified in the design.

### Finding 5: Dependency direction is correct

**Severity**: not a finding (positive)

`pkg/template/compiled.go` imports `pkg/engine` (downward dependency). No new imports were added to the engine package. `engine` remains the anchor with zero imports of other koto packages. The zero-external-dependency constraint (`go.mod` has no `require` block) is maintained.

### Finding 6: Controller not updated for compiled templates

**Severity**: advisory

The controller currently works with `template.Template` (the v1 markdown-parsed type) which has `Sections map[string]string` for directive lookup. The new `CompiledTemplate` has directives embedded in `StateDecl.Directive` and doesn't produce a `template.Template`. There's no bridge between `CompiledTemplate` and the controller yet.

This isn't a problem right now -- the controller integration is an implicit part of #14 or a later issue. But it's worth noting that `CompiledTemplate.BuildMachine()` only builds the `Machine`; it doesn't produce the `Template` struct the controller needs. The design doc's issue table shows #14 depends on #13, so this gap will be filled. No action needed in this PR.

### Finding 7: Validation uses map iteration order -- non-deterministic error messages

**Severity**: advisory

`ParseJSON()` iterates over `ct.States` (a map) to validate states, transitions, and gates (compiled.go:78). If multiple states have errors, the error returned depends on Go's random map iteration order. For example, if both state "a" and state "b" have empty directives, the error message will vary between runs.

This doesn't affect correctness (the function returns on first error). But it means test assertions on specific error messages from multi-error templates could be flaky in theory. The current tests all use single-error inputs, so this doesn't cause problems today. Not worth changing unless error collection is added later.

### Finding 8: Test coverage is thorough for all design-specified validation rules

**Severity**: not a finding (positive)

Tests cover:
- All 11 validation rules from the design doc
- All 3 gate types with their required field combinations
- JSON round-trip (parse, marshal, parse again)
- format_version 0 (Go zero value / omitted field)
- BuildMachine with and without variables/gates
- Engine deep copy isolation for gates and DeclaredVars
- Nil gates/DeclaredVars edge case

The test plan scenarios 1-8 are fully covered by the implementation.
