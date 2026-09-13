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
