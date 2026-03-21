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

## Approaches Investigated (Phase 1)

- fine-grained: Maximum enforcement (9 states, evidence at every boundary) but 500+ line
  template with ceremony-heavy evidence fields that don't affect routing. Jury routing
  and iterative implementation don't map cleanly to state transitions.
- coarse-grained: Simple 3-4 checkpoint template but loses enforcement at the transitions
  that matter (staleness detection invisible to koto). Audit trail fragments.
- auto-advancing: ~8 states, auto-advances through execution phases, evidence gates only
  at 3 decision points (staleness, plan approval, CI). Right level of abstraction.

## Phase 2 Status: Recommendation pending user confirmation

**Recommendation:** auto-advancing with minimal evidence gates.

Rationale: evidence gates belong where routing branches, not where the agent is just
confirming it did what the directive said. Fine-grained over-specifies. Coarse-grained
under-enforces at the staleness branch. Auto-advancing captures the enforcement value
with minimal overhead and a ~150 line template.

**Pending:** User confirmation of approach. Once confirmed, proceed to Phase 3 (deep
investigation of the chosen approach) covering: full state list, directive text, evidence
schema design, shirabe invocation mechanics, session resume behavior.

## Current Status
**Phase:** 2 - Present Approaches (awaiting user confirmation)
**Last Updated:** 2026-03-21
