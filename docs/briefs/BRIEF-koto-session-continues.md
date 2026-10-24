---
schema: brief/v1
status: Draft
problem: |
  When a session's work passes to another session (a successor taking over,
  or an import into another workspace), koto keeps no record that the new
  session carries on the old one. Retention judges the predecessor alone and
  removes it while the successor still depends on it, and import retires its
  source outright.
outcome: |
  A session that takes over another's work says so once, at open, and the
  predecessor and the chain behind it stay readable for as long as anything
  later in that chain is kept, on every host sharing the bucket. An import's
  source stays as a read-only predecessor, and the relation grants no
  authority.
---

# BRIEF: koto-session-continues

## Status

Draft

## Problem Statement

Work in koto often outlives the session that started it. A coordinator that
crashes or runs out of context hands its job to a successor. A session takes
over work another session left unfinished, sometimes started by someone who
had nothing to do with the original. An operator moves a session to another
workspace with `koto session import`. In each case the new session carries on
the old one: it reads the predecessor's decisions, context keys and history to
know where things stand.

koto has no way to record that. The retention rule joins sessions into a
hierarchy only through dispatch links, anchors, `parent_workflow` lineage and
`chain/parent`, and a successor usually has none of those to its predecessor.
So the predecessor is judged on its own. Once it finished and
`retention.keep` passed, its own terminal tick, `koto session prune`, or
`koto workspace prune` removes it, even though a live successor still needs
to read it. The only defence today is pinning each predecessor by hand and
remembering to unpin it later, and a pin keeps one session, not a chain of
them.

Import goes further and discards the source on purpose. It leaves a
migration marker beside the source, the source refuses everything with
`session_migrated`, including reads, and `koto session prune --migrated-from`
deletes it once the marker is old enough. The record the copy came from is
gone, and nothing tells a later reader where the copy's history started.

Hosts sharing one bucket make this worse. A successor opened on one host is
invisible to a prune running on another host that holds only the
predecessor's copy, so even a recorded relationship would protect nothing
unless every removal can see it where the bucket keeps it.

## User Outcome

A session that takes over another's work records, once and at open, which
session it continues. It doesn't matter who launched it: a coordinator's
successor, an operator, or a session started by a stranger to the original
all record the relation the same way, in the successor's own context.

From then on the person or agent running the successor can read the
predecessor, and the predecessor's own predecessors, for as long as the
successor is alive, and for the successor's own retention after it stops.
A pin on the newest session in the chain keeps the whole chain behind it.
The reverse never happens: an old session doesn't hold a newer one in place.
An operator running a prune on any host sharing the bucket sees each kept
predecessor named with the later session, or the pin, that keeps it, and the
chain goes away together once nothing later keeps it.

An operator who imports a session gets a copy that names its source, and the
source stays readable as a read-only predecessor: ticks and writes on it
still refuse and point at the copy, but reads work, and the source is kept
until the copy's lineage releases it. Recording the relation never widens
what the successor is allowed to do.

## User Journeys

### A coordinator's successor picks up the run

A coordinator session stops partway through a long effort. A new agent
session starts to take over and opens its own coordinator session, naming the
one it continues. While the new coordinator runs, prunes and terminal ticks
leave the old coordinator and its earlier predecessor in place, each one
reported as kept by the new coordinator, and the new coordinator reads their
context to pick up where they left off. Once the new coordinator finishes and
its retention passes, the next prune removes all of them together.

### A stranger takes over stalled work

A session's agent is gone and nobody who dispatched it is around. An operator,
or an agent session launched by neither the original session nor anything it
dispatched, opens a session that continues it. The relation keeps the stalled
session readable exactly as it would for a coordinator's successor, and gives
the new session no authority it didn't already have.

### An operator moves a session to another workspace

An operator imports a session into a second workspace and continues it there,
with the cloud backend configured through the environment. The copy names the
source as its predecessor. Back in the original workspace, a tick or write on
the source refuses and names the copy, while a read of the source works. A
`prune --migrated-from` run, or a prune on a second host sharing the bucket,
keeps the source while the copy lives, naming the copy as the reason.

### An operator keeps a finished chain as a record

A three-session chain finished and its retention passed. The operator pins the
newest session. Every later prune keeps all three and names that pin for each.
Pinning the oldest session instead keeps only the oldest; the two newer ones
go, because an older session never keeps a newer one.

## Scope Boundary

**In scope:**

- A directed relation from a successor session to the session it continues,
  written in the successor's own context, like every other link.
- The way a successor records the relation at open, independent of who
  launched it.
- `koto session import` recording the relation in the copy, naming the source.
- Retention counting the relation as a keeper, in one direction only: later
  members keep earlier ones, and a pin on a later member reaches back through
  the chain.
- A new import contract: the source stays as a read-only predecessor (writes
  and ticks still refuse with `session_migrated` and name the copy; reads
  work), and `prune --migrated-from` waits for the lineage to release it.
- Removals reading the relation from the bucket before they delete, so hosts
  sharing a bucket judge the same lineage, and an unreadable relation keeping
  its session and being reported.
- Every removal that keeps a session for this reason naming the later member
  or pin that keeps it.

**Out of scope:**

- Any authority. The relation isn't `dispatched-by`: it feeds no authority
  check, so a successor gains nothing it couldn't do without it.
- Guards on trusted removals, such as a prune floor (koto#458). They change
  when a removal may run at all, which is separate from what keeps a session.
- Removing the coordinator-name rule from prune, and fixes to reads by name.
  Both touch the same commands but are separate changes.
- The takeover flow itself: how a coordinator's successor is started and what
  it does first belongs to the workflow skills that run coordinators, not to
  koto.
- Workspace or instance lifetimes managed by tools outside koto.
- Fixing bucket copies that fork when one session is written from several
  directories (koto#477), beyond anything this feature needs to read the
  relation correctly.
- Retiring workarounds callers built for the missing relation; they can drop
  them once this ships.

## Open Questions

- What the verb a successor uses at open is called, and whether it is a flag
  on `koto init` or a `koto session` subcommand. The PRD fixes the behaviour;
  the design picks the surface.
- How much of the bucket a removal reads to find a successor that only
  another host holds, and what that costs per prune.

None of these block the brief.
