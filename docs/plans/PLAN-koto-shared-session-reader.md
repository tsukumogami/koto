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
- [ ] `git diff --name-only origin/main...HEAD -- src` prints nothing: the change touches no product code, and any manifest change is a dev-dependency only.

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
- [ ] The committed public-API snapshot of `koto::engine::types` and `koto::error` matches a fresh generation: re-running the snapshot command and then `git diff --exit-code` on the snapshot file exits 0. koto has no public-API tooling today (nothing in `Cargo.toml`, `scripts/` or `.github/workflows/` generates one), so the command is this step's to choose (a tool that needs a pinned nightly for rustdoc JSON is fine if the pin is recorded); it must give byte-identical output on repeat runs and be written down next to the snapshot so the crate step can run it unchanged.
- [ ] No koto command opens, reads or writes a file it didn't before: `git diff --color-moved=plain --color-moved-ws=ignore-all-space origin/main...HEAD -- src/engine` shows the write-half functions as moved blocks rather than edited ones; outside those blocks and `prepare_append`'s call into `plan_append`, the only changes there are paths, visibility and `use` lines, with no new file open, read or write in any function body; and the integration suite passes with no existing test file modified, so agents' koto calls do the same work and write nothing new.
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0 and `cargo test --locked -- --test-threads=1` exits 0.

**Check**: From the worktree root, run `cargo test --locked --test dashboard_golden_test && cargo test --locked -p koto-stability-tests -- --test-threads=1 && cargo test --locked --test state_log_append_test a_merged_last_line_is_split_by_the_next_write && cargo clippy --all-targets -- -D warnings && cargo test --locked -- --test-threads=1`, then `git diff --diff-filter=MD --name-only origin/main...HEAD -- tests koto-stability-tests benches`, then the snapshot command followed by `git diff --exit-code` on the snapshot file. Look for `test result: ok.` with `0 failed` on every cargo test run, no output from the first `git diff`, and exit 0 from the second.

**Dependencies**: Step 1

**Files**: `src/engine/persistence.rs`, `src/engine/mod.rs`, `src/template/types.rs`, `src/template/variables.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard.rs`, `src/session/local.rs`

### Step 3: the reader code builds as its own workspace crate that koto re-exports and CI proves cannot write

**Goal**: Someone running `koto dashboard --once` (with or without `--all`) sees exactly what they saw after Step 2, a program written against `koto::engine::types` or `koto::error` compiles unchanged, and every koto command does the same work it did before, except that `read_pointer` now refuses a leg pointer that isn't a regular file instead of blocking on it. Underneath, the read half lives in a new workspace member, `koto-session-reader/`, that koto depends on by path and re-exports at the old paths; the files moved with `git mv`, and the member can't write: a per-crate clippy ban list, `#![forbid(unsafe_code)]`, a dependency allowlist and `scripts/check-reader-readonly.sh` run in a Reader Write Guard CI job. The release guards land in the same pull request, so no release cut after it is unguarded, and the release that carries it waits on the koto#668 fix.

What Step 2 left that the design names differently, so the move follows the code on main rather than the design's wording:

- The overlay-free refresh is `refresh_scan`, and `refresh` stays the overlay path (it calls `refresh_with_overlay`), where the design calls the crate's overlay-free function `refresh`. The crate takes `refresh_scan`; koto keeps `refresh` and `refresh_with_overlay` with their behaviour, so the existing dashboard tests pass unedited.
- Validation sits behind four traits in `src/template/validate.rs` (`ValidateTemplate`, `ValidateSteps`, `ValidateVocabulary`, `VariableDeclExt`), where the design names the headers in `types.rs` and `variables.rs`. `validate.rs` stays in koto whole.
- `SessionSource` lives in `src/cli/dashboard_data.rs`'s `source` module, with `impl SessionSource for LocalBackend` beside `LocalBackend`. The trait moves to the crate's `source.rs`; the impl stays in koto.
- `SessionTree` and `CachedSession` still carry the remote overlay fields (`remote_failure`, `imported_from`). The design moves `remote_failure` into koto's dashboard state, but `tests/dashboard_cross_machine_test.rs` reads `state.tree.remote_failure`, and test files don't change, so the field stays reachable there: the crate's tree carries it under a type koto supplies (or `ReadFailure` moves with it), and the PR says which. `imported_from` becomes the crate's import record (`ImportedSource` moves).
- `plan_append`, `AppendPlan` and `MergedLastLine` are `#[doc(hidden)] pub` in `persistence.rs`; the write half in `persistence/append.rs` uses read-half privates (`join_seqs`, `visit_start`, `opens_visit`), which become `#[doc(hidden)] pub` in the crate. Of the 82 unit tests in `persistence.rs`'s test module, 35 write fixtures, use `tempfile`, call the write half or take a lock (Step 2's review counted four, the ones calling the write half); all 35 move to koto with their names and assertions unchanged, since the crate's `--all-targets` clippy bans writes and plain opens in its own test modules and `forbid(unsafe_code)` bans the lock. koto's remaining half of `persistence.rs` goes to `src/engine/persistence/mod.rs`, so the file itself shows as a rename.
- `tests/decider_check_compile.rs` reads `src/template/decider_check.rs` and `src/template/types.rs` by path, and `tests/value_routing_docs.rs` reads `src/template/types.rs`, so both paths stay in koto: `decider_check.rs` moves to the crate and a koto file stays at the old path that keeps what those tests read.
- `scripts/check-decider-purity.sh` names `src/decider/*.rs` by path, so it reads the decider files where they move, or the Decider Checks job fails.
- `epoch_slice_for_test` (`#[cfg(test)]` in `persistence.rs`) is used by koto's `src/engine/decider.rs` tests and can't reach them from a dependency; koto defines it.
- Every new `tests/*.rs` that names the `koto` crate loads `#[path = "support/host_session.rs"] mod host_session;`, or `tests/host_session_env.rs` fails.

