---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-native-workflows-retention.md
milestone: "Retention and crash-staleness for koto's /workflows entries"
step_count: 3
---

# PLAN: retention and crash-staleness for koto's `/workflows` entries

## Status

Active

## Scope Summary

Every koto write into a Claude Code `workflows` directory ends with a sweep of
that directory. The sweep keeps at most ten koto-owned entries, removing
finished ones oldest `startTime` first and never a live entry or the one just
written; rewrites an entry still `running` whose last write is more than two
hours old as `killed`, with "no activity since <time>" on its summary and
`koto.staleSince` in its `koto` block; clears koto's own temp files older than
a minute; and never touches a file it can't prove koto wrote. The file
contract moves to version 6, and the Claude Code conventions guard learns to
check status words. The work lands in one pull request that closes koto issue
#190.

## Decomposition Strategy

Walking skeleton. Step 1 crosses the whole path the design opens: a real koto
commit through `try_materialize` into a temporary `workflows` directory, then
through the new sweep's listing, ownership check, classification and removal,
observed by counting files after thirty real sessions. Step 2 adds the
crash-staleness behaviour on the same sweep, with the guard proof for `killed`
first, so a release that doesn't know the word stops the step before any
reconciliation lands. Step 3 hardens the sweep against files it must not touch
and against races and errors. Each step carries its own tests and the docs for
the behaviour it adds.

## Steps

### Step 1: a workflows directory never holds more than ten finished koto entries

**Goal**: A koto commit reaches `try_materialize`, which writes the session's entry and then sweeps the same `workflows` directory: koto-owned finished entries beyond ten are removed oldest `startTime` first, live entries and the entry just written stay, and files without koto's marker are left alone. It runs on real koto sessions (the koto binary through `LocalBackend::append_event` into `materialize`) driven to completion into one `KOTO_WORKFLOWS_DIR`, so the bound is proven across the commit-to-disk path, not on the sweep function alone.

**Acceptance Criteria**:

- [ ] After each of 30 koto sessions (three times the bound of ten) is driven to completion into one temporary `workflows` directory, the directory holds at most ten koto-owned entries, and at the end the ten most recently started sessions' entries are the ones left.
- [ ] A directory holding ten finished entries of mixed outcome (completed, failed, cancelled), plus one more session written, ends with the entry that has the oldest `startTime` gone, whatever its status word.
- [ ] A directory seeded with 30 finished entries, the oldest written in the first contract version's shape, drops to ten owned entries after a single koto write, and the first-version entry is among those removed.
- [ ] With the bound set to 1, one finished entry already present, and a new session whose entry has the oldest `startTime`, only the new session's entry is left.
- [ ] In a directory over the bound that holds one live (`running`, recently written) entry older than every finished entry, the finished entries are removed and the live one stays.
- [ ] With the bound plus two live entries and three finished entries in one directory, one more write removes all three finished entries, and every live entry and the entry just written remain, so the directory may hold more than ten.
- [ ] A finished entry with no parseable `startTime` is ordered by its last write time among the other candidates, and two candidates with equal ordering keys are removed in ascending filename order.
- [ ] Writing into one directory at the bound leaves a second directory's entries unchanged, and the operator guide (`docs/guides/native-workflows-verification.md`) states the bound of ten, the oldest-`startTime`-first eviction order, and that more than ten live sessions leave more than ten entries until they finish, with the reproduction command.
- [ ] A `wf_test.json` seeded beside the koto entries is still present and byte-identical after the directory has been driven well past the bound, and a `koto-*.json` file without koto's marker is also left in place.

**Check**: Run `cargo test --test native_workflows_retention_test`. It drives 30 koto sessions to completion into one temporary `KOTO_WORKFLOWS_DIR`, counts the koto-owned files after each session and asserts the count never exceeds 10, that the 10 newest sessions' entries remain at the end, and that a seeded `wf_test.json` is byte-identical to its original. The test passes with niwa and Claude Code absent. The operator guide gives a manual reproduction: build koto, point `KOTO_WORKFLOWS_DIR` and `KOTO_HOME` at a fresh scratch directory, drive 30 one-step sessions to completion with the same commands the test uses, and see `ls "$KOTO_WORKFLOWS_DIR"/koto-*.json | wc -l` print 10 or fewer.

