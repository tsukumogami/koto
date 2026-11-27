---
status: Proposed
decision: |
  `koto session rebind --whole` is same-host. It refuses when the session's
  last cloud push came from another machine and points at `koto session
  import`. Where a tick on another host still straddles a move, the push it
  loses answers `session_rebound`, `koto session resolve --keep local` refuses
  to overwrite a newer anchor epoch, and the move's commit is pushed
  remote-first, so a lost move writes nothing and a re-run finishes.
rationale: |
  A cloud lease covers a tick already running on another host only if every
  cloud tick on every host registers in it, which adds bucket writes to every
  tick, needs renewal or a long expiry for polling gates, and still leaves
  older builds and buckets without conditional writes uncovered. The docs
  already send cross-host continuation to import. Refusing it, and making the
  race that slips past fail cleanly, costs nothing per tick.
---

# ADR: cross-host rebind scope

## Status

Proposed

## Context

The rebind tick exclusion ADR holds a tick off with a host-local `tick.lock`. On the cloud backend, a tick on a second host pulls the session once at its start and pushes once at exit. Until that push, it's unordered against a `rebind --whole` on the first host. Only the state-file PUT is conditional. Context keys, the manifest and `version.json` go up as plain PUTs before it.

So the second host's events can't land on top of the move. Other things can still go wrong:

- Its gates and actions have already run.
- Some of its context can land.
- It gets a generic `session conflict`, and the advice that comes with it is `koto session resolve --keep local`. That force-pushes its copy and undoes the move.
- When the move is the one that loses, its event is already in the mover's local log, and a re-run fails at its own pull.

Nothing lease-like exists in the bucket today.

The case is narrow. Two hosts can tick one session only when the workspace has the same canonical path on both. The cloud sync guide already says rebind is for a checkout that moved on the same host, and that continuing on another host means `koto session import`. Several things the move carries are host-bound anyway: the target directory is canonicalized on the mover's host, the publish location never reaches the bucket, and anchors and background runners are host-scoped.

The question was whether to exclude cross-host ticks with a cloud-side lease, or to declare cross-host takeover out of scope.

## Assumptions

- No shipped flow puts two hosts with the same canonical workspace path on one bucket and hands runs between them. If one does, cross-host takeover is real and this should be revisited for a lease.
- The buckets in use support conditional writes. On a bucket that doesn't, nothing orders two hosts, with or without a lease.
- `version.json` names the machine that last uploaded it. Tick flushes and context writes upload it; a plain state push doesn't, so a host whose only writes were cancel, rewind or plain rebind leaves no trace there. Rebind read none of it before this decision; the move now makes one listing and one GET for it.

## Decision

**Cross-host takeover is out of scope for `rebind --whole`, enforced, with fence-only hardening for the race that slips past.**

1. `rebind --whole` reads the bucket's `version.json` under `tick.lock`, after its pull and before its commit, and refuses `rebind_cross_host` (exit 2), writing nothing, when another machine uploaded it last. The refusal names `koto session import`, whose stop-the-source contract already covers a cross-host handover. After its commit, the move uploads `version.json` with a version above both the one it read and this host's, under this machine's id, so a host that pushed before the move and flushes after it meets a version conflict before any of its writes go up.
2. A push whose copy the bucket has moved past answers `session_rebound` instead of `session conflict`, naming the move. The comparison is between headers of the same session: the bucket's `anchor_epoch` is above the pushed one, or the bucket's header retired the anchor the pushed copy names and its epoch is at least the pushed one. A header with a different session id is left to the ordinary conflict. A `--whole` move raises `anchor_epoch` and sets `whole_epoch` to it, so the move it records is the one named. It's the same shape as the existing superseded refusal. On a refused conditional PUT (412) the fetch after it already happens, so that path adds no request. On a bucket without conditional writes (501) the push reads the bucket's header with a ranged GET first and refuses when the bucket is ahead, then PUTs with no condition; that narrows the window and doesn't close it, since a move landing between the GET and the PUT is still written over. A batch flush that meets a version conflict reads the header the same way and reports the move when there was one.
3. `koto session resolve --keep local` refuses `session_rebound` rather than overwrite a bucket copy that the comparison in point 2 finds ahead of the local one: a higher anchor epoch, or the local anchor retired at an epoch no lower.
4. The move's commit point, the move event plus the header rewrite, is pushed remote-first. It builds the bytes, PUTs them conditionally on the ETag of the copy it pulled under `tick.lock`, and adopts them locally only after the bucket confirms, as the supersede fence does. On a bucket without conditional writes it GETs the state file and compares ETags first, then PUTs with no condition, with the same narrow window as point 2. A lost race then writes nothing anywhere, and a re-run finishes. On either backend a crash can still tear the commit between the move event and the header; a re-run finds the move event without its header and finishes the header without logging a second event.
5. A session with no `origin` has no anchor to fence. The move still records the new directory and logs the move event without an `epoch`, and its later parts log epoch 0. Nothing in the output marks the difference; a reader tells from the event.

