---
schema: prd/v1
status: Accepted
problem: |
  A koto pull request waits 34 to 41 minutes for its test verdict. CI runs the
  whole suite on one thread, twice per round (Unit Tests and Coverage), against
  3 to 4 minutes for every other job, to keep a small number of tests that swap
  process-wide state from racing. The guard came in when the suite had 520
  tests; it now has about 5,460.
goals: |
  The job that carries the whole suite's result, with coverage from the same
  run, finishes in at most 15 minutes on the standard runner, the required
  `validate` check waits on it, and every test that runs today still runs and
  still checks what it checks.
upstream: docs/briefs/BRIEF-koto-ci-parallel-tests.md
---

# PRD: koto CI parallel tests

## Status

Accepted

## Problem Statement

Every push to a koto pull request starts a CI round whose test verdict arrives
34 to 41 minutes later. Two jobs in `.github/workflows/validate.yml` set that
time: Unit Tests runs `cargo test --locked -- --test-threads=1` with default
features, and Coverage runs the same suite again under `cargo llvm-cov
--all-features ... -- --test-threads=1`. In recent runs each took 34 to 41
minutes; the next-slowest job took 3 to 4. The round waits on the same tests,
run one at a time, twice.

The single thread is a guard. It came in with #98, when the suite had 520
tests, because some tests point HOME or XDG_CACHE_HOME at a scratch directory,
change the working directory, or set an environment variable for the whole
process, and two tests doing that at once in one process trip over each other.
The suite has since grown to about 5,460 tests (3,103 in `src/`, 2,357 across
164 files in `tests/`), and the guard's cost grew with it. The tests that need
it are still a small set.

Contributors and agents iterating on a PR pay the wait on every fix-and-push
round. Coverage is also not part of what the required check waits on today, so
a round can go green while the coverage run is still going or has failed.

## Goals

- The job carrying the whole suite's result finishes in at most 15 minutes, a
  third of today's time or less.
- Coverage comes from the same run as the verdict, so the two can't disagree.
- The required check keeps its name and keeps meaning "the whole suite passed".
- Local verification runs tests with the same isolation CI uses.
- A new test that touches process-wide state is isolated from other tests by
  the runner, without its author doing anything special.

## User Stories

- As a contributor fixing a review comment on a koto PR, I want the required
  check to report the whole suite's result within 15 minutes of my push, so that
  I can push the next fix in the same sitting.
- As a maintainer reviewing a PR, I want the Codecov report to come from the
  same run as the test verdict, so that the coverage number describes the tests
  that actually passed.
- As an agent running koto's verification map before pushing, I want the test
  entries to isolate tests the way CI does, so that a test that depends on its
  own HOME or working directory behaves the same locally and on the runner.
- As a contributor adding a test that sets an environment variable or redirects
  HOME, I want it to run isolated from other tests by default, so that it can't
  break an unrelated test running at the same time.

## Requirements

### Functional

- **R1. Parallel, isolated test execution in CI.** The full suite of the `koto`
  package runs in CI with each test in its own process and with the runner's
  concurrency left at the runner's CPU count or higher (not pinned to 1).
- **R2. One run for verdict and coverage.** A single CI job runs the full suite
  once under coverage instrumentation, with `--locked` and `--all-features`, and
  produces both the pass/fail result and the lcov report uploaded to Codecov. No
  second full-suite run remains in `validate.yml`. Doctests, if any exist, run in
  the same job as a separate step; today none are active, since every code fence
  in doc comments is `json`, `text` or `ignore`.
- **R3. Required check keeps its name and gates on the suite.** The job named
  `validate` remains the one aggregate check. Its `needs` list and its result
  check name the full-suite job, and `validate` fails when that job's result is
  anything but `success` (failure, cancelled or skipped). Every other job it
  waits on today is still waited on, with today's result handling. No workflow
  file still refers to the removed Unit Tests or Coverage job ids.
- **R4. Coverage upload stays non-blocking.** A failure to upload the report to
  Codecov does not fail the full-suite job or `validate`. A failure of any test
  does.
