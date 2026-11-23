---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-host-resource-pools.md
milestone: "Host resource pools"
step_count: 6
---

# PLAN: host resource pools

## Status

Active

## Scope Summary

This plan builds the koto side of host resource pools as the DESIGN lays it
out: `[pools.<name>]` sizes in the user's koto config, flock slot files and an
atomic queue under `<koto root>/pools/`, a pool guard in the advance loop that
queues a pooled action without failing the run, and koto-owned background
actions whose runner keeps the slot for as long as any process of the run is
alive. It covers every requirement of the PRD, R1 to R28, in one pull request
to koto.

Moving shirabe's work-on verification step onto the feature is not in this
plan: it needs this koto change released and shirabe's koto floor raised.

## Decomposition Strategy

Walking skeleton, then thickening. Step 1 crosses the whole path for an
ordinary action: a template naming a pool, a size in the user's config, the
slot files, the guard in the advance loop and the queued response, so two
sessions on a size-1 pool already serialize. Step 2 adds background actions on
top of the same guard. The later steps each thicken one part (fairness, crash
and process handling, visibility and config errors, cost and docs) and depend
only on earlier steps, so the order below is the build order.

## Steps

### Step 1: a second session on a size-1 pool is told it is queued

**Goal**: A template whose state's `default_action` names `pool: test-suite`
runs through the pool guard. With `[pools.test-suite] size = 1` in the user's
koto config, a second session reaching that state while the first holds the
slot gets a non-failing queued response, and with no `[pools]` table nothing
changes.

**Acceptance Criteria**:

- [ ] `koto template compile` accepts `pool: test-suite` on `default_action`,
  refuses `pool: Test`, `pool: "{{POOL}}"`, a `size` key under a state and
  `timeout_secs` without `background: true`, and
  compiles a template without `pool` to the same output and template hash as
  before the change.
- [ ] With `[pools.test-suite] size = 1` and `wait_secs = 0`, and two sessions in two different
  directories under one koto root, the first session's tick runs its action;
  the second session's tick, started while the first action runs, exits 0 with
  `action: "gate_blocked"` and a `__pool__` condition with `type: "pool"`,
  `status: "queued"`, `category: "temporal"`, `agent_actionable: false`,
  `position: 1` and one holder naming the first session.
- [ ] After the first action finishes, the second session's next tick runs its
  action.
- [ ] With no `[pools]` table, the same template's tick runs the action with
  the same `koto next` JSON as without `pool`, once `timestamp` and `since`
  fields are removed, and no `pools/` directory is created.
- [ ] A tick that passes through two states whose gates pass and reaches the
  full pool's state reports that state, `advanced: true`, and the queued
  condition, without running the action.

**Check**: Run the new integration test for the pooled-action skeleton
(`cargo test --test pool_skeleton`) and see every case pass; then, by hand, a
two-session run of a fixture template prints the queued condition shown above
for the second session.

**Dependencies**: None

**Files**: `src/template/types.rs`, `src/template/compile.rs`,
`src/config/mod.rs`, `src/config/validate.rs`, `src/pool/`,
`src/engine/advance.rs`, `src/cli/mod.rs`, `src/cli/next_types.rs`,
`tests/pool_skeleton.rs`

**Type**: code

**Complexity**: critical

### Step 2: a background pooled action holds its slot until its command ends

**Goal**: A `default_action` with `background: true` and `timeout_secs` starts
its command through a detached `koto __run-action` runner and returns within 2
seconds; the slot stays held for the command's real length, and the next tick
after it ends delivers the result and moves on. Step 1 left `background`
compiling but running as an ordinary action; this step gives it its runtime
behind the same pool guard (the CLI action closure and the engine's
`ActionResult::PoolBlocked` stop), unix-only like `src/pool/`.

**Acceptance Criteria**:

- [ ] A background action under a size-1 pool whose command sleeps 90 seconds
  and exits 0 returns from its starting tick within 2 seconds, holds the slot
  for the 90 seconds, and every tick meanwhile reports `__action__` with
  status `running`, the same `run_id`, growing `elapsed_secs` and the declared
  `timeout_secs`, starting no second copy.
- [ ] A second session reaching the same pool during those 90 seconds is
  queued, and takes the slot only after the first command has exited.
- [ ] The first tick after the command exits delivers `capture_stdout_as`,
  evaluates the state's gates and advances.
- [ ] A command exiting 3 yields `failure_kind: "nonzero_exit"` with
  `exit_code: 3` on the next tick, and the tick after starts a new run; a
  command outliving `timeout_secs: 5` is killed within 7 seconds and reported
  `timed_out`.
- [ ] `background: true` without `timeout_secs`, and with `polling` or
  `requires_confirmation`, fails `koto template compile` naming the state and
  field; with no `pool`, a background action runs without a slot.

**Check**: Run `cargo test --test pool_background` and see every case pass;
the 90-second case shows the second session's `pool_acquired` timestamp after
the first run's `background_finished`.

**Dependencies**: Step 1

**Files**: `src/engine/background.rs`, `src/cli/run_action.rs`,
`src/cli/mod.rs`, `src/engine/advance.rs`, `src/pool/guard.rs`,
`src/pool/store.rs`, `tests/pool_background.rs`

**Type**: code

**Complexity**: critical

### Step 3: waiters are served in order, with a claim period and a lapse

