---
schema: brief/v1
status: Draft
problem: |
  As koto session count grows, the dashboard can no longer answer the
  one question a developer brings to it -- "which session needs me
  now?" Abandoned sessions are indistinguishable from active ones,
  sessions are labeled only by opaque IDs, and live work is buried
  under finished and never-started noise.
outcome: |
  A developer opens the dashboard and immediately sees which sessions
  are alive and which need attention -- stalled work is visibly
  distinct from progressing work, each session is recognizable by what
  it is doing, and finished or abandoned sessions stay out of the way.
  The list stays readable as session count grows.
---

# BRIEF: Session legibility in the local dashboard

## Status

Draft

Framing for the local-scope slice of the session-legibility work,
reframed from a five-field metadata addition to the dashboard UX
problems that surfaced when the session list was run against a real,
long-lived `~/.koto`. The downstream PRD owns the requirements; the
Open Questions below mark what it must settle first.

## Problem Statement

The koto dashboard lists every session a developer has. That works
when there are a handful of them. It stops working as the count grows
-- and it grows fast, because koto sessions accumulate: every workflow
run, every child spawn, every experiment leaves one behind.

At that point the dashboard can no longer answer the question a
developer actually opens it to ask: *which session needs me right
now?* Three things break that:

- **Alive and dead look the same.** The status the list shows treats
  "actively making progress" and "abandoned weeks ago" as the same
  state. The only time signal is counted from when the session was
  created, so a session that has been idle for a month and one that is
  advancing right now read identically. There is no way to see that a
  session has gone quiet.
- **Sessions aren't recognizable.** A session is identified by an
  opaque id. To know what a row *is*, a developer has to remember what
  that id meant or open the session and read it. The descriptive label
  the system already has room for is, in practice, empty everywhere --
  so the list is a column of names that carry no meaning.
- **Signal is buried in noise.** Long-finished sessions, sessions that
  never got past their first step, and sessions the dashboard can't
  even parse all share the view with live work. There is no way to set
  the finished and abandoned ones aside, so the few sessions that
  matter are lost in the ones that don't.

The compounding effect is the real problem: the more a developer uses
koto, the *less* the dashboard helps them. A tool meant to make work
legible becomes harder to read the more work there is.

## User Outcome

A developer opens the dashboard and, without reading a single log,
sees the shape of their work: which sessions are alive, which have
gone quiet, which are done.

A session that has stopped making progress is visibly distinct from
one that is advancing, so the one that needs attention stands out
instead of hiding. Each session is recognizable by what it is doing --
the developer reads the list and knows which work each row is, rather
than decoding identifiers. And the sessions that no longer need
attention -- finished, abandoned, never-started -- stay out of the way
unless the developer asks for them.

The decisive change is that this holds *as the session count grows*.
The dashboard surfaces the live, attention-worthy sessions instead of
drowning them, so it stays useful on the hundredth session the way it
was on the third.

## User Journeys

### Triage the active set

A developer who has several sessions in flight steps away, comes back,
and opens the dashboard to find the one that is stuck or waiting on
them. They scan the list and immediately tell the sessions still
advancing apart from the ones that have gone quiet -- the stalled one
stands out -- and they go straight to it without opening every session
to check which is which.

### Recognize a session by its work

A developer wants to check on a specific piece of work -- the change
they kicked off earlier. They scan the session list and find it by
what it is, reading a label that names the work, rather than mapping a
cryptic id back to the task in their head or opening sessions one by
one until they hit the right one.

### Declutter to the live set

A developer's session list has grown dominated by old, finished, and
never-started sessions. They want to focus on what is live. The
default view keeps the finished and abandoned sessions out of the way,
so what they see is the handful of sessions actually in progress --
and they can still call up the hidden ones deliberately when they need
the history.

## Scope Boundary

**IN:**

- Legibility of the **local, single-machine** dashboard session list.
- A staleness / idle signal that distinguishes an actively-progressing
  session from an abandoned one, based on time since the session last
  did something (not time since it was created).
- A present, human-readable label for each session in the list, so
  rows are recognizable without opening them.
- Noise reduction: keeping finished, abandoned, and never-started
  sessions out of the default view, and addressing the sessions the
  dashboard silently drops from the list today.

**OUT:**

- **Cross-machine / multi-host visibility** (which machine produced a
  session, ownership across machines). That scope moved to the
  S3-backed cross-machine dashboard, which owns the `host` / `owner`
  metadata; this brief is strictly the single-host view.
- The aggregated cross-machine view and its remote-storage substrate.
- **Multi-user** concerns -- distinguishing whose session is whose
  across people. Single-user koto has one owner; multi-user review is
  a separate product category.
- New event families (coordination, review) and the panels that render
  them -- separate work items.
- The original "five lifecycle-metadata header fields" framing this
  brief supersedes for the local scope; any field that survives is
  whatever the UX outcomes below actually require, decided in the PRD.

## Open Questions

- **The silently-dropped sessions.** Running the list against a real
  `~/.koto` showed the dashboard surfacing only a small fraction of
  the session directories on disk; the rest are dropped without a
  trace. The PRD must settle whether that is a parsing/correctness bug
  to fix or expected unreadable cruft to garbage-collect -- and whether
  fixing it is part of "noise reduction" or a precondition for it.
- **The staleness model.** What defines "stale" -- a fixed idle
  threshold, a threshold that varies by workflow or state, or
  something the user configures -- and whether the signal is computed
  when the list is read or recorded on the session. Deferred to the
  PRD and its design.
- **The label strategy.** Whether the human-readable label populates
  the descriptive field that already exists, is derived from the
  workflow and current state, or is a new field -- and what writes it.
- **The noise-reduction surface.** Whether decluttering is a default
  filter on the view, an explicit prune / garbage-collect action, or
  both.

## References

- `docs/reference/session-feed.md` -- the session-feed data contract,
  including the six columns the non-interactive list emits and the
  "Lifecycle Metadata Surface" note on where session-level metadata
  belongs.
- `docs/prds/PRD-local-dashboard.md` and
  `docs/designs/current/DESIGN-local-dashboard.md` -- the existing
  dashboard requirements and design this builds on.
- `docs/designs/current/DESIGN-session-feed-data-contract.md` -- the
  feed contract the list rendering reads from.
