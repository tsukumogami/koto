---
schema: brief/v1
status: Accepted
problem: |
  koto records each succession and each dispatch only on the newer or lower
  session's side, so a reader that wants a seat's past and present holders,
  or the sessions that report to it, has to read every session in the store,
  and a holder is known only by its assistant session id, with no name.
outcome: |
  A seat's holders, oldest to newest, each named by its assistant session
  name with when it held the seat, and the sessions that report directly to
  it, both read in time that depends on that seat's history and reports and
  not on the size of the store, with nothing needed from niwa.
---

# BRIEF: Reading a seat's holders and direct reports without a store scan

## Status

Accepted

This brief frames the problem in koto#662, #663, #664 and #666 for the
design that answers it, `docs/designs/DESIGN-koto-holder-and-reports-reads.md`.
It restates those issues and adds no requirement of its own: the need is the
issues', and what koto records to meet it is the design's question.

## Problem Statement

A seat is shirabe's read model over facts koto holds: its holders are koto
sessions joined by succession, and its direct reports are the sessions it
dispatched. koto writes each of those facts only in the newer or lower
session. A successor carries its `continues` link and a child carries its
`dispatched-by` link, but the predecessor and the dispatcher carry nothing
on the local store.

So walking a chain forward, finding a seat's members from its name, or
listing a session's reports means reading every session.
`koto session hierarchy` computes `successors` that way, one hop per call,
and no verb prints a whole chain (koto#662). No verb lists the sessions that
name a dispatcher either (koto#666). Each read grows with the store, not
with the seat.

A holder is also recorded only by its assistant session id. A rebind records
no actor (koto#664), and no record carries the assistant session's name
(koto#663), so a reader can't name a holder without a source outside koto.

## User Outcome

Given a seat, a reader gets its current holder and every past holder in
order, each named by its assistant session name with when it held the seat,
and gets the sessions that report directly to it. Both reads take time that
depends on that seat's history and direct reports, not on how many sessions
the store holds. An unknown seat gets a message, not an empty list or a long
wait.

## User Journeys

A fleet view asks for a coordinator seat that has been taken over more than
eighty times. It gets every holder in order, each with its assistant session
name and the current one marked, without one call per hop.

A manager seat with three workers asks for its reports and gets exactly
those three. A worker's seat asks and is told nothing reports to it.

## Scope Boundary

In scope: what koto records at succession and at dispatch so neither read
needs a scan, how the assistant session's name reaches koto, and the read
surface over those records.

Out of scope: the seat definition, which shirabe owns, and anything the
agent itself writes. Status comes from koto, agents write nothing new, and
their koto calls cost what they did.

## References

- koto#662, #663, #664 and #666, the issues this brief restates.
- `docs/designs/DESIGN-koto-holder-and-reports-reads.md`, the design that
  answers it.
