# Decision: Template source format for directive/details split

**Decision ID**: design_koto-next-output-contract_decision_1
**Status**: COMPLETE
**Chosen**: Markdown separator (`<!-- details -->`)

## Alternatives evaluated

### A. Markdown separator (`<!-- details -->`)

Template authors add an HTML comment `<!-- details -->` within a state's `## heading` section. Content before the marker becomes `directive`; content after becomes `details`. States without the marker behave identically to today (full content is `directive`, no `details`).

**Example:**

```markdown
## analyze

Read the issue body and identify acceptance criteria.

<!-- details -->

### Steps

1. Run `gh issue view {{ISSUE}} --json body` to fetch the full issue.
2. Extract each acceptance criterion into a checklist.
3. If the issue references a design doc, read it and cross-reference.
...
```

**Compiler change**: Localized to `extract_directives` in `src/template/compile.rs`. After collecting lines for a state section, split on the first `<!-- details -->` line. Lines before become `directive`, lines after become `details`. No changes to YAML frontmatter parsing.

**Strengths:**
- Single-file, self-contained templates (no external references)
- No new YAML fields -- the split is expressed in the same markdown body where directives already live
- `<!-- details -->` is an HTML comment, invisible in GitHub rendered previews, and unambiguous (unlike `---` which is overloaded in markdown)
- Compiler change is ~10 lines in one function
- Consistent with existing pattern: directive content comes from the markdown body, not YAML

**Weaknesses:**
- HTML comments are less discoverable than YAML fields for authors unfamiliar with the convention
- Authors must remember the exact marker string

### B. YAML summary field

Add an optional `summary` field to the YAML state declaration. When present, `summary` becomes `directive` in compiled JSON and the markdown body becomes `details`. When absent, current behavior applies.

**Strengths:**
- Explicit opt-in via a named YAML field
- Clear separation between summary (structured) and body (prose)

**Weaknesses:**
- Breaks the existing pattern where directives come from markdown body, not YAML -- today there is no `directive` field in YAML at all
- Authors must maintain content in two places (YAML for summary, markdown for details)
- YAML multiline strings are awkward for content that includes markdown formatting
- Requires changes to YAML frontmatter parsing (`SourceState` struct), not just `extract_directives`
- Variable substitution in YAML fields needs separate handling from body substitution

### C. External file reference (`details_file`)

Add an optional `details_file` field to the YAML state declaration pointing to a relative markdown file whose content becomes `details` at compile time.

**Strengths:**
- Keeps main template concise when details are very long
- Natural for instructions that span dozens of lines

**Weaknesses:**
- Template authors manage multiple files per template -- discovery and maintenance burden
- Requires file resolution logic in the compiler (path resolution, error handling for missing files)
- Variable substitution must apply to referenced file content
- Breaks the "single file = single template" model that all current templates follow
- Most invasive compiler change of the three options

## Decision rationale

Option A (markdown separator) wins on three axes that matter most for this project:

1. **Consistency with existing conventions.** Directive content already comes from the markdown body. The separator extends this pattern rather than introducing a new source location (YAML or external files).

2. **Minimal compiler change.** The modification is confined to `extract_directives` -- split on the marker line after collecting a state's content lines. No YAML schema changes, no file resolution, no new parsing logic.

3. **Template self-containment.** Templates remain single-file artifacts. The koto-author skill teaches one format, and template-format.md documents one convention.

The `<!-- details -->` marker was chosen over `---` because horizontal rules (`---`) are already meaningful in markdown (thematic breaks) and in YAML (document delimiters). HTML comments are unambiguous and invisible in rendered previews, so they don't affect how templates read on GitHub.

## Assumptions

1. Template authors will read the koto-author skill or template-format.md reference before authoring templates with the details split. The marker is not self-documenting.
2. The `<!-- details -->` marker will appear at most once per state section. If multiple markers appear, only the first is significant (content after the second marker is still part of `details`).
3. No existing template content uses `<!-- details -->` for other purposes. A search of current templates confirms this.
4. The compiled JSON representation (`TemplateState` with `directive` + `details` string fields) is identical regardless of source format, so this decision does not constrain the response layer or visit-tracking implementation.

## Rejected alternatives

| Alternative | Reason for rejection |
|---|---|
| YAML summary field | Breaks the existing pattern where directives come from markdown body; forces content into two locations (YAML + body); YAML multiline strings are awkward for markdown content |
| External file reference | Introduces file resolution complexity in the compiler; breaks single-file template model; highest maintenance burden for template authors |
