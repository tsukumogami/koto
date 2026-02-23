# Exploration Summary: koto Agent Integration

## Problem (Phase 1)

koto has a working state machine engine and is installable (v0.1.0), but no AI agent will use it because nothing connects the binary on PATH to the agent's execution context. There's no template distribution mechanism (koto init requires an explicit filesystem path), no integration file generation (koto generate doesn't exist), and no discovery protocol for agents to find active workflows or available templates. Without this, koto is dead weight.

## Decision Drivers (Phase 1)

- Must work across agent platforms (Claude Code, Cursor, Codex, generic shell)
- Templates need stable filesystem paths (engine stores absolute paths in state files)
- Generated integration files are committed to repos -- version drift is a concern
- The solution must handle both "start new workflow" and "resume active workflow"
- koto is a CLI tool, not a service -- no background process or MCP server
- Template distribution and agent integration are the same problem

## Research Findings (Phase 2)

- koto's current CLI requires `--template /absolute/path` for init; no name-based resolution exists
- The engine stores absolute template paths in state files and verifies SHA-256 hashes on every operation -- templates can't move after init
- No config file exists; only `KOTO_HOME` env var controls the home directory location
- `koto workflows` already scans `wip/` for state files but can't list available templates
- Claude Code integration uses skills (`.claude/skills/`), commands (`.claude/commands/`), and hooks (`.claude/hooks.json`)
- AGENTS.md is the generic cross-platform format for agent instructions
- The Stop hook pattern (detecting active state files) addresses the most common failure: agents quitting mid-workflow

## Options (Phase 3)

1. **Template Distribution**: Embedded built-in templates with search path (chosen) vs. download-on-demand registry vs. scaffold-only vs. explicit paths only
2. **Agent Integration**: Per-platform `koto generate` command (chosen) vs. auto-discovery via PATH vs. universal file vs. template-embedded instructions
3. **Workflow Discovery**: Extended `koto workflows` covering templates + active state files (chosen) vs. separate commands vs. manifest file

## Decision (Phase 5)

**Problem:**
koto v0.1.0 ships a working state machine engine and is installable, but no AI agent will use it because nothing connects the binary on PATH to the agent's execution context. There's no template distribution, no integration file generation, and no discovery protocol. Without this middle layer, koto is dead weight.

**Decision:**
Add three capabilities: (1) a built-in template registry so koto init can resolve templates by name instead of requiring filesystem paths, (2) a koto generate command that produces platform-specific agent integration files (Claude Code skills/hooks, AGENTS.md sections), and (3) an extended koto workflows command that gives agents a machine-readable view of available templates and active state files. Templates ship embedded in the binary via go:embed with a search path that checks project-local, user-level, and built-in locations.

**Rationale:**
The core constraint is that koto is a CLI tool, not a service. Agents can't discover it through a protocol -- they need static configuration files. Embedding templates eliminates the bootstrap problem while the search path lets projects customize. Generation over auto-discovery means integration files are visible and reviewable. The alternative -- expecting agents to probe PATH -- doesn't work because agents need workflow context, evidence key documentation, and response schemas, not just binary existence.

## Current Status
**Phase:** 5 - Decision
**Last Updated:** 2026-02-23
