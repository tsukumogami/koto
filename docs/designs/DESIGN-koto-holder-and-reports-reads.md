---
schema: design/v1
status: Draft
problem: |
  A reader that wants a seat's past and present holders, or the seats that
  report to it, can only get them from koto today by reading every session in
  the store, because koto records each succession and each dispatch only on
  the newer or lower session's side. The cost grows with the store, not with
  the seat, and the holder of a session is recorded as an assistant session
  id with no name.
---

# DESIGN: Holder and Direct-Report Reads Without a Store Scan

## Status

Draft

## Context and Problem Statement

A seat is the position a dispatcher or a person creates, filled in turn by
koto sessions called its holders. The seat definition decision, built on the
coordinator seat design in tsukumogami/shirabe#1762, treats a seat as a read
model that shirabe's seat reader assembles from facts koto already holds: a
coordinator seat is named `coordinate-<SLUG>`, that name is the lineage, and
koto records facts, never a seat record.

Two of the reads that model needs can't be answered by koto today without
reading every session on the host:

- **A seat's holder history.** Given a seat, the current holder and every
  past holder in order, each named by the coding-assistant session that held
  it and when. A successor records `koto/links/continues/<PRED>` in its own
  context, and a coordinator takeover writes `session_superseded` into the
  predecessor's log. Walking backward from the newest member is a read per
  hop. Walking forward, or finding the members of a seat from its name, isn't:
  `koto session hierarchy` computes `successors` by listing every session and
  reading each one's links (tsukumogami/koto#662). The only forward pointer
  koto writes is the cloud back-reference under the predecessor's
  `ctx/koto/continued-by/`, which no read verb uses for a walk.
- **A seat's direct reports.** Given a seat, the seats dispatched from it.
  A dispatched session records `koto/links/dispatched-by/<DISPATCHER>` in its
  own context, so finding every session that names a given dispatcher means
  reading every session (tsukumogami/koto#666).

The name the milestone asks for isn't recorded at all. koto reads
`CLAUDE_CODE_SESSION_ID` and keeps the id in its anchors' names and in the
host-wide run journal, but not the session's name (tsukumogami/koto#663). A
holder that changes by rebind leaves an `execution_anchor_rebound` event with
directories and no actor, so a session's holders before the latest are only
in the host-local journal (tsukumogami/koto#664).

The work in scope is what koto records at the two moments it already writes
these links, succession and dispatch, and the reads that use those records,
so that each read costs a function of the seat's own history and reports. Out
of scope: the seat definition itself, how shirabe's paths come to open a
linked successor instead of rebinding (tsukumogami/shirabe#1738, #1739,
#1740), and any change in niwa.

## Decision Drivers

- **Cost follows the seat, not the store.** Each read is bounded by the
  number of holders in the seat's history and the number of its direct
  reports, and stays flat as unrelated sessions are added.
- **The agent writes nothing new and its koto calls cost what they did.**
  Anything recorded is written by koto inside a call the agent or launcher
  already makes, or by the launcher; no new agent call, no new agent-written
  key, and no growth in output an agent reads.
- **No niwa.** A machine with no niwa names every holder, and the reads take
  nothing from niwa's attributes or files.
- **Facts, not a seat record.** koto records facts about sessions (links,
  ids, names, times); the seat stays a read model in shirabe.
- **Both backends.** The local store and the cloud bucket answer the same
  read, and a record written under one survives a push to the other.
- **An unknown seat is an answer.** A name nothing holds gets a message, not
  an empty list and not a long wait.
- **Reuse what exists.** The link layout, the reserved `koto/` area, the cloud
  back-reference pattern, the bucket's name index and the
  koto-session-reader crate are the building blocks, before anything new.

## Considered Options

Not written yet: a later /design phase replaces this line.

## Decision Outcome

Not written yet: a later /design phase replaces this line.

## Solution Architecture

Not written yet: a later /design phase replaces this line.

## Implementation Approach

Not written yet: a later /design phase replaces this line.

## Security Considerations

Not written yet: a later /design phase replaces this line.

## Consequences

Not written yet: a later /design phase replaces this line.
