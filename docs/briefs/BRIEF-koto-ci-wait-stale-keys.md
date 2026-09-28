---
schema: brief/v1
status: Accepted
problem: |
  Agents wait on CI and clear a state's stale context keys before a retry by
  following prose, because koto can do neither. A skipped or botched step
  sends an agent to fix a build that was only pending, or lets a stale key
  pass a gate on the retry.
outcome: |
  A template declares the wait and the keys and koto does both: a tick says
  whether the check is done, failed or pending, and a retry into a state
  starts with that state's keys cleared and logged. The prose can go.
---

# BRIEF: koto owns the CI wait and stale-key clearing

## Status

Accepted

This brief frames two engine features that let a template stop asking the
agent to run a protocol step by hand. The downstream PRD owns the
requirements, along with three framing questions this brief leaves open.
None of them blocks the framing.

- **Does a pending polling gate hold the agent's turn or return?** koto's
  wake file only rings on request-store changes, so nothing would wake a
  session when CI finishes.
- **Which entries into a state clear its keys?** A self-transition, an
  override followed by a re-check and a rewind each need an answer, and so
  does whether clearing shares the boundary the visit attempt count uses.
- **How does a command say "pending" as distinct from "failed"?**

## Problem Statement

koto enforces a workflow's order with gates, but two of the steps that make
those gates trustworthy are still the agent's job, written into directive
prose.

The first is waiting. A check that settles later than the tick that asks
about it -- CI on a pull request is the common one -- has no gate shape that
says "not yet". A command gate runs once and passes or fails. So a template
gates on a one-shot `gh pr checks` call and tells the agent, in prose, to
keep polling and re-ticking until the checks are green, how long to wait, and
what to do when something is still running. An agent that misreads "still
running" as "failed" goes off to fix a build that was never broken; one that
stops polling early leaves the run parked.

The second is clearing. A state whose gate asks whether a context key exists
(a review verdict, a summary, a test report) passes the moment the key is
there. When the workflow loops back into that state for a retry, the key
from the previous attempt is still there, so the gate passes on stale work
unless something removes it first. Today that something is an executable
block the agent is told to run before each retry: remove each key, then
check it's really gone. The block is copied into every retry path of the
templates that need it, and it is the largest single piece of protocol prose
those templates carry. A retry path that misses a copy, or an agent that
skips the block, turns a failed check into a silent pass.

Both steps are mechanical. Neither needs judgment, and both are exactly the
kind of thing an engine does more reliably than a model reading
instructions. Until koto does them, the prose can't be deleted.

## User Outcome

A template author declares, on the state, which context keys belong to that
state's attempt, and koto clears them whenever the workflow enters the state
again -- a loop back from a fix, a retry on the same state, a rewind. The
clearing is one record in the session log, so someone reading the log can
see that the retry started clean.

The same author declares a gate that koto re-evaluates on an interval until
the command reports done, failed or still pending, with a deadline. The
agent ticks and learns which of the three it is: done moves the workflow on,
failed comes back with the command-gate failure payload koto already reports,
and pending is reported as a wait, not a failure the agent should fix.

Neither feature knows anything about a particular forge or needs a
credential: the author's command decides what "done" means. Templates that
use neither feature behave exactly as before, and an older koto can still
read the logs.

## User Journeys

### A retry loop that starts clean

A template author maintains a workflow where a review state gates on a
verdict key. When review fails, the workflow routes to a fix state and back.
The author adds the verdict key to the review state's clearing list and
deletes the remove-then-verify block from the review directive. The next run
that fails review and loops back finds the gate blocking on a missing verdict,
and the session log shows one clearing record on the re-entry.

### An agent waiting on CI

An agent running a workflow reaches a state that gates on a CI-status command
the template declares as a polling gate. It ticks. CI is still running, so
koto reports the gate as pending with a hint of when to tick again, and the
agent waits instead of trying to fix anything. On a later tick CI has
finished: green moves the workflow on, red returns the failed check's
output through the usual failure payload, and the agent starts the repair.

### A CI repair loop

After a red run, the agent pushes a fix and the workflow loops back into the
CI-watching state. The deadline for "still pending" restarts with the new
entry, the attempt counts go up, and nothing in the template tells the agent
how to watch CI.

### A log reader after the fact

Someone exporting session logs wants to know why a gate passed on the third
attempt. The log shows each re-entry, the keys cleared on it, and each
evaluation of the polling gate with whether it was pending, failed or done,
using fields documented in the session-feed contract.

## Scope Boundary

**In:**

- A per-state declaration of context keys koto clears when the workflow
  enters that state again, the rule for which entries count, and the one log
  record per clearing.
- A polling gate over a command that reports done, failed or pending, with an
  interval and a deadline, whose failure goes through the existing
  command-gate failure payload and whose attempts are counted but not capped.
- The session-feed contract text for every new field or event, and
  compatibility with koto v0.14.1 reading the new logs.
- A written map of which prose in shirabe's koto templates each feature makes
  deletable, so a later adoption is mechanical.

**Out:**

- Any change to shirabe. It adopts these features later, including raising
  the koto version it requires.
- A built-in CI or forge gate. koto stays free of any forge and holds no
  credentials; the command the template names is where GitHub lives.
- Retry caps and escalation. Attempts are reported, not limited; enforcing
  caps is separate work.
- Routing on variable values, and any registry of rules.
