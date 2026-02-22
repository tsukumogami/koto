# Maintainer Review: Issue #14 (feat(template): implement source format compiler)

**Review focus**: maintainability (clarity, readability, duplication)
**Files reviewed**: `pkg/template/compile/compile.go`, `pkg/template/compile/compile_test.go`, `go.mod`, `go.sum`

## Findings

### 1. Duplicated `splitFrontMatter` across packages -- divergent twins

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compile/compile.go:180-215`
**Counterpart**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:189-227`
**Severity**: Blocking

`compile.go` contains a `splitFrontMatter` function that is a near-identical copy of the one in `template.go`. The logic, variable names, error messages, and edge-case handling are the same. The only differences are cosmetic (fewer inline comments in the compile version).

Both are unexported, in separate packages. There is no compilation signal linking them. When someone fixes a bug in one -- for instance, handling `---` on the final line without a trailing newline -- they won't discover the other copy. The functions share identical error message strings, so even grepping for the error text will find both but without any indication they should stay in sync.

Since `compile` already imports `template`, the dependency direction supports extracting this into a shared function. Either export `SplitFrontMatter` from the `template` package, or add an unexported shared helper that both callers use.

This is blocking because the next developer who encounters a frontmatter parsing bug will fix one copy and leave the other broken, creating a subtle behavioral divergence between `Parse()` and `Compile()`.

### 2. Duplicate validation between Compile() and ParseJSON()

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compile/compile.go:75-96`
**Counterpart**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:49-66`
**Severity**: Advisory

`Compile()` checks for missing `name`, `version`, `initial_state`, empty `states`, and `initial_state` not in `states` (lines 75-96). These same checks exist in `ParseJSON()` with identical error messages. The error strings are literal duplicates across two packages.

Failing early in the compiler before body parsing is a reasonable choice -- it avoids wasting time extracting directives from the markdown body when the frontmatter is already invalid. But there's no comment in either location acknowledging the duplication or explaining the boundary between compiler-level validation and compiled-format validation.

A brief comment in `Compile()` like `// Validate early before parsing body. ParseJSON performs the full set including gate type checks on the compiled output.` would help the next developer understand why validation appears in both places, what the compiler intentionally skips (gate type validation), and that changing one set requires checking the other.

### 3. `Hash()` determinism relies on implicit `encoding/json` behavior

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compile/compile.go:164-178`
**Severity**: Advisory

The `Hash()` function uses `json.MarshalIndent` to produce deterministic JSON output. The design doc requires "sorted keys (deterministic output)" (compilation rule 6). Go's `encoding/json` sorts map keys alphabetically, satisfying this requirement implicitly.

The next developer reading `Hash()` will see `json.MarshalIndent` and wonder where the key sorting happens. The function's godoc says "deterministic JSON" but doesn't explain the mechanism. `TestCompile_DeterministicOutput` verifies the property (compile twice, compare bytes) but also doesn't explain why it works.

Add a one-line comment: `// encoding/json sorts map keys alphabetically, producing deterministic output.`

### 4. Test scenario numbers are orphaned references

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compile/compile_test.go` (lines 78, 194, 249, 332, 364)
**Severity**: Advisory

Test comments reference "scenario 9", "scenario 10", etc.:

```go
// TestCompile_ValidSource (scenario 9) verifies that...
// TestCompile_SubheadingNotBoundary (scenario 10) verifies that...
```

These numbers presumably come from the implementation plan or issue description. The wip/ directory (where plans live) gets cleaned before merge, so the numbering context will be gone. The test names are descriptive enough on their own -- `TestCompile_HeadingCollisionWarning` communicates clearly without knowing it's "scenario 11".

Either drop the scenario numbers or add a one-line comment at the top of the test file explaining the numbering source.

### 5. Package-level doc on `template` package is stale

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:1-20`
**Severity**: Advisory

The `template` package doc still describes only the legacy format with `**Transitions**` in the body. With `CompiledTemplate`, `ParseJSON()`, and `BuildMachine()` from issue #13, and now the `compile` sub-package, the package has two distinct parsing paths. A developer discovering the package through godoc will form an incomplete mental model.

### 6. Heading collision warning and behavior are correct

The `parseBody` function (lines 227-290) correctly implements first-wins semantics: when a `## heading` matches an already-claimed state, it's appended to the current state's `contentLines` (line 268) with a warning, NOT treated as a new state boundary. The warning message matches the design doc verbatim. The test `TestCompile_HeadingCollisionWarning` verifies this by checking that the duplicate `## plan` appears as content inside `assess`'s directive. This is well-implemented.

### 7. Code organization and readability are strong

The `Compile()` function reads top-to-bottom in a natural order: split frontmatter, parse YAML, validate required fields, parse body directives, verify every state has a heading, assemble the compiled template. Internal types (`sourceFrontmatter`, `sourceStateDecl`) are well-named and appropriately scoped. The `parseBody` godoc clearly explains the first-wins rule and the distinction between boundary headings and content headings. The `Warning` type is minimal and appropriate.

Test coverage is thorough: valid compilation with round-trip through `ParseJSON`, subheading-as-content, heading collision with warning, missing heading, deterministic output, non-state heading as content, missing frontmatter, missing required fields, invalid initial_state, and hash determinism. Test names accurately describe what they verify.

## Summary

| Severity | Count |
|----------|-------|
| Blocking | 1 |
| Advisory | 4 |

The blocking finding is the duplicated `splitFrontMatter` across `template.go` and `compile/compile.go`. These are identical functions in separate packages with no link between them. The next developer who fixes a frontmatter bug will fix one and miss the other. Since `compile` already imports `template`, extracting a shared function is straightforward.

The advisory findings are documentation-level: a comment explaining why validation appears in two places, a comment on `Hash()` explaining the determinism mechanism, orphaned scenario numbers in test comments, and a stale package doc. None of these will cause bugs, but they'd reduce the time the next developer spends building a mental model.

Overall the implementation is clean, well-tested, and correctly implements the design doc's compilation rules. The heading collision logic (first-wins semantics, warning, content absorption) works exactly as specified.
