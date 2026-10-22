---
schema: brief/v1
status: Accepted
problem: |
  When one agent session hands work to another, whoever sits above or below
  the worker needs its log and keys afterwards, but koto removes a finished
  session without asking whether any session that dispatched it, or that it
  dispatched, still needs it, and it knows nothing about agent sessions.
outcome: |
  A session stays readable for at least as long as any session that
  dispatched it, or that it dispatched, is alive, an idle but resumable agent
  session included; koto says why it keeps each session, and it all works on
  one machine with no workspace manager and no cloud, and after an import.
---

# BRIEF: Session retention across the dispatch hierarchy

## Status

Accepted

## Problem Statement

Agent sessions hand work to each other. A person starts a coordinator, the
coordinator dispatches workers, and a worker may run a chain of skills or
dispatch workers of its own. Each of them keeps its working state in koto
sessions: an event log and a set of context keys. After a worker finishes,
the sessions above it still need that record. A coordinator reads what the
worker decided and may undo it, and a person reads it to see why something
happened. The worker, in turn, may need to read the record of the session
that dispatched it.

koto doesn't know any of that. A session that reaches a success terminal
without `--no-cleanup` loses its log and keys on the spot. Callers that pass
the flag on every tick keep their sessions, but five other paths still remove
them without asking who still needs them. One is the parent sweep that follows
a parent's removal. Another is the same-name replace when a skill topic runs
again. Then there's `koto workspace prune --root`, and `koto session prune`.
Last are a person's explicit `koto session cleanup` and `koto cancel
--cleanup`. Only `session prune` keeps a finished session because a related
one is running, and only for one hop: across koto's parent-child lineage or
one shirabe-owned key, in the local store, when someone runs it by hand.

The relations koto could follow are the wrong ones. Parent-child lineage
doesn't survive `koto session import`, which refuses a child and carries no
children. Request legs live in a host-local store that import drops. The
workspace manager's map of who dispatched whom holds harness ids, koto never
sees it, and it's deleted when the instance is reaped. So once a worker
finishes, nothing koto keeps ties it to the sessions that dispatched it.

"Alive" is the other gap. koto's only notion is that a session's workflow
hasn't finished. But the sessions that dispatch are agent sessions, and one
of those often sits idle and resumable after its koto run is done, or holds no
koto session at all, as a person's own session usually does. To koto that
session is already gone, so the workers it dispatched lose their protection
while the person can still resume it and ask about them.

## User Outcome

A coordinator, or the person behind it, can read everything a finished
worker recorded for as long as any session in that dispatch hierarchy is
still alive, and a worker can read the record of the session that dispatched
it. An agent session counts as alive while it can still be resumed, even idle
after its work. Where nothing watches for its deletion, it counts as alive
until it has been idle for a configured period. Once the whole hierarchy is
no longer alive and a configured retention has passed, koto reclaims it.

Whoever runs a removal can see what happened and why. koto's removals that
pick sessions by rule or by subtree keep a session whose hierarchy is alive
and say which live session keeps it. A person's explicit removal of a named
session still removes it, and says which live hierarchy it broke. Reading a
session from above leaves the read session exactly as it was.

None of this needs anything beyond koto. A developer with koto and its
plugin on a laptop, with no workspace manager and no cloud, gets the same
behaviour, closed by the idle period instead of by an observed deletion. A
session that moves to another workspace by import keeps its place in the
hierarchy, because the hierarchy rests on links that travel with it rather
than on lineage that import drops. And a workspace manager user who never
installs koto sees nothing change.

## User Journeys

### A coordinator reviews a finished worker after the cleanup has run

A coordinator dispatches a worker, which runs a skill and finishes. Hours
later, after the worker's own terminal step, the parent sweep and a
`koto session prune --apply` have all run, the coordinator needs to check what
the worker decided. It reads the worker's log and keys as it would have the
moment the worker finished. The prune output names the coordinator as the
reason the worker was kept.

### A worker reads the record of the session that dispatched it

A planning session dispatches two workers and then finishes its own run. One
worker is still working and needs to re-read a decision the planner recorded.
The planner's session is still there, because a session it dispatched is
alive, and the prune names that worker as why.

### A developer on a laptop with no workspace manager

A developer runs koto and its Claude Code plugin with no workspace manager
installed. Their agent session dispatches a helper, which finishes. While the
developer keeps working, the helper's record stays readable. They walk away,
and after the configured idle period their session stops holding it. Once the
retention runs out, the next prune removes the helper and says the idle close
is why the hold ended.

### An idle coordinator in a reaped instance

A coordinator launched into an ephemeral instance finishes its koto run and
sits idle, resumable, for a day, longer than the idle period and the
retention. The workers it dispatched stay readable, because the workspace
manager watches for that session's deletion. Only after the session is
deleted and the instance reaped does the hold end, and the retention then
runs before anything is removed.

### A hierarchy moves by import

An operator moves a coordinator's session to another workspace with
`koto session import`. The moved copy still reaches the workers it dispatched
by session name, and the agent session that imported it now holds it. The
old copy, marked migrated, pins nothing. A session imported from a bare shell
with no agent session behind it drops its hold and falls back to the plain
rule, so it is never kept forever.

### An operator cleans up by hand

An operator runs `koto session cleanup` on a session a live coordinator still
reads. koto removes it, because a person named it, and reports which live
hierarchy lost a member.

## Scope Boundary

**In:**

- A koto-owned record of who dispatched whom, kept by each session about
  itself and naming its peers by session name, so it survives import.
- Each agent session represented in koto, so an idle, resumable one counts
  as alive until its deletion is observed or, where nothing observes it,
  until a configured idle period passes.
- One retention rule, "is any session in my connected hierarchy alive?",
  applied by every removal koto makes by rule or subtree, with a reported
  reason for each kept session; explicit removals that report what they
  broke.
- Reads from above that leave the read session byte-identical.
- What import does with a moved session's hold on its anchor.
- Making the koto plugin's hooks load at all, since every anchor depends on
  its session-start hook.
- The small generic pieces outside koto this needs: the workspace manager
  passing declared variable names through to workers, marking sessions it
  will reap, and running a declared command when it removes one; and
  shirabe recording a dispatch link on each dispatch path that uses no
  request, and retiring its own reclaimable rule.

**Out:**

- Recording delegated decisions, reversing them, and who may do so. That
  work builds on this retention later.
- Turning on koto's cloud backend, changing its configuration file, or
  testing across hosts. Import is exercised on one host with per-command
  configuration in the environment; cross-host reads wait.
- The workspace manager reading or writing any workflow state, naming koto,
  or depending on it; koto depending on the workspace manager.
- Authority: belonging to the same hierarchy keeps a session readable and
  grants nothing.
- Session-start equivalents for harnesses other than Claude Code. A session
  with no anchor falls back to the koto-level rule, which is the expected
  behaviour there.
- Retention of host-level stores (the request store, the decider ledger, the
  terminal index, finished records).
