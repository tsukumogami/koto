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

**Builds on Step 1**: the sweep is `retention::apply(dir, written, &Limits)` in `src/workflows_surface/retention.rs`, called from `try_materialize` in `materialize.rs` right after the entry's atomic write. `Limits` holds only `keep` (default `KEEP = 10`) and `apply` takes no clock, so this step adds the two-hour window to `Limits` and passes the current time in, keeping both injectable for the fixed-clock tests. Owned entries are recognized by an id allowlist that accepts a leading digit (session UUIDs), not `validate_session_id`. The operator guide's Retention section says a directory within the bound costs one listing and nothing is read; that stops being true once the stale check reads running entries, so this step rewrites that sentence. Step 1's tests live in `tests/native_workflows_retention_test.rs`.

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

**Goal**: The sweep opens a candidate only through a no-follow, non-blocking open whose handle is checked as a regular file, so a `koto-*.json` swapped for a symlink, a FIFO or a directory between listing and open is never followed and never hangs a koto command; it clears koto's own `.koto-wf-*.tmp` files older than a minute; it re-checks a candidate's modification time before removing it; and within the bound it reads only the entries old enough to be stale, so one old finished entry no longer costs a read of every other entry on each write. Lookalike, unparseable, oversized, symlinked and directory `koto-*.json` files stay byte-identical, and a vanished, unremovable or concurrently swept file never fails a koto command or costs it more than one listing.

**Builds on Step 2**: `retention::apply(dir, written, now, &Limits)` lists the directory once with `read_dir`, keeps only `DirEntry::file_type().is_file()` entries whose name passes `entry_id`, and takes the fast path when `matched <= keep` and no other matched file's last write is older than `stale_after`. Past the fast path it calls `owned()` on every other matched file. `owned()` opens with a plain `File::open(path)`: a link swapped in after the listing is followed, and a FIFO swapped in blocks the open. `Limits` holds `keep` and `stale_after`; `Sweep` counts `matched`, `read`, `owned`, `reconciled` and `removed`. Eviction removes by name with no re-check. Reconcile stages through `materialize::stage` (which names temp files `.koto-wf-*.tmp`) and re-checks `symlink_metadata` before `persist`. No `.koto-wf-*.tmp` file is swept, and `TEMPFILE_SWEEP_MIN_AGE` in `src/cli/batch.rs` is private. Every failure is skipped or warned about and none reaches the koto command, since `try_materialize` ignores the returned `Sweep`.

**Acceptance Criteria**:

- [ ] `owned()` opens with no-follow and non-blocking flags and checks the opened handle's file type with `fstat` before reading. A unit test that swaps a listed `koto-<id>.json` for a FIFO between listing and open sees the sweep return without blocking and leave the FIFO in place. A test that swaps one for a symlink to an owned-looking entry elsewhere sees that target neither read as owned nor changed.
- [ ] In a directory driven past the bound, a `koto-*.json` symlink (to an owned-looking entry and to a lookalike), a `koto-*.json` directory, an unparseable `koto-*.json`, a `koto-*.json` larger than 1 MiB, and lookalikes each missing one marker field (`id`, `koto.sessionId`, `koto.contractVersion`, string `status`) are all present and byte-identical afterwards, and none counts toward the bound.
- [ ] A `.koto-wf-*.tmp` regular file whose last write is more than 60 seconds ago is removed by the next koto write into the directory. One written less than 60 seconds ago, and a `.koto-wf-*.tmp` symlink or directory of any age, are left in place. `Sweep` reports the temp files removed. `retention::TEMP_MIN_AGE` is 60 seconds, and a unit test pins it equal to `TEMPFILE_SWEEP_MIN_AGE` in `src/cli/batch.rs`, which becomes `pub(crate)`.
- [ ] A finished candidate whose modification time changed between the listing and its removal (rewritten by its session as `running`) is not removed, and holds the rewritten content afterwards.
- [ ] A candidate removed by another process between listing and removal counts as removed, and the sweep goes on to the next candidate. A candidate whose removal fails with an error other than not-found (its directory made read-only) is skipped with one warning that names the file and the error kind, and the koto command exits with its normal result.
- [ ] Two sweeps started together on two threads over the same over-bound directory both return, the directory ends at the bound, and no file outside the finished candidates is removed or changed.
- [ ] Within the bound, the sweep reads only matched files whose last write is older than the stale window. A directory holding one old finished entry and nine fresh ones reads one file per write, not nine. Over the bound, every matched file is read as before. In every case the sweep lists the directory once.
- [ ] An unlistable `workflows` directory (no read permission) leaves the koto command's result unchanged.
- [ ] The operator guide's Retention section and the field reference say that only koto-owned regular files are counted or touched, that `.koto-wf-*.tmp` files older than a minute are cleared, and what a write costs within and over the bound.

**Check**: Run `cargo test --lib workflows_surface::retention` and see the FIFO-swap, symlink-swap, temp-file age, modification-time re-check, vanished-candidate, unremovable-candidate, double-sweep, unlistable-directory and read-count cases pass, and the `TEMP_MIN_AGE` pin hold. Run `cargo test --test native_workflows_retention_test` and see the lookalike, unparseable, oversized, symlinked and directory `koto-*.json` files byte-identical after the directory is driven past the bound through the koto binary. Run `cargo test --test doc_names` after the docs change.

**Dependencies**: Step 2

**Files**: `src/workflows_surface/retention.rs`, `src/cli/batch.rs`, `tests/native_workflows_retention_test.rs`, `docs/guides/native-workflows-verification.md`, `docs/reference/session-feed.md`

## Implementation Sequence

The steps run in order, each extending the sweep the step before it landed.
The plan's owner details step 3 after step 2's work reaches its terminal, in a
commit that touches only this plan.
