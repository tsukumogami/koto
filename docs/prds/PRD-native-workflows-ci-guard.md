---
schema: prd/v1
status: Draft
upstream: docs/briefs/BRIEF-native-workflows-ci-guard.md
problem: |
  koto's native `/workflows` rendering depends on four Claude Code conventions
  that Claude Code doesn't publish as an interface: the workflows file shape,
  the per-session directory layout, the `CLAUDE_CODE_SESSION_ID` variable and
  the SessionStart hook's input fields. koto's CI checks none of them against
  Claude Code. Its shape test compares koto's output with a fixture koto wrote,
  and the hook script that hands koto its directory has no test. When a Claude
  Code release changes a convention, the first person to notice is an operator
  looking at an empty `/workflows`.
goals: |
  koto's maintainers learn from koto's CI, before a koto release, when a Claude
  Code release changes the file shape, the directory convention or the session
  input koto relies on, and the failing check names which one. A maintainer can
  also run a rendered smoke check against a real Claude Code and read whether an
  entry koto wrote renders. The hook script that derives koto's directory is
  tested, and extending the file shape later extends the guard by a written step.
---

# PRD: a CI guard over the Claude Code conventions behind koto's `/workflows` rendering

## Status

Draft

Requirements for the hardening step of koto's native `/workflows` rendering,
written from the Accepted `BRIEF-native-workflows-ci-guard`. The two questions
the brief left open, the guard's form and where the rendered smoke check runs,
are design questions; this PRD fixes what the guard must detect and report and
what done looks like.

## Problem Statement

koto shows a session in Claude Code's `/workflows` screen by writing a
`koto-<session-uuid>.json` file into the session's workflows directory. The
whole arrangement rests on Claude Code behavior that isn't a published
interface. koto relies on four conventions:

1. **The file shape.** The screen lists the `*.json` files in a session's
   workflows directory and reads fields such as `name`, `workflowName`,
   `summary`, `status`, `startTime`, `phases` and `workflowProgress` (with
   `workflow_phase` and `workflow_agent` nodes). Claude Code doesn't document
   the format, and its reader is lenient: a file it can't parse is dropped with
   no visible error.
2. **The directory convention.** A session's transcript sits at
   `~/.claude/projects/<slug>/<session-id>.jsonl` and its workflows directory at
   `<slug>/<session-id>/workflows/` beside it. koto finds the directory by
   scanning for the transcript.
3. **The session variable.** Claude Code sets `CLAUDE_CODE_SESSION_ID` for the
   processes it starts. koto reads it to know which session it runs in. The
   variable isn't in Claude Code's hooks documentation.
4. **The SessionStart hook input.** koto's hook script reads `session_id`,
   `transcript_path` and `cwd` from the JSON Claude Code sends on stdin.

What koto knows about these was read from specific Claude Code releases
(2.1.209, 2.1.296 and 2.1.314) and checked by hand against the live screen.
It lives in design-doc prose. No file in the repository says which Claude Code
release the conventions were last confirmed against.

