# Lead: What does koto's template format look like?

## Findings

### Source and compiled architecture

Koto uses a two-format design:
- **Source format**: Markdown files with YAML frontmatter (what authors write)
- **Compiled format**: JSON (what the engine consumes)

Compilation is deterministic. The `koto template compile` command validates and converts source to compiled form.

### YAML frontmatter schema

Templates define their state machine in YAML frontmatter with these key sections:

- **States**: nodes with directives (instructions for the agent) and outgoing transitions
- **Transitions**: edges between states, optionally gated by `when` conditions
- **Gates**: three types -- `command` (run a shell command), `context-exists` (check file exists), `context-matches` (regex match on file content)
- **Variables**: named parameters with `{{UPPERCASE}}` interpolation syntax
- **Evidence**: structured data agents submit to trigger routing (accepts blocks define the schema)

### Template body

The Markdown body after frontmatter contains the directive content for each state, organized by headings that match state names.

### Validation

The compiler performs 13+ validation rules:
- Transition target verification (no dangling references)
- Regex pattern validation
- Variable reference checking
- Evidence routing rules (mutual exclusivity of `when` conditions)

### Notable features

- `when` conditions on transitions enable evidence-based routing (AND logic across conditions)
- `default_action` declarations enable automated state entry with polling support
- `integration` field for external system hooks
- Deprecated field-based gate syntax (field_not_empty, field_equals) replaced by accepts/when pattern

## Implications

The skill needs to teach agents the source format only -- the compiled JSON is an internal detail. The key authoring concepts are: states with directives, transitions with optional `when` conditions, gates as prerequisites, and variables for parameterization. The compiler provides a strong validation backstop, which the skill should invoke after drafting.

## Surprises

The two-format design means template authors never touch JSON. The source format is deliberately readable (Markdown + YAML), which makes it teachable. The 13+ compile-time validations are a strong safety net for AI-authored templates.

## Open Questions

- Should the skill teach the deprecated field-based gate syntax, or only the current accepts/when pattern?
- How should the skill handle the `integration` field and `default_action` -- are these advanced topics to defer?
- What's the minimal template that compiles successfully? This would be a good starting scaffold.

## Summary

Koto templates are Markdown files with YAML frontmatter that compile deterministically to JSON. The source format uses states, transitions with optional `when` conditions, three gate types, and `{{UPPERCASE}}` variable interpolation. Compile-time validation (13+ rules including evidence routing mutual exclusivity) provides a strong safety net that the skill should leverage as a validation step after drafting.
