---
schema: brief/v1
status: Accepted
problem: |
  On a host where many workflows share one koto store and bucket, a bulk prune
  can empty the store, a test that looks isolated can reach it, prune keeps
  sessions by name, and a read by name depends on its directory, can write into
  the session it reads, and refuses with advice to delete.
outcome: |
  An operator prunes without risking a store or a live lineage, a scratch-root
  test can't touch the real store, a successor reads its predecessor by name
  from its own directory without changing it, and no refusal advises deletion,
  so the host needs no freeze.
---

# BRIEF: Removal a lineage can trust

## Status

Accepted

Frames the work that makes koto's removals and reads safe enough for a host
to stop freezing them. Part of it has already landed on main; the Problem
Statement says which, so the PRD can hold only what's left.

## Problem Statement

koto keeps every workflow session on a host in one local store, and under the
cloud backend in one bucket that several hosts can share. A session is often
part of a lineage: a coordinator's run continued by a successor, a session
imported into another workspace, a hierarchy of children. Whoever removes or
reads sessions on that host, a person, a maintenance script, a test harness or
another agent, needs to know the command can't destroy what other work still
depends on, and that a read returns the truth without changing anything.

That trust broke on 2026-10-24. A test script meant for a scratch store ran
`koto session prune --apply --all` with a one-second retention against a host's
real store. It inherited the cloud backend from environment variables, so
setting a scratch home didn't help, and koto didn't ask before removing every
finished session no keeper held, each one's bucket copy included. About 700
sessions went. Since then the host has run under a freeze: nobody prunes,
cleans up, imports or deletes against the real store, and finished sessions are
read from archives, not from koto.

The freeze exists because of six gaps. Some are already closed on main:

- **A bulk removal didn't ask.** A short-retention, whole-store prune on the
  cloud backend ran without a look first. koto#470 now refuses it unless
  confirmed, names the retention, backend, bucket and store path, and prints
  the store before any removal.
- **Isolation followed the home directory.** Every koto path was built from the
  home directory, so `KOTO_HOME` did nothing. koto#471 now resolves every path
  from one root that `KOTO_HOME` moves, and `koto session dir` prints it. What
  it doesn't cover: a cloud backend configured through the environment still
  reaches the real bucket under a scratch root, and no test drives cleanup
  under one.
- **Prune keeps coordinators by name.** Any session whose name starts with
  `coordinate-` is kept forever, whether or not anything still needs it, while a
  session that some live successor depends on gets no protection unless its name
  happens to match. Nothing replaces the name with the fact that matters: a later
  session continues this one.
