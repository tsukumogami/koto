# /prd scope: self-loop-suppresses-details

## Upstream

`docs/briefs/BRIEF-self-loop-suppresses-details.md` (Accepted). Its framing is
carried forward rather than restated; only the problem is stated in full here so
the PRD stands alone.

## Visibility

Public.

## Invocation context

Invoked by `/scope` with the `parent_orchestration:` sentinel present
(`invoking_child: prd`, `rationale: fresh-chain`). Execution mode `auto`.

## Problem, in requirements terms

koto decides per response whether to include a phase's long-form instructions.
The rule is "once per occupancy", and an occupancy is bounded by state-entry
events, so a phase transitioning to itself opens a new one and the instructions
are re-sent. koto#90's acceptance criterion 3 says the opposite and has been
ruled to govern. The PRD's job is to state the rule as a requirement per arrival
class, testably, and to record the two decisions the brief left it.

## What the brief already settled (inputs, not questions)

- Looping inside a phase suppresses.
- Arriving from a different phase delivers, including a loop-back.
- A directed transition into the already-occupied phase suppresses.
- Any rewind delivers, including one landing on the phase it started from.
- The forcing flag still delivers; the read-only retrieval still returns.
- No new event, no new field, no schema version bump.

## What this PRD owes

1. A requirement per arrival class, stated so a reader can tell which side of the
   line any reachable arrival falls on without reading the engine.
2. The argument for the asymmetry the brief left open: why a directed transition
   into the occupied phase suppresses while a rewind into it delivers. Lands in
   Decisions and Trade-offs, which is the documented closure surface for a
   brief's deferred question.
3. Acceptance criteria that are binary and checkable by someone who did not write
   the PRD, naming the harness each one belongs in.
4. A non-functional requirement covering the surfaces that describe the rule:
   after this lands, no documentation or durable artifact may state the old one.

## Research leads (Phase 2)

1. **Every arrival path, and which are self-entries.** The requirements cannot be
   stated per arrival class against a partial list. Dispatched as
   `wip/research/prd_self-loop-suppresses-details_phase2_arrival-paths.md`.
2. **Harnesses and what a testable criterion looks like here.** Acceptance
   criteria have to name a real gating command and a real harness. Dispatched as
   `wip/research/prd_self-loop-suppresses-details_phase2_verification.md`.

Both leads are scoped to what the PRD needs. The mechanism-level questions —
where the boundary rule lives, whether the shared occupancy helper is forked —
are deliberately not leads here; they are the DESIGN's to decide, and the
`/explore` findings already carry the evidence for that decision.
