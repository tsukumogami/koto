# Architect Review: Issue #7 - Template Parsing and Interpolation

## Files Reviewed

- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go` (new, 395 lines)
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go` (new, 633 lines)

## Architecture Alignment

### Dependency Direction: Correct

The design document (DESIGN-koto-engine.md, Decision 3) specifies:

> Import direction: `template` imports `engine` (to return `*engine.Machine`). `controller` imports `engine` and `template`. `discover` imports only `engine`. CLI imports all four.

The template package imports only `engine` (plus stdlib). This matches the specified dependency direction exactly. No circular imports, no upward dependencies.

### Type Contract: Matches Design Spec

The `Template` struct at `template.go:35-44` matches the design's Template Types section (DESIGN-koto-engine.md, lines 484-493) field-for-field:

| Design field | Implementation | Match |
|---|---|---|
| `Name string` | `Name string` | Yes |
| `Version string` | `Version string` | Yes |
| `Description string` | `Description string` | Yes |
| `Machine *engine.Machine` | `Machine *engine.Machine` | Yes |
| `Sections map[string]string` | `Sections map[string]string` | Yes |
| `Variables map[string]string` | `Variables map[string]string` | Yes |
| `Hash string` | `Hash string` | Yes |
| `Path string` | `Path string` | Yes |

### Function Signatures: Match Design Spec

- `Parse(path string) (*Template, error)` -- matches design exactly.
- `Interpolate(text string, ctx map[string]string) string` -- matches design exactly.

### Machine Construction: Correct

`Parse` constructs an `*engine.Machine` from parsed template content (lines 104-125). The first `## ` heading becomes `InitialState`, states without `**Transitions**` lines are marked `Terminal: true`, and all transition targets are validated against defined states. This is the correct integration point -- template package owns construction, engine package owns the type.

### Hash Format: Consistent

Uses `"sha256:" + hex` prefix format (`template.go:66`), matching the state file format shown in DESIGN-koto-engine.md line 538 (`"sha256:e3b0c44298fc1c149afbf4c8996fb924..."`).

## Findings

### Finding 1: Controller-Template Integration Gap

**Severity: Advisory**

The design document specifies (lines 457-460):

```go
func New(eng *engine.Engine, tmpl *template.Template) (*Controller, error)
```

But the current controller implementation (`controller.go:32`) accepts `templateHash string` instead of `*template.Template`:

```go
func New(eng *engine.Engine, templateHash string) (*Controller, error)
```

The controller's `Next()` method currently returns a stub directive (`"Execute the " + current + " phase of the workflow."`) rather than reading from `Template.Sections` and calling `Interpolate`. This means the template package produces `Sections` and `Variables` that no consumer reads yet.

This is **not blocking** because:
1. The controller was deliberately built with a simplified signature in Issue #4 (before template parsing existed), with the comment "Pass an empty string to skip hash verification (useful when the template package is not yet available)."
2. Issue #9 ("feat(cli): add remaining CLI subcommands") explicitly covers wiring template parsing into `init`/`next`, which is the natural point to update the controller signature to accept `*template.Template`.
3. The template package's `Sections` and `Variables` fields will have consumers once the controller is updated -- no orphaned data.

However, this should be tracked: when Issue #9 lands, the controller's `New` signature should change to accept `*template.Template` (matching the design), and `Next()` should call `template.Interpolate(section, vars)` instead of returning stubs.

### Finding 2: No YAML Parser -- Manual Parsing of YAML-like Header

**Severity: Advisory**

The `parseHeader` function at `template.go:231` implements a manual key-value parser for what the design calls "YAML front-matter." The implementation handles a flat `key: value` structure and one level of nesting for `variables:`, but does not handle:
- Quoted strings with colons inside (e.g., `description: "Step 1: do X"` would parse incorrectly, splitting on the first colon)
- Multi-line YAML values (block scalars with `|` or `>`)
- YAML comments (lines starting with `#`)

This is **not blocking** because:
1. The package doc explicitly states "This is a simple manual parser for the flat key-value structure used in koto templates; it does not handle arbitrary YAML."
2. There are no external dependencies in the go.mod, which is a design goal ("custom implementation ~200 lines for the core... avoids external dependencies").
3. The limitation is self-contained -- if a YAML parser is adopted later, only `parseHeader` changes; no other package depends on the parsing internals.

That said, the colon-in-value edge case (`description: "Step 1: do thing"`) will produce `"Step 1` as the description since `SplitN(line, ":", 2)` splits on the first colon but `unquote` only strips surrounding quotes. The `SplitN` with N=2 actually handles this correctly -- it splits into key=`description` and val=`"Step 1: do thing"`, then `unquote` strips the quotes. On re-reading, this is fine. No issue.

### Finding 3: `contains` Helper Name Collision with engine Package

**Severity: Advisory**

Both `pkg/engine/engine.go:387` and `pkg/template/template_test.go:622` define unexported `contains` functions. In Go, unexported functions are package-scoped, so there's no actual collision. However, the test file defines a custom `contains` and `containsSubstring` pair (lines 622-633) that reimplements `strings.Contains` from the standard library:

```go
func contains(s, substr string) bool {
    return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsSubstring(s, substr))
}
```

This could simply use `strings.Contains`. This is test code with no callers outside the file, so it doesn't compound.

## Summary

The template package fits the architecture cleanly. The dependency direction is correct (template imports engine, not the reverse). The type contract matches the design specification exactly. The `Parse` function produces `*engine.Machine` instances as specified, making the template package a proper factory for the engine's core type. `Interpolate` is a standalone pure function with the correct semantics (single-pass, unresolved placeholders preserved).

The main architectural concern -- the gap between the controller's current simplified signature and the design's specified signature -- is a known intermediate state that Issue #9 is designed to resolve. No structural violations that would compound.
