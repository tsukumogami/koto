---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-koto-ci-parallel-tests.md
milestone: "koto CI parallel tests"
step_count: 4
---

# PLAN: koto CI parallel tests

## Status

Active

## Scope Summary

Replace the two single-threaded full-suite CI runs with one coverage-instrumented
cargo-nextest run that the required `validate` check waits on, and give the
verification map the same per-test isolation with a serial fallback.

## Decomposition Strategy

Walking skeleton. Step 1 crosses the whole path a PR round takes, on the real
koto suite in the PR's own CI run: the nextest `ci` profile, the new `tests`
job with coverage and the Codecov upload, and `validate`'s gate on it. Whether
the suite passes in parallel is the main unknown, so it is answered first.
Step 2 proves every gate path and the test-name parity, step 3 moves local
verification onto the same isolation, and step 4 brings the comments and the
PR's evidence in line.

## Steps

### Step 1: A PR's CI runs the whole suite once, in parallel, with coverage, and validate waits on it

**Goal**: A pull request's own CI run executes the full koto suite one time, one process per test and more than one at a time, under coverage instrumentation, and the required `validate` check waits on that run. It crosses `.config/nextest.toml`'s `ci` profile, the new `tests` job (`cargo llvm-cov nextest --locked --all-features --profile ci`, the doctest step, the junit artifact and the Codecov upload) and `validate`'s `needs` list and strict success check, on the real koto suite in the PR's own CI run. The old `unit-tests` and `coverage` jobs are removed in this step.

**Acceptance Criteria**:

- [ ] The PR's CI run shows one job named "Tests and Coverage" that runs the whole suite under coverage instrumentation, and its log shows the command ran with `--locked` and `--all-features`
- [ ] The "Tests and Coverage" job's log has no `--test-threads=1` on its test command, and the nextest output shows more than one test running at once (a status line with a running count above 1)
- [ ] Each test runs in its own process: the nextest output lists every test by binary and name with its own result line, and the summary's started/passed counts match the number of tests listed
- [ ] The nextest output for the run names the `ci` profile, and the profile it prints has `retries` at 0 and no filter or default-filter: no test is shown as retried (no "TRY 2" lines) and the summary reports no skipped-by-filter tests
- [ ] The "Tests and Coverage" job runs the doctest step, uploads a `junit` artifact (junit.xml) that can be downloaded from the run, and reaches the Codecov upload step with the lcov report as its input
- [ ] A Codecov upload that errors does not turn the "Tests and Coverage" job red, while a failing test does
- [ ] The `validate` job's log shows `tests` among the jobs it needs, and its result check fails the job unless the `tests` result is exactly `success`, so a cancelled or skipped `tests` job leaves `validate` red
- [ ] `git grep -nE 'unit-tests|needs\.coverage|^  coverage:' -- .github/` returns no match: no job, `needs` entry or result check refers to the old `unit-tests` and `coverage` job ids
- [ ] Any test that failed in the PR's CI only when run in parallel is either listed in a named nextest test group whose override carries a comment saying what it conflicts with, or has its test code fixed; none is retried, skipped or filtered, and the final run is green with no such test unresolved

**Check**: Push the branch and open the PR, then wait for its CI run to finish and open the run page. Confirm the "Tests and Coverage" job is green. In its log, find the nextest line listing the `ci` profile and the running-test count, and confirm the count goes above 1, each test reports separately, and the summary shows no retried or filtered tests. Confirm the doctest step ran, the `junit` artifact is attached to the run, and the Codecov upload step ran. Open the `validate` job and confirm `tests` is in its needs and it is green. Then run `git grep -nE 'unit-tests|needs\.coverage|^  coverage:' -- .github/` in the branch and confirm it prints nothing (exit code 1). Record the run id and the "Tests and Coverage" duration in the PR description.

**Dependencies**: None

**Files**: `.github/workflows/validate.yml`, `.config/nextest.toml`

### Step 2: Every way the suite can fail turns validate red, and nothing else does

**Goal**: A contributor can trust the `validate` check: a failing koto test, a failing stability test, or a test that only breaks under parallel runs turns it red, while a Codecov upload fault leaves it green. The stability-tests job runs on nextest the same way the `tests` job does (one process per test, no `--test-threads=1`, read-only job token), and the new job is shown to execute every test name the last `--all-features` Coverage run on `main` did.

