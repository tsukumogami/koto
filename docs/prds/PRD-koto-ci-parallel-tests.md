---
schema: prd/v1
status: Draft
problem: |
  A koto pull request waits about 33 minutes for its test verdict. CI runs the
  whole suite on one thread, twice per round (Unit Tests and Coverage, 34 to 41
  minutes each in recent runs, against 3 to 4 for every other job), to keep a
  small number of tests that swap process-wide state from racing. The guard came
  in when the suite had 520 tests; it now has about 5,460.
goals: |
  The required `validate` check reports the whole suite's result, with coverage
  from the same run, in at most 15 minutes on the standard runner, with every
  test that runs today still running and still checking what it checks.
upstream: docs/briefs/BRIEF-koto-ci-parallel-tests.md
---

# PRD: koto CI parallel tests

## Status

Draft

## Problem Statement

Every push to a koto pull request starts a CI round whose test verdict arrives
about 33 minutes later. Two jobs in `.github/workflows/validate.yml` set that
time: Unit Tests runs `cargo test --locked -- --test-threads=1`, and Coverage
runs the same suite again under `cargo llvm-cov --all-features ... --
--test-threads=1`. Recent runs put each at 34 to 41 minutes; the next-slowest
job takes 3 to 4. The round waits on the same tests, run one at a time, twice.

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

- A contributor learns whether a push passed the whole suite in minutes, not
  half an hour.
- Coverage comes from the same run as the verdict, so the two can't disagree.
- The required check keeps its name and keeps meaning "the whole suite passed".
- Local verification runs tests with the same isolation CI uses.
- A new test that touches process-wide state is isolated without its author
  having to know about a single-thread convention.

## User Stories

- As a contributor fixing a review comment on a koto PR, I want the required
  check to report the whole suite's result within a few minutes of my push, so
  that I can push the next fix without switching to other work.
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
  package runs in CI with more than one test executing at a time, and with each
  test isolated from every other test's process-wide state (environment
  variables, HOME, XDG_CACHE_HOME, working directory).
- **R2. One run for verdict and coverage.** A single CI job runs the full suite
  once under coverage instrumentation and produces both the pass/fail result and
  the lcov report uploaded to Codecov. No second full-suite run remains in
  `validate.yml`.
- **R3. Required check unchanged in name, complete in coverage.** The job named
  `validate` remains the one aggregate check. It waits on the job that carries
  the full-suite result, and it fails when that job fails. Every job it waits on
  today, other than the replaced Unit Tests job, is still waited on.
- **R4. Coverage upload stays non-blocking.** A failure to upload the report to
  Codecov does not fail the suite job or `validate`. A failure of any test does.
- **R5. Same test set.** The set of tests executed in CI after the change equals
  the set executed before it: same feature set as today's coverage run
  (`--all-features`), no test removed, newly ignored, filtered out, or skipped,
  and no retry that lets a failing test pass on a later attempt. Tests that are
  `#[ignore]` today stay ignored. Any doctest that `cargo test` would run is
  still run.
- **R6. Serialization only where needed.** A test that still cannot run
  concurrently with some other test (for example one that writes a fixed path
  outside its own temporary directory) is serialized with the tests it
  conflicts with, by configuration that names it. It is never skipped, removed
  or retried to make the suite pass.
- **R7. Tests that re-run their own binary keep working.** The tests that spawn
  their own test binary as a child process with `--test-threads=1` (for example
  in `tests/test_home_env.rs` and `tests/host_session_env.rs`) still pass and
  still check what they check.
- **R8. Stability tests.** The `koto-stability-tests` package's tests still run
  in CI and are still waited on by `validate`, with the same per-test isolation
  as R1.
- **R9. Local verification follows CI.** The `cargo-test` and `stability-tests`
  entries in `.claude/shirabe-extensions/verification-map.json` run the same
  tests with the same isolation as CI when the parallel runner is installed.
  When it isn't, the entry still runs the suite (serially, as today) and says
  that it fell back. The entries keep their `scripts/without-session-env.sh`
  prefix, which `tests/verification_map_env.rs` checks. Any other file in the
  repo that copies the CI test command (such as
  `.claude/shirabe-extensions/work-on.md`) is updated to match.
- **R10. Single-thread notes corrected.** Comments in test code that say a test
  relies on `--test-threads=1` are updated or removed so they describe how the
  test is isolated after the change.

### Non-functional

- **R11. Round time.** On the standard GitHub-hosted `ubuntu-latest` runner, the
  job carrying the full-suite result finishes in at most 15 minutes of wall
  time on a warm cache, measured on the PR that makes the change and on the next
  run of `main`.
- **R12. Tool install is fast and pinned.** The test runner and coverage tool
  are installed from prebuilt, version-pinned releases, not compiled from
  source on a cache miss.
