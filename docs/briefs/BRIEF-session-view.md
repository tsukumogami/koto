---
schema: brief/v1
status: Accepted
problem: |
  A person responsible for a koto session cannot read what it holds. The
  session's working state lives in its context keys, but `context list` prints
  only names, `context get` dumps one key's raw bytes, and `status` and the
  dashboard show position and directive without content, sizes, anchor or
  store origin.
outcome: |
  A person opens a session in a surface koto already ships — the dashboard or
  the workflow view — and reads its working state like a status page: state
  and directive, anchor and store origin, and every context key with its size
  and legible content, with absences and migrations said plainly.
---

# BRIEF: A human-readable view of a session

## Status

Accepted

## Problem Statement

A koto session accumulates working state as it runs: notes a long-running
workflow keeps for itself, artifacts one phase leaves for the next, records a
coordinating session maintains about the work it oversees. A person who needs
to know what a session is working with has to script their own loop over raw
dumps and hope nothing in the values wrecks their terminal, because all of
that state lives in the session's context keys and no surface koto ships lets
them read it.

`koto context list` prints key names and nothing else — no sizes, no hint of
what is behind each name. `koto context get` prints exactly one key's raw
bytes to stdout, which is unusable for a binary value, hazardous for a large
one, and tells a reader nothing about the rest of the session. `koto status`
and the dashboard answer a different question: where the state machine stands
and what its current directive says, not what the session has accumulated.
Nothing shows where a session is anchored or which store backs it without
reading state files by hand.

The people who most need to read a session — an operator checking on a
long-running run, a teammate picking up someone else's, a maintainer debugging
a stuck one — all hit the same wall. For a teammate taking over a session that
was migrated to another workspace, even the scripted loop fails: reads refuse,
and the surfaces that bypass the backend show holes instead of saying what
happened, so a correct migration reads like a broken session.

## User Outcome

A person reads a session's working state in a surface they already use, the
way they would read a status page: where it stands (state and directive),
where it runs (execution anchor and store origin), and everything it holds —
every context key with its size and its content in a form a person can read,
large or binary values presented by size and type with a bounded excerpt,
never a byte dump. The depth of the reading fits the surface they are in, from
a full per-key view where they focus one session to a compact account of its
holdings where sessions are listed; the journeys below walk the surfaces.

When a key is absent or unreadable, the view says so in place of the content.
When the session was migrated, the view names the newer session and its
workspace instead of failing. The person never has to choose between raw
bytes and no information.

## User Journeys

### An operator reads a long-running session's working state

An operator with several koto-backed workflows in flight opens the dashboard
and focuses one session. Beyond the state, directive and history the detail
pane already shows, they read the session's anchor, its store origin, and the
full list of context keys with sizes and content. They learn in one look what
the session has accumulated and how large it has grown, without running a
single `context get`.

### A session owner glances at the workflow view

A person driving work from a coding session comes back after stepping away and
wants to know whether any of their koto sessions needs attention. They open
the workflow view, where each koto session carries, alongside its phases, a
compact account of what it holds: how many keys, their names and sizes, where the session is
anchored, which store backs it. The person decides whether anything needs a
closer look — and takes that closer look in the dashboard, since the workflow
view stays a summary surface.

### A teammate inspects a session from a script

A teammate wants a one-shot, parseable reading of one session. They run the
dashboard's existing non-interactive mode with its single-session detail
option and get a bounded rendering of the session's facts and keys — sizes
always, content excerpted legibly — without the multi-session feed's columns
changing shape under their existing scripts.

### A teammate meets a migrated session

A teammate takes over a workflow and opens the session named in the handover,
not knowing it was imported into another workspace. The view tells them so:
the newer session's name and workspace, in place of content that no longer
lives here. They continue where the message says instead of debugging an
apparent failure.

## Scope Boundary

### In scope

- Reading one session's working state: its state and directive, its execution
  anchor and store origin, and every context key with size and content.
- Legible presentation of large and binary values: size and type always, a
  bounded excerpt where content can be shown, never raw bytes dumped to a
  terminal.
- Saying plainly when a key is absent or unreadable, and naming the newer
  session when the session was migrated.
- Working on local and cloud-backed sessions.
- Carrying the reading on the two surfaces koto already ships: the local
  dashboard (interactive and one-shot) and the workflow view's session
  rendering, each to the depth that fits it.

### Out of scope

- Any new top-level verb, new subcommand tree, or third rendering surface (a
  web page, a report file): the maintainers ruled the view rides existing
  surfaces (2026-10-10).
- Session hygiene: listing sessions by execution directory, pruning finished
  ones, signalling terminal cleanup (koto#308, koto#162, koto#234), or
  clearing migration markers the view encounters.
- Changing the dashboard's existing multi-session feed columns: scripts
  depend on their positions.
- Writing or editing session content: the view reads; `context add`/`remove`
  remain the write path.
- Key content in the workflow view's rendering: its file is a compact status
  projection written under the hosting coding session's directory, so it
  carries names, sizes and counts at most.
