---
schema: brief/v1
status: Accepted
problem: |
  On the cloud backend, a `--supersede --carry` of 1024 keys takes about 18
  minutes, a run that fails partway gives up all its progress, and the exit 3
  reads as a lock problem, so on an unstable network a takeover of a long
  session may never finish while the predecessor is already fenced.
outcome: |
  A takeover carry of a long session on the cloud backend finishes in
  reasonable time; a re-run after an interruption keeps the keys an earlier
  run already carried; and a failed carry says it failed on the network,
  not on a lock.
---

# BRIEF: Takeover carry on the cloud backend

## Status

Accepted

This brief frames the problem in koto#623 for the design that answers it,
`docs/designs/DESIGN-takeover-carry.md`. It restates the issue and adds no
requirement of its own: the expectation is the issue's, and the open
questions are all about how to meet it.

## Problem Statement

A takeover is how a long-running session hands its work to a successor:
`koto init <new> --continues <old> --supersede --carry <prefix>...` fences the
old session, creates the new one and copies the context keys under each
`--carry` prefix. The sessions that most need a takeover are the ones with
the most keys.

On the cloud backend the copy is slow and fragile. The issue reports a carry
of 1024 keys that took about 18 minutes, with one HTTPS connection to the
bucket and almost no CPU, and the new session stayed at 2 keys until the
carry finished. Two earlier runs of the same call had exited 3 after about 6
and about 14 minutes, and neither carried anything: the next run carried all
1024 keys from the start. The network was intermittent at the time.

At about one second per key, a session with a thousand keys needs an
unbroken quarter of an hour of bucket access, and one dropped request loses
all of it. The exit 3 that results reads like a lock problem, and the caller
in the issue reported it as `fence_unavailable`. On an unstable network,
retrying can keep failing while the predecessor is already fenced and the
successor holds nothing.

## User Outcome

Whoever runs a takeover of a long session on the cloud backend, a person or
a script, gets one that finishes in reasonable time. When a run is
interrupted, the next run keeps what the earlier one already carried rather
than starting from the first key. When a carry fails on the network, the
failure says so, and a caller that reads only the exit code can tell it from
a lock failure.

## User Journeys

### Hand over a long session

A session holding about a thousand keys is superseded with `--carry`. Today
the carry takes about a quarter of an hour of uninterrupted bucket access.
The outcome is a carry that finishes in reasonable time for that size.

### Re-run after a dropped request

A carry fails partway because one bucket read timed out. Today the re-run
starts again from the first key, so on an intermittent network it may never
finish. The outcome is a re-run that doesn't lose the keys an earlier run
already carried.

### Tell a network failure from a lock failure

A script runs the takeover and keeps only the exit code. Today a failed
carry exits 3, the same exit the script reads as `fence_unavailable`. The
outcome is a failure that says it failed on the network.

## Scope Boundary

In scope: the carry step of `koto init --continues --supersede --carry` on
the cloud backend, its time, what an interrupted run keeps, and how a
failed carry reports itself.

Out of scope: the fence and create steps that run before the carry, the
local backend's carry, and any other command's use of the bucket.

## References

- koto#623, the issue this brief restates.
- `docs/designs/DESIGN-takeover-carry.md`, the design that answers it.
