# Autonomous Mode Directive

**Written:** 2026-05-07  
**Branch:** docs/local-dashboard  
**Status:** In progress — refer back to this after every context compaction and after each phase completes.

## Standing Order

Work autonomously, all the way through to a complete implementation. Do NOT stop. Do NOT wait for user input except when a skill's safety gate requires it (W3/W4 gates in work-on). Optimize for completeness and correctness, not velocity.

## Full Sequence

1. **Complete `/explore`** — finish convergence (Round 2 already done), crystallize to PRD artifact type, produce handoff
2. **Run `/prd`** — write the full PRD for F3: Local Dashboard, using `/decision` for any choices that arise
3. **Run `/design`** — produce a complete design doc, using `/decision` for architectural decisions, thoroughly reviewed against the VISION doc
4. **Run `/plan --single-pr`** — decompose the design into implementable issues
5. **Run `/work-on` the PLAN** — implement all issues to completion, passing CI

All work stays on this branch (`docs/local-dashboard`). Do everything in a single PR.

## Product Context (Critical — informs PRD)

The user's explicit framing: "What we are doing here is exactly the kind of work that I want the dashboard to be useful for. A very, very long running process. Today, koto is only used in the `/work-on` workflow, but in the future, this entire entire sequence will be managed by koto. I want you to design and implement a solution that will enable us to monitor the execution of hours-long workflows."

**This means:**
- The target use case is the FULL orchestration pipeline: explore → prd → design → plan → work-on
- Each of these phases is a koto-managed workflow (today only work-on, eventually all of them)
- Sessions can last hours; the dashboard must be usable for monitoring these long multi-phase pipelines
- Batch coordination (plan creating many issues, work-on implementing each in parallel) is the key pattern to support
- The session hierarchy view must handle: root orchestrator → phase workflows → parallel work-on instances

## Resumption Protocol

After any context compaction, read this file first. Then check:
1. What phase am I in? (explore/prd/design/plan/work-on)
2. What artifacts exist? (wip/, docs/prds/, docs/designs/, docs/plans/)
3. What's the current branch? (`git branch --show-current`)
4. Resume at the correct phase.

## Quality Bar

- PRD: complete requirements, sufficient to implement without guesswork
- Design: reviewed against VISION, all major decisions made via `/decision`, security section included
- Plan: single-pr mode, all issues have acceptance criteria
- Implementation: all CI checks passing, no wip/ artifacts left in the branch

## Context: F3 in the Observability Roadmap

F2 (session-feed data contract) just merged as koto#153. F3 is the "first tangible observability experience" — the baseline that makes F5 (S3-backed dashboard) and F6 (Hosted Relay) legible. F3 must be implementable immediately after this PR merges.

Upstream: tsukumogami/vision#366