koto's CI can't see a Claude Code change. The shape test compares koto's
serialized output with `tests/fixtures/native-workflows/enriched-shape.json`,
which koto generated, so it passes whatever Claude Code does (koto#178). The
directory and variable conventions are tested only against synthetic trees koto
builds itself. The hook script `session-start-workflows.sh` runs in no CI job,
and its fallback for hosts without `jq` emits invalid JSON because the quotes
around the path in its `export KOTO_WORKFLOWS_DIR="..."` text aren't escaped
(koto#243).

Claude Code ships often. When a release renames a field, moves the directory or
stops setting the variable, koto's CI stays green, koto keeps writing its file
without error, and the operator opens `/workflows` to an empty screen. They
can't tell whether koto, their setup or Claude Code is at fault, and the
maintainer hears about it after the break has shipped.

## Goals

- A koto maintainer finds out from koto's CI, before cutting a koto release,
  that a Claude Code release changed one of the four conventions, and the
  failure names which one.
- A maintainer can check, against a real Claude Code, whether an entry koto
  wrote actually renders, and knows how to run that check and what a passing
  run looks like.
- The hook script that derives koto's workflows directory produces valid output
  on every host it supports, and CI would catch it if it didn't.
- A later, deliberate change to koto's file shape extends the guard in the same
  change, by a written procedure.
- Operators stop meeting a silent, empty `/workflows` caused by a Claude Code
  change nobody noticed.

## User Stories

- As a koto maintainer, I want koto's CI to fail when a Claude Code release
  changes the workflows file shape, the directory convention or the session
  input, with a message naming the convention, so that I start from the broken
  convention instead of from an operator's report of an empty screen.
- As a koto maintainer about to cut a release, I want to run a rendered smoke
  check against an installed Claude Code and read a plain pass or fail for
  whether koto's entry shows up, so that I know the render works end to end and
  not only that its inputs look unchanged.
- As a koto contributor adding a field to the workflows file, I want a written
  step for extending the guard's fixture, so that the guard keeps describing
  both what koto writes and what Claude Code reads, and the new field is
  covered when it lands.
- As an operator on a host without `jq`, I want the session-start hook to emit
  valid JSON, so that Claude Code accepts it and koto gets its directory.
- As a reviewer of the implementing pull request, I want to see the guard fail
  against a deliberately altered input and to know how to reproduce that, so
  that I can trust a green guard means something.

## Requirements

### Functional

- **R1. Claude Code reference.** The guard checks koto's conventions against
  what a Claude Code release actually ships or does, not against koto's own
  output. A check whose only input is koto's serialized file doesn't satisfy
  this requirement.
- **R2. Recorded release.** The repository records, in a machine-readable
  file, the Claude Code release the conventions were last confirmed against.
  The guard reads it, and moving it to a newer release is a reviewable change.
- **R3. Per-convention checks.** The guard has a separate check for each of the
  four conventions: the file shape, the directory convention, the session
  variable and the SessionStart hook input fields.
- **R4. Named failures.** When a check fails, its output names the convention
  that changed and the specific element that no longer matches (for example, a
  field name, a directory name, a variable name or a hook input field). A bare
  "fixture mismatch" or a raw diff with no convention named doesn't satisfy
  this requirement.
- **R5. Newer releases.** The guard can be run against a Claude Code release
  newer than the recorded one, so that a new release is checked without waiting
  for a koto commit. A maintainer can also name a specific release to check.
- **R6. Demonstrated failure.** The guard can be shown failing against a
  deliberately altered input for each convention, by a documented command a
  reader can run. The implementing pull request shows this happening and says
  how to reproduce it.
- **R7. Rendered smoke check.** A rendered smoke check runs a real Claude Code
  with an entry koto wrote and reports pass or fail for whether the entry
  renders in `/workflows`. Its documentation says how to run it, what it needs
  (credentials, an installed Claude Code, a terminal), and shows the result of
  one run.
- **R8. Hook coverage.** `session-start-workflows.sh` is exercised in CI with
  representative SessionStart payloads, on the path where `jq` is available and
  on the path where it isn't, and every output it prints is valid JSON that
  carries the derived workflows directory. This fixes and closes koto#243.
- **R9. Fixture extension procedure.** Written maintainer documentation says
  how to extend the guard when koto's file shape changes on purpose: what to
  update, in which order, and how to confirm the extended guard still fails
  against an altered input.
- **R10. Confirming a new release.** Written maintainer documentation says how
  to confirm the conventions against a new Claude Code release and move the
  recorded release forward.

### Non-functional

- **R11. No secret on pull requests.** Any check that needs a paid credential
  or a Claude Code login never runs on pull-request events, and fails with a
  clear message, rather than passing, when its credential is missing.
- **R12. Pull-request cost.** Checks that do run on pull requests need no
  credential and add no more than five minutes to the CI run.
- **R13. Tool independence.** Neither the guard nor the smoke check requires
  niwa or any tool outside koto's repository beyond Claude Code itself and
  standard CI tooling.
- **R14. Host constraints.** Running the guard locally needs no more than one
  `cargo` invocation at a time and runs koto with its session environment
  isolated from the maintainer's real sessions.

## Acceptance Criteria

- [ ] A CI job runs a check whose inputs include material from a published
      Claude Code release, and that check passes against the recorded release.
      (R1)
- [ ] The repository contains a file naming exactly one Claude Code release,
      and the guard's output states which release it checked. (R2, R5)
- [ ] Altering the file-shape reference (for example, renaming a field the
      screen reads) makes the guard fail with a message naming the file shape
      and the field. (R3, R4, R6)
- [ ] Altering the directory-convention reference (for example, the
      `workflows` directory name) makes the guard fail with a message naming
      the directory convention. (R3, R4, R6)
- [ ] Altering the session-variable reference (the `CLAUDE_CODE_SESSION_ID`
      name) makes the guard fail with a message naming the session variable.
      (R3, R4, R6)
- [ ] Altering the hook-input reference (for example, removing
      `transcript_path`) makes the guard fail with a message naming the
      SessionStart hook input and the field. (R3, R4, R6)
- [ ] The guard can be pointed at a Claude Code release other than the recorded
      one by a documented input, and a scheduled or manually triggered run
      checks the newest published release without a koto commit. (R5)
- [ ] The implementing pull request shows output from at least one failing run
      against an altered input and the command a reader runs to reproduce it.
      (R6)
- [ ] Documentation for the rendered smoke check gives the command, the
      prerequisites and the output of one real run, including the Claude Code
      release it ran against and whether the entry rendered. (R7)
- [ ] The rendered smoke check reports failure when no koto entry appears in
      `/workflows`. (R7)
- [ ] A CI job runs `session-start-workflows.sh` with a payload that has
      `session_id` and `transcript_path`, once with `jq` on the path and once
      without, and both outputs parse as JSON and contain the derived
      workflows directory. (R8)
- [ ] The same job covers a payload with no `transcript_path` (directory from
      `cwd`) and one with no `session_id` (no output, exit 0). (R8)
- [ ] The implementing pull request closes koto#243. (R8)
- [ ] Maintainer documentation contains a section on extending the guard for a
      deliberate file-shape change, and a section on confirming a new Claude
      Code release and moving the recorded release. (R9, R10)
- [ ] No job that needs a credential has `pull_request` among its triggers, and
      running it without the credential exits non-zero with a message naming
      the missing credential. (R11)
- [ ] The guard's pull-request jobs reference no secret, and the guard's own
      steps finish in under five minutes on a standard GitHub-hosted runner, as
      shown by the implementing pull request's CI run. (R12)
- [ ] The documented local command for the guard runs at most one `cargo`
      process, and any koto it starts runs with its session variables unset and
      `KOTO_HOME` pointed at a scratch directory. (R14)
- [ ] Neither the guard nor the smoke check invokes `niwa` or reads niwa
      configuration. (R13)

## Out of Scope

- **The list view's blocked bucket.** It waits on Claude Code adding one; the
  guard covers what koto relies on today.
- **Fan-out and hierarchy.** Coordinators and their delegates rendering as
  related entries is a later step of the same rendering work.
- **Retention and crash staleness** of the files koto writes (koto#190).
- **Changing what koto writes or how it resolves the directory.** The guard
  observes the conventions; it doesn't revise the file contract or the
  directory lookup. Gaps the research found in koto's own handling (below) are
  recorded, not fixed here.
- **Registering `session-start-workflows.sh`** in the plugin's `hooks.json`.
  The script stays opt-in; this work tests it, it doesn't turn it on.
- **Any coupling to niwa**, in either direction.

## Known Limitations

- **A lenient reader.** Claude Code fills in defaults for missing fields and
  drops unparseable files silently. A static check can confirm that the names
  koto relies on are still there; it can't prove the screen still draws them
  the way it did. Only the rendered smoke check gets close to that, and it
  isn't a pull-request check.
- **No headless list.** Claude Code 2.1.316 has no non-interactive command
  that lists workflows, so the rendered smoke check needs a terminal session
  and a real login. That's why it can't run on every pull request.
- **Feature gating.** `/workflows` can be disabled by setting, environment
  variable or account policy. A smoke check run under an account where it's off
  fails for a reason unrelated to koto, and its output should make that
  distinguishable.
- **Things the guard will see but koto doesn't handle.** Claude Code 2.1.316
  can send an empty `transcript_path` in some storage modes, and it resets
  `CLAUDE_CODE_SESSION_ID` on `/clear`. Neither is in scope to fix here.

## Decisions and Trade-offs

- **Two layers, not one.** Research found that the conventions are visible as
  string literals in the Claude Code release itself, which can be fetched
  without credentials, while seeing the actual render needs a terminal and a
  login. The requirements therefore split a credential-free guard that can run
  often (R1 to R6, R12) from a rendered smoke check that runs with credentials
  and off pull requests (R7, R11). A single live check on every pull request
  was rejected for cost, secrets exposure on fork pull requests, and
  flakiness.
- **koto#243 is in.** The hook script is how koto receives the SessionStart
  input and the directory, so it is part of the session-input convention the
  guard covers. Testing it without fixing the invalid-JSON fallback would leave
  a known failure in the path the guard is meant to protect.
- **Run against newer releases, not only the recorded one.** Checking only the
  recorded release would pass forever, like the fixture it replaces. R5 makes
  a new Claude Code release something the guard looks at before a koto release.
- **The guard's form and the smoke check's location are left to the design.**
  The brief deferred them; the requirements above constrain them (no secret on
  pull requests, named failures, a demonstrated failing run) without choosing.
