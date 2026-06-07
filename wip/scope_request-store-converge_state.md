---
topic: request-store-converge
chain_started: 2026-06-07T16:16:41Z
last_updated: 2026-06-07T16:21:53Z
phase_pointer: phase-2
exit: UNSET
exit_artifacts: []
planned_chain:
  - brief
  - prd
  - design
  - plan
chain_skipped: []
visibility: Public
plan_execution_mode: UNSET
r6_predicates:
  p1: "fires — architectural alternatives open (slot-result payload schema; auto-promote terminal evidence vs explicit post; layer-on dispatch protocol vs stand-beside)"
  p2: "does-not-fire — extends existing koto engine/types/terminal-index; no new component"
  p3: "fires — engine-substrate change (new converge event family + converge-gate semantics) warrants a DESIGN"
worktree_rebases: []
worktree_divergences: []
chain_ran:
  - brief
child_snapshots:
  brief:
    status: Accepted
    content_hash: 1d5753e9f6f8f57f173366a9c5d546561d09aba8
    captured_at: 2026-06-07T16:21:53Z
---

# Scope state: request-store-converge

Tactical-chain scope run. Visibility: Public (koto). Execution mode: --auto.

## Chain proposal (Phase 1) — auto-proceeded

- /brief — fires (R4 EITHER-signal: no upstream BRIEF)
- /prd — fires (R5 Mandatory-with-auto-skip: no Accepted PRD)
- /design — fires (R7 shape-dependent: P1 fires, P2 does-not-fire, P3 fires)
- /plan — fires (ALWAYS)

## Framing (from brainstorm, seeds the children)

KT1 reframed: koto's fan-out/dispatch half already shipped in v0.10.0
(materialize_children, session start --needs-agent, koto next ->
unassigned_children, claim_and_dispatch, _terminal_index.jsonl). The gap
is the CONVERGE half: a child's completion carries only a terminal-state
NAME, not a closed RESULT payload — so a coordinator must read child logs
(ingestion) to learn outcomes. This feature completes the converge half:
workflow completion carries a typed closed result, surfaced to the parent
at a converge gate via koto next. No new "koto request" noun; reuse the
existing children/gate/terminal-index machinery. Uniform + recursive
(a parent completes the same way; ties to cleanup). Open design decisions:
(C1) auto-promote terminal evidence as the result vs explicit post;
(C2) result payload schema (free JSON vs typed {status,summary,payload});
(C3) where the result lives (child session, index stays lean + pointer);
(C4) converge wait = poll koto next at a GateBlocked converge gate.
Standalone koto value holds (validation-without-ingestion for a solo
coordinator); shirabe/niwa are enrichments, not prerequisites.
PUBLIC REPO: artifacts MUST NOT reference the private vision repo or its
issue numbers; frame standalone, reference only public koto paths.