- **R13. No koto behavior change.** No file under `src/` changes outside
  `#[cfg(test)]` code and comments, and no change alters what the koto binary or
  library does at runtime.
- **R14. Shared-host safety.** Verification of this change on a shared
  development host runs only targeted tests; the full suite runs only in CI.

## Acceptance Criteria

- [ ] `validate.yml` contains exactly one job that runs the full `koto` package
      suite, and that job runs it under coverage instrumentation (R2).
- [ ] No step in `validate.yml` passes `--test-threads=1` to a full-suite or
      stability-tests run (R1, R8).
- [ ] The full-suite job's log shows more than one test running at a time and
      each test in its own process, or the equivalent isolation the chosen
      runner reports (R1).
- [ ] The required check is still named `validate`; its `needs` list and its
      failure check both name the full-suite job and the stability-tests job,
      and every other job listed there before the change is still listed (R3, R8).
- [ ] On a throwaway branch, a deliberately failing test makes the full-suite
      job and `validate` fail (R3, R4).
- [ ] With the Codecov step forced to fail, the full-suite job and `validate`
      still pass (R4).
- [ ] The Codecov report for the PR's run is produced from the full-suite job's
      lcov output (R2).
- [ ] The number of tests executed in the new full-suite job, plus any doctests
      it runs, equals the number executed by the last `--all-features` coverage
      run before the change, and the ignored count is unchanged; the PR
      description states both numbers (R5).
- [ ] No test is newly marked `#[ignore]`, deleted, filtered out by runner
      configuration, or given a retry count (R5, R6).
- [ ] Every test serialized by runner configuration is named in that
      configuration with a comment saying what it conflicts with (R6).
- [ ] The tests that re-run their own binary pass in the new job (R7).
- [ ] The full-suite job finishes in 15 minutes or less on the change's PR run
      and on the next `main` run (R11).
- [ ] Tool installs use pinned versions of prebuilt binaries (R12).
- [ ] The diff touches no non-test, non-comment line under `src/` (R13).
- [ ] The verification map's `cargo-test` and `stability-tests` entries run with
      the parallel runner when present, fall back to serial `cargo test` with a
      printed notice when absent, keep the `scripts/without-session-env.sh`
      prefix, and `cargo test --test verification_map_env` passes (R9).
- [ ] No comment in the repo still says a test relies on `--test-threads=1`
      unless that test passes the flag to a child process it starts itself
      (R7, R10).

## Out of Scope

- Changing koto's engine or CLI behavior. This is CI and test-harness work.
- Removing, skipping, merging or weakening tests to save time.
- Speeding up the other CI jobs (compat checks, clippy, MSRV, audit), which take
  three or four minutes and don't set the round time.
- Build caching changes or larger runners as the main lever. They leave the
  single-thread double run in place.
- Splitting the suite across several machines. It's a later option if one
  runner still can't meet R11.
- The release-only retention-cost test, the pinned-version compat test and the
  `/bin/sh` reproduction job, which run single targeted tests and don't need
  parallelism.
- The cloud integration job, which uses a real bucket and stays as it is.

## Known Limitations

- The suite has only ever run in one fixed order on one thread. A test that
  quietly depends on an earlier test's side effects will only show up on the
  first parallel run; R6 says how such a test is handled, but finding them is
  part of the work.
- Coverage instrumentation slows the run. The 15-minute target assumes a 4-vCPU
  runner and an instrumented build; a plain run would be faster, but R2 trades
  that for one run instead of two.
- Timing-sensitive tests may behave differently under more CPU contention.
  R5 rules out retries, so any such test has to be fixed or serialized.

## Decisions and Trade-offs

- **One instrumented run instead of a fast plain run plus coverage.** Keeping
  two runs would give a faster verdict but still spend two runners on the same
  tests and leave coverage able to disagree with the verdict. One run makes
  coverage part of the required check. Chosen because the brief's goal is the
  round, not the fastest single job.
- **Verdict uses `--all-features`.** Today Unit Tests runs default features and
  Coverage runs all features. The coverage run is a superset and already passes
  without cloud credentials, so the merged run keeps `--all-features` rather
  than dropping coverage of the feature-gated tests.
- **Fifteen-minute target, not "a few minutes".** Research on recent runs and a
  4-vCPU runner put a realistic instrumented parallel run at 10 to 15 minutes.
  A tighter target would push toward sharding or larger runners, which are out
  of scope.
- **Local fallback instead of a hard prerequisite.** Requiring the parallel
  runner locally would break agents on hosts without it. Falling back to serial
  `cargo test` with a notice keeps the map usable everywhere while giving CI's
  isolation wherever the runner is installed.
- **Serialize by configuration, never retry.** Retries would hide real
  order-dependence or flakiness, which is the same as weakening a test.