**Acceptance Criteria**:

- [ ] Step 1's golden test passes unchanged: `cargo test --locked --test dashboard_golden_test` exits 0, and `git diff --diff-filter=MD --name-only origin/main...HEAD -- tests benches koto-stability-tests` prints nothing, so the expected file, its store builder and every existing test, fixture and bench are as they were (new test files may be added).
- [ ] The frozen paths resolve through the re-exports: `cargo test --locked -p koto-stability-tests -- --test-threads=1` exits 0, including a new test that compiles against `use koto::engine::types::*;` and names `koto::engine::types::derive_state_from_log` and `koto::error::Error`; and `scripts/public-api-snapshot.sh | diff - docs/reference/public-api/koto-engine-types-and-error.txt` prints nothing, so the public surface Step 2 snapshotted is unchanged after the move.
- [ ] `koto-session-reader` is a workspace member that builds and tests on its own: `cargo build --locked -p koto-session-reader` and `cargo test --locked -p koto-session-reader` exit 0; koto's manifest names it as `koto-session-reader = { path = "koto-session-reader" }`; `git diff -M --name-status origin/main...HEAD` shows each file that moved whole (`engine/types.rs`, `engine/errors.rs`, `findings.rs`, `redact.rs`, the decider files, `template/{assignments,decider,result_map}.rs`, `session/{row,validate}.rs` and `engine/persistence.rs`) as an `R` line into `koto-session-reader/src/`, not a delete and an add; and each file that stays mixed or keeps a koto copy at its old path (`template/types.rs`, `template/decider_check.rs`, `cli/dashboard_data.rs`, `engine/leg_pointer.rs`, `engine/terminal_result.rs`, `engine/context_assign.rs`, `session/version.rs`) pairs with its crate file as an `R` line under `git diff -B -M --name-status origin/main...HEAD`.
- [ ] Everything that writes stays in koto: `grep -rnE 'append_event|AppendLock|acquire_state_flock|write_backup|rewrite_tail|write_pointer|SessionVersion|get_or_create_machine_id|fn idempotency_hash|idempotency_hash\(|tempfile' koto-session-reader/src | grep -vE ':[0-9]+:\s*//'` prints nothing (the frozen `Event` keeps its `idempotency_hash` field, which this doesn't match, and comments may name koto's writers); the 35 writing unit tests from `persistence.rs`'s test module pass under the same names with their assertions unchanged in koto; and the dashboard data layer's unit tests, which build fixtures with `append_event` and `tempfile`, stay in koto and pass unchanged.
- [ ] The crate cannot write, and CI shows it: `cargo clippy -p koto-session-reader --all-targets --locked -- -D warnings` exits 0; `bash scripts/check-reader-readonly.sh` exits 0 and `bash scripts/check-reader-readonly.sh --self-test` exits 0 after planting each violation the design lists; and `.github/workflows/validate.yml` runs both in a job named "Reader Write Guard" that the `validate` job needs. A deliberate violation shows the bans resolve, including a trait-method path: adding `std::fs::write("x", "")` to a crate file, or an `OpenOptionsExt::custom_flags` call outside `fs_read.rs`, makes that clippy command fail naming `clippy::disallowed_methods`; removing it makes it pass. The PR records both runs.
- [ ] The crate's dependencies are the allowlist: `cargo tree -p koto-session-reader --depth 1 -e normal,build` lists no direct dependency outside anyhow, serde, serde_json, thiserror, regex, aho-corasick, toml, libc, sha2, hex and dirs, and the full `-e normal,build` graph names none of rust-s3, rustls, attohttpc, http, url, ratatui, crossterm, tempfile, fs2, fs4, fd-lock, nix or rustix. The guard script runs both checks.
- [ ] Every file open in the crate goes through `fs_read.rs`: `open_plain` is called only from the three log readers, `generate_session_id` and, until Step 5, `is_terminal_state` and the dashboard's template read (the design's Phase 3 row, which the guard script enforces); `read_pointer` reads through `read_store_file` with a 64 KiB cap and the session's store root. A unit or integration test shows a regular leg pointer reads exactly as before, and a FIFO, a symlink and an over-cap file at the pointer's path are refused without blocking.
- [ ] koto's commands do the same work and write nothing new: outside the two reads the design names (`read_pointer` here, `is_terminal_state` in Step 5), no function body that moved changes beyond paths, visibility, `use` lines and its opens going through `open_plain`; a reviewer reads this in `git diff -M origin/main...HEAD`. `cargo clippy --all-targets -- -D warnings` and `cargo test --locked -- --test-threads=1` exit 0 with no existing test file modified.
- [ ] Versions move together: the root manifest carries `[workspace.package] version`, koto and `koto-session-reader` say `version.workspace = true`, and `koto-stability-tests` keeps its own; `cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "koto" or .name == "koto-session-reader") | .version' | sort -u` prints one line; `.release/set-version.sh`'s lock check covers the crate's lock entry, so a run with a version that leaves the crate's entry behind fails; and a validate.yml job fails when the two versions differ or the crate declares a path dependency, then runs `cargo package -p koto-session-reader --locked`, which exits 0.
- [ ] CI covers the crate: validate.yml runs `cargo test`, `cargo clippy` and coverage with `-p koto-session-reader`, since the root `cargo test` runs only the root package, and every CI job on the PR's head passes.
- [ ] Release gate (koto#668): no koto release that carries the reader split is cut before the koto#668 fix is on main. A reviewer checks it at this step's merge and before any later release by running, from a clone with tags fetched (`git fetch --tags origin`):

  ```
  FIX=$(gh api graphql -f query='{repository(owner:"tsukumogami",name:"koto"){issue(number:668){closedByPullRequestsReferences(first:5){nodes{mergeCommit{oid}}}}}}' -q '.data.repository.issue.closedByPullRequestsReferences.nodes[].mergeCommit.oid' | head -1)
  for t in $(git tag --contains cc9479be); do
    if [ -z "$FIX" ] || ! git merge-base --is-ancestor "$FIX" "$t"; then echo "release gate broken: $t"; fi
  done
  ```

  and seeing no output. `cc9479be` is Step 2's merge, the first commit of the split, so every tag carrying any later step contains it too. While koto#668 has no merged fix, `FIX` is empty and any tag that contains `cc9479be` prints a line. If koto#668 closes through a commit no pull request links, the reviewer sets `FIX` to that commit by hand. Run at this PR's review, the check prints nothing.

**Check**: From the worktree root, run `cargo test --locked --test dashboard_golden_test && cargo test --locked -p koto-stability-tests -- --test-threads=1 && cargo test --locked -p koto-session-reader && cargo clippy -p koto-session-reader --all-targets --locked -- -D warnings && bash scripts/check-reader-readonly.sh && bash scripts/check-reader-readonly.sh --self-test && cargo clippy --all-targets -- -D warnings && cargo test --locked -- --test-threads=1`, then `scripts/public-api-snapshot.sh | diff - docs/reference/public-api/koto-engine-types-and-error.txt`, then `git diff --diff-filter=MD --name-only origin/main...HEAD -- tests benches koto-stability-tests`, then the release-gate check above. Look for `test result: ok.` with `0 failed` on every cargo test run, exit 0 from every clippy and script run, and no output from the `diff`, the `git diff` or the release-gate check.

**Dependencies**: Step 2

**Files**: `Cargo.toml`, `Cargo.lock`, `koto-session-reader/`, `src/engine/`, `src/template/types.rs`, `src/session/`, `src/cli/dashboard_data.rs`, `src/lib.rs`, `koto-stability-tests/tests/`, `tests/`, `scripts/check-reader-readonly.sh`, `scripts/check-decider-purity.sh`, `.release/set-version.sh`, `.github/workflows/validate.yml`

### Step 4: a dependent finds the store and reads one context value through the hardened, capped read surface

sessions_dir, LocalDirSource, read_context and the layout marker work, the dashboard scan refuses planted FIFOs, symlinks and over-cap files, and STABILITY.md promises the read surface (DESIGN Phase 4).

### Step 5: terminal detection tolerates unknown template fields and reads templates through the hardened read

A finished session stays finished when an older reader meets a newer template, with the same answer on every template under plugins/ and the fixtures (DESIGN Phase 5).

### Step 6: koto-session-list prints the dashboard rows without cloud or network dependencies and a hundred reads change nothing

The minimal program matches the dashboard in CI, links no storage, TLS or HTTP crate, leaves a FakeS3 cloud host unchanged with zero requests, and carries the reviewer real-store run (DESIGN Phase 6).

## Implementation Sequence

The steps run in order, one pull request each, and each reaches main before
the next starts; none can run beside another. Steps 1, 2 and 3 are detailed.
The plan's owner details step 4 after step 3's work reaches its terminal,
and each later step the same way, in a commit that touches only this plan.

**Release gate.** No koto release ships the reader split without the fix for
koto#668: the release that carries any of this plan's steps is grouped with
that fix and is cut only once the fix is on main, so the two ship together.
Step 3's release-gate criterion is the check a reviewer runs. It starts at
Step 2's merge (`cc9479be`): v0.39.0 already carries Step 1, a test-only
change that touches no product code, cut before this check existed.