**Goal**: Sessions waiting on a full pool take freed slots first come first
served, through the queue file and its claim. Steps 1 and 2 admit any session
that finds a free slot and only sweep waiters whose tickets expired; this step
makes the queue decide who goes next, holds a freed slot for the head waiter
for `claim_secs`, drops a waiter that stops ticking for `lapse_secs`, and
applies the resize and removal rules.

**Acceptance Criteria**:

- [ ] With size 1 and waiters W1, W2 and W3 queued in that order, the slot goes
  to W1, then W2, then W3, even when W3 ticks every second and the others
  every 10 seconds.
- [ ] A session that arrives while a slot is free but W1 is waiting queues
  behind W1 and reports `position: 2`.
- [ ] With `claim_secs = 5`, when a slot frees and W1 doesn't tick for 6
  seconds, W2's next tick takes the slot, and W1 takes the following free
  slot; the claim starts whether the holder exited cleanly or was killed.
- [ ] With `lapse_secs = 5`, a waiter that doesn't tick for 6 seconds loses its
  place and the waiters behind it move up; its next tick queues it at the back
  and its log holds `pool_released` with reason `lapsed`.
- [ ] A queued tick reports `retry_after_secs` equal to `min(15, claim_secs)`,
  and two queued ticks 3 seconds apart report `waited_secs` larger by at least
  2 on the second; with `wait_secs = 5` and a holder finishing 2 seconds into
  the waiter's tick, that tick takes the slot without returning queued.
- [ ] Twenty sessions racing for the last free slot of a size-1 pool produce
  exactly one holder.
- [ ] Lowering `size` from 2 to 1 while two sessions hold slots keeps both and
  admits a third only after both release; removing the pool lets a waiter's
  next tick run unthrottled with `pool_released` reason `pool_removed`.
- [ ] A waiter whose session is rewound out of the pool state, or completes,
  leaves the queue at once with `pool_released` reason `left_state`.

**Check**: Run `cargo test --test pool_queue` and see every case pass; the
ordering case prints the three acquisition times in W1, W2, W3 order.

**Dependencies**: Step 2

**Files**: `src/pool/store.rs`, `src/pool/guard.rs`, `src/cli/mod.rs`,
`tests/pool_queue.rs`

**Type**: code

**Complexity**: critical

### Step 4: crashes, cancels and moves never leak a slot or signal a stranger

**Goal**: A background run holds its slot until every process of it has
exited, the runner stops the command's descendants when the command ends or
times out, and koto signals only a live runner it has verified. Step 2 left no
descendant kill, no SIGTERM or cancel handling, and no record when a surviving
process finally frees a slot; step 3 left `koto cancel` and session removal
without leaving the queue. This step closes those, adds the `[background]`
switch and timeout cap, and refuses moves while a run is in flight.

**Acceptance Criteria**:

- [ ] `kill -9` of a background runner's whole process tree frees the slot
  within 5 seconds with no file removed by hand, and the head waiter's next
  tick takes it.
- [ ] After `kill -9` of a runner whose command survives, the session's ticks
  report `__action__` status `orphaned` and the slot stays held, no signal
  reaches the surviving command, and once it exits the next tick reports
  `failure_kind: "lost"` with `pool_released` reason `lost`, and the tick after
  starts a new run.
- [ ] On Linux, a background command that starts a child with `setsid` and
  exits leaves no process of the run alive once the runner ends, and the slot
  is free.
- [ ] `koto cancel` on a holder stops its runner, the command's processes are
  gone and the slot is free within 5 seconds, with `pool_released` reason
  `cancelled`; `koto cancel` on a waiter, and removing a waiting session,
  remove it from the queue before the command returns.
- [ ] Re-entering the pool state while a previous visit's runner is alive stops
  that runner before the new run starts.
- [ ] With `[background] enabled = false`, a background action stops with
  `background_disabled`; with `max_timeout_secs = 10`, an action declaring
  `timeout_secs: 60` is killed after 10 seconds and its `running` condition
  shows `timeout_capped: true`.
- [ ] A background command printing 100 KiB to stdout and exiting 1 yields a
  failure whose stdout is its first 64 KiB with `truncated: true`.
- [ ] Relocate, rebind and cross-host import of a session refuse with
  `background_run_in_flight` while its run is in flight, and a run whose
  `background_started` event names another machine reads as `lost`.

**Check**: Run `cargo test --test pool_crash` and see every case pass; the
orphaned case shows the surviving command's pid still alive after the runner's
death and no signal recorded against it.

**Dependencies**: Step 3

**Files**: `src/engine/background.rs`, `src/cli/run_action.rs`,
`src/cli/mod.rs`, `src/pool/store.rs`, `src/pool/config.rs`,
`src/session/`, `tests/pool_crash.rs`

**Type**: code

**Complexity**: critical

### Step 5: koto pool status shows holders and waiters, and bad config fails only where used

`koto pool status` and pool events report holders and waiters, and invalid config, project keys, an unsafe pools directory and nested requests behave as R5, R7, R13, R14, R25 and R26 require.

### Step 6: an undefined pool costs under 5 ms, older koto runs pooled templates, and the guide explains pools

The cost benchmark (R28), previous-release compatibility (R3, R4), the unsupported-platform path (R27), the authoring guide and the config-precedence correction land together.
