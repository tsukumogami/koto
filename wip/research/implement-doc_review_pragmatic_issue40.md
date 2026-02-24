# Pragmatic Review: Issue #40 -- Cross-Platform Agent Files

## Files Reviewed

- `plugins/koto-skills/AGENTS.md`
- `plugins/koto-skills/.cursor/rules/koto.mdc`
- Reference: `plugins/koto-skills/skills/hello-koto/SKILL.md`

## Summary

Both files are direct translations of the SKILL.md content into platform-specific formats. The content is correct and matches the reference. The scope is tight -- two files, no extras.

No blocking findings. One advisory finding.

## Findings

### 1. AGENTS.md and .mdc are nearly identical -- Advisory

`plugins/koto-skills/AGENTS.md` and `plugins/koto-skills/.cursor/rules/koto.mdc` share ~95% of their content verbatim (lines 8-138 of AGENTS.md vs lines 7-138 of koto.mdc). The only differences are:

- The placement blockquote (line 3-5 vs line 9-11)
- The `.mdc` YAML frontmatter (lines 1-5)
- One phrase in Template Setup: "distributed alongside this file" vs "distributed alongside this file (under the koto-skills plugin directory)"

This is expected duplication for platform-specific files and isn't actionable -- there's no shared-include mechanism across these formats. Noting it for awareness: if the koto CLI interface changes, both files need updating alongside the SKILL.md.

**No action needed.** The design doc explicitly calls this out as a Phase 6 deliverable and the CI validation workflow (#36) should catch template compilation drift. Content drift between these files and the SKILL.md is a maintenance concern but not a pragmatic blocker.

## Not Flagged

- **File placement under `plugins/koto-skills/`**: Matches the design doc's guidance ("include under `plugins/`"). The placement blockquotes tell users to copy them to the project root -- correct for both platforms.
- **`.mdc` frontmatter fields**: `alwaysApply: false` with no `globs` is the right configuration for an on-demand rule. Not speculative.
- **"What is koto?" section**: Added to both files but absent from SKILL.md. This is appropriate -- SKILL.md is discovered via the plugin system where context exists; AGENTS.md and `.mdc` are standalone files where a reader needs orientation.

## Verdict

Clean. Ship it.
