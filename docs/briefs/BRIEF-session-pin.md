---
schema: brief/v1
status: Accepted
problem: |
  koto keeps a finished session only while something live holds it, then
  removes it at the next cleanup, prune or terminal tick, on every host
  sharing a cloud bucket. A session that is the record of finished work has
  no way to be kept, so a later reader loses it on a schedule nobody chose.
outcome: |
  Whoever knows a finished session matters marks it to be kept, with a
  reason, and stops worrying about it: no removal koto applies takes it on
  any host that shares the store, each removal that passes it over says why,
  and it goes only when someone allowed to release the mark does so.
motivating_context: |
  Session retention now keeps a finished session while any member of its
  hierarchy is alive, and for the retention period after. That covers the
  sessions a live coordinator still reads. It doesn't cover a record that
  matters after the whole hierarchy is gone, which is the case an issue
  (tsukumogami/koto#441) asks for.
---

# BRIEF: Pin a finished session so no removal rule takes it

## Status

Accepted

## Problem Statement

Some koto sessions matter after their work is over. A session that drove a
piece of work to a merged pull request holds what was done, in what order and
with what results, and its log and keys are the record a reviewer, an
auditor or a later agent reaches from that pull request weeks afterwards.

koto has no way to keep such a session. Session retention keeps a finished
session only while something live holds it: an open anchor with a held-by
link to it, or a live member anywhere in its hierarchy, and then for the
retention period after the hierarchy's release instant. Once the agent
sessions involved are gone and the retention passes, the next
`koto session prune`, `koto workspace prune`, terminal tick, subtree removal
or `koto session cleanup` takes it. Under the cloud backend that removal
deletes the session from the bucket, so it's gone for every host that shares
the bucket, including hosts that never ran the work.

The person or agent who knows the record matters can't say so in any form koto
reads. Their options today are to keep an agent session open forever so its
anchor holds the record, to keep tick commands from ever pruning, or to copy
the session out of koto by hand. None of these is a statement that this
session should stay, and none survives an operator's routine prune on a
second host.

## User Outcome

An agent or person who finishes work worth keeping marks the session to be
kept and says why, in one step, and then doesn't have to think about it
again. Every removal koto makes by rule or subtree, on this host or on any
other host sharing the cloud bucket, passes the session over and says it did
so because of that mark, naming the reason given. An operator who runs a
prune can see at once which kept sessions are kept on purpose and why, and
tell them apart from sessions kept only because something is still alive.

The mark stays until someone deliberately releases it. Idle time, staleness
and the retention period never take it off. When it's released, the session
goes back to the ordinary rule, and the next removal that applies takes it. A
release by someone who isn't allowed to release that mark is refused, and the
refusal says what they'd need. A mark koto can't read keeps its session
rather than freeing it, and the removal says the mark couldn't be read.

Nothing changes for sessions nobody marked: they're kept and removed exactly
as they are today. And all of this works on a host with koto and no niwa.

## User Journeys

### Keeping the record of finished work

An agent driving a piece of work to a merged pull request reaches the end of
its koto run. It knows a later reader will follow the pull request back to
this session's log, so it marks the session to be kept with a reason that
names what the session is the record of. (Whether setting a mark needs any
authority is for the requirements to say.) The agent session ends, its anchor
closes, the retention passes, and weeks later a reviewer opens the session
from the pull request and reads its log and keys as they were left.

### An operator's routine prune on a second host

An operator on a second host that shares the cloud bucket runs
`koto session prune --apply --all` to clear out old sessions. Finished
sessions nobody marked, past their retention, are removed from the bucket as
before. The marked session is listed as kept, with the mark and its reason in
the listing, and it's still in the bucket afterwards for the first host and
any other to read.

### Releasing a record nobody needs any more

A maintainer who owns the work a marked session records decides, months
later, that the record isn't needed. They release the mark. A prune run after
that treats the session like any other finished session and removes it once
its hierarchy is released and the retention has passed. A worker agent that
tries the same release without being allowed to is refused, and the refusal
names what it lacks.

### A mark that can't be read

An operator runs a prune after a marked session's mark was damaged, for
example a value truncated by a failed write. The prune keeps the session, and
its listing says the session is kept because its mark couldn't be read, so
the operator knows to repair or release it rather than guess.

## Scope Boundary

**IN:**

- A way to mark a finished koto session to be kept, with a reason, and to
  release that mark explicitly. Whether one mark covers a single session or
  its whole hierarchy is left to the requirements.
- The mark is a keeper beside the ones session retention already counts: it
  keeps its session the way a live member does, with no release instant.
- Every removal koto makes by rule or subtree honors it: the terminal tick,
  the parent sweep, the same-name replace, `koto workspace prune`, and
  `koto session prune`. So does `koto session cleanup`, which today removes a
  session whatever retention says; a marked session survives it.
- Each removal that keeps a marked session says so, naming the mark and its
  reason, in the same places it already names live members.
- The cloud backend: a removal on any host sharing the bucket keeps the marked
  session and says why, including where the anchors that hold sessions stay on
  a host's local store while the sessions they hold live in the bucket.
- Failing toward keeping: an unreadable mark keeps its session and is
  reported.
- Who may release a mark follows the authority a session holds, handed to
  the session the way its anchor name is. koto never reads another tool's
  record to decide it. Who may release a mark, how that authority reaches
  koto, and whether setting one needs any authority are left to the
  requirements.
- The whole feature on a host with koto and no niwa.

**OUT:**

- Releasing a mark by idle time, staleness, age or any rule. Only an explicit
  release takes it off.
- Changing how sessions nobody marked are kept or removed. Session retention's
  rule, its settings and its reasons stay as they are for those sessions.
- An automatic pruner, or any change to when removals run.
- Keeping host-level stores (the request store, the decider ledger, the
  terminal index, finished records). The mark is about koto sessions.
- Defining the authority itself: what authorities exist, who grants them and
  how a dispatcher passes them on. This feature reads what's handed to a
  session and records who released a mark; the authority work owns the rest.
- Enforcement stronger than today's. A person at a shell with direct access to
  the session store can still delete files; the mark governs what koto's own
  commands do.
- Making koto require niwa, or niwa require koto, to set, read or release a
  mark.
- Keeping a session that koto's batch machinery removes and respawns inside a
  run it's still driving. Those removals are part of a live run, not removals
  of finished sessions.
