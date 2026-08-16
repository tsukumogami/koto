# Brief Discovery: inline-phase-details

## Context

- Visibility: Public (koto `CLAUDE.md`, `## Repo Visibility: Public`).
- Invoked under `/scope`'s `parent_orchestration:` sentinel with
  `rationale: fresh-chain`, so Phase 5 takes the parent-delegated-approval
  fallback: the BRIEF lands in Draft and `/scope` owns the transition.
- No `--upstream`. `docs/roadmaps/` does not exist in this repo, so there is no
  ROADMAP to ground on and no ancestor to resolve. The `upstream:` field is
  omitted.
- Source: koto#90, plus `wip/scope_inline-phase-details_handoff.md` from the
  `/explore` run and the ten research files under `wip/research/`.

## Feature Being Framed

The delivery of a workflow phase's full instructions to the agent driving it:
when koto sends them, when it withholds them, and how an agent that no longer
has them gets them back.

## Problem / Outcome Pair

**Problem.** koto already sends a phase's long-form instructions on what it
believes is the agent's first visit and withholds them afterward. The belief is
computed from the wrong quantity -- entries into a state rather than deliveries
of its instructions -- so the behavior inverts. A gate-blocked agent that never
moves re-receives the instructions on every tick; an agent told to rewind and
redo a step receives nothing. Underneath sits a harder fact: the session log
records nothing about who is attached to it, so no rule derived from the log can
tell an agent that still holds the instructions from one whose context was
compacted or which respawned onto its predecessor's log. And there is no way
back -- `koto status` is read-only but carries neither the directive nor the
details, and `koto next --full` carries the text but evaluates gates, can
auto-advance, re-runs any `default_action` shell command, and can clean up a
terminal session.

**Outcome.** An agent driving a koto workflow has the current phase's procedure
whenever it needs it, and does not pay for it on every tick when it already has
it. When it has lost the procedure -- to compaction, to a respawn, to a fresh
process picking up the session -- it can ask for it back with one call that
changes nothing about the workflow's state.

## Journeys Identified

Four distinct entry points, each measured or evidenced in the exploration:

1. A long gate-blocked loop, where the same instructions arrive on every tick.
2. A rewind, where the agent is told to redo a step and is denied the procedure
   for it.
3. A context-loss recovery, where the agent needs the procedure and must not
   move the workflow to get it.
4. A template author adding a details block and getting behavior that matches
   what the documentation says.

## Scope Decisions Taken During Discovery

- **The `--to` directed-transition path is IN scope.** `dispatch_next` applies
  no visit check at all, so today's contract is "first visit only, except under
  `--to`". Nothing in the code or the design docs says whether that is a
  deliberate carve-out for explicit operator intent or an oversight. Framing it
  as in-scope commits the chain to answering the question rather than leaving
  it open; the DESIGN records which reading wins. Keeping it out would leave the
  contract non-uniform with nothing recording why.
- **Auto-advance discarding crossed states is OUT of scope.** A `koto next` that
  advances through an intermediate state surfaces neither its `details` nor its
  `directive`. This predates the details mechanism, is broader than it, and
  folding it in would mis-frame a fix as "make details work on crossed states"
  when the honest framing is that auto-advance has never surfaced intermediate
  instructions at all. Filed separately.
- **Three incidental bugs are OUT of scope**, filed separately: `koto rewind`
  moving forward on a second consecutive rewind; `accepts:` not gating
  advancement; and the migration-scan stderr flood, which is very likely open
  issue #193.

Recording these here means the brief carries no Open Questions section into
Phase 5, which is what the Draft to Accepted transition requires.
