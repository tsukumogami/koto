---
schema: brief/v1
status: Accepted
problem: |
  koto's S3 remote stores a session but can't move one. The remote keys
  each session by the workspace that created it, `rebind` is undone under
  the cloud backend, context keys never follow a session anywhere, and the
  guide still tells users a second machine picks up by running `koto next`.
outcome: |
  An operator on a second host brings a session over with one command,
  state and context together, and keeps working there. Any command on the
  first host that reads the session says it moved and where, so nothing
  there continues it by accident.
motivating_context: |
  koto#313 records the maintainers' model for multi-host use: a session is
  never shared live across hosts; it is migrated, by import, and the old
  object is marked. A measurement on koto 0.15.0 moved a session between
  two workspaces through a MinIO remote and found that none of the
  documented paths carried its context keys or its anchor.
---

# BRIEF: Session migration

## Status

Accepted

## Problem Statement

koto can keep a session in any S3-compatible bucket, and the guide sells
that as a way to resume work on a different machine. In practice a session
can't get from one host to another, and every route a user would try
fails in a different place.

The remote keys each session under a prefix derived from the working
directory of the workspace that created it. That is the session's identity
on its host, and the maintainers mean to keep it (koto#309 was closed as
intended), but it means a second host, or a second workspace on the same
host, looks under its own prefix and finds nothing. The one command that
moves a session, `koto session rebind`, appends its event through the cloud
backend and then rewrites the header locally without pushing it, so the next
read pulls the stale header back and every `koto next` refuses on the old
anchor (koto#310). Even if the anchor stuck, `rebind` moves the state file
only. The context keys, which is where a coordinator keeps the things a
replacement needs (its workers, its holdings, its standing answers), stay
under the first workspace's prefix and never arrive. A host with no local
session directory can't receive a pull at all, because nothing creates the
directory first.

Three more things sit underneath and would bite a working import. A
non-failure terminal tick deletes the session's whole remote prefix. Session
names are machine-wide, so a session arriving on a host that already has one
by that name has nowhere to go. And the session's first event records the
compiled template's absolute path in the origin host's cache, a file the
second host doesn't have.

The guide (`docs/guides/cloud-sync-setup.md`) describes none of this. It says
machine B runs `koto next` and picks up where A left off, it documents a
`--project` flag and an `allow_insecure` key that don't exist, its MinIO
example can't work because koto addresses buckets virtual-host style, and it
lists three of the eight commands that talk to the remote (koto#312). A
first-time user who follows it ends up with a second copy of their session
stuck at the first host's anchor and no way to tell why.

There is also nothing that stops two hosts from advancing the same session.
The version counter in `version.json` guards context writes only: a state
pull overwrites the local state file whole, with no version check, and
state-log writes take no lock (koto#239). The cheapest form of "multi-host"
(two hosts both ticking one session) is the one most likely to corrupt it,
and `koto session resolve` can only pick a side after the fact.

## User Outcome

An operator who has to continue a session somewhere else (a coordinator
restarted on a new machine, or a workspace moved to a fresh checkout)
brings the session over with one command and keeps working. The session
they get holds everything the original held: the state log and every
context key, byte for byte. It runs where the operator brought it, and from
then on it belongs to that workspace the way a session started there would.

Nobody on the first host continues the session by mistake. Any koto command
there that reads it stops and says where the session went. A user who reads the guide learns
that moving a session is an import, sees the steps that actually work, and
isn't told a second machine resumes on its own.

## User Journeys

### An operator moves a coordinator's session to a second host

A coordinator session has been running on one host with the cloud backend
on. The host is being retired, so the operator configures the same bucket on
a second host and runs the import there, naming the session and where it
came from. koto creates the local session, anchors it in the operator's
working directory, and reports what it carried. The operator runs
`koto context get` on the coordinator's keys and gets the same bytes the
first host stored, then runs `koto next` and the workflow advances from the
state it was in.

### An agent on the first host touches a migrated session

After the import, an agent on the first host, which never heard about the
move, runs `koto next` on the session as part of its loop. koto reads the
remote object, finds the migrated marker, and refuses with an error that
names the newer session and the workspace it now runs in. The agent's
caller sees a refusal instead of two copies of the session advancing in
parallel. If the first host is offline when the marker is written and later
resumes from its local copy, that is the risk the maintainers accepted; the
marker is the only lock.

### A maintainer checks that the carrier still works

A koto maintainer changing the cloud backend wants to know whether a
session still survives a move. They run the measurement harness that ships
with the feature. It starts a session in one workspace, stores context
keys, imports the session into a second workspace that looks like a
separate host, advances it there, compares every key byte for byte, and
checks that the first workspace's `koto next` now refuses. The harness
reports pass or names the step that failed.

### A first-time user sets up cloud sync from the guide

A user wants their sessions backed up to a self-hosted MinIO. They follow
the guide: every flag and key it shows exists, it tells them how to reach a
bucket on an IP-address endpoint, it lists every command that talks to the
remote, and it says plainly that sessions belong to the workspace that made
them and move between hosts by import. They don't try `rebind` on a second
machine and they don't expect `koto next` there to find anything.

## Scope Boundary

**IN:**

- A koto verb that imports one session from its S3 object (state file and
  context keys both) into a fresh local session on this host, anchored where
  it runs, creating the local session directory itself.
- A migrated marker on the source object that points at the newer session,
  and a refusal, naming the newer session, from every koto command that
  reads a migrated source.
- What an imported session needs to run on its new host: an anchor that
  survives the cloud backend (koto#310, fixed or made moot by the import
  path), the compiled template it was started from, and a rule for a name
  that's already taken on the importing host.
- An answer for the remote prefix a terminal tick deletes, as it bears on a
  source being imported and on the imported session.
- The state log's safety while an import reads it: either koto#239's lock or
  a stated interim rule that an import runs with the source session stopped,
  with the lock named as in scope or deferred with its reason.
- A repeatable measurement harness, in koto, for a session moved between
  two workspaces.
- Corrections to `docs/guides/cloud-sync-setup.md` covering every gap
  koto#312 lists, and the guide's account of moving a session, with dated
  notes on the two cloud designs where this changes what they say.

**OUT:**

- A session shared live between hosts. Two hosts never advance one session;
  the feature moves a session, it doesn't synchronize one.
- Preventing the original from resuming on a host that was offline when the
  marker was written. The maintainers accepted that risk; the marker is a
  flag, not a distributed lock.
- Carrying requests, wakes, request legs or the decider ledger. They don't
  replicate today and the import doesn't make them.
- Moving a session's child sessions, or a batch parent's children, with it.
- Listing sessions by execution directory, pruning finished ones, or a
  terminal signal for sessions that vanish from `status` (koto#308,
  koto#162, koto#234), beyond what the import itself needs.
- A human-readable view of a session's context keys and their contents.
- How any orchestration layer above koto decides to call the import.
- A managed or hosted store, or any remote other than koto's self-managed
  S3 bucket.

## References

- koto#313: the import verb and the migrated marker
- koto#310: `rebind` undone under the cloud backend
- koto#312: the guide's gaps
- koto#239: unlocked state-log writes
- `docs/designs/current/DESIGN-config-and-cloud-sync.md`
- `docs/designs/current/DESIGN-backend-state-persistence.md`
- `docs/guides/cloud-sync-setup.md`
