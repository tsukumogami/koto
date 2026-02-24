# Architect Review: #40 feat(plugin): add cross-platform agent files

**Reviewer**: architect
**Files reviewed**:
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/plugins/koto-skills/AGENTS.md`
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/plugins/koto-skills/.cursor/rules/koto.mdc`

**Reference**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/plugins/koto-skills/skills/hello-koto/SKILL.md`

## Summary

These two files translate the hello-koto SKILL.md content into platform-specific formats: AGENTS.md for Codex/Windsurf, and `.cursor/rules/koto.mdc` for older Cursor versions. This matches the design document's Phase 6 scope (DESIGN-koto-agent-integration.md, lines 446-451) and the issue #40 description in the dependency graph.

## Findings

### F1: File placement -- inside plugin, not at repo root (Advisory)

**Files**: `plugins/koto-skills/AGENTS.md`, `plugins/koto-skills/.cursor/rules/koto.mdc`

Both files are placed inside the plugin directory (`plugins/koto-skills/`). The design says "The koto repo includes these alternative formats as documentation or as ready-to-copy files under `plugins/`" (DESIGN-koto-agent-integration.md, line 364), and both files include placement instructions telling the user to copy them to the appropriate location (AGENTS.md line 3-5: "Copy this file to the root of your project repository"; koto.mdc line 9-10: "Copy this `.cursor/rules/koto.mdc` file into your project's `.cursor/rules/` directory").

This placement is consistent with the design. The files live in the plugin as ready-to-copy artifacts rather than being placed at the koto repo root (where they'd affect the koto project itself rather than user projects). The self-describing placement notes are a good convention.

No issue here. The placement is correct.

### F2: Content consistency between AGENTS.md, koto.mdc, and SKILL.md (Advisory)

**Files**: All three files

The three files contain near-identical instructional content, which is the stated goal. I verified the following sections match:

- Prerequisites: identical across all three
- Template Setup: identical, except AGENTS.md says "the `hello-koto.md` file distributed alongside this file" (line 34) while koto.mdc adds "under the koto-skills plugin directory" (lines 40-41). Both are accurate for their respective placement contexts.
- Execution Loop (steps 1-5): identical across AGENTS.md and koto.mdc. SKILL.md has the same commands but structures them slightly differently (e.g., step 4 header says "Transition to the terminal state" vs "Transition to the next state" in the other two). This is a minor wording difference, not a behavioral divergence.
- Error Handling: identical across AGENTS.md and koto.mdc. SKILL.md has more specific error text (e.g., "The greeting file doesn't exist yet" for gate failure vs the generic "The transition's precondition isn't met" in the other two).
- Resume: AGENTS.md and koto.mdc are identical. SKILL.md adds "The Stop hook detects active workflows and reminds the agent to resume" (line 119), which is appropriate since SKILL.md is loaded by Claude Code where the Stop hook is active.

The divergences are small and intentional (SKILL.md is richer because it operates in the Claude Code plugin context). The cross-platform files appropriately omit Claude Code-specific features (Stop hook mention) and use more generic phrasing. **Advisory** -- if these files start diverging significantly in future skills, consider a generation mechanism, but for a single skill the manual approach is fine.

### F3: koto.mdc frontmatter uses correct Cursor format (No issue)

**File**: `plugins/koto-skills/.cursor/rules/koto.mdc`

The `.mdc` frontmatter (lines 1-5) includes `description`, `globs` (empty, meaning not file-scoped), and `alwaysApply: false`. This matches the older Cursor rules format where `alwaysApply: false` means the rule is available but not injected into every conversation. The `description` field acts as the trigger condition. Correct for a workflow-specific rule.

### F4: No CI validation for new files (Advisory)

**Files**: `plugins/koto-skills/AGENTS.md`, `plugins/koto-skills/.cursor/rules/koto.mdc`

The `validate-plugins.yml` workflow validates template compilation, hook behavior, and schema correctness, but has no checks for AGENTS.md or `.cursor/rules/*.mdc` files. These are plain markdown/text files, so there's no structural validation to run (unlike templates which can be compiled). However, if the koto CLI command interface changes, all three files (SKILL.md, AGENTS.md, koto.mdc) need coordinated updates. The design acknowledges this: "CLI-changing PRs must update the affected skills in the same commit" (DESIGN-koto-agent-integration.md, line 174). Code review is the enforcement mechanism, which is reasonable.

No structural issue. The existing convention (manual coordination via PR review) applies to these new files too.

### F5: No eval coverage for cross-platform files (Advisory)

**Files**: `plugins/koto-skills/AGENTS.md`, `plugins/koto-skills/.cursor/rules/koto.mdc`

The eval harness (`eval.sh`) tests SKILL.md content against expected koto command patterns. The cross-platform files contain the same commands but aren't eval-tested. Since these files target different platforms (Codex, Windsurf, older Cursor) where the eval harness can't run, this is expected. The content fidelity between SKILL.md and these files is the reviewable surface -- if the SKILL.md eval passes, the cross-platform files should be correct assuming the content was kept in sync.

No structural issue, but worth noting: if these files start diverging from SKILL.md, there's no automated catch. The manual test checklist (`docs/testing/MANUAL-TEST-agent-flow.md`) doesn't currently reference cross-platform files either.

### F6: No parallel pattern introduction (No issue)

These files don't introduce a new mechanism or abstraction. They're documentation artifacts in platform-specific formats, which is exactly what the design called for in Phase 6. They don't bypass the plugin distribution system, duplicate the SKILL.md registration in plugin.json, or create an alternative dispatch path. The files are inert from koto's perspective -- they're instructions for other agent platforms to consume.

## Verdict

**0 blocking, 2 advisory**

The change fits the existing architecture cleanly. Both files are correctly placed inside the plugin directory as ready-to-copy artifacts. The content faithfully translates the hello-koto SKILL.md for Codex/Windsurf (AGENTS.md) and older Cursor (.mdc). No new patterns, no contract violations, no dependency issues.

The advisory notes are about long-term maintenance: keeping three files in sync manually is fine for one skill, but will need tooling or process attention if the plugin grows to many skills. The design already acknowledges this path ("a `koto generate` convenience command could be added in a future release").
