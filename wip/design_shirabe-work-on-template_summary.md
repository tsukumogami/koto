# Design Summary: shirabe-work-on-template

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** shirabe's work-on and just-do-it skills are structurally identical
single-session workflows maintained separately. Merging them into a koto-backed
template proves koto's engine on a real workflow and enforces phase structure
without adding surface to koto.
**Constraints:**
- koto surface stays minimal — no new subcommands, no integration runner config required
- Agent handles all external actions (git, PR, CI); koto only enforces phase order
- Must support two entry points: GitHub issue (work-on) and free-form description (just-do-it)
- Skip pattern needed: staleness check must route directly to analysis, bypassing introspection

## Key Findings from Exploration

- Merge cost is low: phases 0-2 differ (GitHub issue vs. free-form input), phases 4-6 are identical
- koto template: ~7-8 states, mostly auto-advancing, one branch at staleness/introspection check
- No integrations needed: agent-as-integration model works for all external actions
- Open design questions: full state list + evidence schema, skip pattern mechanics,
  shirabe invocation (SessionStart hook, koto init, directive loop), session resume behavior

## Current Status
**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-03-21