- **R5. Same test set.** The set of test names executed by the new full-suite
  job equals the set executed by the last `--all-features` Coverage run on
  `main` before the change (a superset of the old Unit Tests run, which used
  default features). No test is removed, newly marked `#[ignore]`, filtered out
  by runner configuration, or given a retry. Tests that are `#[ignore]` today
  stay ignored.
- **R6. Serialization only where needed.** A test that fails when run
  concurrently with another, or that inspection shows writes a shared fixed
  location outside its own temporary directory, is serialized with the tests it
  conflicts with by runner configuration that names it. It is never skipped,
  removed or retried to make the suite pass.
- **R7. Tests that re-run their own binary keep working.** The tests that spawn
  their own test binary as a child process with `--test-threads=1` (for example
  in `tests/test_home_env.rs` and `tests/host_session_env.rs`) still pass and
  still pass that flag to the child.
- **R8. Stability tests.** The `koto-stability-tests` package's tests still run
  in CI, each in its own process, without `--test-threads=1`, and `validate`
  still fails when they fail.
- **R9. Local verification follows CI.** The `cargo-test` and `stability-tests`
  entries in `.claude/shirabe-extensions/verification-map.json` run the same
  tests with the same per-test isolation as CI when the parallel runner is
  installed. When it isn't, the entry runs the suite serially with `cargo test
  -- --test-threads=1`, as today, after printing a fallback notice to stderr that
  names the missing runner and says the run is serial. The entries keep their
  `scripts/without-session-env.sh` prefix, which `tests/verification_map_env.rs`
  checks. Every file under `.claude/` that contains the CI test command is
  updated to match.
- **R10. Single-thread notes corrected.** Comments under `src/` and `tests/`
  that mention `--test-threads=1` or running single-threaded are updated so they
  describe how the test is isolated after the change. The only remaining
  mentions are in tests that pass the flag to a child process they start.

### Non-functional

- **R11. Suite job time.** On the GitHub-hosted `ubuntu-latest` runner this
  public repository gets (4 vCPU), the full-suite job finishes in at most 15
  minutes of wall time on a run whose dependency cache was restored (the cache
  step reports a hit). Changes to cache configuration are allowed when they
  serve this target.
- **R12. Tool install is pinned and prebuilt.** The test runner and coverage
  tool are installed from version-pinned prebuilt releases; no `cargo install`
  step compiles them.
- **R13. No koto behavior change.** No line under `src/` changes outside
  `#[cfg(test)]` code and comments, and no change alters what the koto binary or
  library does at runtime.

## Acceptance Criteria

- [ ] `validate.yml` contains exactly one job that runs the full `koto` package
      suite, and its suite step runs under coverage instrumentation with
      `--locked` and `--all-features` (R2).
- [ ] The full-suite and stability-tests steps run each test in its own process
      and pass no `--test-threads=1` (or a concurrency of 1) to the runner (R1,
      R8).
- [ ] The required check is still named `validate`; its `needs` list and result
      check name the full-suite job and the stability-tests job; the result
      check fails when the full-suite job's result is not `success`; every other
      job listed before the change is still listed (R3, R8).
- [ ] `git grep` over `.github/` finds no reference to the old Unit Tests or
      Coverage job ids (R3).
- [ ] A run on a throwaway branch with one deliberately failing test in the
      `koto` package fails the full-suite job and `validate`, and a second run
      with one failing stability test fails `validate`; both run links are in the
      PR description (R3, R4, R8).
- [ ] A run on a throwaway branch with the Codecov upload pointed at a missing
      file still passes the full-suite job and `validate`; the run link is in the
      PR description (R4).
- [ ] The Codecov upload step uploads the lcov file the full-suite job's own
      coverage step wrote (R2).
- [ ] The PR description records the run id of the last `--all-features`
      Coverage run on `main` before the change, its executed and ignored test
      counts, and the same counts for the new full-suite job; the two sets of
      executed test names, diffed, show no test missing (R5).
- [ ] The diff adds no `#[ignore]`, removes no `#[test]` or `#[tokio::test]`,
      and the runner configuration sets no retries and no filter that excludes
      tests (R5, R6).
