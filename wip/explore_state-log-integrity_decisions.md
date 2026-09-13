# Exploration Decisions: state-log-integrity

## Round 1

Proposed by the worker from round-1 evidence; pending coordinator approval at
the explore checkpoint.

- Refuse appends to a missing log, at the `context add`/`context remove` verbs
  (exit 2, `workflow '<name>' not found`, before `store.add`) and inside
  `persistence::append_event`: it's the single root cause of #236 and #200's
  shape, measured and read in code.
- `Fixes #200` is supported: #200's exact shape reproduces from the CLI via the
  same missing-log path; no rewrite race lost a header in ~7,700 racing writes.
- Lock coverage: every state-log writer takes a short sidecar write lock with
  bounded wait; rewrites go through temp+rename under it; one `write` per event.
  Reason: duplicate seqs, lost appends and fused lines are measured corruption
  from unlocked writers.
- #171: the `koto next` tick lock is already non-blocking fail-fast as designed;
  keep it. The blocking `LOCK_EX` is request-store-internal. The real gap is
  coverage, closed by the write lock.
- Reject a tolerant reader that rebuilds headers: it can't yield a tickable
  session and would hide #200-style history loss.
- Recover stranded batches: the scheduler's half-init repair moves a headerless
  child log aside (keeping `ctx/`) with a warning, plus a clearer "no header"
  error. `koto session recover` stays unchanged.
- No new error code for the refusal; flat envelope, exit 2.

## Round 1, coordinator verdict (approved with changes)

- Split: PR 1 is the refusal plus a header guard and closes #236 and #200, via
  /work-on. The write lock, atomic rewrites and cloud pull become a later
  /scope task. Reason: they carry different risk, and the lock needs design
  decisions.
- The guard is "the first line must parse as a header", not "the file exists".
  It covers missing, empty and headerless files, including the cloud pull and
  crash-mid-rewrite paths.
- The scheduler rename-aside is out of PR 1; it's the user's call.
- Adjacent defects stay out; propose issues to the coordinator.
- Approved behaviour change: `context add`/`context remove` on a session with no
  valid log exit 2. Call it out in the PR body and CHANGELOG. Leave #171 open.
- Tests assert exit 2, the not-found message, no ctx/ file left, no session
  directory for a never-initialized name. The batch test runs until the blocked
  child spawns cleanly. Add a rewind test and a persistence unit test for the
  guard.
