---
status: Proposed
problem: |
  koto has no reference template demonstrating its template engine on a real,
  multi-phase workflow. shirabe's work-on and just-do-it skills are structurally
  the same single-session workflow — both implement one task and produce a PR —
  but are maintained separately. Merging them into a single koto-backed template
  proves the engine on a real workflow, eliminates duplication, and gives shirabe
  enforcement-backed phase structure without adding surface to koto.
---

# DESIGN: shirabe work-on koto template

## Status

Proposed

## Context and Problem Statement

shirabe provides workflow skills for AI coding agents. Two of its skills — work-on
(implement a GitHub issue) and just-do-it (implement a free-form task) — follow the
same structure: gather context, plan, implement, create a PR, verify CI, done. They
differ only in their starting point: work-on pulls context from a GitHub issue,
just-do-it starts from a user-provided description. Today these are maintained as
separate skills with duplicated phase logic.

koto enforces workflow phase structure through a state machine template. A koto template
for the merged work-on skill would: enforce that the agent completes each phase before
advancing, persist progress across sessions, and make the workflow auditable. The merged
skill is a natural first koto template because it's linear (one workflow, one session),
maps cleanly to koto's state machine model, and requires no external integrations — the
agent handles all external actions (git, GitHub, CI) within state directives.

## Decision Drivers

- koto's surface must stay minimal — no new subcommands or integration runner config
  required for this template to work
- The agent handles all external actions; koto enforces phase order via evidence-gated
  transitions
- The merged template must support both entry points: GitHub issue (work-on) and
  free-form description (just-do-it), via optional template variable
- Session resumability: koto's event log handles mid-session interruption without
  additional state management
- The skip pattern: staleness/introspection check must be able to route directly to
  analysis without forcing an introspection state

## Decisions Already Made

- No koto integrations needed for phase 1: agent-as-integration model (agent handles
  git, PR creation, CI monitoring via evidence submission)
- GitHub issue is an optional template variable, not a structural difference
- CI monitoring is agent-driven: agent waits for CI and submits evidence
  (`koto next --with-data decision=approved`) rather than using a koto command gate
- /implement's multi-issue orchestration is out of scope: deferred pending design of
  a multi-workflow orchestrator layer
