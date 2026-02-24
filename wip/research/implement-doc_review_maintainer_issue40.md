# Maintainer Review: Issue #40 (Cross-Platform Agent Files)

**Focus**: Can the next developer understand and modify these files with confidence?

**Files reviewed**:
- `plugins/koto-skills/AGENTS.md`
- `plugins/koto-skills/.cursor/rules/koto.mdc`

**Reference**: `plugins/koto-skills/skills/hello-koto/SKILL.md`

## Summary

Two cross-platform translations of the hello-koto SKILL.md. The content is well-structured and follows each platform's conventions. One blocking finding: the template location instructions break when the files are deployed to their intended destinations.

## Findings

### 1. Template reference breaks after deployment -- AGENTS.md and koto.mdc

**Blocking.**

`plugins/koto-skills/AGENTS.md:33-34`:
```
Then write the template file to `.koto/templates/hello-koto.md`. The template content
is the `hello-koto.md` file distributed alongside this file.
```

`plugins/koto-skills/.cursor/rules/koto.mdc:39-41`:
```
Then write the template file to `.koto/templates/hello-koto.md`. The template content
is the `hello-koto.md` file distributed alongside this file (under the koto-skills
plugin directory).
```

Both files include placement instructions telling the user to copy them somewhere else:
- AGENTS.md: "Copy this file to the root of your project repository" (line 3)
- koto.mdc: "Copy this `.cursor/rules/koto.mdc` file into your project's `.cursor/rules/` directory" (line 9)

After copying, "alongside this file" is wrong. The template (`hello-koto.md`) is at `plugins/koto-skills/skills/hello-koto/hello-koto.md`. It isn't alongside AGENTS.md at the repo root, and it isn't alongside koto.mdc in `.cursor/rules/`. The agent (or a human following the instructions) will look for a file that isn't there.

This is the same class of issue flagged in #35, where SKILL.md used `dirname "$0"` which doesn't work because SKILL.md content is injected as text, not executed from a filesystem path. The fix in SKILL.md was to say "the file alongside this SKILL.md" -- which works because SKILL.md and hello-koto.md are in the same directory and the agent resolves the path from the skill's directory context. But AGENTS.md and koto.mdc don't have that luxury: they're meant to be copied to a different location.

The next developer adapting these files for a new workflow template will follow the same pattern ("distributed alongside this file") and produce the same broken reference.

**Fix options**:
- Embed the template content inline (fenced code block) so the agent has it regardless of file location. The design doc (Solution Architecture, Template Locality) mentions this approach.
- Replace the vague "alongside this file" with an explicit path relative to the project root, like "found at `plugins/koto-skills/skills/hello-koto/hello-koto.md` if you installed the koto-skills plugin."
- Add a note that if the template isn't available, the agent should ask the user to provide it, or reference the koto repo URL.

### 2. Divergent twins: three files with near-identical content, no single source of truth

**Advisory.**

AGENTS.md (132 lines), koto.mdc (139 lines), and SKILL.md (119 lines) contain substantially the same execution loop, error handling, and resume sections. The differences are:

| Section | SKILL.md | AGENTS.md | koto.mdc |
|---------|----------|-----------|----------|
| Frontmatter | Agent Skills YAML (name, description) | None (blockquote placement) | Cursor YAML (description, globs, alwaysApply) |
| "What is koto?" | Not present (implicit from frontmatter description) | Present as dedicated section | Present as dedicated section |
| Heading style | "## Execution" | "## Execution Loop" | "## Execution Loop" |
| Step 5 completion | Includes "Output a message to the user..." | Omits that instruction | Omits that instruction |
| Resume: Stop hook | Mentions it | Omits it (correct: not available) | Omits it (correct: not available) |
| Template reference | "the file alongside this SKILL.md" | "distributed alongside this file" | "distributed alongside this file (under the koto-skills plugin directory)" |

The step 5 omission (completion message) could be intentional (Codex/Cursor agents may not need it) or accidental. There's no comment explaining why it differs. If intentional, a comment in the source files would help the next developer who updates one file and wonders whether to update the others.

The heading difference ("Execution" vs "Execution Loop") is cosmetic but adds to the "are these supposed to be identical?" uncertainty. A developer updating the execution steps needs to update all three files. Nothing signals this obligation.

Consider adding a comment at the top of AGENTS.md and koto.mdc noting: "Adapted from skills/hello-koto/SKILL.md. When updating the canonical skill, update these files to match." This makes SKILL.md the source of truth and reduces the chance of silent drift.

### 3. Empty `globs` field in koto.mdc

**Advisory.**

`plugins/koto-skills/.cursor/rules/koto.mdc:4`:
```yaml
globs:
```

In Cursor's `.mdc` format, the `globs` field controls when the rule is automatically attached to a conversation based on open file patterns. An empty `globs` with `alwaysApply: false` means the rule is never auto-attached and must be explicitly referenced. This may be the intended behavior (the agent picks it up via manual invocation), but the next developer might read the empty field as "applies to all files" rather than "applies to no files."

If the intent is "never auto-apply, only manual," add a comment:
```yaml
globs:  # No auto-attach; agent must reference this rule explicitly
alwaysApply: false
```

If the intent is "apply to all files in the project," the field should be omitted entirely or set to `**/*`.

## What's clean

The overall structure is sound. Each file follows its platform's conventions correctly (`.mdc` frontmatter with description/globs/alwaysApply for Cursor, blockquote placement instructions for AGENTS.md). The error handling and resume sections are complete and match the SKILL.md content. The removal of the Stop hook reference from the non-Claude Code files is correct -- those platforms don't have that hook mechanism.

## Carried findings

- `deepCopyState` godoc still stale (from #15, carried through every review since).

## Verdicts

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1 | Template "alongside this file" breaks after deployment | Blocking | Embed template inline or use explicit path |
| 2 | Three near-identical files, no documented source of truth | Advisory | Add "adapted from" comment, document differences |
| 3 | Empty `globs` in koto.mdc ambiguous | Advisory | Add clarifying comment |
