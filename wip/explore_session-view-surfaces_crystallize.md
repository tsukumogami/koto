# Crystallize Decision: session-view-surfaces

## Chosen Type

/scope (a chain, entering at the tactical chain)

## Candidacy

- /execute: not a candidate — no PLAN exists under docs/plans/ covering this
  topic (checked; the directory holds no PLAN-session-view*).
- Competitive analysis: not a candidate (repo visibility: Public).

## Rationale

The exploration converged on one bounded feature — a human-readable view of a
session riding the two existing surfaces — and left exactly the kind of open
questions a chain settles: requirements parameters (excerpt bound, binary
criterion, what "remote" means on screen), architectural choices (how the
dashboard gains `ContextStore` access, eager vs on-demand content loads, the
`--once` detail mode's shape, the workflow-view contract bump), and decisions
already made during exploration that need a durable home (the surface split,
the no-new-columns rule, the metadata-first read strategy). Someone will build
this; the work is not one decision, not a feasibility verdict, and not a
landscape.

## Stage 1 Evidence

### Signals Present (A Chain)

- Converged on something someone will build: the dashboard detail view plus a
  compact workflow-view summary.
- Requirements and architecture questions remain open: excerpt bound, binary
  rule, pull policy, plumbing for `ContextStore`.
- Decisions made during exploration need a durable home: the surface split and
  presentation rules (wip decisions file).
- A scope boundary emerged: content never on the workflow view; hygiene out of
  scope; `--once` columns untouched.
- The core question is "what do we build, and how?".

### Anti-Signals Checked

- Nothing left to build: not present.
- Output is one choice between named options: not present (many coupled
  choices).
- Output is an unactioned feasibility verdict: not present.
- Findings center on external products: not present.

### Ranking

- A Chain: 5, no anti-signals
- Spike Report: 2, demoted (anti-signal: the question is "what should we
  build?", not "can we?")
- Decision Record: 1, demoted (anti-signal: multiple interrelated decisions
  with work attached)
- Rejection Record: 0, demoted (no rejection evidence; conclusion is proceed)

## Stage 2 Evidence

Stage 2 ran because stage 1 returned a chain.

### Signals Present (/scope)

- A single coherent feature emerged.
- What to build is clear; how is not fully (plumbing, loading model, contract
  bump shape).
- Technical decisions between approaches remain (detail tab vs DetailData
  extension; flag shape on `--once`).
- Architectural decisions made during exploration should be on record.
- Acceptance criteria are not yet written down in the repo.

### Anti-Signals Checked

- Multiple independent features needing ordering: not present (one feature).
- One person can act without a written contract: not present (requirements and
  presentation rules need a record future contributors read).
- A qualifying PLAN already covers the work: not present.

### Ranking

- /scope: 5, no anti-signals
- File an Issue: demoted (anti-signals: architectural decisions were made
  during exploration; documentation needed; multiple PRs likely)
- /charter: demoted (anti-signals: the project exists; one bounded feature)
- /execute: not a candidate

## Tiebreakers Applied

None needed; /scope led by a clear margin with no anti-signals.

## Alternatives Considered

- **Spike Report**: ranked lower because feasibility was never the open
  question — both surfaces demonstrably exist and the data layer is mostly
  present; the open questions are what to build on them.
- **File an Issue**: ranked lower because the exploration settled a surface
  split and presentation rules that need a durable record, and the work spans
  data-layer, dashboard and workflow-view changes.
- **/charter**: ranked lower because this is one feature inside an existing
  project, with no multi-feature sequencing question.
