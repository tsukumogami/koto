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

Waiters are served first come first served with the claim period, lapse, in-tick wait and resize or removal rules (R8, R9 race, R11, R12, R15, R16), and leaving the state drops a waiter.

### Step 4: crashes, cancels and moves never leak a slot or signal a stranger

Crash release, orphaned and lost runs, cancel and removal, moves refused in flight, the background switch and timeout cap, and output truncation behave as R10, R19 to R24 require.

### Step 5: koto pool status shows holders and waiters, and bad config fails only where used

`koto pool status` and pool events report holders and waiters, and invalid config, project keys, an unsafe pools directory and nested requests behave as R5, R7, R13, R14, R25 and R26 require.

### Step 6: an undefined pool costs under 5 ms, older koto runs pooled templates, and the guide explains pools

The cost benchmark (R28), previous-release compatibility (R3, R4), the unsupported-platform path (R27), the authoring guide and the config-precedence correction land together.
