---
schema: plan/v2
status: Active
execution_mode: multi-pr
split_mode_source: default
split_rationale: |
  Hard Constraint. The design makes each phase its own pull request with a
  merge gate between them: the dashboard golden test's expected file must be
  produced by main's own dashboard and be on main before any file moves, so
  the later steps pass it without touching it, and the in-place split must be
  on main before the crate step diffs its public-API snapshot.
tracking_level: none
upstream: docs/designs/DESIGN-koto-shared-session-reader.md
milestone: "A session reader koto shares with other programs"
step_count: 6
---

# PLAN: A session reader koto shares with other programs

## Status

Active

## Scope Summary

Move the code `koto dashboard` reads sessions with into a new workspace
member, `koto-session-reader`, that koto depends on and re-exports at the old
paths and that CI proves can't write. Promise the narrow store layout a
reader relies on, give a dependent a hardened read surface and a four-outcome
context read, and ship `koto-session-list`, a minimal second program CI
checks against the dashboard. `koto dashboard` shows the same rows before
and after every step.

## Decomposition Strategy

**Walking skeleton.** Step 1 crosses the path every later step is judged on:
a session store built at run time, read end to end by the koto binary's
`koto dashboard --once` and `--once --all`, compared with an expected file
main's own dashboard produced. The design requires that harness on main
before any file moves. Each later step is one of the design's phases, adds
one behaviour on top of it and must pass it unchanged: the reader and writer
separated in place, the reader as its own crate that can't write, the
hardened read surface a dependent uses, terminal detection that tolerates
newer templates, and the second program.

Every step that changes what a session read does carries the owner's
constraint as evidence a reviewer checks: status comes from koto, agents
write nothing new, and their koto calls cost what they did.

## Steps

### Step 1: koto dashboard --once output on a generated store matches a committed expected file

**Goal**: The walking skeleton crosses the whole path from a session store on disk, through the koto binary's read and liveness classification, to the tab-separated feed that `koto dashboard --once` and `koto dashboard --once --all` print. Its input is a store a test-side builder creates at run time: one session per liveness class, an abandoned session, a parent with two children, a session holding an unknown event type, a corrupt state file and a staging directory, with every timestamp an offset from the run's clock and at least 4 minutes from each liveness threshold (5 minutes, 2 hours, 7 days). Columns 1, 2, 4, 5, 6 and 8 (id, current state, status, intent, template, liveness), in printed order, plus the unreadable-session note, are compared with a committed expected file that main's own dashboard produced. This is a test-only change that lands on main before any file moves, so status keeps coming from koto, agents write nothing new, and their koto calls cost what they did.

**Acceptance Criteria**:

- [ ] Against the generated store, `koto dashboard --once` exits 0, and columns 1, 2, 4, 5, 6 and 8 of its stdout lines, in the order printed, equal the default-view rows of the committed expected file line for line; the abandoned session's row isn't among them.
- [ ] Against the same store, `koto dashboard --once --all` exits 0 and its columns 1, 2, 4, 5, 6 and 8, in order, equal the `--all` rows of the expected file, which include the abandoned session and the done session that the default view leaves out.
- [ ] Across the two expected outputs, column 8 shows each of `needs-you-blocked`, `needs-you-failed`, `needs-you-stalled`, `active`, `idle`, `pending` and `done` at least once; the parent and its two children appear as three rows; and the session with the unknown event type appears as an ordinary row showing the state it reached.
- [ ] Each run's stderr is exactly `note: 1 unreadable session(s) skipped`, as the expected file records; neither the corrupt session's id nor the staging directory's name appears in any stdout row, and the staging directory isn't counted in the note.
- [ ] The expected file holds no elapsed or idle duration (columns 3 and 7) and no calendar date, so nothing in it depends on when the test runs.
- [ ] Running `cargo test --test dashboard_golden_test` five times in a row, with at least a minute between the first and last run, passes every time, and `git status --porcelain tests/` prints nothing afterwards, so the test never rewrites its expected file.
- [ ] Changing the liveness token in one row of the expected file (for example `idle` to `active`) makes the test fail with a message that names that row's session id and shows the expected and actual values; swapping the order of two rows also fails; restoring the file makes it pass again.
- [ ] `git diff --name-only origin/main...HEAD` lists only paths under `tests/`: no file under `src/` changes and no dependency is added to `Cargo.toml`.

**Check**: Run `cargo test --test dashboard_golden_test` and see `test result: ok.` with `0 failed`, covering both the `--once` and the `--once --all` comparison. Then change one liveness token in the expected file, rerun the same command, and see it fail naming that row's session id; restore the file with `git checkout -- tests/` and rerun to see it pass.

**Dependencies**: None

**Files**: `tests/dashboard_golden_test.rs`, `tests/support/`, `tests/fixtures/`

### Step 2: koto dashboard reads sessions through a SessionSource with the writer split off, output unchanged

**Goal**: Someone running `koto dashboard --once` (with or without `--all`) sees exactly what they saw after Step 1, and every other koto command does the same work it did before. Underneath, koto now reads sessions through a `SessionSource` that `LocalBackend` implements, the append path asks a pure `plan_append` what to do before it writes, and a committed public-API snapshot of `koto::engine::types` and `koto::error` gives the later crate step something to diff against. No new crate, no file moves out of koto.

**Acceptance Criteria**:

