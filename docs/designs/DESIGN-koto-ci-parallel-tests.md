---
schema: design/v1
status: Planned
problem: |
  validate.yml runs the koto suite twice per round, each time on one libtest
  thread, because a few tests change HOME, the cache directory, the working
  directory or process environment, and parallel threads in one process would
  race on them. Each run takes 34 to 41 minutes, and coverage is not gated.
decision: |
  Run tests under cargo-nextest, one process per test, in a single `tests` job
  that runs `cargo llvm-cov nextest --locked --all-features --profile ci`,
  runs doctests, and uploads lcov to Codecov without letting the upload fail
  the job. `validate` gates on that job's success. A `ci` profile in
  `.config/nextest.toml` forbids retries, the same file names any serialized
  test, and
  `scripts/run-tests.sh` gives the verification map the same isolation with a
  serial fallback.
rationale: |
  Process-per-test isolation removes the races without editing the tests that
  cause them, and `cargo llvm-cov nextest` gives verdict and coverage from one
  run. Locks in tests were rejected because they need edits to every racing
  test and every reader. A second fast plain run was rejected because it runs
  the suite twice and lets coverage disagree with the verdict.
upstream: docs/prds/PRD-koto-ci-parallel-tests.md
user_visible_surface: false
decision_provenance: inline-resolved
---

# DESIGN: koto CI parallel tests

## Status

Planned

## Context and Problem Statement

`.github/workflows/validate.yml` runs the `koto` package's test suite twice per
CI round. The `unit-tests` job runs `cargo test --locked -- --test-threads=1`
with default features, and the `coverage` job runs `cargo llvm-cov
--all-features --lcov --output-path lcov.info -- --test-threads=1` and uploads
the result to Codecov. Each takes 34 to 41 minutes; every other job takes 3 to
4. `coverage` is not in the `validate` aggregate job's `needs`, so the one
required check doesn't wait for it. `stability-tests` runs `cargo test -p
koto-stability-tests -- --test-threads=1` as well.

The technical reason for `--test-threads=1` is that libtest runs every test in
one binary as a thread of one process, and a handful of tests change state that
belongs to the whole process. `ScratchHome` in `src/cli/mod.rs`, `HomeGuard` in
`src/workflows_surface/materialize.rs` and an `XDG_CACHE_HOME` swap in
`src/cli/init_child.rs` and `src/cli/mod.rs` reassign HOME or the cache
directory without a lock; `src/config/resolve.rs` changes the working directory
and writes environment variables (partly behind `process_env_lock()`);
`src/engine/reentrancy.rs` sets a process environment variable in `mark_tick`;
and `tests/decider_proxy.rs`, `tests/decider_client.rs` and
`tests/host_session_env.rs` set environment variables. Two of these running in
parallel threads see each other's values. In `tests/`, every file already gets
its own HOME from a pre-main hook in `tests/support/test_home.rs`, but that is
per process, not per test.

The PRD asks for (its R-numbers in parentheses):

- every test isolated from the others' process-wide state, with more than one
  test running at a time (R1);
- one CI job that runs the full suite once under coverage instrumentation, with
  `--locked` and `--all-features`, and produces both the verdict and the lcov
  report, with any doctests run in the same job (R2);
- `validate` keeping its name, naming that job in its `needs` and failing when
  its result is anything but `success`, while every other job stays gated as
  today and nothing references the old job ids (R3);
- a Codecov upload that can't fail the job, while any test failure does (R4);
- the same set of test names as the last `--all-features` coverage run, with
  nothing removed, newly ignored, filtered or retried (R5);
- serialization, only for a test shown to conflict, by configuration that
  names it (R6);
- the tests that re-run their own binary with `--test-threads=1` still working
  (R7); stability tests isolated the same way and still gating (R8);
- the verification map's `cargo-test` and `stability-tests` entries matching CI
  when the runner is installed and falling back to the serial run with a
  notice on stderr when it isn't, keeping the `scripts/without-session-env.sh`
  prefix, with the other `.claude/` copies of the command updated (R9);
- comments that claim a single thread updated (R10);
- the full-suite job at 15 minutes or less on a 4-vCPU `ubuntu-latest` runner
  with a cache hit (R11), tools installed from pinned prebuilt releases (R12),
  and no runtime behavior change in `src/` (R13).

## Decision Drivers

- **Isolation without editing tests.** The fix must remove the races the
  single thread guards against, without rewriting the tests that cause them,
  since test edits risk weakening them (R1, R5, R13).
- **One run, two outputs.** Verdict and coverage must come from the same run
  (R2), and the run must keep `--locked` and `--all-features`.
- **The required check can't silently lose the suite** (R3).
- **No retries, no filters** (R5, R6).
- **Wall time on a 4-vCPU runner** under instrumentation (R11).
- **Local runs keep working on hosts without new tools** (R9).
- **Small, reviewable change**: one workflow, one config file, one script, the
  verification map and comments.

## Considered Options

### Decision 1: How tests get per-test isolation

Resolved inline.

The races come from libtest's model of many test threads in one process.
Something has to give each test its own copy of HOME, the working directory and
the environment.

Key assumptions:

- No test depends on running in the same process as another test (a shared
  `static` written by one test and read by another). The order-dependence
  criterion in the PRD is the check.
- The tests that start their own binary as a child (`tests/test_home_env.rs`
  and its peers) only need a libtest binary to exist, which nextest still
  builds.

#### Chosen: cargo-nextest, one process per test

cargo-nextest runs each test as its own process from the libtest binaries cargo
builds. A test that reassigns HOME or changes directory changes only its own
process, so every race listed above goes away without touching the tests.
nextest runs tests in parallel up to the CPU count by default, supports named
test groups for any test that still conflicts through the filesystem, and is
supported directly by `cargo llvm-cov nextest`, which collects coverage from
every per-test process. The pre-main HOME hook in `tests/support/test_home.rs`
runs once per process, so under nextest each test gets its own HOME as well.

#### Alternatives Considered

**Keep `cargo test` and add locks.** Put every state-changing test behind a
shared mutex (extending `process_env_lock()`) or the `serial_test` crate. This
needs an edit to each racing test and to any test that reads the same
variables, the set has to be found by inspection and kept complete by every
future author, and a missed reader is a flaky test. Rejected: it edits tests to
serialize them, which is what the PRD wants to avoid, and it serializes more
than nextest's process boundary does.

**Keep `cargo test` and split the suite by binary.** Run the integration
binaries in parallel (`cargo test` runs binaries one after another but threads
within each) and keep only the lib binary single-threaded. The lib binary holds
3,103 of the 5,460 tests and most of the races, so the slow part stays slow.
Rejected.

### Decision 2: The CI job that produces the verdict and coverage

Resolved inline.

`unit-tests` and `coverage` have to become one job that `validate` gates on.

Key assumptions:

- The `--all-features` run already passes without cloud credentials, since
  today's `coverage` job runs it on every PR.
- `--all-features` is a strict superset of the default-feature run that
  `unit-tests` does today: the only feature, `cloud-integration-tests`, gates
  one file (`tests/cloud_integration_test.rs`, `#![cfg(feature = ...)]`), and
  no code under `src/` or `tests/` has a `cfg(not(feature ...))` branch. So
  dropping the separate default-feature run loses no test and no code path.
