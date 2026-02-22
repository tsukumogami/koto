# Architect Review: Issue #14 (feat(template): implement source format compiler)

## Review Scope

Files in the issue's commit:
- `pkg/template/compile/compile.go` (new)
- `pkg/template/compile/compile_test.go` (new)
- `go.mod` / `go.sum` (modified, adds `gopkg.in/yaml.v3`)

## Findings

### Finding 1: Duplicated `splitFrontMatter` function (Blocking)

**Files:**
- `pkg/template/compile/compile.go:182-215`
- `pkg/template/template.go:189-227`

The compiler introduces a second `splitFrontMatter` function that is functionally identical to the one in `pkg/template/template.go`. Both take a string, trim leading whitespace, split on `---` delimiters, and return header+body. Same logic, same edge case handling, same variable names.

This is a parallel pattern. Two copies of the same parsing logic will diverge when one gets a bug fix or behavioral change and the other doesn't. The `compile` package already imports `pkg/template`, so exporting the existing function (e.g., `template.SplitFrontMatter`) adds no new dependency.

The previous architect review classified this as advisory based on the assumption that `template.Parse()` (the v1 parser) would be deprecated. That assessment depends on a future deprecation that hasn't been decided and isn't tracked in any issue. Until then, two callers share identical logic with no shared implementation. Export the function from `pkg/template` and call it from both places.

**Severity:** Blocking. The duplication is concrete and fixable now. Waiting for a hypothetical deprecation is speculative.

### Finding 2: No structural guard on engine's zero-dependency invariant (Advisory)

**File:** `go.mod:5`

The design doc's core architectural constraint: "Zero dependencies for core engine: The engine reads the compiled format using only Go's standard library." The go-yaml dependency is correctly confined at the source level -- only `compile/compile.go` imports it. But Go modules are module-wide; nothing prevents a future change from importing `gopkg.in/yaml.v3` in `pkg/engine/`.

A dependency-boundary test in `pkg/engine/` (using `go list -deps` or inspecting imports) would make this invariant enforceable, not just conventional. This is a standard Go pattern for protecting package boundaries.

**Severity:** Advisory. The constraint is correctly implemented today. The guard test is a defense-in-depth measure, not a fix for a current violation. It doesn't compound -- a bad import to `pkg/engine/` would be caught in review.

### Finding 3: Dependency direction and package placement are correct (No issue)

The dependency graph:

```
pkg/template/compile/ --> pkg/template/   (parent: CompiledTemplate, StateDecl, VariableDecl)
pkg/template/compile/ --> pkg/engine/     (GateDecl type)
pkg/template/compile/ --> gopkg.in/yaml.v3
```

This matches the design doc exactly. The compiler depends on types defined in template and engine. Neither template nor engine imports the compiler. go-yaml is confined to the compiler sub-package. The `compile` package sits above `template` and `engine` in the dependency graph, which is the correct direction.

The `compile` package directly constructs `engine.GateDecl` values (line 123). This is necessary because `template.StateDecl.Gates` is typed as `map[string]engine.GateDecl` -- the compiler must produce the concrete type. Issue #13's architect review already resolved the GateDecl type placement (unified in engine, not duplicated in template), and this code correctly follows that decision.

### Finding 4: Compiler/parser responsibility split is clean (No issue)

The compiler validates source-level concerns (frontmatter presence, required fields, heading/state matching). It does NOT re-validate compiled-format constraints that `ParseJSON` already checks (transition targets referencing declared states, gate type validation, empty directives). The test round-trips through `ParseJSON` to verify correctness.

This is the right split. The compiler is a producer; `ParseJSON` is the validator. No validation duplication.

### Finding 5: `Hash` determinism approach is correct (No issue)

`Hash()` uses `json.MarshalIndent` which sorts map keys lexicographically (Go's documented behavior since 1.12). The function is separate from `Compile()`, so callers can use the compiled template in-memory without serializing. The test verifies byte-identical output across compilations. This matches the design doc's compilation rule #6.

## Summary

One blocking finding: the duplicated `splitFrontMatter` function. Export the existing function from `pkg/template` and call it from the compiler. The fix is contained to two files and doesn't affect any other code paths.

The overall architecture is sound. Package placement, dependency direction, go-yaml confinement, and the compiler/parser responsibility split all follow the design doc and existing codebase patterns.