- [ ] Every test the runner configuration serializes is named there with a
      comment saying what it conflicts with (R6).
- [ ] The tests that re-run their own binary pass in the new job and still pass
      `--test-threads=1` to the child (R7).
- [ ] On a throwaway branch, two added tests, one setting a process environment
      variable and one asserting that variable is unset, both pass in the new job
      (R1).
- [ ] The full-suite job passes on three consecutive runs of the PR's final
      commit, each with a dependency cache hit, and each finishes in 15 minutes
      or less (R1, R6, R11).
- [ ] No workflow step installs the test runner or coverage tool with `cargo
      install`; the install step names a pinned version (R12).
- [ ] The diff touches no non-test, non-comment line under `src/` (R13).
- [ ] The verification map's `cargo-test` and `stability-tests` entries keep the
      `scripts/without-session-env.sh` prefix and `cargo test --test
      verification_map_env` passes; run with the parallel runner absent from
      PATH, the `cargo-test` entry prints the fallback notice to stderr and runs
      `cargo test -- --test-threads=1` (R9).
- [ ] `git grep -- '--test-threads=1' .claude/` matches only fallback commands
      (R9).
- [ ] `git grep -n -e 'test-threads' -e 'single-thread' -e 'single thread' --
      src tests` matches only tests that pass the flag to a child process they
      start (R7, R10).

## Out of Scope

- Changing koto's engine or CLI behavior. This is CI and test-harness work.
- Removing, skipping, merging or weakening tests to save time.
- Speeding up the other CI jobs (compat checks, clippy, MSRV, audit), which take
  three or four minutes and don't set the round time.
- Larger runners, or build caching as the main lever. They leave the
  single-thread double run in place.
- Splitting the suite across several machines. It's a later option if one
  runner still can't meet R11.
- The release-only retention-cost test, the pinned-version compat test and the
  `/bin/sh` reproduction job, which run single targeted tests and don't need
  parallelism.
- The cloud integration job, which uses a real bucket and stays as it is.
- Changing how `validate` treats a skipped or cancelled result for jobs other
  than the full-suite job.

## Known Limitations

- The suite has only ever run in one fixed order on one thread. A test that
  quietly depends on an earlier test's side effects will only show up on the
  first parallel runs; R6 says how such a test is handled, and the
  three-consecutive-runs criterion is the check for it.
- Coverage instrumentation slows the run. A plain parallel run would be faster,
  but R2 trades that for one run instead of two.
- Timing-sensitive tests may behave differently under more CPU contention.
  R5 rules out retries, so any such test has to be fixed or serialized.
- The development host is shared, so the full suite is verified only in CI;
  local checks of this change run named test targets only.

## Decisions and Trade-offs

- **One instrumented run instead of a fast plain run plus coverage.** Keeping
  two runs would give a faster verdict but still spend two runners on the same
  tests and leave coverage able to disagree with the verdict. One run makes
  coverage part of the required check. Chosen because the brief's goal is the
  round, not the fastest single job.
- **Verdict uses `--all-features`.** Today Unit Tests runs default features and
  Coverage runs all features. The coverage run is a superset and already passes
  without cloud credentials, so the merged run keeps `--all-features` and the
  baseline for R5 is the coverage run.
- **Fifteen-minute target.** Recent runs and a 4-vCPU runner put a realistic
  instrumented parallel run at 10 to 15 minutes. A tighter target would push
  toward sharding or larger runners, which are out of scope.
- **The suite job fails `validate` on cancelled or skipped too.** Today
  `validate` fails only on `failure`. For the one job that carries the suite, a
  skipped or cancelled run would otherwise read as a pass, which is the gap this
  feature closes for coverage.
- **Local fallback instead of a hard prerequisite.** Requiring the parallel
  runner locally would break agents on hosts without it. Falling back to the
  serial run with a notice keeps the map usable everywhere while giving CI's
  isolation wherever the runner is installed.
- **Serialize by configuration, never retry.** Retries would hide real
  order-dependence or flakiness, which is the same as weakening a test.
