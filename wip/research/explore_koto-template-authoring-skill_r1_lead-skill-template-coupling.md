# Lead: How do skills and koto templates couple at the file-system level?

## Findings

### Directory structure

Skills bundle templates as sibling files within the skill directory:

```
skills/
  work-on/
    SKILL.md
    koto-templates/
      work-on.md
    references/
      ...
```

The SKILL.md references templates via `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md` at runtime.

### Template discovery

Koto doesn't auto-discover templates. Agents must pass absolute template paths to `koto init`. The SKILL.md instructs agents on how to find and use the bundled template.

### Template stability

For plugin-distributed skills, the SKILL.md instructs agents to copy the bundled template to a project-stable path (like `.koto/templates/<name>.md`) before initialization. This makes the template immutable for the workflow's lifetime.

Koto stores and verifies SHA-256 hashes of the template source to prevent path drift during workflow execution.

### Existing examples

The shirabe marketplace has skills that bundle koto templates (e.g., `work-on`). These follow the `koto-templates/` subdirectory convention.

## Implications

The skill we're building should produce output that follows this exact convention:
- A `SKILL.md` that references `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md`
- A `koto-templates/` subdirectory containing the template file
- Instructions in the SKILL.md for copying the template to a stable path before `koto init`

The `${CLAUDE_SKILL_DIR}` variable is key -- it lets the skill reference its own directory regardless of where it's installed.

## Surprises

The SHA-256 hash verification is a strong integrity guarantee. Template authors don't need to think about it, but the skill should be aware that templates are hash-locked once a workflow starts.

## Open Questions

- Is `koto-templates/` the canonical directory name, or do some skills use other names?
- Should the authored skill always copy templates to `.koto/templates/`, or is direct reference from `${CLAUDE_SKILL_DIR}` acceptable?
- How does the authored skill handle template versioning when the plugin updates?

## Summary

Skills bundle templates as sibling files under `koto-templates/`, referenced via `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md`. Koto doesn't auto-discover templates -- agents pass absolute paths to `koto init`. Templates are SHA-256 hash-locked once a workflow starts, and plugin-distributed skills copy templates to project-stable paths for immutability.
