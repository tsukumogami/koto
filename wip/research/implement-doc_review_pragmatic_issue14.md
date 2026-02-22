# Review: Issue #14 - Source Format Compiler (Pragmatic)

## Files Reviewed

- `pkg/template/compile/compile.go` (new, 282 lines)
- `pkg/template/compile/compile_test.go` (new, 599 lines)
- `go.mod` (modified to add `gopkg.in/yaml.v3`)

## Findings

### 1. BLOCKING: Heading collision warning fires on every state transition

**File:** `pkg/template/compile/compile.go:256-264`
**Severity:** Blocking

The warning logic at line 256 checks `if currentState != ""` before emitting a collision warning. Since every state boundary after the first one is encountered while `currentState` is non-empty (pointing to the previous state), every well-formed template with N states produces N-1 warnings.

For `scenario9Source` (5 states), `Compile()` silently produces 4 warnings. `TestCompile_ValidSource` discards them (`ct, _, err`), masking the problem. `TestCompile_HeadingCollisionWarning` tests the same behavior that occurs in every normal compilation, so it doesn't actually test the "accidental collision" case the design describes.

The design doc says: "The compiler emits a warning when a ## heading inside a state's directive section matches the name of another declared state. This catches accidental content reassignment." The intent is to warn about genuinely ambiguous content, not every state boundary.

**Suggested fix:** Only warn when a state heading appears for a state that has *already been seen* (duplicate heading), which is the actual ambiguous case. Normal sequential state boundaries should not warn.

```go
// Track which states have been assigned content.
seenStates := make(map[string]bool)

// In the loop, when a heading matches a declared state:
if currentState != "" && seenStates[headingName] {
    // This state already got content - warn about reassignment
    warnings = append(warnings, Warning{...})
}
flushState()
seenStates[headingName] = true
currentState = headingName
```

Alternatively, if the design truly intends N-1 warnings per template, `TestCompile_ValidSource` should assert the warning count to document this is intentional. Currently it looks like an oversight.

### 2. BLOCKING: Duplicate state headings silently overwrite directives

**File:** `pkg/template/compile/compile.go:266-267`
**Severity:** Blocking

If the markdown body contains `## assess` twice, `flushState()` writes to `directives["assess"]` each time, and the second occurrence silently overwrites the first. No error, no warning. The design doc says "content is assigned by the first ## heading match for each declared state," but the code assigns by the *last* match.

**Suggested fix:** After flushing, check if the state already has a directive:

```go
if _, exists := directives[currentState]; exists {
    return nil, nil, fmt.Errorf("state %q has duplicate ## heading in body", currentState)
}
directives[currentState] = strings.TrimSpace(strings.Join(contentLines, "\n"))
```

Or take first-wins semantics: skip subsequent headings for already-seen states.

### 3. ADVISORY: `splitFrontMatter` duplicated between packages

**File:** `pkg/template/compile/compile.go:182-215` vs `pkg/template/template.go:189-227`
**Severity:** Advisory

The two implementations are nearly identical. Not blocking because the old parser is the v1 format and will likely be removed, but worth noting. If the old parser sticks around, extract to a shared utility.

### 4. ADVISORY: `Warning.String()` method and test

**File:** `pkg/template/compile/compile.go:27-29`, `compile_test.go:593-598`
**Severity:** Advisory

`Warning` is a struct with one field (`Message`) and a `String()` method that returns that field. The test for `String()` is 6 lines testing that `w.String()` returns the message. If `Warning` is just a string carrier, a `type Warning string` would be simpler. If it needs to grow (file/line info), the struct is fine but the test is trivial. Not blocking.

### 5. ADVISORY: `Hash` appends a trailing newline to JSON

**File:** `pkg/template/compile/compile.go:171-173`
**Severity:** Advisory

`Hash()` appends `\n` if the JSON doesn't end with one. `json.MarshalIndent` never produces a trailing newline, so this always fires. The comment says "for consistency" but doesn't specify consistent with what. Not a bug, but the implicit mutation of the serialized bytes before hashing is a minor surprise for callers who might hash the JSON themselves and get a different result.

## Overall Assessment

The implementation is clean and follows the design doc's intent. The compiler correctly uses YAML frontmatter for structure, matches `## headings` against declared states, and produces `CompiledTemplate` values that round-trip through `ParseJSON`. Test coverage is good for the happy path and common error cases.

Two correctness issues need fixing: (1) the heading collision warning is broken -- it fires on every normal state transition, making it useless as a diagnostic, and (2) duplicate state headings silently overwrite content instead of erroring. Both are straightforward fixes.
