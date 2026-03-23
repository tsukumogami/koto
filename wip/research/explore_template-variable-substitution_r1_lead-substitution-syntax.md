# Lead: What substitution syntax and edge-case behavior should koto use?

## Findings

### Existing usage in koto
The codebase already uses `{{KEY}}` syntax in template directives. The hello-koto
example and DESIGN-koto-template-format.md both use this convention. The compiled
template stores these patterns literally — `"Analyze the task: {{TASK}}"` is kept
as-is in the directive string (confirmed by test in src/template/compile.rs:363-415).

### Industry precedents
- **Mustache**: `{{KEY}}` syntax. Undefined variables render as empty strings silently.
- **Terraform**: `${var.name}` syntax. Requires `$${` to escape literals. Errors on
  missing required values.
- **GitHub Actions**: `${{ vars.NAME }}` syntax. Undefined context variables become
  empty/null.
- **Helm**: Go template syntax `{{ .Values.name }}`. Errors on undefined by default
  but has `default` function.

### Parent design position
The parent design and issue #67 explicitly state: undefined variable references must
produce an error, not silent empty-string substitution.

### Edge cases to handle
- `{{{KEY}}}` (triple braces): treat outer pair as literal `{` + `{{KEY}}` + literal `}`
- `{{KEY` (unclosed): pass through literally (not a valid reference)
- `{{ KEY }}` (whitespace): reject — require exact `{{KEY}}` with no internal spaces
- `{{unknown}}` (undefined): error at runtime
- Empty value `--var KEY=`: valid — substitutes empty string (user explicitly set it)

## Implications

The `{{KEY}}` syntax is already established in the codebase and matches the most
common template convention. The strict error-on-undefined policy aligns with koto's
philosophy of explicit state management. No syntax decision needed — just implement
what's already in use with strict undefined handling.

## Surprises

None significant. The syntax choice is effectively pre-made by existing template
content. The main question was edge-case handling, not syntax choice.

## Open Questions

1. Should `{{` be escapable (e.g., `\{{` renders literal `{{`)? Probably not needed
   for initial implementation — no known use case in current templates.

## Summary

The `{{KEY}}` syntax is already established in koto templates and should be kept.
Undefined references must error (per parent design), whitespace inside braces should
be rejected, and unclosed patterns pass through literally. No escape mechanism is
needed initially since no template requires literal `{{` in gate commands or directives.
