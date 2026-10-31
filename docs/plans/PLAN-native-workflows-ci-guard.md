---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-native-workflows-ci-guard.md
milestone: "a CI guard over the Claude Code conventions behind koto's /workflows rendering"
step_count: 5
---

# PLAN: a CI guard over the Claude Code conventions behind koto's `/workflows` rendering

## Status

Active

## Scope Summary

A guard that reads a Claude Code release's shipped bundle without running it
and names which of koto's four `/workflows` conventions broke (the file shape,
the directory convention, the session variable and the SessionStart hook
input), run in CI on relevant pull requests, weekly against the newest release
and on demand; a maintainer-run smoke check that reports whether koto's entry
renders in a real Claude Code; and a fixed, tested session-start hook, which
closes koto#243. Everything lands in one pull request.

## Decomposition Strategy

Walking skeleton. Step 1 crosses the whole path the design opens, on one real
input: the recorded release file, an npm fetch of the real Claude Code 2.1.316
Linux package, a byte scan with one probe per convention, a named result per
convention, and the new workflow running the same command on the pull request.
Step 2 thickens the guard to every field koto relies on, with named failures
for each convention and for bad inputs, and the offline test that keeps the
shape fixture and the probes in step. Step 3 points the guard at newer
releases on a schedule and on demand. Step 4 fixes the session-start hook,
which doesn't depend on the guard. Step 5 uses the fixed hook to drive the
rendered smoke check.

## Steps

### Step 1: the guard reads a real Claude Code release and names each convention's result

**Goal**: The guard crosses the whole path the design opens, on one real input: it reads the recorded release (2.1.316) from the committed release file, fetches `@anthropic-ai/claude-code-linux-x64` at that version from npm into a fresh temporary directory without running it, scans the executable's bytes with one probe per convention from the committed expectations file, and prints one result line per convention. The same command runs in a new workflow on pull requests that touch the surface, with read-only permissions and no secret.

**Acceptance Criteria**:

- [ ] Run with no arguments, the guard prints `checking Claude Code 2.1.316` as its first line, then `ok` for each of `file shape`, `directory convention`, `session variable` and `SessionStart hook input`, and exits 0.
- [ ] Run with `--alter session-variable`, the guard prints a `FAIL` line naming `session variable` and `CLAUDE_CODE_SESSION_ID`, still prints a result for the other three conventions, and exits 1.
- [ ] After a run, no copy of the downloaded package remains under the repository, and no process from the package was started.
- [ ] On the implementing pull request, the `claude-code-conventions` job runs the guard against 2.1.316 and passes, and its workflow declares `permissions: contents: read` and references no `secrets.` value.

**Check**: From the repository root, run `scripts/check-claude-code-conventions.sh` and see `checking Claude Code 2.1.316` followed by four `ok` results, exit 0; then run `scripts/check-claude-code-conventions.sh --alter session-variable` and see `FAIL session variable: CLAUDE_CODE_SESSION_ID ...`, exit 1. On the pull request, open the `claude-code-conventions` job and see the same passing output.

**Dependencies**: None

**Files**: `scripts/check-claude-code-conventions.sh`, `scripts/claude_code_conventions.py`, `tests/fixtures/native-workflows/claude-code-release`, `tests/fixtures/native-workflows/claude-code-conventions.json`, `.github/workflows/claude-code-conventions.yml`

### Step 2: the guard covers every field koto relies on and fails by name on each convention and on bad inputs

**Goal**: A maintainer can make the guard fail on purpose for each convention and trust that a failure names the cause. Every screen-read field of koto's shape fixture has a probe, an offline test keeps it that way, bad inputs fail with their own messages, and the verification guide says how to run the guard and how to extend it when koto's file shape changes.

**Acceptance Criteria**:

- [ ] `--alter file-shape`, `--alter directory`, `--alter session-variable` and `--alter hook-input` each exit 1 and print a `FAIL` line naming that convention (`file shape`, `directory convention`, `session variable`, `SessionStart hook input`) and the altered item.
- [ ] `--version 0.0.0` exits 2 with a message containing `could not obtain` and naming no convention.
- [ ] Removing `tests/fixtures/native-workflows/claude-code-release`, emptying it, or writing `latest` into it makes the guard exit 3 with a message naming that file.
- [ ] `--self-test` exits 0 with no network access, and exits 1 if a convention's mutation goes undetected.
- [ ] `--evidence <file>` scans a local executable and prints the same per-convention results as a download of that release.
- [ ] Adding a field outside the `koto` block of `tests/fixtures/native-workflows/enriched-shape.json` without a matching probe makes `cargo test --test native_workflows_conventions` fail with a message naming the field; adding one inside the `koto` block doesn't.
- [ ] `cargo test --test native_workflows_shape` still passes, and the test file's header describes it as koto's own shape check rather than the future Claude Code guard.
- [ ] `docs/guides/native-workflows-verification.md` has a section that runs the guard, lists the four `--alter` commands, and gives the steps for extending the probes when koto's file shape changes, ending with `--alter file-shape`.

**Check**: Run `scripts/check-claude-code-conventions.sh --alter <slug>` for each of the four slugs and see exit 1 with the convention and item named; run `scripts/check-claude-code-conventions.sh --version 0.0.0` and see exit 2 and `could not obtain`; run `scripts/check-claude-code-conventions.sh --self-test` and see exit 0. Add a field such as `"probeCheck": 1` at the top level of the shape fixture, run `CARGO_BUILD_JOBS=2 cargo test --test native_workflows_conventions` and see it fail naming `probeCheck`, then revert the fixture.

**Dependencies**: Step 1

**Files**: `tests/fixtures/native-workflows/claude-code-conventions.json`, `scripts/check-claude-code-conventions.sh`, `scripts/claude_code_conventions.py`, `tests/native_workflows_conventions.rs`, `tests/native_workflows_shape.rs`, `src/workflows_surface/contract.rs`, `docs/guides/native-workflows-verification.md`

### Step 3: a newer Claude Code release is checked weekly and on demand with no koto commit

**Goal**: A Claude Code release that ships while koto sits still gets checked anyway. The guard accepts `--version latest`, resolving it to a concrete version before anything else uses it, and `--cache-dir <dir>`, where it keeps downloaded tarballs and reuses one only when its sha512 matches the `dist.integrity` value the registry reports for that version. The workflow gains a weekly schedule that checks the newest release and a `workflow_dispatch` `version` input that checks a named one, caches the guard's `--cache-dir` under a key naming the resolved version, runs `--self-test` before the real check, and stays read-only with no secret and no commit. The verification guide says how to start these runs and how to move the recorded release.

**Acceptance Criteria**:

- [ ] `--version latest` prints `checking Claude Code <MAJOR.MINOR.PATCH>` as its first line, naming the version `npm view @anthropic-ai/claude-code-linux-x64 version` reports (never the word `latest`), then the four per-convention results. With the registry unreachable it exits 2 with `could not obtain` and names no convention; any other value that isn't `MAJOR.MINOR.PATCH` (for example `v2.1.316` or `2.1.316;id`) still exits 64.
- [ ] Run with `--cache-dir <dir>` (an empty directory outside the repository), the guard exits as it would without the flag and leaves exactly one file in `<dir>`, the release's `.tgz` tarball, with no extracted tree. Run without the flag, it still leaves no copy of the package anywhere.
- [ ] A second run with the same `--cache-dir` and release reuses the cached tarball: `ls -li <dir>` shows the same file, inode and modification time before and after, and the run prints the same results.
- [ ] After overwriting the cached tarball with other bytes, the next run with that `--cache-dir` prints a line naming the cached tarball and its failed `integrity` check, doesn't extract it, fetches the release again, prints four `ok` results and exits 0. Afterwards the tarball in `<dir>` has a sha512 that matches `npm view @anthropic-ai/claude-code-linux-x64@2.1.316 dist.integrity`.
- [ ] A `--cache-dir` inside the repository exits 64 with a message saying so, the way a `TMPDIR` inside the repository already does.
- [ ] `.github/workflows/claude-code-conventions.yml` triggers on `schedule` with cron `0 6 * * 1` (Mondays 06:00 UTC) and on `workflow_dispatch` with an optional string input `version` whose default is empty, alongside the unchanged `pull_request` and `push` path filters.
- [ ] Pull requests, pushes and a dispatch with an empty `version` check the recorded release; the schedule checks the newest release; a dispatch with `version` set to `2.1.320` or `latest` checks that release. The input reaches the guard only as `--version`'s value through an environment variable: `inputs.version` (or `github.event.inputs`) appears only inside an `env:` mapping, never inside a `run:` script, and neither `--alter` nor `--evidence` appears in the workflow.
- [ ] The job caches the directory it passes as `--cache-dir`, which sits outside the checkout (under `runner.temp`), with an `actions/cache` step referenced by version tag like the repository's other actions, a key that contains the concrete version the guard checks (the same version as its `checking Claude Code` line, never `latest` or empty) and no `restore-keys`.
- [ ] On every trigger the job runs `scripts/check-claude-code-conventions.sh --self-test` as a step before the real check, so a failed self-test stops the job before any release is checked.
- [ ] The workflow still declares only `permissions: contents: read`, with no job-level `permissions` override, keeps `persist-credentials: false` on checkout and its `timeout-minutes`, and contains no `secrets.` reference, no `pull_request_target`, and no `git commit`, `git push` or other write command, so the scheduled run can't change the repository.
- [ ] `actionlint .github/workflows/claude-code-conventions.yml` exits 0.
- [ ] The step 1 and 2 behaviour holds: with no arguments the guard prints `checking Claude Code 2.1.316` and four `ok` results, exit 0; `--version 0.0.0` exits 2 with `could not obtain`; `--self-test` exits 0 with no network.
- [ ] The guard section of `docs/guides/native-workflows-verification.md` lists `--version latest` and `--cache-dir <dir>`, says which release each workflow trigger checks, gives the command to start a dispatch run for a named release (`gh workflow run claude-code-conventions.yml -f version=<v>`), and notes that GitHub disables scheduled workflows after 60 days without repository activity. A new subsection on moving the recorded release says to run the guard with `--version <new>`, update koto or the probes if anything fails, write the new version into `tests/fixtures/native-workflows/claude-code-release`, and open a pull request, which the workflow checks against the new recorded release.

