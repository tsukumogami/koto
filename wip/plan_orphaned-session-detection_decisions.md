# Plan Decisions: orphaned-session-detection

## Round 1

- **Decomposition strategy: horizontal, not walking skeleton.** The design
  refactors existing code and extends an already-working check layer by
  layer; no new end-to-end user flow needs early stub-and-refine
  validation. `confirmed` -- clear fit, no ambiguity.
- **Value confirmation (step 3.5a): pass by construction.** Single-pr mode
  means one unit (the whole plan); the "standalone increment" question
  only applies when a plan is split across multiple PRs. `confirmed`.
- **Execution mode: single-pr.** Set directly by explicit user instruction
  ("make it a single-pr plan"), not derived from the surfaced rule's
  default heuristic. `confirmed` -- no ambiguity, no override needed.
- **Docs-coverage emit: folded into Issue 4's acceptance criteria**, not a
  dedicated docs issue. The user-visible surface (two new JSON fields) is
  small enough to document alongside the issue that introduces it, rather
  than warranting its own issue. `confirmed`.
