---
schema: brief/v1
status: Accepted
problem: |
  koto's session store holds every kind of state, but nothing finds it by
  where it ran or clears it as a set, a removed finished session looks like
  a lost one, and the run journal and the bucket's migrated markers only
  ever grow.
outcome: |
  People and teardowns find what a piece of work left, clear it without
  risking a session something still needs, and confirm a session finished
  and how; the journal stays bounded with its readers losing no line, and
  stale markers can be cleared.
motivating_context: |
  Skill sessions, coordinator records and test fixtures all moved onto koto
  sessions, so the store is now where everything lives. One worker's
  teardown left 87 sessions behind (koto#308), the store held 983 session
  directories on 2026-10-08, and the run journal shipped with its retention
  explicitly left to this work (koto#324's storage review).
---

# BRIEF: session-hygiene

## Status

Accepted

This brief frames the work that keeps koto's session store findable and
clearable. It stops before requirements; the PRD that follows picks them up.

## Problem Statement

koto keeps a session directory per workflow run under `~/.koto/sessions`.
That store started as a place for short workflow runs that cleaned up after
themselves on their terminal tick. It's now where everything lives: skill
sessions kept with `--no-cleanup` so a later run can read them, coordinator
records, sessions a parent reads after its children finish, and the fixture
sessions every test suite creates. On 2026-10-08 the store held 983 session
directories.

Four things are missing, and each one costs someone today.

**Nobody can tell whose sessions they are.** The only link from a session
to the work that made it is the execution directory recorded in the first
line of its state log. `koto session list` doesn't print it and can't filter
on it. A teardown that wants to clear what its worker left has to read every
log's first line itself, so in practice it clears nothing. One worker's
teardown on 2026-10-06 left 87 sessions behind, 72 of them test fixtures
(koto#308).

**Nothing removes finished sessions as a set.** `koto session cleanup`
removes one session by name, and `koto workspace prune` removes one finished
tree under a root you name. An operator who wants the space back, or a
teardown that wants to leave the store as it found it, has no verb that
says "the finished ones" (koto#162). Writing one by hand is dangerous: a
finished session can still be needed. A parent reads its finished children,
a resumed run expects a cancelled chain's children to still be there, a
coordinator's session is its record, and a finished session can be the thing
another live session points at. Nothing in koto states those rules, so
nothing can apply them.

**A finished session vanishes.** A session that reaches a success terminal
without `--no-cleanup` is removed on that tick. The `koto next` response now
says so, but every later look at it, `koto status` included, answers
`workflow '<name>' not found`, exactly as it would for a session that was
never there or was lost (koto#234). An agent that checks status after a
terminal tick can't tell a clean finish from a lost record, and can't learn
how the session ended.

**The files beside the store only grow.** The run journal
(`~/.koto/_run_journal.jsonl`, and a redirected store's own) gains a few
hundred bytes for every state any session enters, forever; its retention was
left to this work when it shipped. Two readers on the host depend on it, one
following it by byte offset to ship it off the host and one archiving it, so
a careless rotation loses or repeats their lines. In the bucket, every
`koto session import` leaves a `migrated.json` marker beside the source, and
no command removes one. Each marker keeps the old workspace's session showing
up in that workspace's listings, and every cloud command that touches the old
name pays a request to find it, for as long as the bucket lasts.

## User Outcome

The people who run koto at volume (an operator keeping a workstation's
store in check, a coordinator's teardown cleaning up after a worker, an
agent driving a workflow) can treat the session store as something they
manage rather than something that accumulates.

They find what a piece of work left behind without reading logs, clear
it without the risk of removing something another session still needs, and
know what they'll remove before they remove it. An agent never has to guess
whether a session it can't find finished cleanly or was lost. The tools that
read the run journal keep reading it whole while it stops growing without
bound, and nothing in the bucket outlives its purpose by default.

## User Journeys

### A teardown clears what its worker left

A coordinator tears down a finished worker's instance. Its teardown asks
koto for the sessions whose execution directory is under that instance's
path, gets each one's name and whether it's finished, previews the finished
ones koto would remove, and removes them. A session that's still running,
one a live session still points at, and the coordinator's own session are
not in the list. The teardown reports what it removed and what it kept, and
why.

### An operator reclaims the store on a schedule

An operator who runs many agents a day puts a weekly prune in cron. The
first time, they run it as a preview and read the list. After that the job
removes the finished sessions koto would have listed, and the store stops
growing past a week's worth of finished work.

### An agent confirms a session finished

An agent's `koto next` lands on a terminal state, and koto removes the
session. A moment later, the agent's recovery path, or a parent checking on
a child, runs `koto status <name>`. It answers that the session reached its
terminal, which state, and the result it recorded, instead of
`not found`. A name koto never saw still answers `not found`.

### The journal's readers survive a rotation

An operator runs a shipper that sends the journal off the host, and it last
read the journal up to some byte offset. Before it runs again, the journal
crosses its size bound and koto rotates it. On its next pass the shipper
finds the file it was reading under its rotated
name, reads it from its saved offset to the end, then reads the new journal
from its first byte. Nothing is lost or sent twice. The archiver, which keeps
a changed file as a new version, finds every rotated file by the same
naming rule.

### An operator clears migrated markers

An operator moved a set of sessions from an old laptop to a new one months
ago. The old copies in the bucket refuse with `session_migrated`, as they
should, and their markers are still there. The operator asks koto which
markers are safe to remove, sees why each one qualifies, and removes them.

## Scope Boundary

**IN:**

- Listing sessions by execution directory, with each session's state and
  whether it's finished, fast enough for a store of about a thousand
  sessions.
- Removing finished sessions as a set: opt-in, with a preview, and a run
  that removes exactly the previewed list. The rules that keep a session out
  of the set (a session still running, a chain child whose parent hasn't
  read it, a coordinator's session, a session a live session still points
  at, a resumed run's children after a cancel, refusal or error) are stated,
  and so is how koto knows each one.
- A finished session saying so, with its outcome, when `koto status` asks
  about it after koto removed its directory.
- Rotation and retention for the run journal, the default one and a
  redirected store's, that never loses a line a reader is still owed, with
  a rule the readers can follow to find rotated files.
- Clearing `migrated.json` markers in the bucket, with a stated rule for
  when one is safe to remove.

**OUT:**

- New run-journal record kinds, or changes to existing ones. Retention
  treats every kind alike; the kinds are another feature's work.
- The human-readable session view, which shipped, and the layout of a
  coordinator's own session, which belongs to the skills that use it.
- A bucket listing of another workspace's sessions before an import
  (koto#343). It stays its own issue unless the listing this work adds
  turns out to cover it with no extra surface, which the design decides.
- Changing when koto removes a session at its terminal tick. The default
  of removing a success terminal stays; this work makes the removal
  visible, not rarer.
- A retention verb for the request store or the decider ledger. Both are
  authoritative and documented as growing; neither is a session.
- Rotating or pruning anything on another host. The journal and the
  finished-session records are host-local, and so is what clears them.

## Downstream Artifacts

- `docs/prds/PRD-session-hygiene.md` (planned): the requirements this
  brief frames.

## References

- koto#308: sessions left by a worker's test suites can't be listed by
  execution directory or removed as a set.
- koto#162: the prune verb.
- koto#234: a session disappears from status with no signal at its
  terminal.
- koto#324: the run journal, whose retention was left to this work.
- koto#343: listing a source workspace's sessions in the bucket.
- `docs/workspace-layout.md`: what koto writes under `~/.koto` and what
  is safe to delete.
- `docs/reference/run-journal.md`: the run journal's format and reader
  rules.
- `docs/designs/current/DESIGN-session-migration.md`: the import verb and
  its marker.