**Check**: Run `actionlint .github/workflows/claude-code-conventions.yml` and see exit 0. Run `yq '.on | keys' .github/workflows/claude-code-conventions.yml` and see `pull_request`, `push`, `schedule` and `workflow_dispatch`; `yq '.on.schedule[0].cron'` prints `0 6 * * 1` and `yq '.on.workflow_dispatch.inputs.version'` shows an optional string with an empty default. Run `grep -nE 'inputs\.version|github\.event\.inputs' .github/workflows/claude-code-conventions.yml` and see every hit under an `env:` key; run `grep -nE 'secrets\.|pull_request_target|restore-keys|git (commit|push)|--alter|--evidence' .github/workflows/claude-code-conventions.yml` and see nothing. Then, from the repository root: `d=$(mktemp -d)`; run `scripts/check-claude-code-conventions.sh --cache-dir "$d"` and see four `ok` results, exit 0, and one `.tgz` in `ls -li "$d"`; run it again and see the same inode and time; run `printf 'not the package' > "$d"/*.tgz`, run it again and see the integrity line, four `ok` results and exit 0, then compare `printf 'sha512-%s\n' "$(openssl dgst -sha512 -binary "$d"/*.tgz | base64)"` with `npm view @anthropic-ai/claude-code-linux-x64@2.1.316 dist.integrity` and see them match. Run `scripts/check-claude-code-conventions.sh --version latest` and see its first line name the version `npm view @anthropic-ai/claude-code-linux-x64 version` prints. Run `scripts/check-claude-code-conventions.sh --cache-dir "$PWD/scripts"` and see exit 64 with nothing written there. Finish with `scripts/check-claude-code-conventions.sh`, `--version 0.0.0` and `--self-test` and see exit 0, 2 and 0.

**Dependencies**: Step 2

**Files**: `.github/workflows/claude-code-conventions.yml`, `scripts/check-claude-code-conventions.sh`, `scripts/claude_code_conventions.py`, `docs/guides/native-workflows-verification.md`

### Step 4: the session-start hook prints valid JSON naming the right directory with and without jq

The session-start hook escapes its output, decodes escaped input, shell-quotes its export line and refuses hostile values, shown by a payload-matrix test in the plugin CI job, closing koto#243.

### Step 5: a maintainer can see whether koto's entry renders in a real Claude Code

A maintainer-run tmux harness drives a real Claude Code and reports rendered, not rendered or unavailable with distinct exit statuses, with one recorded run in the guide and a before-release step that records the outcome.

## Implementation Sequence

The steps run in order in one pull request. Step 4 depends on nothing before
it and could run beside steps 2 and 3; step 5 needs step 4's hook.