- The repository has no active doctests: every code fence in a doc comment is
  `json`, `text` or `ignore`.

#### Chosen: a new `tests` job running `cargo llvm-cov nextest`, plus a doctest step

Replace both jobs with one job, id `tests`, name `Tests and Coverage`:

1. checkout, `dtolnay/rust-toolchain@stable` with `llvm-tools-preview`,
   `Swatinem/rust-cache@v2`;
2. `taiki-e/install-action`, pinned by commit SHA, installing `cargo-llvm-cov`
   and `cargo-nextest` at pinned versions (prebuilt binaries, no compile);
3. `cargo llvm-cov nextest --locked --all-features --profile ci --lcov
   --output-path lcov.info`, the verdict and the report in one run;
4. `cargo test --locked --all-features --doc`, which runs doctests if any are
   ever added (nextest and `llvm-cov nextest` don't run them);
5. `codecov/codecov-action@v4` with `files: lcov.info` and
   `fail_ci_if_error: false`, as today.

In `validate`, `unit-tests` is replaced by `tests` in `needs`, and its line in
the result check becomes a test for `!= "success"`, so a cancelled or skipped
suite fails the required check. The other lines keep their `= "failure"` test.

The new job id is deliberate: R3 asks that nothing still refer to the old ids,
and a new id makes a stale reference fail loudly (`needs` on a missing job makes
the workflow invalid) instead of pointing at a job that now does something
else.

#### Alternatives Considered

**Two jobs: fast plain nextest for the verdict, instrumented nextest for
coverage, both gating.** Gives the fastest verdict, but runs the suite twice and
leaves room for the two to disagree. Rejected by the PRD's one-run requirement.

**Split instrumented run and report steps** (`cargo llvm-cov nextest
--no-report`, then `cargo llvm-cov report --lcov` with `continue-on-error`).
Keeps a report-generation fault from reddening the check. Rejected: a report
step that can fail silently means coverage can go missing without anyone
noticing, and the report step has not been a source of failures in the current
`coverage` job.

**Keep the `unit-tests` id.** Smallest diff to `validate`, but the id would
then name a job that also produces coverage, and R3's "no reference to the old
ids" criterion would be meaningless. Rejected.

### Decision 3: Where runner configuration lives

Resolved inline.

Retries, failure handling and any serialization have to be written down where a
reviewer can see them.

#### Chosen: `.config/nextest.toml` with a `ci` profile

nextest reads `.config/nextest.toml` at the workspace root. The file defines:

- `[profile.ci]` with `retries = 0`, `fail-fast = false` (report every failure
  in one run), `failure-output = "immediate-final"`, and a `slow-timeout` that
  warns (`period = "120s"`) without terminating, so a slow test is reported but
  never killed into a pass or fail by the runner;
- `[profile.ci.junit]` writing `junit.xml`, which the job uploads as an
  artifact so the executed test names can be diffed against the baseline (R5);
- `[test-groups]`, empty at first. A test is added to a group with
  `max-threads = 1` only when it fails under parallel runs or inspection shows
  it writes a shared fixed location, and each `[[profile.default.overrides]]`
  entry naming it carries a comment saying what it conflicts with (R6). The
  overrides sit under `default` on purpose: every nextest profile, `ci`
  included, inherits them, so local and CI runs serialize the same tests;
- a top-level `nextest-version` minimum, so an older local nextest is refused
  clearly instead of misreading the file.

No filter, `default-filter` or `retries` value other than 0 appears in the file.

#### Alternatives Considered

**Command-line flags only.** `--retries 0 --no-fail-fast` on the CI command
works for the profile settings, but test groups can only be declared in the
config file, and the local wrapper would have to repeat the flags. Rejected.

### Decision 4: Local verification with and without nextest

Resolved inline.

The verification map's commands run on agent hosts, some of which won't have
nextest installed.

#### Chosen: a `scripts/run-tests.sh` wrapper that falls back

`scripts/run-tests.sh` takes cargo package arguments (for example `-p
koto-stability-tests`). When `cargo nextest --version` succeeds and reports a
version at or above the `nextest-version` minimum in `.config/nextest.toml`,
it runs `cargo nextest run --locked --profile ci "$@"`, then, when it was
called with no arguments (the `koto` package, the workspace default),
`cargo test --locked --doc`. Otherwise it prints `run-tests: cargo-nextest not
found; running cargo test serially (--test-threads=1)` to stderr and runs
`cargo test --locked "$@" -- --test-threads=1`, which is today's command. Like today's map, it runs with default features; the
isolation, not the feature set, is what it shares with CI, and the one
feature-gated file needs cloud credentials anyway. The
`cargo-test` and `stability-tests` map entries become `bash
scripts/without-session-env.sh env CARGO_INCREMENTAL=0 bash
scripts/run-tests.sh` (with `-p koto-stability-tests` for the second), keeping
the prefix `tests/verification_map_env.rs` checks. The entry whose paths
trigger those commands gains `scripts/run-tests.sh` and `.config/nextest.toml`.

#### Alternatives Considered

**Call nextest directly in the map and require it.** Simpler entries, but an
agent host without nextest would fail the default checks on every change.
Rejected by R9.

**Keep the map on `cargo test` serially.** No new script, but local runs would
keep a different isolation model from CI, which is the gap the PRD's third user
story names. Rejected.

## Decision Outcome

Tests run under cargo-nextest, one process per test, so the process-wide state
that `--test-threads=1` protected is private to each test without editing any
test (Decision 1). CI gets one `tests` job that runs `cargo llvm-cov nextest`
with `--locked --all-features` under a `ci` profile that forbids retries,
writes the result and the lcov report, runs doctests separately, and uploads to
Codecov without letting an upload fault fail the job; `validate` gates on it
with a strict `success` check (Decision 2). The profile, the JUnit output used
for the test-name parity check and any serialization group live in
`.config/nextest.toml`, where a reviewer sees every exception by name (Decision
3). Locally, `scripts/run-tests.sh` gives the same isolation when nextest is
installed and today's serial run otherwise, so the verification map works on
every host (Decision 4). The stability-tests job uses nextest the same way,
without coverage.

## Solution Architecture

### Overview

Five files change, plus comments: `.github/workflows/validate.yml`, a new
`.config/nextest.toml`, a new `scripts/run-tests.sh`,
`.claude/shirabe-extensions/verification-map.json` and
`.claude/shirabe-extensions/work-on.md`. Comments in `src/` and `tests/` that
mention `--test-threads=1` are updated. No Rust code changes outside comments.

### Components

- **`tests` job** (`validate.yml`, replaces `unit-tests` and `coverage`):

  ```yaml
  tests:
    name: Tests and Coverage
    runs-on: ubuntu-latest
    timeout-minutes: 45
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: llvm-tools-preview
      - uses: Swatinem/rust-cache@v2
      - uses: taiki-e/install-action@<pinned-sha>  # vX.Y.Z
        with:
          tool: cargo-llvm-cov@<version>,cargo-nextest@<version>
          fallback: none
      - name: Run tests with coverage
        run: cargo llvm-cov nextest --locked --all-features --profile ci --lcov --output-path lcov.info
      - name: Run doctests
        run: cargo test --locked --all-features --doc
      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: junit
          path: target/**/nextest/ci/junit.xml
          retention-days: 7
          if-no-files-found: warn
      - name: Upload to Codecov
        uses: codecov/codecov-action@v4
        with:
          files: lcov.info
          fail_ci_if_error: false
  ```

  The comment that today ties `unit-tests` to the `/work-on` verification map
  moves to this job.

- **Job permissions**: `permissions: contents: read` on the `tests` and
  `stability-tests` jobs only, so no other job's token changes.

- **`stability-tests` job**: the same install step (nextest only), then `cargo
  nextest run --locked -p koto-stability-tests --profile ci`.

- **`validate` job**: keeps `if: always()`, so it runs and reports even when a
  need failed. `needs` lists `tests` in place of `unit-tests`; the result
  check's line for it is `[ "${{ needs.tests.result }}" != "success" ]`.

- **`.config/nextest.toml`**:

  ```toml
  [profile.ci]
  retries = 0
  fail-fast = false
  failure-output = "immediate-final"
  slow-timeout = { period = "120s" }

  [profile.ci.junit]
  path = "junit.xml"

  # A test goes in a group only when it fails under parallel runs or writes a
  # shared fixed location; each override says what it conflicts with.
  [test-groups]
  ```

- **`scripts/run-tests.sh`**: the fallback wrapper from Decision 4, `set -euo
  pipefail`, no arguments beyond those passed to cargo.

### Key Interfaces

- `validate`'s result check: the only gate on the suite. Its contract becomes
  "fails unless `tests` succeeded"; for every other need it stays "fails if the
  need failed".
- The nextest `ci` profile: the contract that no test is retried or filtered.
- The verification map's command arrays: still prefixed by
  `scripts/without-session-env.sh`, checked by `tests/verification_map_env.rs`.
- Tests that start their own binary (`tests/test_home_env.rs`,
  `tests/host_session_env.rs`, `tests/session_store_env.rs`,
  `tests/decider_client.rs`, `tests/koto_command_env.rs`): they call
  `std::env::current_exe()` with `--exact <name> --test-threads=1`. Under
  nextest, `current_exe()` is still the libtest binary, so they keep working and
  keep that flag.

### Data Flow

A push starts the workflow. The `tests` job builds the instrumented test
binaries once, nextest lists every test in them and runs each in its own
process, up to four at a time, writing per-process coverage data. `cargo
llvm-cov` merges it into `lcov.info`. The doctest step runs. JUnit and lcov are
uploaded. `validate` reads `needs.tests.result` and fails unless it is
`success`.

For the parity check (R5), the PR author takes the test names from the last
`main` run of the old `coverage` job (its log lists each test) and from the new
job's `junit.xml`, diffs them, and records the run id, counts and diff result in
the PR description.

## Implementation Approach

### Phase 1: Runner configuration and local wrapper

Add `.config/nextest.toml` and `scripts/run-tests.sh`; switch the two
verification-map entries and update the map's path list and `work-on.md`'s
default-check list. Verify locally with targeted tests only:
`cargo test --test verification_map_env`, and `cargo nextest run -p
koto-stability-tests` (12 tests).

Deliverables:
- `.config/nextest.toml`
- `scripts/run-tests.sh`
- `.claude/shirabe-extensions/verification-map.json`
- `.claude/shirabe-extensions/work-on.md`

### Phase 2: CI jobs and gate proofs

Replace `unit-tests` and `coverage` with `tests`, move `stability-tests` to
nextest, update `validate`. Push and let CI run the full suite. Any test that
fails only under parallel runs is investigated: if it conflicts through the
filesystem it is added to a test group with a comment; if it has a real
order dependence, its test code is fixed in this PR. It is never retried or
skipped, and the PR doesn't merge while any such test is unresolved. Then run
the gate proofs on throwaway branches: a failing test, a failing stability
test, a broken Codecov input and the environment-variable pair.

Deliverables:
- `.github/workflows/validate.yml`

### Phase 3: Comments and evidence

Update the comments that mention `--test-threads=1` (for example
`src/workflows_surface/materialize.rs` near `HomeGuard` and `src/cli/mod.rs`
near `ScratchHome`) to say the isolation comes from nextest's process per test.
Collect the PR's evidence: the gate-proof run links from Phase 2, the parity
diff and three consecutive green runs with their times.

Deliverables:
- comment updates under `src/` and `tests/`
- PR description evidence

## Security Considerations

### External artifacts

The `tests` job installs cargo-nextest and cargo-llvm-cov as prebuilt binaries
through taiki-e/install-action, where it used to build cargo-llvm-cov from
crates.io source. The action is pinned by commit SHA and both tools are pinned
to exact versions, which must be versions the action's manifest carries
checksums for, so the downloaded binaries are verified. The Codecov uploader is
downloaded and verified by the Codecov action, as today. The job still runs the
pull request's own code, as the old jobs did, and it receives no secrets. The
files `.config/nextest.toml` and `scripts/run-tests.sh` are ordinary reviewed
repository files.

### Permissions

The job needs only read access to the repository. The workflow gains a
top-level `permissions: contents: read`, so a compromised action can't write to
the repository with the default token. The Codecov step passes no token, so
fork pull requests upload tokenless, and `fail_ci_if_error: false` keeps a
rejected upload from failing the job. `validate` fails unless `tests`
succeeded, so a cancelled or skipped suite can't pass the required check.

### Supply chain

The design adds install-action, two prebuilt tools and upload-artifact to the
chain. install-action is pinned by SHA, which is stricter than the tag pins the
other actions in `validate.yml` use; those tag pins are an existing, accepted
risk this design doesn't change, and that includes `codecov/codecov-action@v4`,
`actions/upload-artifact@v4` and `dtolnay/rust-toolchain@stable` in the new
job. `fallback: none` makes install-action fail rather than build a tool from
source when no verified prebuilt binary is available. Updating the pinned SHA
and tool versions is a manual edit, reviewed like any other workflow change.

### Runner use and checkout token

nextest's slow-timeout only warns, so the job carries `timeout-minutes: 45`;
a hung test, including one from a fork pull request, holds a runner for at
most that long. Checkout runs with `persist-credentials: false`, so the token
isn't left on disk for test code to read. The workflow triggers on
`pull_request`, not `pull_request_target`, so fork code never runs with
secrets; the rust cache is keyed per branch by `Swatinem/rust-cache`, as
today.

### Residual risk

The no-retry, no-filter contract lives in files a pull request can edit
(`.config/nextest.toml`, `scripts/run-tests.sh`, the workflow). Review is what
keeps a later change from adding a retry or a filter, as it is today for the
`--test-threads=1` flag; the parity record in this change's PR is a one-time
check, not a standing one.

### Data exposure

The lcov report goes to Codecov and holds coverage data for source that is
already public. The JUnit file is uploaded as a workflow artifact with
`retention-days: 7`. It lists test names and timings and may include output
from failing tests. Artifacts aren't masked the way logs are, which is
acceptable because the job receives no secrets and keeps no checkout token on
disk.

## Consequences

### Positive

- One run instead of two; the suite job should drop from 34 to 41 minutes to
  10 to 15 on four vCPUs.
- Coverage becomes part of the required check and comes from the run that
  produced the verdict.
- A cancelled or skipped suite can no longer read as a pass.
- New tests that touch HOME, the working directory or the environment are
  isolated by default, with no convention to learn.
- Every exception to parallel execution is named in one file.

### Negative

- Process-per-test adds start-up cost for each of about 5,460 tests.
- Coverage instrumentation now slows the verdict, not just the coverage report.
- A new third-party action and two tools join the CI supply chain.
- Hidden order dependence, if any, surfaces as new failures on the first runs.
- The doctest step compiles the library once more, without instrumentation.

### Mitigations

- Process start-up is milliseconds against tests that mostly spawn the koto
  binary already; the 15-minute target and three-run criterion measure the net
  effect.
- If instrumentation pushes the job past 15 minutes, the first place to look
  is the merge of per-process coverage data, which `cargo llvm-cov` does once
  at the end and which the job's step timings show separately; sharding stays
  the follow-up the PRD names. The target is checked before merge.
- The install action is pinned by commit SHA and the tools by version.
- Order-dependent tests are fixed or grouped per R6, with the reason written
  next to the group.
- The doctest step reuses the cached dependencies; if it proves costly while no
  doctests exist, it is still cheap insurance against a doctest silently not
  running.
