# Crystallize Decision: inline-phase-details

## Chosen Type

`/scope` -- the tactical chain, entering at its first hop.

## Candidacy

- `/execute`: **not a candidate.** No qualifying PLAN exists. `docs/plans/` is
  empty and no `.md` anywhere under `docs/` carries `schema: plan/v1`.
- Competitive analysis: **not a candidate.** `## Visibility` in the scope file
  reads `Public`.

## Confirmation Mode

Running in `--auto`, so Step 4.9's AskUserQuestion is replaced by the
research-first protocol: the scoring below is the evidence, the recommendation is
followed, and the reasoning is recorded here for audit. The outcome also matches
the session's standing instruction, which named `/scope` as the next step after
exploration -- but the scoring was run independently and would have reached
`/scope` on the evidence alone, by a margin of 9 to -2 over the nearest
alternative.

## Rationale

The exploration converged on one coherent feature: make koto's `details` delivery
correct, and give an agent a non-mutating way to recover a phase's instructions.
It is one feature rather than four bug fixes because three of the five measured
defects trace to a single wrong predicate -- koto counts entries into a state when
the quantity that matters is deliveries of its instructions -- and the fourth,
the missing recovery path, is what makes any suppression rule safe at all.

It enters at `/scope` rather than lower for two reasons. First, the requirements
are not merely missing, they are wrong: issue #90's acceptance criteria describe a
YAML template format koto never had, and four of its six criteria have been
satisfied since March by PR #109. Anyone building from the issue as written would
build the wrong thing. Second, real technical decisions remain open with costed
alternatives on the table and no obvious winner -- what to record and where
(three R9-compliant mechanisms, two shortcuts already ruled out on evidence),
which surface carries the recovery read (three candidates with measured cost
differences), and what the command is called (the codebase has a strong,
documented opinion that conflicts with the issue's own wording).

## Stage 1 Evidence

### Signals Present -- A Chain (5)

- *Exploration converged on something someone will build*: five defects
  reproduced empirically against a binary built at `1e3a515`, with transcripts.
- *Requirements, architecture, or sequencing questions remain open*: what to
  record and where, under R9's constraint; whether the `--to` carve-out is
  deliberate; whether auto-advance's discarding of crossed states is in scope.
- *Decisions made during exploration need a durable home and downstream work*:
  seven decisions (D1-D7), plus the root-cause decomposition, none of which is
  recoverable from the code or from issue #90 as written.
- *A scope boundary emerged, not just an answer*: three incidental bugs pushed
  out (D6), batch-child retry ruled out as a break case, auto-advance carried as
  an explicit open question rather than folded in.
- *The core question is "what do we build, and how?"*: yes, after D1 established
  it is no longer "should we build this".

### Anti-Signals Checked -- A Chain

- *Nothing was left to build*: not present. Two acceptance criteria are unmet and
  three further defects were measured.
- *The whole output is one choice between named options*: not present; at least
  three interlocking decisions.
- *The output is a feasibility verdict nobody has committed to acting on*: not
  present. The issue author's 2026-08-16 comment commits to the remaining scope.
- *Findings center on external products*: not present; the external lead was one
  of ten and informed the framing rather than supplying the substance.
- *The conclusion is that the work should not happen*: not present.

### Ranking (stage 1)

| Category | Signals | Anti-signals | Score | |
|---|---|---|---|---|
| A Chain | 5 | 0 | **5** | |
| Rejection Record | 0 | 0 | 0 | |
| Decision Record | 2 | 1 | 1 | demoted |
| Spike Report | 2 | 2 | 0 | demoted |
| Competitive Analysis | -- | -- | -- | not a candidate (public) |

Decision Record matched on "future contributors need to understand why" and
"compared specific alternatives with trade-offs", but carries the anti-signal
"multiple interrelated decisions came with work attached" -- the trade-off
comparisons are inputs to a build, not the whole output. Spike Report matched on
"time-boxed investigation produced concrete findings" and "specific technical
risks identified and tested", but carries both "the question is what should we
build" and "exploration was broad, not focused on a specific technical risk".

## Stage 2 Evidence

Stage 2 ran because "A Chain" is the top-ranked stage-1 category.

### Signals Present -- `/scope` (9)

- *A single coherent feature emerged*: D5.
- *Requirements are unclear or contested*: #90's acceptance criteria are
  superseded and partly fictional.
- *User stories or acceptance criteria are missing*: the criteria that exist
  describe behavior that already shipped and a format that never existed.
- *What to build is clear, but how to build it is not*.
- *Technical decisions need to be made between approaches*: three recording
  mechanisms, three recovery surfaces.
- *Architecture, integration, or system design questions remain*: where the
  boundary-aware checks live, given `derive_visit_counts` is shared with
  `workflows_surface`.
- *Exploration surfaced multiple viable implementation paths*.
- *Architectural or technical decisions were made during exploration that should
  be on record*: R9's actual wording, the `--full` mutation analysis, the
  vocabulary evidence.
- *The core question is "what should we build, and how?"*.

### Anti-Signals Checked -- `/scope`

- *Multiple independent features whose order affects delivery*: not present; one
  feature with internal sequencing.
- *One person can act on this without a written contract*: not present. The
  written contract is the point -- the existing one is actively misleading.
- *A qualifying PLAN already covers this work*: not present; none exists.
- *The exploration produced no work*: not present.

### Ranking (stage 2)

| Entry point | Signals | Anti-signals | Score | |
|---|---|---|---|---|
| `/scope` | 9 | 0 | **9** | |
| File an issue | 1 | 3 | -2 | demoted |
| `/charter` | 0 | 3 | -3 | demoted |
| `/execute` | -- | -- | -- | not a candidate (no PLAN) |

## Tiebreakers Applied

None. The stage-1 margin (5 over 0) and the stage-2 margin (9 over -2) both
exceed one point, so no tiebreaker rule was triggered and stage 2 ran on the
chain outcome rather than on a near-tie.

## Alternatives Considered

- **File an issue** -- ranked lowest of the entry points despite being the
  cheapest path. It matched only "one person can implement without
  coordination". Three anti-signals fired: architectural decisions were made
  during exploration and need a durable home, scope was debated and narrowed
  across two rounds, and anyone building from the existing issue needs
  documentation because the issue itself is wrong about the template format and
  about which criteria remain. Filing would discard the root-cause decomposition
  and leave the next implementer to rediscover that `--full` mutates.

- **`/charter`** -- ranked lowest overall. koto is a mature project, the work is
  one bounded feature, and the users and needs are identified and uncontested.
  Nothing strategic is in question.

- **Decision Record** -- the closest terminal outcome, and a real near-miss on
  substance: the recording-mechanism question and the naming question are both
  genuine decisions with named options and trade-offs. It ranked lower because
  they are not the exploration's entire output. Both are inputs to a build that
  also has defects to fix, criteria to rewrite, and downstream skill and eval
  work. The chain records them as it goes.

- **Spike Report** -- feasibility was never the open question. Every candidate
  approach is clearly feasible; round 2 measured *defects*, not viability, and
  the exploration was broad across grammar, contract, prior art, and naming
  rather than focused on one technical risk.

- **Rejection Record** -- scored zero. Nothing in the research argues the work
  should not happen; the issue author's own most recent comment argues the
  opposite.

## Deferred Type

Prototype did not score. The mechanism is already built and measured; there is
nothing to prove by building a throwaway.

## Handover

Next step: `/scope inline-phase-details`. No `--upstream` flag -- the exploration
found no ROADMAP covering this work. The handoff artifact is
`wip/scope_inline-phase-details_handoff.md`, and the research files under
`wip/research/` stay in place for the chain to read.
