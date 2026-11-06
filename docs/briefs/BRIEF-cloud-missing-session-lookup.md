---
schema: brief/v1
status: Accepted
problem: |
  On the cloud backend, koto takes far longer to say a session doesn't exist
  than to read one that does: koto#597 saw about 2m16s against about 4s. The
  wait grows with every workspace that ever pushed to the bucket, so a caller
  that checks a few names under a short timeout never gets an answer.
outcome: |
  Someone asking koto about a name no session has gets the not-found answer in
  about the time a read of an existing session takes, however many workspaces
  share the bucket, and every other answer koto gives for a name with no local
  copy, the `session_unlocated` refusal included, stays exactly as it is.
---

# BRIEF: Cloud missing-session lookup

## Status

Accepted

This brief frames the problem behind koto#597 for the design that answers it,
`docs/designs/DESIGN-cloud-missing-session-lookup.md`. No PRD sits between
them: the requirement is the issue's own expectation, read alongside the
replies koto already promises, and the open questions are all about how to
meet it.

## Problem Statement

With `session.backend` set to `cloud`, a command that names a session
(`koto status`, `koto context get`, `koto context exists` and the rest) is
slow to report that the session doesn't exist. koto#597 timed it on koto
0.34.0: about 2m16s for `koto status` on a name no session has, against about
4s for one that exists on the same host. A caller that checked four names in
turn behind a 30-second timeout never got through, because the first missing
name alone outran the timeout.

The time isn't spent reading the session. Before a command reads a session
with no copy on this host and no state under this directory's prefix, koto
asks whether any other directory's prefix in the bucket holds it
(`CloudBackend::check_located` and `held_elsewhere` in
`src/session/cloud.rs`). That question costs a listing per workspace prefix
the bucket has ever had, and every clone, instance and worktree that pushed a
session adds a prefix. On a healthy bucket the not-found answer gets slower as
the bucket ages; on a degraded one, as koto#597's was, every one of those
listings can also wait out retries.

The question can't simply be dropped. Its answer is what lets koto refuse with
`session_unlocated`, reason `no_local_copy`, when another directory holds the
session, instead of reading it as absent and letting a caller create a second
session of the same name. The replies a name with no local copy can get are
documented for callers in
`plugins/koto-skills/skills/koto-user/references/error-handling.md` and pinned
by `tests/cloud_bound_dir_test.rs`, and callers outside koto parse them. Hosts
on older koto releases also keep writing sessions to the same buckets, so
whatever answers the question faster has to stay right while they do.

## User Outcome

A person or script working against a shared cloud bucket asks koto about a
name, and when no session has that name, hears so in roughly the time an
existing session takes to read. That holds on a bucket many workspaces have
used for months, not only on a fresh one. A caller that probes several names
under a short timeout finishes.

Nothing else they rely on moves. A session another directory holds still
refuses with `session_unlocated` and `no_local_copy`; a bucket that answers
but can't be read in full still gets "koto can't tell" rather than a guess; a
host that can't reach the bucket still reads a fresh name as absent. A bucket
shared with older koto releases keeps working for both.

Where the fast answer and the unchanged replies pull against each other, the
replies win. While older releases may still create sessions in a bucket, a
fast not-found can't always be told apart from a session one of them made, and
the brief doesn't ask the design to guess; how the design settles that, and
who decides when a bucket can trust the fast answer, is the design's call.

## User Journeys

### Probe several names under a timeout

A script that coordinates koto sessions checks whether each of four names
exists before deciding which to create, with a 30-second limit on the whole
check. Trigger: some of the names have no session yet. Outcome shape: each
missing name answers not-found in about the time an existing one takes, and
the script finishes inside its limit.

### Name a session from the wrong directory

A developer runs `koto status` on a session from a directory other than the
one it was created in, on a host that has no copy of it. Trigger: the command
reaches the bucket and finds no state under this directory's prefix. Outcome
shape: koto still refuses with `session_unlocated`, reason `no_local_copy`,
naming the prefix, exactly as before; the faster not-found path never answers
this case as absent.

### Share a bucket with an older koto

A team runs a mix of koto releases against one bucket while hosts upgrade at
their own pace. Trigger: a host on an older release creates a session that a
newer host then looks up by name. Outcome shape: the newer host still finds
that the session is held elsewhere and refuses as before, and the older host
sees nothing new that it misreads.

### Look up a name on a degraded bucket

An operator's bucket is throttling and resetting connections, as koto#597's
was. Trigger: a command names a session no one has, on a bucket where the fast
answer can be trusted. Outcome shape: the answer is still slower than on a
healthy bucket, but the wait no longer multiplies by the number of workspace
prefixes the bucket holds.

## Scope Boundary

**In:**

- How a command on the cloud backend learns, for a name with no copy on this
  host and no state under this directory's prefix, whether another prefix
  holds it, and what that costs in requests as the bucket gains prefixes.
- Keeping every reply that case can get today, `session_unlocated` /
  `no_local_copy`, "can't tell" and absent-when-unreachable included.
- Buckets written by a mix of koto releases.
- Anything koto has to write when a session copy is created so that the
  lookup can be cheap later.

**Out:**

- The dashboard's reads of the whole bucket, which don't go through this
  lookup.
- The sweeps that follow continues links and back-references between
  sessions; they have their own issues.
- Changing any reply, or the `session_unlocated` body callers parse, to make
  the answer faster.
- How long koto waits on a single request, and how often it retries one: the
  lookup asks fewer requests, it doesn't change the retry policy.
- Making a degraded bucket itself faster.

## References

- koto#597 -- the report this brief frames.
- `docs/designs/DESIGN-cloud-missing-session-lookup.md` -- the design that
  answers it.
- `src/session/cloud.rs` -- `CloudBackend::check_located` and
  `held_elsewhere`, the lookup as it stands.
- `plugins/koto-skills/skills/koto-user/references/error-handling.md` -- the
  `session_unlocated` reasons callers rely on.
- `tests/cloud_bound_dir_test.rs` -- the tests that pin those replies.
