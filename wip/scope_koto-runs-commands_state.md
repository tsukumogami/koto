---
topic: koto-runs-commands
chain_started: 2026-08-21T02:31:24Z
last_updated: 2026-08-21T03:44:47Z
phase_pointer: phase-3
exit: full-run
exit_artifacts: [docs/plans/PLAN-koto-runs-commands.md]
planned_chain: [brief, prd, design, plan]
chain_skipped: []
chain_ran: [brief, prd, design, plan]
child_snapshots:
  brief:
    path: docs/briefs/BRIEF-koto-runs-commands.md
    status: Accepted
    validator: clean
  prd:
    path: docs/prds/PRD-koto-runs-commands.md
    status: Accepted
    validator: clean
  design:
    path: docs/designs/current/DESIGN-koto-runs-commands.md
    status: Accepted
    validator: clean
  design:
    path: docs/designs/current/DESIGN-koto-runs-commands.md
    status: Planned
    validator: clean
  plan:
    path: docs/plans/PLAN-koto-runs-commands.md
    status: Active
    validator: clean
    execution_mode: single-pr
    issue_count: 15
consolidation_judgments:
  brief-to-prd: keep  # all six prior koto BRIEFs coexist with their PRDs
  prd-to-design: keep  # koto keeps PRDs alongside their DESIGNs throughout
  design-to-plan: keep  # the PLAN cites the DESIGN as upstream; the DESIGN is the decision authority the plan defers to
framing_shift: yes
r6_predicates:
  p1_architectural_alternatives: fires
  p2_new_components: does-not-fire
  p3_complex_classification: fires
design_roster: sized-up
visibility: Public
consumed_handoff: wip/scope_koto-runs-commands_handoff.md
---

# /scope state: koto-runs-commands

Single-repo run (no coordination headers in koto's CLAUDE.md).
No `--upstream`; no ROADMAP covers this topic.
Entered via Slot 7, the `/explore` handoff feeder-doc clause.