Step 1 landed the `tests` job (display name "Tests and Coverage") running `cargo llvm-cov nextest --locked --all-features --profile ci`, writing `target/**/nextest/ci/junit.xml` as the `junit` artifact, and `.config/nextest.toml` with the `ci` profile (`retries = 0`, `fail-fast = false`, no filter) and an empty `[test-groups]` table: locally all 5,449 tests passed in parallel with no test group needed. `stability-tests` still runs `cargo test -p koto-stability-tests -- --test-threads=1`; this step moves it onto nextest, reusing the `ci` profile or a profile beside it in the same file. The branch has not been pushed yet, so the PR's first hosted run is part of this step's evidence.

**Acceptance Criteria**:

- [ ] On a throwaway branch with one deliberately failing test in the `koto` package, the `tests` job result is `failure` and `validate` is red. The failing test is named in the `tests` job log.
- [ ] On a throwaway branch with one deliberately failing test in `koto-stability-tests`, the `stability-tests` job result is `failure` and `validate` is red.
- [ ] On a throwaway branch where the Codecov upload step points at a file that does not exist, the `tests` job result is `success` and `validate` is green, and the step log shows the upload was attempted and reported a problem.
- [ ] On a throwaway branch with two added tests, one that sets a process environment variable and one that asserts the same variable is unset, both pass in the `tests` job, and the job's JUnit output lists both as passed.
- [ ] The `stability-tests` job log shows nextest running all 12 stability tests, each reported individually as passed, with no `--test-threads=1` and no concurrency of 1 in the invocation.
- [ ] The `tests` and `stability-tests` job logs show the job token with `contents: read` and no other write scope. Jobs other than these two show the same token permissions they had before the change.
- [ ] The diff of test names executed by the new `tests` job (from `junit.xml`) against the names in the last `--all-features` Coverage run log on `main` before the change shows no name missing from the new run. The PR description records that baseline run id, its executed and ignored counts, and the same two counts for the new job.
- [ ] No test is newly `#[ignore]`d or removed, and no retry is used. The PR diff adds no `#[ignore]` and removes no `#[test]` or `#[tokio::test]`, and the `ci` profile's log header reports zero retries with no filter excluding tests. Tests that are `#[ignore]` on `main` still show as skipped, not run.
- [ ] Every test the nextest configuration serializes is named there with a comment saying what it conflicts with. If no test needed serializing, the configuration names none.
- [ ] The PR description links all four throwaway-branch runs, each with the job results and the `validate` result it produced. The throwaway changes are not on the PR's own branch, whose final run has `tests`, `stability-tests` and `validate` all green.

**Check**: Push four throwaway branches off the PR branch, one per case: (a) a `#[test]` in the `koto` package that panics, (b) a panicking test in `koto-stability-tests`, (c) the Codecov step's `files:` input set to a path that does not exist, (d) two tests in the `koto` package where one calls `std::env::set_var` on a unique variable name and the other asserts `std::env::var` for that name is `Err`. For each, open the Actions run and read the `tests`, `stability-tests` and `validate` results. Expected: (a) `tests` failure and `validate` red; (b) `stability-tests` failure and `validate` red; (c) `tests` success and `validate` green; (d) `tests` success with both new tests passed in the JUnit artifact and `validate` green. Paste the four run URLs and results into the PR description. For parity, find the last `--all-features` Coverage run on `main` before the change with `gh run list --workflow validate.yml --branch main`, save its log with `gh run view <id> --log` and the new run's `junit` artifact with `gh run download`, extract the executed test names from each into two sorted lists, and run `comm -23 old.txt new.txt`. It must print nothing. Record the baseline run id and the executed and ignored counts for both runs in the PR description. Finally run `git diff main... | grep -E '^\+.*#\[ignore'` and see no output, and check that `git grep -n retries .config/nextest.toml` shows only `retries = 0`.

**Dependencies**: Step 1

**Files**: `.github/workflows/validate.yml`, `.config/nextest.toml`

### Step 3: Local verification runs tests with CI's isolation and falls back to the serial run with a notice

The verification map's test entries go through a wrapper that uses nextest when it is installed and prints a fallback notice and runs serial cargo test when it is not.

### Step 4: The suite's notes and the PR's evidence match the new isolation

Comments that claim a single thread describe process-per-test isolation, and three consecutive timed green runs are recorded in the PR.

## Implementation Sequence

The steps run in order, each on top of the one before, all in one pull request.
