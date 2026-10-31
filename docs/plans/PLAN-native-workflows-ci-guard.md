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

**Goal**: An operator on a host without `jq` gets the same working hook as one with it. `plugins/koto-skills/hooks/session-start-workflows.sh` gains a `json_escape` function (backslash, double quote and every control character below U+0020) applied to the context before the `printf` fallback interpolates it, and an escape-aware no-`jq` `read_field` that decodes `\"`, `\\` and `\/` in one pass and refuses any other escape. A `session_id` outside `[A-Za-z0-9_-]+` counts as absent, a control character or DEL in the derived directory makes the hook print nothing, and the `export` line in the context single-quotes the directory for the shell. The derivation rule and the output contract don't change, and the script stays unregistered. A sibling test runs a fixed payload matrix with `jq` on the `PATH` and again with a `PATH` holding only the commands the script needs, and the plugin CI job runs it. The pull request body, not this entry, carries `Closes #243`.

**Acceptance Criteria**:

- [ ] For `{"session_id":"abc123","transcript_path":"/work/proj/t.jsonl"}`, run once with `jq` on the `PATH` and once with a `PATH` of symlinks to only `bash`, `cat`, `dirname`, `grep`, `sed` and `head` (where `command -v jq` fails), the hook exits 0 and prints one JSON object that `jq` parses, with `hookSpecificOutput.hookEventName` equal to `SessionStart` and `additionalContext` containing `/work/proj/abc123/workflows`.
- [ ] For a `transcript_path` whose directory contains `"`, `\` and a space, and for one whose directory contains `$(`, a backtick and `'`, both runs print JSON that `jq` parses, `additionalContext` contains `<dirname of transcript_path>/<session_id>/workflows` byte for byte in its plain sentence, and the two runs' `additionalContext` values are identical.
- [ ] For an empty or absent `transcript_path` with a `cwd`, both runs name `<cwd>/<session_id>/workflows` the same way.
- [ ] The context's export line reads `export KOTO_WORKFLOWS_DIR='<dir>'`, with each `'` in the directory written `'\''`. Evaluated in a subshell for each of the cases above, it sets `KOTO_WORKFLOWS_DIR` to exactly the derived directory and runs nothing else: a directory containing `$(touch <sentinel>)` or a backtick form of it leaves no sentinel file.
- [ ] Each of these prints nothing and exits 0 in both runs: a payload with no `session_id`; a `session_id` containing `/` (for example `../x`); a `transcript_path` whose directory holds a raw DEL byte; and stdin that isn't JSON.
- [ ] Without `jq`, a `transcript_path` written with `\/` resolves as `/`, while one holding any other escape (`\n`, `\t` or `A`) makes the hook print nothing and exit 0 even when the payload has a usable `cwd`, so a refused field never falls back to another. With `jq`, a `\n` in the directory also prints nothing, through the control-character rule.
- [ ] `plugins/koto-skills/hooks/test/session-start-workflows.test.sh` runs the whole matrix in both passes. It fails if `jq` is missing from the normal `PATH` or present on the restricted one, exits 0 when every case passes, and otherwise exits non-zero with each failing case named by payload and pass.
- [ ] Run against the script as it is on `origin/main`, the test exits non-zero, and its failures include the no-`jq` pass of the quote-and-backslash payload.
- [ ] `.github/workflows/validate-plugins.yml`'s `hook-smoke-test` job has a new step that runs `bash plugins/koto-skills/hooks/test/session-start-workflows.test.sh`, carrying the same "Copied into .claude/shirabe-extensions/work-on.md" comment as its neighbours, and on the implementing pull request that step runs and passes.
- [ ] `.claude/shirabe-extensions/work-on.md` lists the same command under its `plugins/**` entry, and its no-local-check entry excepts `session-start-workflows.sh` alongside `session-start.sh`, so a change to the script runs the test rather than stopping for a person.
- [ ] The script stays unregistered: `plugins/koto-skills/hooks/hooks.json` is unchanged, the script's header still says it isn't registered and no longer says the context has no quotes to escape, and `cargo test --test plugin_hooks_shape` passes.

**Check**: From the repository root, run `bash plugins/koto-skills/hooks/test/session-start-workflows.test.sh` and see both passes report every case passing, exit 0. Then exercise the no-`jq` path by hand: `d=$(mktemp -d); for c in bash cat dirname grep sed head; do ln -s "$(command -v "$c")" "$d/$c"; done`, then `printf '%s' '{"session_id":"abc123","transcript_path":"/tmp/a \"b\\c d/t.jsonl"}' | PATH="$d" "$d/bash" plugins/koto-skills/hooks/session-start-workflows.sh | jq -r .hookSpecificOutput.additionalContext` and see `/tmp/a "b\c d/abc123/workflows` in the sentence and `export KOTO_WORKFLOWS_DIR='/tmp/a "b\c d/abc123/workflows'` in the export line. Run `printf '%s' '{"session_id":"../x","transcript_path":"/tmp/p/t.jsonl"}' | bash plugins/koto-skills/hooks/session-start-workflows.sh | wc -c` and see `0`, exit 0. To see the test catch the old bug, copy the fixed script aside, write `git show origin/main:plugins/koto-skills/hooks/session-start-workflows.sh` over it, run the test and see it exit non-zero naming the no-`jq` quote-and-backslash case, then copy the fixed script back. Run `git diff --exit-code origin/main -- plugins/koto-skills/hooks/hooks.json`, `CARGO_BUILD_JOBS=2 cargo test --test plugin_hooks_shape` and `actionlint .github/workflows/validate-plugins.yml` and see exit 0 from each. On the pull request, open the `hook-smoke-test` job and see the new step pass.

**Dependencies**: Step 3

**Files**: `plugins/koto-skills/hooks/session-start-workflows.sh`, `plugins/koto-skills/hooks/test/session-start-workflows.test.sh`, `.github/workflows/validate-plugins.yml`, `.claude/shirabe-extensions/work-on.md`

### Step 5: a maintainer can see whether koto's entry renders in a real Claude Code

**Goal**: A maintainer about to cut a release learns in about a minute whether Claude Code actually draws koto's entry. `scripts/smoke-native-workflows-render.sh [--no-entry] [--koto <path>]` starts a real interactive `claude --session-id <uuid> --settings <file>` in a detached tmux session, from a mode-700 scratch project directory made by `mktemp -d`. The settings file registers the fixed `session-start-workflows.sh` as the SessionStart hook through a wrapper that saves the hook's stdin and output in the scratch directory. The harness answers the folder-trust prompt only when the pane shows it, reads the derived directory from the saved hook output, has koto write an entry there (skipped with `--no-entry`), sends `/workflows` and reads the pane. It reports one line, `<outcome>: Claude Code <version>`, with `rendered` (exit 0), `not rendered` (1) or `unavailable` (3), and 2 for a usage error, followed by the captured pane on anything but `rendered`. It cleans up only what it created. The verification guide documents it with one real run, and `.claude/shirabe-extensions/release.md` gains an advisory `## Before release` section that asks the releaser to confirm the newest scheduled guard run passed and to put the smoke check's outcome in the release notes.

**Acceptance Criteria**:

The first four criteria need `tmux` and a logged-in Claude Code. The host this pull request is built on has no `tmux`, so a maintainer checks those four on their own machine; every other criterion is checked where the step is built.

- [ ] With `claude`, `tmux` and `python3` on the `PATH`, a built koto, and a logged-in Claude Code at the recorded release, `scripts/smoke-native-workflows-render.sh` prints `rendered: Claude Code 2.1.316` as its last line and exits 0. The version is the one `claude --version` prints, and the entry's `workflowName` is read back from the file koto wrote and is what the harness looks for in the pane.
- [ ] Run the same way with `--no-entry`, it prints `not rendered: Claude Code 2.1.316`, then the captured pane showing the workflows list or its empty state, and exits 1.
- [ ] Run with `CLAUDE_CODE_DISABLE_WORKFLOWS=1` in its environment, it prints `unavailable: Claude Code 2.1.316` and the pane, and exits 3, never 1.
- [ ] Run with `CLAUDE_CONFIG_DIR` pointed at an empty scratch directory (a Claude Code with no login and no onboarding done), it meets a screen other than the folder-trust prompt, prints `unavailable` with the captured pane and exits 3 within about 30 seconds; it never answers that screen.
- [ ] With `tmux`, `claude` or `python3` missing from the `PATH`, or `--koto` naming a path that isn't an executable file, it prints `unavailable` naming the missing prerequisite and exits 3 before starting tmux or Claude Code. With no `--koto`, it uses `target/debug/koto`, then `target/release/koto`, as `scripts/verify-native-workflows.sh` does. An unknown option, or `--koto` with no value, exits 2 with a usage line.
- [ ] Every koto it starts runs through `scripts/without-session-env.sh`, with `KOTO_HOME` and `KOTO_SESSIONS_BASE` inside the scratch directory and the local session backend, so the maintainer's real sessions are never read or written.
- [ ] After every outcome, and after a SIGINT sent while it waits for the hook output, no tmux session it started is left (`tmux ls` names none), the scratch directory is gone, and under `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/` the only things removed are `<uuid>.jsonl` and `<uuid>/` for the harness's own UUID, plus that project's directory when it's left empty (removed with `rmdir`). The UUID is checked against a UUID pattern before any removal, each path variable is checked non-empty, no removal uses a glob and none follows a symlink.
- [ ] It types only the trust-prompt answer and `/workflows`, through `tmux send-keys -l`, and sends no prompt to the model. The script contains no `niwa`, no `claude -p` and no `--dangerously-skip-permissions`, and `bash -n scripts/smoke-native-workflows-render.sh` exits 0.
- [ ] `docs/guides/native-workflows-verification.md` gains a section for the smoke check giving its command and options, its prerequisites (an installed, logged-in Claude Code, `tmux`, `python3`, a built koto), one exit status per outcome (0 rendered, 1 not rendered, 2 usage, 3 unavailable), a note that the pane and saved payload can hold the maintainer's email, organization and home path and are kept only in the scratch directory, and one real run's full output, naming the release and the outcome, with nothing personal in it (on a host without `tmux` that run is the `unavailable` one that names `tmux`). The manual section says the smoke check automates its first three steps.
- [ ] `.claude/shirabe-extensions/release.md` has a `## Before release` section above `## After release`, with the two advisory items (the newest scheduled guard run, and the smoke check's outcome in the release notes as `Checked against Claude Code <version>: /workflows entry renders`, or the outcome it got). It adds no `## Release checks` or other heading `/shirabe:release` runs commands from, so a release runs nothing new by itself.
- [ ] The earlier steps hold: `scripts/check-claude-code-conventions.sh` and `--self-test` exit 0, and `bash plugins/koto-skills/hooks/test/session-start-workflows.test.sh` exits 0.

**Check**: Build koto (`CARGO_BUILD_JOBS=2 cargo build`). Where `tmux` is installed, in a terminal with a logged-in Claude Code at 2.1.316, run `scripts/smoke-native-workflows-render.sh` and see `rendered: Claude Code 2.1.316`, exit 0; run it with `--no-entry` and see `not rendered`, the pane, exit 1; run `CLAUDE_CODE_DISABLE_WORKFLOWS=1 scripts/smoke-native-workflows-render.sh` and see `unavailable`, exit 3; run `CLAUDE_CONFIG_DIR=$(mktemp -d) scripts/smoke-native-workflows-render.sh` and see `unavailable` with the pane, exit 3. Anywhere, run it with a `PATH` lacking `tmux` and see `unavailable` naming `tmux`, exit 3; run `--bogus` and `--koto` alone and see exit 2. Before and after these runs, list `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/` and see no new entry left, and `tmux ls` shows no harness session. Start a run and send it SIGINT while it waits, and see the same clean state. Run `grep -nE 'niwa|claude -p|dangerously' scripts/smoke-native-workflows-render.sh` and see nothing, and `bash -n` on it exit 0. Read the guide's new section and `release.md`'s `## Before release` against the criteria above. Finish with `scripts/check-claude-code-conventions.sh`, `scripts/check-claude-code-conventions.sh --self-test` and the hook test, each exit 0.

**Dependencies**: Step 4

**Files**: `scripts/smoke-native-workflows-render.sh`, `docs/guides/native-workflows-verification.md`, `.claude/shirabe-extensions/release.md`

## Implementation Sequence

The steps run in order in one pull request. Step 4 depends on nothing before
it and could run beside steps 2 and 3; step 5 needs step 4's hook.
