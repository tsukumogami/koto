---
schema: brief/v1
status: Draft
problem: |
  koto can't reliably say which session dispatched a session on the same host:
  the automatic link has stopped being written on the reference host, the
  working route is a manual step the agent has to run, and a session nobody
  dispatched has no defined record, so "no parent" reads as "the link failed".
outcome: |
  For any session started on a host, a reader of koto sees the session that
  dispatched it, or an explicit record that it has none, without the agent
  doing anything, without its koto calls costing more, and without koto or
  shirabe needing a workspace launcher to be installed.
---

# BRIEF: Where a session came from

## Status

Draft

This brief frames the first piece of a larger effort to read a fleet of agent
sessions from what koto records. It covers only the link from a session to the
session that dispatched it. Seats, roles and any view over the records are
later work and are out of scope here.

## Problem Statement

A coordinating agent session hands work to other sessions. On this project's
reference host it does that through a workspace launcher (niwa), which starts
each worker as a fresh coding-assistant session in its own checkout. Anyone
who later wants to know how the work is organized, a person or a program,
needs to ask koto one simple question about a session: who started it?

koto has a place for the answer. A session can carry a dispatched-by link
naming another session, `koto init` and `koto session link` both write one,
and koto's decision records already read the nearest such link as the
decider's dispatcher. Each agent session also gets an anchor session from the
koto-skills start hook, and that anchor is meant to carry the link
automatically: the hook names, as the dispatcher, whatever anchor the new
session inherited in its environment from the session that launched it.

On the reference host the automatic link has stopped being written. Workers
dispatched from a coordinator now get anchors with no dispatcher, and the only
route that still works is a manual one: the worker's brief tells its agent to
run a script that writes the link once its own session is open. That spends
the agent's tokens and turns, and it depends on every brief carrying the step
and every agent running it. A worker whose agent skips it is indistinguishable
from one that has no dispatcher.

The launcher already knows the answer. It puts the parent session's id, among
other facts, into each worker's environment. To find the dispatcher, the
start hook reads only the inherited anchor name, and the launcher's own
variables for the parent go unused.
koto and shirabe also run on machines with no launcher at all, and neither may
come to depend on one, so the answer can't be "ask niwa".

A session nobody dispatched has no defined shape either. A person who starts a
shirabe session by hand, with or without the launcher installed, gets a record
that looks the same as a dispatched session whose link failed. A reader can't
tell a session with no parent from a session whose parent was lost.

## User Outcome

Anyone reading koto on a host can tell, for every session started there,
which session dispatched it, whether the launcher started it or a person did.
A session nobody dispatched is recorded as having no parent, on purpose, and a
reader can tell that apart from a link that failed to be written.

The agent in the session does none of this. It writes nothing new, says
nothing about its dispatcher, and its koto calls cost what they cost before.
A machine with koto and shirabe and no launcher gets the parentless record
without an error, a warning or a prompt to install anything.

## User Journeys

### Ask koto who dispatched a worker

A coordinator on the reference host dispatches a worker through the launcher.
Later the operator asks koto about the worker's session. Today the answer is
usually nothing, unless the worker's agent ran the manual link step. The
outcome is that the coordinator's session is recorded as the dispatcher, every
time, ten dispatches in a row.

### Start a session by hand with no launcher

A person on a machine with koto and shirabe installed, and no launcher, starts
a shirabe session from a plain directory. Today nothing says whether that
session has a parent. The outcome is a record that says it has none, reached
with no error and no prompt to install anything.

### Check that the agent paid nothing for it

A reviewer compares measured reads of a worker run taken before and after the
change. The outcome is that the cost of the agent's koto calls is unchanged
beyond run-to-run variation, and that the agent's transcript holds no text
about a dispatcher: no instruction to link one, and no command that does.

## Scope Boundary

In scope: how the thing that starts a session hands its dispatcher to koto;
the name and shape of what koto records for a session's dispatcher, including
the record for a session with none; why the automatic link stopped being
written on the reference host, and whether its repair is the same change;
and whether the launcher has to change at all.

Out of scope: seats, roles, holders and links between a session and its
successor; any reader, listing or view over these records; a coordinator
adopting a session that has no recorded parent; sessions started on another
machine; and any workspace notion in the record, since a session's origin here
is its dispatcher and nothing else.

## References

- `docs/designs/current/DESIGN-session-retention.md`, which introduced the
  per-agent-session anchor, and `src/session/anchor.rs`.
- `plugins/koto-skills/hooks/session-start.sh`, the start hook that opens the
  anchor and is meant to write the automatic link.
- `koto session link`, the verb the manual workaround runs.