- [ ] Step 1's golden test passes unchanged: `cargo test --locked --test dashboard_golden_test` exits 0, and `git diff --diff-filter=MD --name-only origin/main...HEAD -- tests koto-stability-tests benches` prints nothing, so its expected file, its store builder and every other existing test, fixture and bench are as they were (new test files may be added).
- [ ] The frozen external surface still compiles and holds: `cargo test --locked -p koto-stability-tests -- --test-threads=1` exits 0.
- [ ] The existing dashboard tests that exercise the bucket overlay and cross-machine rows pass with no edits: `cargo test --locked --test dashboard_test --test dashboard_cross_machine_test --test dashboard_detail_test` exits 0.
- [ ] A session whose last log line holds two merged events is repaired exactly as before: `cargo test --locked --test state_log_append_test a_merged_last_line_is_split_by_the_next_write` passes, and the unit tests `append_splits_a_merged_last_line_and_keeps_a_backup`, `idempotent_append_splits_an_unterminated_merged_first_event_line`, `append_still_refuses_a_truncated_last_line_and_touches_nothing`, `append_refuses_merged_events_that_do_not_follow_and_names_the_line` and `reader_reads_a_merged_final_line_as_its_events` pass under the same names with their assertions unchanged, wherever the split places them.
- [ ] Given log text held in memory, with no file or temp directory, `plan_append` returns the next sequence number and the merged-line classification that `prepare_append` then acts on; unit tests show this for a clean log, a log whose last line holds merged events, and a log whose last line is truncated, where the truncated case returns the same error a write returns today.
- [ ] Template validation reports the same errors and warnings: `cargo test --locked --lib template::` exits 0, and `git diff -w origin/main...HEAD -- src/template/types.rs src/template/variables.rs` shows changes only to lines that open or close an `impl` or `trait` block, declare a trait method signature, or add a `use` line, with no validator body or test body changed.
- [ ] The dashboard scan runs against any `&dyn SessionSource`: a unit test builds a sessions directory in a temp dir, refreshes once through a `LocalBackend` passed as a `SessionSource` and once through the `refresh_with_overlay` path, and gets the same rows, tree roots and unreadable count from both.
- [ ] The committed public-API snapshot of `koto::engine::types` and `koto::error` matches a fresh generation: re-running the snapshot command and then `git diff --exit-code` on the snapshot file exits 0. koto has no public-API tooling today (nothing in `Cargo.toml`, `scripts/` or `.github/workflows/` generates one), so the command is this step's to choose; it must run on the stable toolchain koto already uses, give byte-identical output on repeat runs, and be written down next to the snapshot so the crate step can run it unchanged.
- [ ] No koto command opens, reads or writes a file it didn't before: `git diff --color-moved=plain --color-moved-ws=ignore-all-space origin/main...HEAD -- src/engine` shows the write-half functions as moved blocks rather than edited ones; outside those blocks and `prepare_append`'s call into `plan_append`, the only changes there are paths, visibility and `use` lines, with no new file open, read or write in any function body; and the integration suite passes with no existing test file modified, so agents' koto calls do the same work and write nothing new.
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0 and `cargo test --locked -- --test-threads=1` exits 0.

**Check**: From the worktree root, run `cargo test --locked --test dashboard_golden_test && cargo test --locked -p koto-stability-tests -- --test-threads=1 && cargo test --locked --test state_log_append_test a_merged_last_line_is_split_by_the_next_write && cargo clippy --all-targets -- -D warnings && cargo test --locked -- --test-threads=1`, then `git diff --diff-filter=MD --name-only origin/main...HEAD -- tests koto-stability-tests benches`, then the snapshot command followed by `git diff --exit-code` on the snapshot file. Look for `test result: ok.` with `0 failed` on every cargo test run, no output from the first `git diff`, and exit 0 from the second.

**Dependencies**: Step 1

**Files**: `src/engine/persistence.rs`, `src/engine/mod.rs`, `src/template/types.rs`, `src/template/variables.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard.rs`, `src/session/local.rs`

### Step 3: the reader code builds as its own workspace crate that koto re-exports and CI proves cannot write

koto-session-reader holds the moved files behind the Reader Write Guard job, with frozen paths and the golden test unchanged, the release version guards in place, and its release waiting on the koto#668 fix (DESIGN Phase 3).

### Step 4: a dependent finds the store and reads one context value through the hardened, capped read surface

sessions_dir, LocalDirSource, read_context and the layout marker work, the dashboard scan refuses planted FIFOs, symlinks and over-cap files, and STABILITY.md promises the read surface (DESIGN Phase 4).

### Step 5: terminal detection tolerates unknown template fields and reads templates through the hardened read

A finished session stays finished when an older reader meets a newer template, with the same answer on every template under plugins/ and the fixtures (DESIGN Phase 5).

### Step 6: koto-session-list prints the dashboard rows without cloud or network dependencies and a hundred reads change nothing

The minimal program matches the dashboard in CI, links no storage, TLS or HTTP crate, leaves a FakeS3 cloud host unchanged with zero requests, and carries the reviewer real-store run (DESIGN Phase 6).

## Implementation Sequence

The steps run in order, one pull request each, and each reaches main before
the next starts; none can run beside another. Steps 1 and 2 are detailed.
The plan's owner details step 3 after step 2's work reaches its terminal,
and each later step the same way, in a commit that touches only this plan.

**Release gate.** No koto release ships the reader split without the fix for
koto#668: the release that carries any of this plan's steps is grouped with
that fix and is cut only once the fix is on main, so the two ship together.
