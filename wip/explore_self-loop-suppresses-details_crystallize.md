# Crystallize Decision: self-loop-suppresses-details

## Chosen Type

A chain, entering at `/scope`.

## Candidacy

- `/execute`: not a candidate. `docs/plans/` does not exist in this repo at
  `b7b0799`; no file matches `docs/plans/PLAN-*.md` and no `.md` in the tree
  carries `schema: plan/v1`. #197's PLAN was deleted by its finalization cascade.
- Competitive analysis: not a candidate. `## Visibility` in the scope file is
  `Public`.

## Rationale

The exploration did not end at a question answered; it ended at a commitment to
change shipped behaviour, with three design decisions attached that a future
reader will need and that `wip/` cannot keep. Those decisions are: fork the
occupancy slice rather than edit the shared one, exempt `Rewound` from the
sameness test, and amend rather than supersede the upstream PRD and DESIGN. Each
has a live alternative that a reasonable person would pick differently, and each
has consequences outside the three response bytes that move.

The work is one bounded feature -- a predicate boundary and the surfaces that
describe it -- so it enters the tactical chain rather than the strategic one, and
it enters at the top of that chain because there is no qualifying PLAN and no
licence to skip hops on the strength of the upstream documents that already exist.

## Stage 1 Evidence

### Signals Present (A Chain)

- **Commits someone to building**: the ruling is made; three response cases must
  change and every other case must be proven not to. Evidence: the measured
  baseline table in the findings file, cases B, B2, F.
- **Decisions were made that need a durable home**: the decisions file records
  five, all of which outlive `wip/`. The fork-vs-edit call in particular
  overrules a doc comment that argues explicitly against it
  (`src/engine/persistence.rs:1017-1019`).
- **The work has structure beyond one edit**: predicate, two call-site comments,
  five test files, an evals suite, seven documentation surfaces, and two upstream
  documents whose normative definitions move.
- **A durable artifact is already wrong and must be reconciled**: the Current
  DESIGN's "A contradiction in the PRD was corrected" passage argues the opposite
  of the ruling.

### Anti-Signals Checked (A Chain)

- "The output is a single choice between named options": present in part -- the
  fork-vs-edit and rewind questions are each one choice -- but they are inputs to
  a build, not the whole output. Not counted as present.
- "Feasibility was the open question": not present. Nothing was ever in doubt
  about whether this can be built.
- "Nothing was decided": not present.

### Ranking

- A chain: 4, no anti-signals.
- Decision Record: 2, demoted -- the choices are inputs to a build that has to
  happen either way, not the terminal deliverable.
- Spike Report: 0 -- "can we?" was never the question.
- Rejection Record: -2 -- the conclusion is "proceed".
- Competitive Analysis: not a candidate (public repo).

## Stage 2 Evidence

Stage 2 ran because "a chain" is top-ranked.

### Signals Present (`/scope`)

- **One bounded feature**: the delivery boundary rule. Not several features whose
  order affects delivery.
- **The project exists**: koto ships; this changes one predicate in it.
- **A written contract is needed before code**: the change reverses a documented,
  argued decision. Someone other than the implementer has to be able to read why.
- **Requirements and approach both have open ground**: the rewind case is not
  covered by the brief's own semantics list, and the fork-vs-edit call has
  consequences on two surfaces outside the feature.

### Anti-Signals Checked (`/scope`)

- "One person can act on it without a written contract": not present -- see the
  Current DESIGN that argues the other way.
- "Spans several separately-sequenced features": not present.

### Ranking

- `/scope`: 4, no anti-signals.
- File an issue: 1, demoted -- an issue body cannot carry the reversal record the
  DESIGN needs, and koto#90 already exists and is the thing being closed.
- `/charter`: -1 -- one feature, no sequencing question, project exists.
- `/execute`: not a candidate (no qualifying PLAN).

## Tiebreakers Applied

None required; no pair came within one point after demotion.

The entry-assessment section of the scope file said "needs investigation", which
is consistent with the chain outcome and did not have to be overridden.

## Alternatives Considered

- **File an issue and run `/work-on`**: the code change is genuinely small -- one
  closure and a guard. It ranks lower because the expensive half of this work is
  not the closure; it is reconciling two durable documents that argue for the
  behaviour being removed, and an issue is not where that reconciliation lives.
- **Decision Record**: the rewind question and the fork-vs-edit question would
  each make a clean ADR. They rank lower because neither is the deliverable; both
  are arguments the DESIGN hop has to make anyway, and splitting them into
  separate records would scatter one feature's rationale across three files.

## Deferred Type

Not applicable. Prototype scored 0 -- nothing here needs to be built to be
understood.
