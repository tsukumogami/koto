---
schema: brief/v1
status: Accepted
problem: |
  As koto session count grows, the dashboard can no longer answer the
  one question a developer brings to it -- "which session needs me
  now?" Abandoned sessions are indistinguishable from active ones,
  sessions are labeled only by opaque IDs, and live work is buried
  under finished and never-started noise.
outcome: |
  Opening the dashboard, the developer's first answer is whether
  anything needs them: needs-you sessions lead, ordered by urgency.
  Life is measured from last activity, so a silent run looks distinct
  from an advancing one and a parked run is not mistaken for a dead
  one; rows are recognizable by their work; finished and abandoned
  sessions recede on their own -- so the needs-you set stays small and
  trustworthy as sessions accumulate.
---

# BRIEF: Session legibility in the local dashboard

## Status

Accepted

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
gets one answer first: *is the ball in my court?* The view leads with
the sessions that need a decision -- an agent waiting on input, a run
that failed -- and orders them so the most urgent sits on top. When
nothing needs them, that reads instantly too.

The dashboard tells alive from dead honestly. A session's life is
measured from when it last *did* something, not from when it was
created, so a run that has gone silent is visibly distinct from one
advancing right now -- and a session legitimately parked, waiting on a
slow step, is not mistaken for one that died. Each session is
recognizable by the work it is doing rather than by an opaque
identifier. And the sessions that no longer need attention -- finished,
idle, abandoned -- recede on their own and stop accumulating, instead
of being filtered around forever.

The decisive change is trust at scale. The "needs you" set stays small
and honest -- every session in it genuinely wants a decision -- so the
developer keeps believing it. The dashboard surfaces the few live,
attention-worthy sessions instead of drowning them, and it holds that
way on the thousandth session as well as the third.

## User Journeys

### Land on what needs a decision

A developer with several sessions in flight steps away and comes back.
The first thing the dashboard shows is the short list of sessions
waiting on them -- an agent stopped for input, a run that failed --
ordered with the most urgent on top. They act on the top one without
scanning past anything that does not need them; if that list is empty,
they see that everything is clear and move on.

### Recognize a session by its work

A developer wants to check on a specific piece of work -- the change
they kicked off earlier. They scan the session list and find it by
what it is, reading a label that names the work, rather than mapping a
cryptic id back to the task in their head or opening sessions one by
one until they hit the right one. A session names itself from the
moment it starts, not only once someone fills in a description.

### Trust that the quiet ones are really quiet

A developer's attention stays on the live set because the finished,
idle, and abandoned sessions have receded on their own -- they are not
filtered around by hand every time, and, just as important, a run that
actually died is not quietly receding among them but surfaces as
something that needs a look. When the developer does want history, the
receded set is one deliberate step away, not gone.

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

- **The staleness model.** How "alive vs dead" is decided. The signal
  is time-since-last-activity rather than since-creation, but the PRD
  must settle the shape: a grace window before a quiet session is
  called stale (so a brief pause does not flap), how the threshold is
  chosen (fixed, per-workflow, or configurable), and -- the
  load-bearing rule -- that a session legitimately parked on a gate
  must not be classified as dead just because it has been quiet.
- **"Waiting on you" as a state.** Whether the dashboard's lead state
  -- a session that needs a human decision -- is a status the session
  records, or one the dashboard derives from the event log at read
  time. This decides how much, if anything, gets written to the
  session versus computed on display.
- **Recede by decay vs. filter, and the dropped sessions.** Whether
  finished and abandoned sessions leave the default view by aging out
  on their own (auto-archive) or by a filter applied each time -- and,
  relatedly, why so many session directories are silently dropped from
  the list today (a parsing bug to fix, or dead cruft an auto-archive
  would carry off anyway).
- **The label chain.** How a session names itself: deriving a label
  from what already exists (workflow, current state, a distinguishing
  variable), and/or giving the descriptive field a sensible default at
  session start so it stops being empty -- with a fallback order that
  guarantees a row is never just a bare id.
- **Ordering and grouping the attention set.** How the needs-you set
  is ranked (by urgency and age), and whether related sessions fold
  together -- for instance a blocked parent standing in for the
  children waiting on it -- so the set stays short.
- **Pull vs. push.** Whether "what needs me" is only the dashboard a
  developer opens, or whether the strongest case -- a session blocked
  on a decision past the grace window -- also earns a single,
  sparingly-used notification.

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
