---
schema: brief/v1
status: Draft
problem: |
  A koto pull request waits about 33 minutes for its test verdict, because the
  whole suite runs on one thread, twice per CI round, to keep tests that share
  process state from racing. Every fix-and-push round pays that wait.
outcome: |
  A contributor pushing to a koto PR gets the required verdict and the coverage
  report in a small fraction of today's round time, from the same tests, none
  weakened or removed, so a fix-push-check loop takes minutes, not half an hour.
---

# BRIEF: koto CI parallel tests

## Status

Draft

## Problem Statement

Every push to a koto pull request starts a CI round whose verdict arrives about
33 minutes later. Two jobs set that time. Unit Tests runs the whole suite with
`cargo test --locked -- --test-threads=1`, and Coverage runs the same suite again
under `cargo llvm-cov ... -- --test-threads=1`. Each takes 33 minutes; the
next-slowest job in `.github/workflows/validate.yml` takes 3. So the round is
waiting on the same 5,460 tests, run one at a time, twice.

The single thread is a guard, not a preference. It came in when the suite had 520
tests, to stop tests that swap process-wide state from tripping over each other:
some tests in `src/` point HOME or XDG_CACHE_HOME at a scratch directory, change
the working directory, or set an environment variable for the whole process, and
their own comments say they rely on running alone. The suite has grown tenfold
since. The guard's cost grew with it, while the reason for it stayed confined to
a small number of tests.

The people who pay are whoever iterates on a PR: a contributor fixing a review
comment, or an agent working a red check, waits half an hour to learn whether a
one-line change passed. A slow round also invites batching several fixes into
one push, which makes a failure harder to pin on a cause.

## User Outcome

A contributor or agent pushing to a koto pull request learns whether the change
passed in minutes rather than half an hour. The required `validate` check keeps
its name and still means "the whole suite passed", so nobody's branch protection
or habits have to change.

Coverage still reaches Codecov from the same round, and the suite that ran is the
suite that runs today: every test is still there and still checks what it checked
before. A reader comparing a PR's test count before and after this work sees the
same number.

An agent running koto's verification steps locally gets the same isolation CI
relies on, so a test that passes in one place does not fail in the other because
the two ran it differently.

## User Journeys

### Contributor waits on a PR round

A contributor pushes a fix to an open koto pull request. CI starts, and the
`validate` check turns green or red within a few minutes, carrying the result of
the whole suite. The contributor reads the failure, pushes again, and gets the
next verdict without switching to other work in between.

### Maintainer reads coverage

A maintainer reviewing a PR opens the Codecov report for it. The report comes
from the same round as the test verdict and covers the same tests, so the
coverage number is not a second, separately timed run that can disagree with the
verdict.

### Agent runs the verification map locally

An agent working a koto change runs the `cargo-test` and `stability-tests`
entries from `.claude/shirabe-extensions/verification-map.json` before pushing.
The tests run with the same isolation CI uses, so a test that depends on its own
HOME or working directory behaves the same locally as on the runner.

### Test author adds a state-touching test

A contributor writes a new test that sets an environment variable or points HOME
at a scratch directory. It runs isolated from every other test without the author
having to know about a single-thread convention or take a shared lock, and it
cannot quietly break an unrelated test running at the same time.

## Scope Boundary

### In scope

- The Unit Tests and Coverage jobs in `.github/workflows/validate.yml`, including
  whether the test verdict and the coverage report come from one run.
- The `validate` aggregate job's needs list, kept under the same name as the one
  required check.
- The Stability Tests job and the verification map entries that use
  `--test-threads=1`.
- Test-harness changes needed for the suite to run in parallel safely: the tests
  that swap HOME, XDG_CACHE_HOME, the working directory or process environment,
  and the comments that tell readers those tests rely on a single thread.

### Out of scope

- Any change to koto's engine or CLI behavior. This is CI and test-harness work;
  a fix that only works by changing what koto does at runtime belongs elsewhere.
- Removing, skipping, merging or weakening tests to save time. The suite that
  runs stays the suite that runs today.
- Speeding up the other CI jobs (compat checks, clippy, MSRV, audit). They take
  three minutes or less and are not what sets the round time.
- Build caching or larger runners as the main lever. They may help at the
  margins, but they leave the single-thread double run in place.
- Splitting the suite across several machines (sharding). It adds coordination
  the verdict then has to reassemble, and is a later option if one machine is
  still too slow.

## Open Questions

- Whether Coverage becomes part of what `validate` waits on once the test verdict
  comes from the coverage-instrumented run, or stays a separate, non-required
  signal.
- How much of the remaining round time the PRD should target as "done", given
  that an instrumented run is slower than a plain one.

None of these questions block the brief.
