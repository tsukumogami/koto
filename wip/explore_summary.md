# Exploration Summary: koto Agent Integration

## Problem (Phase 1)

koto has a working state machine engine and is installable (v0.1.0), but no AI agent will use it because nothing connects the binary on PATH to the agent's execution context. The missing piece isn't template distribution -- it's that the agent skill is the natural integration unit that bundles template + agent instructions.

## Decision Drivers (Phase 1)

- The agent skill is the natural integration unit -- it bundles template + agent instructions
- koto already compiles and caches templates on first use
- koto template compile already exists for the author validation flow
- Must work across agent platforms (Claude Code, Cursor, Codex, generic shell)
- Templates need stable filesystem paths (engine stores absolute paths in state files)
- koto is a CLI tool, not a service -- no background process or MCP server

## Research Findings (Phase 2)

- koto's current CLI requires `--template /absolute/path` for init; compilation and caching already work
- The engine stores absolute template paths in state files and verifies SHA-256 hashes on every operation
- Agent platforms have their own distribution mechanisms: skills (Claude Code), rules (Cursor), AGENTS.md (generic)
- koto doesn't need to build its own distribution -- it needs to fit into existing platform conventions
- The Stop hook pattern (detecting active state files) addresses the most common failure: agents quitting mid-workflow

## Options (Phase 3)

1. **Template distribution**: Skill-as-distribution-unit (chosen) vs. embedded built-in templates with search path vs. download-on-demand registry vs. scaffold-only
2. **koto generate output**: Platform-specific skill scaffolds (chosen) vs. generate-only-skill-with-path-reference vs. no generator (manual authoring)

## Decision (Phase 5)

**Problem:**
koto v0.1.0 has a working engine and is installable, but no agent will use it because nothing connects the binary to the agent's context. The agent skill is the natural integration unit that bundles the template with agent instructions.

**Decision:**
Focus on two flows. The agent-driven flow uses skills that contain the template alongside agent instructions. koto generate scaffolds these from a template. The author-driven flow uses koto template compile (already exists). Add a Stop hook to prevent mid-workflow abandonment.

**Rationale:**
The skill is the right distribution unit because agent platforms already have conventions for project-specific instructions. koto doesn't need search paths, go:embed, or extraction -- it needs to fit into the patterns that already exist. The compile-and-cache infrastructure from v0.1.0 handles the performance side.

## Current Status
**Phase:** 8 - Final Review
**Last Updated:** 2026-02-23
