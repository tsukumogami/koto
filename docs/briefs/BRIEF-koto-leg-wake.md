---
schema: brief/v1
status: Accepted
problem: |
  A coordinator session that parks on a `request-leg` gate has no way to
  learn that the leg changed. koto records the child's result on the leg
  and tells nobody, so the coordinator sits idle until something unrelated
  makes its agent tick, or it burns a turn polling with `koto request wait`.
outcome: |
  A coordinator author parks a session on a leg and walks away. When the
  leg resolves or is abandoned, the harness running the coordinator hears
  about it within a known, short time and ticks the session, without koto
  knowing which harness that is, and a missed or repeated signal costs nothing.
motivating_context: |
  koto's request legs let one session wait on another session's result, and
  a coordinator workflow that dispatches several workers and waits on their
  legs is now being built on top of them. The wake machinery koto already
  carries runs inside the waiting session's own tick and its only waker
  prints a line, so the one thing a waiting coordinator needs is missing.
---

# BRIEF: koto-leg-wake

## Status

Accepted

## Problem Statement

A koto request leg is how one session waits on another. A coordinator
creates a request, a worker session attaches to a leg, and when the worker
reaches its terminal state koto promotes the worker's result onto the leg.
The coordinator's template reads that leg through a `request-leg` gate and
stays blocked until the leg resolves.

Nothing tells the coordinator the leg resolved. The worker's terminal tick
writes the result to the request log and returns. The coordinator is a
different process, usually a different agent session, and it only learns
anything when it next runs `koto next`. Its agent has no reason to run that
command: from the agent's side the workflow said "blocked" and there is
nothing to do.

koto does carry wake machinery, but it cannot close this gap as it stands.
The wake-candidates pass runs inside the waiting session's own `koto next`,
so even a real waker there could only fire after the coordinator is already
awake. And the one waker the crate ships logs "not yet wired" and returns.

What that leaves coordinator authors with is two bad choices. They can let
the session sit until an unrelated cue (a human message, a timer) makes the
agent tick, which puts an unbounded delay between a worker finishing and
the coordinator reacting. Or they can hold the agent's turn open with
`koto request wait --leg <name> --timeout-secs <N>`, which watches one leg
at a time, blocks the agent from reacting to anything else (including
harness messages from other workers), and still needs a timeout guess.

## User Outcome

A coordinator author parks a session on a `request-leg` gate and stops
thinking about it. When any leg that session waits on resolves, by the
worker's own terminal tick or by any other route, or is abandoned, the
harness running the coordinator is told within a documented, short time
and ticks the session. The author wires this up once, with whatever their
harness already offers for noticing that something happened, and koto
does not need to know which harness that is.

The author also stops worrying about the signal itself. If a signal is
missed, the coordinator still finds the leg's state on its next tick; if
it arrives twice, the second tick finds nothing new. Where no harness is
listening at all, `koto request wait` with a timeout remains the documented
way to wait, so nothing that works today stops working.

## User Journeys

### A coordinator parked on a worker's leg is woken by the worker finishing

A workflow author runs a coordinator session in an agent harness. The
coordinator has dispatched a worker session bound to a request leg and its
template now blocks on that leg's `request-leg` gate. The trigger is the
worker session's terminal tick, which promotes its result onto the leg.
Within the documented bound, the harness running the coordinator is told
the coordinator should look again; it ticks the coordinator, the gate
passes, and the workflow moves on without the author doing anything.

### A leg abandoned by someone else still wakes the coordinator

A coordinator author's session is parked on a leg when someone else, an
operator or a newer run of the same topic, abandons the request. No worker
ever finishes. The trigger is the abandonment write, not a terminal tick.
The author's coordinator is told to look again just as it would be for a
result, ticks, and routes on the abandoned disposition instead of waiting
forever for a worker that will not answer.

### A harness integrator subscribes without koto knowing the harness

A maintainer wiring koto into an agent harness wants coordinators woken.
The trigger is reaching the point of wiring wake delivery into the
harness's own event loop. They read koto's
documentation, find the one thing koto produces when a leg changes and how
to watch for it, and connect it to whatever their harness uses to notice
events (a file watcher, a background command whose exit re-invokes the
agent). No koto configuration names their harness, and a second harness
could subscribe to the same signal the same way.

### A coordinator with no subscriber falls back to waiting

A coordinator author runs in an environment with nothing listening for
wakes, such as a plain shell script driving koto. The trigger is the
coordinator blocking on a leg with no harness to wake it. The author
follows the documented fallback, `koto request wait --timeout-secs`, and
gets the same behaviour they have today; the new signal is written but
nothing depends on it being read.

## Scope Boundary

**In:**

- A wake that originates when a leg resolves by any route (a promoted
  worker result, an explicit resolve, a refusal koto records) or is
  abandoned, including abandonment of the whole request.
- Delivery of that wake to the sessions a request names as its requester
  and coordinator of record, on the same machine.
- A documented way for a harness to subscribe to the wake without koto
  knowing which harness it is, with a documented bound on how soon after
  the leg change a subscriber hears of it.
- Harmlessness of lost and duplicate wakes: a wake carries no state, only
  "look again", and the coordinator resumes from its own log either way.
- Documenting `koto request wait --timeout-secs` as the fallback when no
  subscriber is present.
- Replacing the logging-only waker as the default in the coordinator's
  tick, or keeping it only as an explicit no-subscriber fallback.

**Out:**

- Wakes across hosts. koto's session and request stores live under the
  user's home directory on one machine, and request records do not
  replicate under the cloud backend, so a coordinator on one host is never
  woken by a leg resolved on another.
- A harness-specific integration inside koto: koto does not call into any
  particular agent harness, message bus, or notification service.
- Changes to the coordinator workflow that consumes the wake. That
  workflow lives in another project and adapts after this lands.
- Carrying the leg's result, or any other state, inside the wake. The
  coordinator reads state from the request store, never from the wake.
- A long-running koto daemon or service. koto stays a set of short-lived
  commands.
- The cloud session backend. Request records are local-only today and this
  feature does not change that.
