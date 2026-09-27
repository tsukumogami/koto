---
schema: brief/v1
status: Accepted
problem: |
  koto deletes a session and its context on reaching any terminal, failure
  included. For a failed child that's the record recovery needs: no retry, no
  rewind, no failure reason. The one flag that keeps the record also withholds
  the child's result from its parent.
outcome: |
  A coordinator or operator can still retry a failed child, rewind it and read
  why it failed through koto's own commands, while the parent gets the result
  on the same tick either way. The terminal response says whether the record
  was kept, and a kept record has a stated point where it's reclaimed.
motivating_context: |
  In one overnight batch, three /work-on children ended at done_blocked with
  their work complete and pushed. koto disposed of each child's session at the
  terminal tick, the parent's retry_failed answered unknown_children, and the
  parent ended at escalate. Each one cost a full re-run of the parent. koto
  issue 240 carries the analysis and a runnable reproduction.
---

# BRIEF: koto-child-retention

## Status

Accepted

Phase 4 jury returned all-PASS (content-quality and structural-format). The
downstream PRD owns the behaviour, its acceptance criteria, and the name and
shape of the terminal response's retention field. The retention shape (what
survives, for how long, and which command or event reclaims it) is the DESIGN's
to settle with the alternatives written down; the retry and rewind journeys
need the child's own log, which bounds what can be kept.

## Problem Statement

A koto session ends when it reaches a terminal state, and on that tick koto
removes the session's directory. The removal takes everything with it: the
event log, and the context store where skills keep a work item's running
record. Nothing about the terminal state changes this. A state declared
`failure: true` is removed exactly like a successful one.

For a root session that's a lost record, and a root can opt out by passing
`--no-cleanup`. For a child session materialized by a batch parent it's worse,
because the failure terminal is exactly where the parent's recovery starts:

- **Retry can't find the child.** `retry_failed` works by rewinding the failed
  child's own log. With the session gone, the parent's submission is refused
  with `unknown_children`, and a parent like `/execute` has nowhere left to go
  but escalate.
- **Rewind can't find it either.** `koto rewind` on the child reports the
  workflow missing, so a failure that one resubmission would fix forces a
  re-run of everything above it.
- **The reason is gone before anyone reads it.** The child's `failure_reason`
  and the rest of its context were deleted on the tick that set them. A
  coordinator that wants to know why a child failed, after the fact, has
  nothing to ask.

The obvious way around it doesn't work for a child. `--no-cleanup` keeps the
session, but it also suppresses the child's result on its own log and the
completion notice on the parent's, which are the only two places the parent's
`children-complete` gate reads. A child that passes it keeps its record and
its parent never learns the result: a parent that waits on the gate stalls for
good, and one keyed on `all_complete` advances without it. Keeping the record
and delivering the result are fused into one switch, so a child has no safe
setting.

The runs that reach a failure terminal are the ones whose record somebody
needs next, and today they're the ones guaranteed to lose it.

## User Outcome

A coordinator driving a batch, or an operator looking after one, can still act
on a child that ended at a failure terminal after that terminal tick has
passed. The parent's `retry_failed` reaches the child and rewinds it. An
operator can `koto rewind` it one step and resubmit instead of re-running the
parent. Anyone can read what the child recorded, its failure reason included,
through `koto status` and `koto context get`, without knowing where koto keeps
its files.

None of that costs the parent anything. The child's result reaches the parent
on the terminal tick whether or not the child's record is kept, so the gate
reports the result as in for every parent shape.

The terminal tick's response says what happened to the session, so a caller
finds out from koto whether its record is still there instead of inferring it
from a template's prose. And a kept record doesn't pile up forever: there's a
stated point at which koto, or an operator with one command, reclaims it.

## User Journeys

### A parent retries a child that ended done_blocked

`/execute` spawns a `/work-on` child per issue. One child reaches `done_blocked`
at its evidence gate after finishing and pushing its work. The parent's gate
reports the child as failed, with its result in. The coordinator submits
`retry_failed` naming the child, koto rewinds the child's own log, and the
child runs again from its initial state. The batch never escalates.

### A coordinator reads why a child failed, after its terminal

A coordinator template's reconcile step re-checks what each child holds before
deciding what to do next. It runs `koto status` and `koto context get` against
a child that failed an hour ago and gets back the final state, the result and
the `failure_reason` the child wrote. The record survived the tick that ended
the child, and the coordinator never read a file under `~/.koto`.

### An operator rewinds a failed child one step

A child failed because the agent submitted evidence before the context key it
depended on was written. The operator runs `koto rewind` on the child, writes
the key, resubmits, and the child reaches its success terminal. The parent's
gate picks up the new result, and the parent continues from where it was
instead of being re-run from the start.

### A skill author keeps a child's record on purpose

A skill author wants a child's context readable after a successful run, the
way `/scope` keeps a root's, and asks koto to keep it (today that means
`--no-cleanup` on the child's ticks). The
child's session stays on disk and its parent's gate still reports its result
as in, so the parent converges normally.

### An operator reclaims retained records

A batch has settled and its failed children's records aren't needed any more.
The operator reclaims them with one koto command aimed at the batch's root,
and every retained session under it is removed. A child retained under a parent that koto
disposes of itself goes with its parent, so nothing is left without an owner.

## Scope Boundary

**IN**

- What happens to a session, root or child, on the tick that reaches a
  `failure: true` terminal: its log and context stay readable afterwards.
- Separating keeping a session's record from delivering its result, so a child
  that keeps its record still delivers its result to its parent and to any
  request leg it's bound to.
- The terminal tick's response stating whether the session was kept or
  removed.
- The lifetime of a retained record and the command or event that reclaims it,
  with its storage cost stated.
- Updating koto's own docs where session lifecycle and cleanup are described,
  and the koto-skills that describe `--no-cleanup`.

**OUT**

- Keeping every successful terminal session by default. Issue 234's ask, a
  completed run staying queryable, overlaps but is a different change with a
  different storage cost; this brief covers failure terminals and deliberate
  retention only.
- Moving the context store out of the session directory, or giving context a
  lifetime separate from the session's. The issue raises it as a possibility;
  this work keeps context inside the session.
- Any change to shirabe's skills, including removing `/work-on`'s root-only
  retention workaround and changing its summary shape. Those are the
  consumer's follow-ups once this lands.
- The request store's wake signal, the `koto next --to` guard for
  non-overridable gates, and the decider ledger, which other work covers.
- A general time-based retention policy or background sweeper. koto has no
  daemon, and a window that expires records on wall-clock time is a separate
  decision.

## References

- `docs/designs/current/DESIGN-request-lifecycle.md`: the terminal-tick
  ordering this work changes, and why the parent notice was kept behind the
  cleanup guard.
- `docs/designs/current/DESIGN-request-store-converge.md`: how the
  `children-complete` gate reads a child's result from a live child or from
  the parent's copy.
- `docs/designs/current/DESIGN-batch-child-spawning.md`: `retry_failed` and
  the batch scheduler.
