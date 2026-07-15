---
topic: native-workflows-render
chain_started: 2026-07-15T22:26:37Z
last_updated: 2026-07-15T22:40:00Z
phase_pointer: phase-3
exit: full-run
exit_artifacts:
  - path: docs/briefs/BRIEF-native-workflows-render.md
    status: Accepted
  - path: docs/prds/PRD-native-workflows-render.md
    status: Accepted
  - path: docs/designs/DESIGN-native-workflows-render.md
    status: Planned
  - path: docs/plans/PLAN-native-workflows-render.md
    status: Active
planned_chain:
  - brief
  - prd
  - design
  - plan
chain_skipped: []
visibility: Public
plan_execution_mode: single-pr
execution_mode: auto
---

# Scope state — native-workflows-render (full-run complete)

Feature 1 (walking skeleton) of ROADMAP-koto-agent-surface-legibility. Scope
chain ran brief -> prd -> design -> plan (all fired, cold-start, no on-disk
artifacts). Forks resolved inline at highest recommendation (see DESIGN
Considered Options). Terminal artifact: docs/plans/PLAN-native-workflows-render.md
(single-pr, Active). Validator: full chain passes `shirabe validate
--lifecycle-chain --mode=draft`. Handoff to /execute.
