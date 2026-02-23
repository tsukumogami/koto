# Exploration Summary: koto Agent Integration

## Problem (Phase 1)

koto has a working state machine engine and is installable (v0.1.0), but no AI agent will use it because nothing connects the binary on PATH to the agent's execution context. The missing piece isn't template distribution -- it's that the agent skill is the natural integration unit that bundles template + agent instructions. The real question is how these skills get distributed and installed.

## Decision Drivers (Phase 1)

- Agent platforms already have distribution mechanisms (plugins, marketplaces, filesystem conventions)
- The Agent Skills open standard (agentskills.io) provides cross-platform reach
- koto already compiles and caches templates on first use -- no engine changes needed
- Templates need stable filesystem paths (engine stores absolute paths in state files)
- Must work for both reference templates (published by koto) and custom templates (project-specific)

## Research Findings (Phase 2)

- Agent Skills standard (Dec 2025) makes SKILL.md work across Claude Code, Codex, Cursor, Windsurf, Gemini CLI
- Claude Code has a full distribution stack: project-scoped skills, plugins, marketplaces, team auto-install
- Plugin system bundles skills + hooks + commands into one installable unit
- Other platforms: Cursor has marketplace + rules, Codex uses AGENTS.md, Windsurf uses rules + AGENTS.md
- koto's compile-and-cache already handles template processing; cache keys on SHA-256 of source bytes
- Every `koto next` re-reads template from disk and verifies hash -- no cache involved after init

## Options (Phase 3)

1. **Plugin for reference, project-scoped for custom** (chosen): Publish reference skills as a Claude Code plugin. Custom skills committed to project repos. No koto code changes.
2. **Project-scoped only**: Publish reference skill directories in koto's repo for manual copying. Highest friction.
3. **koto generate as primary**: Build distribution into koto CLI. Rejected -- duplicates ecosystem capabilities, couples skill updates to koto releases.

## Decision (Phase 5)

Plugin model for reference templates, project-scoped for custom. No koto code changes.

## Current Status
**Phase:** 6 - Architecture (complete, proceeding to reviews)
**Last Updated:** 2026-02-23
