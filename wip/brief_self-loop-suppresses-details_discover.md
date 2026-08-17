# /brief discover: self-loop-suppresses-details

## Visibility

Public (koto `CLAUDE.md`, `## Repo Visibility: Public`).

## Grounding Path

None. No `--upstream` supplied and no ROADMAP sequences this feature, so the
produced BRIEF records no `upstream:` field.

## Invocation context

Invoked by `/scope` with the `parent_orchestration:` sentinel present in
`wip/scope_self-loop-suppresses-details_state.md`
(`invoking_child: brief`, `rationale: fresh-chain`). Discovery input is the
`/explore` handoff at `wip/scope_self-loop-suppresses-details_handoff.md`.

## The problem/outcome pair

**Problem.** koto sends a phase's long-form instructions once per *occupancy*,
and it counts a phase transitioning to itself as leaving one occupancy and
entering another. An agent going around a loop it is already inside is therefore
re-sent a procedure it already holds, on every lap. koto#90 said the opposite in
writing before the mechanism was built, and so did the older output-contract PRD
it was drawn from; the definition that overrode them was settled during a later
scoping pass and never flagged as overriding anything.

**Outcome.** An agent looping inside one phase pays for that phase's procedure
once. Arriving somewhere new, coming back from somewhere else, or being sent
back deliberately still delivers it, and the read-only retrieval still returns it
on demand, so nothing the agent needs becomes unreachable.

## Journeys surfaced during discovery

1. An agent driving a long loop (the 14-week sweep in koto#90's own audit) ticks
   the same phase over and over and stops paying for the procedure after the
   first lap.
2. A template author writes a self-transition and needs to know what koto will
   and will not repeat, without reading the engine.
3. An agent that lost the procedure to compaction gets it back without moving
   the workflow.
4. A maintainer reading the durable design record finds a definition that matches
   the code rather than one the code contradicts.

## Scope edges named during discovery

In: the boundary rule, its call sites, the tests and evals that encode it, the
agent-facing skills and guides, and the two upstream documents whose normative
definition moves.

Out: a `koto phase-info` command (the retrieval already exists on `koto status`),
the four filed-and-independent koto defects, and anything that would move
`CURRENT_SCHEMA_VERSION`.

## Uncertainty carried forward

- Whether a rewind that lands on the phase it started from should deliver.
  Reachable today, absent from every upstream document. The BRIEF names it as an
  open question; the PRD owns the answer.
- Whether the shared occupancy helper changes meaning for its other consumer.
  A framing question at brief altitude, a requirement at PRD altitude, and a
  decision at design altitude.