**Dependencies**: None

**Files**: `src/workflows_surface/retention.rs`, `src/workflows_surface/mod.rs`, `src/workflows_surface/materialize.rs`, `tests/native_workflows_retention_test.rs`, `docs/guides/native-workflows-verification.md`

### Step 2: a koto session quiet for over two hours reads stopped on the next koto write

**Goal**: The sweep rewrites an owned entry still `running` whose last write (file modification time) is more than two hours old as `killed`, with ` · no activity since <YYYY-MM-DD HH:MM UTC>` on its summary and `koto.staleSince` in its koto block, contract version 6; the owning session's next commit restores `running`; and the Claude Code conventions guard proves the recorded release knows `killed` and checks top-level status words.

**Acceptance Criteria**:

- [ ] Running `scripts/check-claude-code-conventions.sh` against the recorded release passes the probe for `killed` (and the probes for `running`, `completed` and `failed`), and the same run with `--alter` fails on them. If no acceptable anchor in the recorded release proves `killed`, the step stops here, before any reconciliation code lands.
- [ ] The conventions coverage test fails, naming the word, when an unprobed top-level `status` word is added to a shape fixture, and passes again once the word is removed.
- [ ] With a fixed clock, a `running` owned entry whose last write is more than two hours old becomes `killed` after a koto write from another session into the same directory. Its summary ends with ` · no activity since <YYYY-MM-DD HH:MM UTC>` showing its last-write time, its koto block carries `staleSince` as that time in epoch milliseconds, `koto.status` keeps its last value, and `koto.contractVersion` is 6.
- [ ] An entry whose last write is exactly two hours ago stays `running`, one second past two hours it becomes `killed`, and an entry with a last write in the future stays `running`.
- [ ] After reconciliation, the owning session's next commit rewrites its entry as `running` with no `staleSince` and no `no activity since` suffix.
- [ ] When the owning session's commit lands after the sweep judged the entry stale and before the sweep rewrites it, the entry is still `running` and holds that commit's content.
- [ ] In a directory over the bound, a reconciled entry is removed in `startTime` order like any other finished entry.
- [ ] When the reconcile rewrite fails (the entry or its directory made unwritable), the koto command exits with its normal result and the entry is unchanged.
- [ ] The field reference, the operator guide and the koto-user skill's `/workflows` section each state that the window is 2 hours, that a stale entry is reconciled to `killed`, that reconciliation needs a later koto write into the same directory, and that `koto dashboard --once` is the live view.
- [ ] The reconciled-shape fixture holds a reconciled entry and the shape test passes against it.

**Check**: Run `scripts/check-claude-code-conventions.sh` against the recorded Claude Code release and see the `killed` probe pass, then run it with `--alter` and see it fail. Run the unit tests in the retention module (fixed clock) and see the window, restore, race, eviction and failed-rewrite cases pass. Run `cargo test --test native_workflows_conventions` and `cargo test --test native_workflows_shape` and see both pass, including the coverage test failing when an unprobed status word is added to the shape fixture. If the first run shows no acceptable anchor for `killed`, stop and do not continue to the reconciliation work.

**Dependencies**: Step 1

**Files**: `src/workflows_surface/retention.rs`, `src/workflows_surface/contract.rs`, `src/workflows_surface/materialize.rs`, `tests/fixtures/native-workflows/claude-code-conventions.json`, `tests/native_workflows_conventions.rs`, `tests/fixtures/native-workflows/reconciled-shape.json`, `tests/native_workflows_shape.rs`, `docs/reference/session-feed.md`, `docs/guides/native-workflows-verification.md`, `plugins/koto-skills/skills/koto-user/SKILL.md`

### Step 3: the sweep leaves every file it can't prove koto wrote untouched, and survives races and errors

Lookalike, unparseable, symlinked and directory `koto-*.json` files stay byte-identical, koto temp files older than a minute are cleared, and vanished, unremovable or concurrently swept files never fail a koto command or cost it more than one listing.

## Implementation Sequence

The steps run in order, each extending the sweep the step before it landed.
The plan's owner details step 3 after step 2's work reaches its terminal, in a
commit that touches only this plan.
