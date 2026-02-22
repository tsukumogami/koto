# Exploration Summary: koto Template Format

## Problem (Phase 1)
koto has a working template parser but the format is undocumented, limited, and makes assumptions that will break as templates grow more complex. Evidence gates have no syntax. The header parser silently ignores unknown keys. State section boundaries collide with normal markdown. There's no template search path, no validation contract, and no way to declare variable types or requirements. The format needs to be formalized before other designs (quick-task template, agent integration) can build on it.

## Decision Drivers (Phase 1)
- Zero external dependencies (current constraint, manual parsing)
- Backward compatibility with existing templates
- Evidence gates must be declarable per-transition
- Template search path needed for built-in and user templates
- YAML vs TOML header format is an open question
- Heading collision (`## state-name` in directive text) must be resolved
- Variables vs evidence interpolation namespace needs rules

## Research Findings (Phase 2)
- YAML frontmatter + Markdown body is the industry standard (Claude Code skills, Gemini CLI, GitHub Actions)
- Three evidence gate types: command (exit code), field check (declarative), prompt (LLM evaluation)
- Project-directory state storage with ephemeral lifecycle validated by Beads and TaskMaster
- Evidence from one state should feed into interpolation context for later states
- Typed variable inputs (type, description, required) improve usability over bare key-value
- No tool combines state machine enforcement with file-based simplicity -- koto's gap is real

## Current Status
**Phase:** 4 - Review complete, feedback incorporated
**Last Updated:** 2026-02-22