## Rationale

To cover the case that matters, a tick already in flight on the second host, a lease needs every cloud tick on every host to register in it. That costs one or two bucket writes per tick, and polling gates need renewal or a long expiry. A crashed holder strands the lease, and expiry has to be judged on the bucket's clock. It still does nothing for older builds or for buckets without conditional writes. All of that would be spent on a deployment shape the docs already send to import.

The chosen option costs nothing per tick. It turns a silent cross-host takeover into a visible refusal. Where a straddling tick slips past the refusal, it keeps the log clean, and koto's own advice no longer reverses the move.

## Options Considered

### Symmetric cloud lease

Every tick registers in the bucket at its start and releases at exit, and the move waits for registrations to clear. Rejected for the costs above. It's the design to pick up if cross-host takeover is ever meant to be supported.

### Asymmetric move lease plus open-batch refusal

Only the move writes a lease. New ticks refuse or wait when they see it, and the move refuses while another host has an open batch or an unsettled background run. Rejected because it catches a straddling tick only from that tick's first write. A stranded marker blocks the move until someone overrides it, and the option adds a bucket object without closing the case.

### Out of scope, documented only

Rejected because it leaves the conflict advice that undoes the move, and a lost move that a re-run can't finish.

### Fence-only hardening without a scope rule

Folded into the chosen option. Without the refusal, an unsupported cross-host takeover still looks supported until it races.

## Consequences

Cross-host takeover becomes an explicit non-goal of `rebind --whole`, refused wherever the move can see it. It can't see a host whose only writes were cancel, rewind or plain rebind, because those don't upload `version.json`; that host stays part of the cross-host residual. A tick on a second host that straddles a move still runs its gates and actions, which no host-local lock can hold off. Its log writes are refused with a reason that names the move, and following koto's advice can no longer undo the move. Some of that host's context writes may still land in the window before the move's first context write.

The rebind tick exclusion ADR's cross-host residual narrows to this unsupported shape but doesn't disappear. Together with that ADR's other residuals (older builds, writers outside a tick, commands that outlive their tick, a tick that never stops, a move into a subdirectory), the "never in between" ask is still not fully solved in every deployment.

Every host that touches a session moved with `--whole` needs this release or later. An older build that rewrites the session header drops `whole_epoch`, and a stale process of the new build then passes the whole-binding rule.

Point 2's header check has two more windows beside the gap between the ranged GET and the PUT. First, it compares epochs and the retired list, so a pushed copy at the same `anchor_epoch` as the bucket's but bound somewhere else (two rebinds that raised the epoch from the same read) isn't refused, and on a bucket without conditional writes the push writes over it. Second, `koto session resolve --keep local` reads the bucket's header once with a ranged GET, then uploads the context keys and the manifest, then PUTs the state file with no condition on any bucket. A move landing between that read and the state PUT is written over, and the context keys and manifest go up first in any case. A failed state upload there is reported only as a warning, and `version.json` still goes up after it. `--children=accept-local` makes the same single read before each child's unconditional PUT.

A cloud `--whole` move that changes a session's home in the bucket (a session whose header records no store prefix) carries its context keys to the new home last, after its other parts. Its held-by part has by then pushed this host's context manifest to the new home, so for that gap the manifest there names keys whose objects aren't there yet. A reader on another host who asks for one of them in that gap gets an error saying the bucket holds a different version of the key and fetching it failed. The carry closes the gap, and a move stopped before it closes it when run again. The carry decides what to copy from a listing of the new home's objects, compared by name and, for a key the manifest names at this host's copy, by size. So a move back to a home the session once had can leave one case open: an older object still there under the same key, at the same length, isn't copied over. A reader then gets that older content, stamped as synced, with no error. Later carries skip the key the same way, and so does the repair `koto session resolve --keep local` makes, so the case lasts until this host writes the key again.

The restart-replaces-run close carries the same cross-host residual, and the same scope statement covers it: a run replaced across hosts is a handover for import, not for `--replaces`.
