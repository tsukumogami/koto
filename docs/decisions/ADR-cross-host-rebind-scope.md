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
- `version.json` records the machine that last pushed, and the move reads it on the path it already takes.

## Decision

**Cross-host takeover is out of scope for `rebind --whole`, enforced, with fence-only hardening for the race that slips past.**

1. `rebind --whole` refuses when `version.json` shows that the session's last push came from another machine. The refusal names `koto session import`, whose stop-the-source contract already covers a cross-host handover.
2. Sometimes a push is refused because the bucket's header carries a higher anchor epoch than the pushed copy, or retires the pushing prefix. That push answers `session_rebound` instead of `session conflict`, naming the move. It's the same shape as the existing superseded refusal, and the fetch after the refused PUT already happens, so it adds no request.
3. `koto session resolve --keep local` refuses to overwrite a bucket copy whose anchor epoch is higher than the local one.
4. The move's commit point, the move event plus the header rewrite, is pushed remote-first. It builds the bytes, PUTs them conditionally on the version it pulled under `tick.lock`, and adopts them locally only after the bucket confirms, as the supersede fence does. A lost race then writes nothing anywhere, and a re-run finishes.

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

Cross-host takeover becomes an explicit non-goal of `rebind --whole`, refused wherever the move can see it. A tick on a second host that straddles a move still runs its gates and actions, which no host-local lock can hold off. Its log writes are refused with a reason that names the move, and following koto's advice can no longer undo the move. Some of that host's context writes may still land in the window before the move's first context write.

The rebind tick exclusion ADR's cross-host residual narrows to this unsupported shape but doesn't disappear. Together with that ADR's other residuals (older builds, writers outside a tick, commands that outlive their tick, a tick that never stops, a move into a subdirectory), the "never in between" ask is still not fully solved in every deployment.

The restart-replaces-run close carries the same cross-host residual, and the same scope statement covers it: a run replaced across hosts is a handover for import, not for `--replaces`.
