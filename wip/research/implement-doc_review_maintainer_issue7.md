# Maintainer Review: Issue #7 - Template Parsing and Interpolation

## Files Reviewed

- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go`
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go`

## Overall Assessment

The code is well-structured, well-documented, and closely follows the design doc's specifications for `pkg/template/`. The package doc comment, the `Template` struct comments, and the `Parse` function's error contract are all clear enough that the next developer can understand what this code does and modify it confidently. The test file covers the main paths and edge cases.

Below are the specific issues I found.

---

## Findings

### 1. `Template.Machine` exposes a mutable pointer -- diverges from engine convention

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:39`
**Severity**: Advisory

The `Template` struct holds `Machine *engine.Machine`, and `Parse` returns this directly. The engine package established a convention: `Engine.Machine()` returns a deep copy to prevent callers from mutating internal state. `Template.Machine` is a raw pointer field, so any caller that receives a `*Template` can mutate the machine's state map and transition slices.

This is advisory rather than blocking because `Template` is a data-transfer struct returned from `Parse`, not a long-lived object with concurrent access. But the next developer who reads both packages will wonder whether the inconsistency is intentional or accidental.

**Suggestion**: Either document that `Template` fields are owned by the caller after `Parse` returns (making mutation the caller's problem), or make `Machine` a value field with a copy. At minimum, add a one-line comment explaining that `Template` is a data-transfer object and the caller owns the returned value.

---

### 2. Implicit contract: first `## heading` is the initial state, with no validation or documentation at the call site

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:123`
**Severity**: Blocking

Line 123 sets `InitialState: stateNames[0]` -- the first `## heading` in the markdown body becomes the initial state. The `Parse` function's doc comment (lines 46-55) describes error conditions but never mentions this rule. The inline comment says `// first state is the initial state` but only someone reading the implementation will see it.

A template author who reorders sections during editing (moving a new "setup" state above the old first state) will silently change the initial state of every workflow that uses that template. There's no way to declare the initial state explicitly, so the template format encodes meaning in document order without telling the author.

This is blocking because the next developer (or template author) will not know that heading order determines initial state, and a casual reorder creates a silent behavioral change that the template hash will catch only if there's already a running workflow.

**Suggestion**: Add to the `Parse` function's doc comment: "The first ## heading in the body becomes the machine's initial state." Consider also logging or validating this in a future iteration -- potentially allowing `initial_state:` in the front-matter as an explicit override.

---

### 3. `parseHeader` silently ignores unknown keys and malformed lines

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:264-266`
**Severity**: Advisory

Lines 264-266: if a line doesn't split into two parts on `:`, it's silently skipped. If a key is not one of `name`, `version`, `description`, `variables`, it's silently ignored. This means a typo like `naem: my-workflow` or `desciption: ...` produces a template with an empty name or description, with no error and no warning.

The design doc says the header parser is intentionally simple and doesn't handle arbitrary YAML, which is fair. But silent key misses are a debugging trap: the next developer will write a header with a typo, get empty fields, and spend time debugging the template format rather than the typo.

**Suggestion**: This is a known trade-off of the minimal parser approach. Consider returning a warning (not an error) for unrecognized top-level keys, or at minimum document in the `parseHeader` comment that unknown keys are silently dropped. If warnings aren't worth the API complexity now, a comment noting the trade-off will help the next developer understand the decision was deliberate.

---

### 4. `parseHeader` returns an error parameter that is always nil

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:231`
**Severity**: Advisory

The function signature includes `err error` in the return, but no code path in `parseHeader` ever returns a non-nil error. The caller in `Parse` (line 75-78) dutifully checks this error, creating dead code.

This isn't actively harmful -- it's reasonable to have the error return as a forward-looking API decision (future validation might fail). But the next developer will look for the error paths and find none, then wonder if they're missing something.

**Suggestion**: Either add a comment `// err is reserved for future validation` or remove the error return until there's a real error path. Given the silent-typo issue in finding #3, this is probably the right place to add validation errors in the future.

---

### 5. Custom `contains`/`containsSubstring` in test file reimplements `strings.Contains`

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go:622-633`
**Severity**: Advisory

Lines 622-633 define `contains` and `containsSubstring` functions that replicate `strings.Contains` from the standard library. The implementation is correct but will confuse the next developer: "Why didn't they use `strings.Contains`? Is there a subtle difference I'm missing?" There isn't -- the behavior is identical.

The `strings` package is already imported in `template.go`; adding it to the test file's imports is trivial.

**Suggestion**: Replace with `strings.Contains`. The custom implementations add cognitive load for no behavioral difference.

---

### 6. `TestParse_HashChangesWithContent` has dead code that obscures intent

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go:303-304`
**Severity**: Advisory

Lines 303-304 construct a `modified` variable by appending to `validTemplate`, then lines 305-318 immediately reassign `modified` to a completely different string. The first assignment is dead code. The next developer will read the append logic, form a mental model of what `modified` contains, then find it's overwritten. The overwrite suggests the original approach didn't work (the appended `## extra` state would need transitions or be terminal, and the original template doesn't reference it), but the dead code doesn't explain this.

**Suggestion**: Remove the dead `modified := validTemplate + "\n## extra\n\nExtra state.\n"` line and its comment. If the intent was to show why simple appending doesn't work, a comment explaining the rewrite is better than dead code.

---

### 7. Transitions line matching is case-sensitive and whitespace-sensitive with no documentation of exact format

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:348`
**Severity**: Advisory

`isTransitionsLine` checks for the exact prefix `**Transitions**:`. A template author who writes `**transitions**:` (lowercase) or `**Transitions** :` (space before colon) will get a state with no transitions, which makes it terminal. The state content will include the malformed transitions line as directive text.

The failure mode is silent: the state becomes terminal, the transitions line appears in the directive, and the template still parses without error. The template author won't know their transitions line was ignored until runtime.

This is advisory because the format is clearly documented in the package doc comment and tests, but the silent failure mode (terminal state + leaked transitions line in content) is a debugging trap.

**Suggestion**: Document the exact format requirement in the `parseSections` doc comment. Consider a heuristic warning: if a line contains `transitions` (case-insensitive) and `[` but doesn't match the exact pattern, that's likely a malformed transitions line.

---

## Summary

The code is clean, well-commented, and matches the design doc. The blocking finding is the undocumented initial-state-from-document-order convention, which creates a silent behavioral change risk when templates are edited. The advisory findings are all about reducing debugging time for the next developer: custom stdlib reimplementations, dead code in tests, silent ignoring of typos in headers, and exact format requirements for the transitions line.

| Severity | Count |
|----------|-------|
| Blocking | 1 |
| Advisory | 6 |