- **A read by name depends on where it runs.** Under the cloud backend the
  bucket copy a command reads is picked from the directory it runs in. A read
  from the wrong directory used to return a stale value silently; since
  koto#463 it refuses instead, and since koto#480 a pull no longer drops events.
  But a successor running in its own directory still can't read its
  predecessor, and a tick from a subdirectory still writes a stray copy under
  another prefix (koto#465, koto#466). Every lane on the host works around
  it by running each koto call from the run's bound directory.
- **A read can write into the session it reads.** After a lost store write, a
  `koto context get` restores the assigned value into the session it reads, from
  whatever process ran the read (koto#436). Reading a finished session is
  supposed to leave it byte-identical, so lanes avoid reading finished sessions
  at all.
- **Refusals point at deletion.** When `koto init --attach-live` finds a session
  from another worktree or store (`origin_mismatch`), its message tells the
  caller to remove it with `koto session cleanup`. That's the advice that
  destroys another workspace's live run.

## User Outcome

An operator who runs a store-wide removal on a shared host knows the command
can't empty the store by accident or take a session that a live lineage still
needs: it stops and says what it would hit unless confirmed, and every session
it keeps carries the reason, naming the successor that keeps it. A test author
points `KOTO_HOME` at a scratch directory, can confirm before the first command
that koto resolved a local store under scratch, and knows nothing the run does
can reach the real store. A successor session reads its predecessor's
keys by name from its own directory and gets the current value, and the
predecessor is byte-identical afterwards. An agent that hits a mismatched
origin is told how to go on without deleting anything. With those in place the
host-wide freeze has nothing left to protect against, and the lanes can drop
the habit of running every koto call from one directory.

## User Journeys

### An operator prunes the shared store

A person (or a maintenance script) on a host whose koto runs on the cloud
backend runs `koto session prune --apply --all` with a retention that came
from a test's environment. koto prints the store it's about to act on, sees a
whole-store selection with a retention under the floor, and refuses, naming
the retention, the backend, the bucket and the store path; nothing is removed
locally or in the bucket. The operator previews the set instead, or confirms
on purpose, and only what the retention rules release goes.

### A test harness runs in a scratch root

A QA seat sets `KOTO_HOME` to a fresh directory, asks koto where its root is
and which backend it resolved, and fails closed unless both are local and under
scratch. It then opens, ticks, prunes and cleans up sessions. Afterwards the
real store, the request store and the config are byte-identical to a listing
taken before.

### A prune keeps what a successor still needs

A coordinator's successor, started in a new instance, has recorded that it
continues the previous run's session. When a prune runs on the host, the
finished predecessor is kept, and the prune's report names the successor as
why. A finished `coordinate-` session that nothing continues, holds no pin and
has no live relative is removed like any other.

### A successor reads its predecessor from its own directory

The same successor, running in its own instance directory under the cloud
backend, reads a key the predecessor just changed by naming the predecessor.
It gets the new value, not a stale copy and not a refusal. A lane reading a
finished session the same way finds the session's log and bucket copy
byte-identical afterwards, so finished sessions can be read through koto
again instead of from archives.

### An agent hits a mismatched origin

An agent's `koto init --attach-live` finds a running session with the same
name, started in another worktree. The refusal tells it the name is taken
elsewhere and how to proceed (another name, or continuing that session from
here) and never suggests `koto session cleanup`.

## Scope Boundary

**IN:**

- The short-retention guard on bulk removals and the store line printed before
  removal, as landed in koto#470; this feature counts it as delivered and
  checks it against the outcome.
- Isolation by `KOTO_HOME`, as landed in koto#471, plus what it leaves open: a
  cloud backend configured through the environment that still reaches the
  bucket under a scratch root (whether koto stops it or only reports it so a
  harness refuses to start is the PRD's call), and a scratch-root run that also
  covers cleanup.
- Replacing prune's `coordinate-` name rule with the continues relation another
  feature adds, so a session is kept because a later session continues it.
- A read by name under the cloud backend that returns the session's current
  value from any directory, or refuses where the session can't be located,
  and the matching change so writes land on the session's own copy instead
  of a per-directory one.
- Removing the last path by which a read writes into the session it reads.
- Refusal text for `origin_mismatch` and `execution_anchor_mismatch` that names
  a way forward other than `koto session cleanup`, with a check that keeps it
  that way.

**OUT:**

- The continues relation itself: the verb that writes it, how retention counts
  it, the bucket walk and import keeping its source are a separate feature this
  one depends on.
- A coordinator's successor writing the relation at takeover, and lineage
  scripts reading it. That's the shirabe side of succession, not koto's
  removals.
- Lifting the freeze. That's the host owner's decision, made after this ships
  and a prune preview on the real store names a keeper for every live lineage.
- A guard on the local backend or on marker-mode prune (`--migrated-from`).
  koto#470 scoped the guard to removals that also delete bucket copies; this
  feature doesn't revisit that.
- Explicit single-session removal (`koto session cleanup <name>`,
  `koto cancel --cleanup`). A person naming one session is a deliberate act; a
  pin already refuses it.
- Restoring the sessions the 2026-10-24 prune removed. That recovery is done.
- Other cross-session writers (a context add into another session, a child's
  completion event in its parent's log) beyond the read path.
- A prune in an old workspace deleting a same-host import's moved copy
  (koto#473). It's a removal a lineage can't trust, but its fix is the import's
  local guard, and it stays tracked there.

