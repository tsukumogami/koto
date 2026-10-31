---
schema: prd/v1
status: Done
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
absorbed:
  - docs/briefs/BRIEF-native-workflows-ci-guard.md
---

# PRD: a CI guard over the Claude Code conventions behind koto's `/workflows` rendering

## Status

Done

Absorbed [BRIEF-native-workflows-ci-guard](docs/briefs/BRIEF-native-workflows-ci-guard.md); carried in Absorbed Brief.

Requirements for the hardening step of koto's native `/workflows` rendering.
The two questions the framing left open, the guard's form and where the
rendered smoke check runs, are design questions; this PRD fixes what the guard
must detect and report and what done looks like.

## Absorbed Brief

The feature was framed before these requirements existed. koto's `/workflows`
rendering rests on four Claude Code conventions nobody promised to keep: the
file shape the screen reads, the per-session directory layout, the
`CLAUDE_CODE_SESSION_ID` variable and the SessionStart hook's input. When a
Claude Code release changes one, koto's CI stays green and an operator meets an
empty `/workflows` with no error to explain it. The framed outcome: a koto
maintainer learns from koto's own CI, before a koto release, that a Claude Code
release broke one of these conventions, with the failing check naming which
one, and can run a smoke check against a real Claude Code to read whether an
entry koto wrote actually renders. For operators the result is an absence: no
more silent empty screens caused by a change nobody noticed. The framing named
four journeys, carried as this PRD's user stories: a Claude Code release
changing a convention, a pre-release rendered check, a contributor changing the
file shape on purpose, and the session-start hook on a host without `jq`. It
fixed the boundary the Requirements and Out of Scope operationalize: the guard,
a recorded Claude Code release, the rendered smoke check, hook coverage
including koto#243 and a fixture extension procedure are in; the list view's
blocked bucket, fan-out and hierarchy, retention and crash staleness, changes
to what koto writes, and any coupling to niwa are out.

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
  with and without `jq` on the host, and CI would catch it if it didn't.
- A later, deliberate change to koto's file shape extends the guard in the same
  change, by a written procedure.
- Operators stop meeting a silent, empty `/workflows` caused by a Claude Code
  change nobody noticed.

## User Stories

- As a koto maintainer, I want koto's CI to fail when a Claude Code release
  changes the workflows file shape, the directory convention or the session
  input, with a message naming the convention, so that I start from the broken
  convention instead of from an operator's report of an empty screen.
- As a koto maintainer who made no koto change this week, I want a new Claude
  Code release checked anyway, so that a break shows up as a failed run before
  the next koto release rather than as an operator's report after it.
- As a koto maintainer about to cut a release, I want to run a rendered smoke
  check against an installed Claude Code and read whether koto's entry showed
  up, didn't, or couldn't be checked, so that I know the render works end to end
  and not only that its inputs look unchanged.
- As a koto contributor adding a field to the workflows file, I want the guard
  to fail until its expectations cover the new field, and a written step that
  says how to extend them, so that the new field is checked against Claude Code
  from the day it lands.
- As an operator on a host without `jq`, whose project path contains a quote or
  a space, I want the session-start hook to emit valid JSON, so that Claude Code
  accepts it and koto gets its directory.
- As a reviewer of the implementing pull request, I want to run one command per
  convention that makes the guard fail against a deliberately altered input, so
  that I can trust a green guard means something.

## Requirements

Two terms recur below. A **convention check** compares koto's *expectation*
for one convention, kept in koto's repository, with the *release evidence* for
that convention: what the guard reads from or observes in a specific Claude
Code release. Whether the evidence comes from the release's shipped files or
from running it is the design's choice. **Altering an input** means changing
either side for a test run (removing or renaming the item in the evidence, or in
the expectation) without changing koto's code.

### Functional

- **R1. Evidence from Claude Code.** Every convention check reads release
  evidence from a published Claude Code release. A check whose only inputs are
  koto's serialized file and files koto generated doesn't satisfy this
  requirement.
- **R2. Recorded release.** The repository records the Claude Code release the
  conventions were last confirmed against, as a version string of the form
  `MAJOR.MINOR.PATCH` in a file the guard reads. The guard fails with a message
  naming that file when it is missing, empty or not a version string.
- **R3. One check per convention.** The guard has a separate convention check
  for each of the four conventions: the file shape, the directory convention,
  the session variable and the SessionStart hook input. All four run and report
  on every guard run; one failing doesn't stop the others.
- **R4. Named failures.** A failing convention check prints the convention's
  name (`file shape`, `directory convention`, `session variable` or
  `SessionStart hook input`) and the item that no longer matches (a field name,
  a directory name, a variable name or a hook input field), and the guard exits
  non-zero.
- **R5. Choosing the release.** The guard checks the recorded release by
  default and accepts a version string to check a different release. Its output
  states which release it checked. When the release can't be obtained (an
  unpublished version, a registry or network failure), the guard exits non-zero
  with a message saying the release couldn't be obtained, distinct from every
  convention-check failure.
- **R6. Newest release on a schedule.** A scheduled CI run, at least weekly,
  checks the newest Claude Code release published at run time, with no koto
  commit needed. Its failure is a failed workflow run on koto's default branch.
- **R7. Demonstrated failure.** For each of the four conventions there is a
  documented command that runs the guard against an altered input and exits
  non-zero with that convention's name in its output. The implementing pull
  request includes the output of these runs and the commands.
- **R8. Fixture coverage.** Every field name in koto's shape fixture outside
  its `koto` block (the block Claude Code doesn't read) is one koto relies on
  the screen reading, and the file-shape check's expectation covers each of
  them. A field added to the fixture outside the `koto` block without a
  matching expectation makes the guard fail.
- **R9. The existing shape test.** The test that compares koto's output with
  `tests/fixtures/native-workflows/enriched-shape.json` stays, as the check that
  koto writes the shape it intends to; it is no longer described as the
  Claude Code guard. The implementing pull request states whether it closes
  koto#178.
- **R10. Rendered smoke check.** A rendered smoke check starts a real Claude
  Code with an entry koto wrote for that session and reports one of three
  outcomes, each with its own exit status: *rendered* (the entry's
  `workflowName` appears in the `/workflows` list Claude Code draws), *not
  rendered* (the list is shown and the entry isn't in it), or *unavailable*
  (Claude Code couldn't start, isn't logged in, or `/workflows` is disabled).
  Its output names the Claude Code release it ran against.
- **R11. Smoke check documentation.** The rendered smoke check's documentation
  gives the command, its prerequisites (credentials, an installed Claude Code,
  a terminal) and the full output of one real run.
- **R12. Hook coverage.** A CI job runs `session-start-workflows.sh` against a
  fixed set of SessionStart payloads, once with `jq` on the `PATH` and once
  without. On every payload that carries `session_id`, the output is valid JSON
  whose context names the derived workflows directory exactly, including when
  the path contains a double quote, a backslash or a space. The derived
  directory is `<dirname of transcript_path>/<session_id>/workflows`, or
  `<cwd>/<session_id>/workflows` when `transcript_path` is absent or empty (the
  script's current rule, unchanged). On a payload with no `session_id`, or stdin
  that isn't JSON, it prints nothing and exits 0. This fixes and closes
  koto#243.
- **R13. Maintainer procedures.** Maintainer documentation, in a guide under
  `docs/guides/`, gives (a) the steps to extend the guard's expectations when
  koto's file shape changes on purpose, ending with running the R7 command for
  the file shape, and (b) the steps to confirm the conventions against a new
  Claude Code release and move the recorded release forward.
- **R14. Release step.** koto's release procedure gains a step to confirm,
  before tagging, that the newest scheduled guard run passed and to record the
  smoke check's outcome for the release being shipped. The step is advisory:
  it doesn't block a release mechanically.

### Non-functional

- **R15. No credentials on pull requests.** A job that needs a paid credential
  or a Claude Code login has neither `pull_request` nor `pull_request_target`
  among its triggers. Run without its credential, or with the credential set to
  an empty value, it exits non-zero with a message naming the missing
  credential.
- **R16. Pull-request cost.** Guard and hook jobs that run on pull requests
  reference no secret and finish within five minutes of wall-clock time each on
  a GitHub-hosted runner, as shown by the job durations in the implementing
  pull request's CI run.
- **R17. Tool independence.** The guard and the smoke check don't invoke niwa
  or read niwa configuration, and pass on a runner where niwa isn't installed.
- **R18. Host constraints.** The documented local command for the guard runs at
  most one `cargo` process at a time. Any koto it starts runs with koto's
  session variables unset and `KOTO_HOME` pointed at a scratch directory, so the
  maintainer's real sessions aren't read or written.

## Acceptance Criteria

- [ ] With unaltered inputs, the guard run against the recorded release exits 0
      and its output names that release. (R1, R2, R5)
- [ ] Removing the recorded-release file, emptying it, or replacing its content
      with `latest`, makes the guard exit non-zero with a message naming the
      file. (R2)
- [ ] Renaming a file-shape item the screen reads (for example `workflowName`)
      in the release evidence makes the guard exit non-zero and print
      `file shape` and the item's name. (R3, R4, R7)
- [ ] Renaming the `workflows` directory name in the release evidence makes the
      guard exit non-zero and print `directory convention` and `workflows`.
      (R3, R4, R7)
- [ ] Renaming `CLAUDE_CODE_SESSION_ID` in the release evidence makes the guard
      exit non-zero and print `session variable` and
      `CLAUDE_CODE_SESSION_ID`. (R3, R4, R7)
- [ ] Removing `transcript_path` from the hook-input release evidence makes the
      guard exit non-zero and print `SessionStart hook input` and
      `transcript_path`. (R3, R4, R7)
- [ ] With two conventions altered at once, the guard's output names both.
      (R3)
- [ ] Run with an explicit version string, the guard checks that release and
      its output names it. (R5)
- [ ] Run with a version string that isn't published (for example `0.0.0`), the
      guard exits non-zero with a could-not-obtain message that names no
      convention. (R5)
- [ ] A workflow file in `.github/workflows/` runs the guard on a `schedule`
      trigger at least weekly, against the newest published release. (R6)
- [ ] The implementing pull request's description includes the four R7
      commands and the output of each failing run. (R7)
- [ ] Adding a new field outside the `koto` block of the shape fixture, without
      adding a matching expectation, makes the guard exit non-zero, naming the
      field. Adding one inside the `koto` block doesn't. (R8)
- [ ] `tests/native_workflows_shape.rs` still passes, and its header no longer
      calls it the future Claude Code guard. (R9)
- [ ] Run by a logged-in maintainer against the recorded release, with koto's
      entry written, the smoke check reports *rendered*, exits with the
      *rendered* status and names the Claude Code release. (R10)
- [ ] Run with no koto entry in the session's workflows directory, the smoke
      check reports *not rendered*. (R10)
- [ ] With `CLAUDE_CODE_DISABLE_WORKFLOWS=1` set, the smoke check reports
      *unavailable*, not *not rendered*. (R10)
- [ ] The smoke check's documentation lists one exit status per outcome, and
      the three statuses differ. (R10)
- [ ] The smoke check's documentation contains its command, its prerequisites
      and the output of one real run, including the Claude Code release and the
      outcome. (R10, R11)
- [ ] The hook CI job passes these payloads, with and without `jq`: a normal
      `session_id` and `transcript_path`; a `transcript_path` whose directory
      contains `"`, `\` and a space; an empty `transcript_path` with `cwd`; no
      `session_id`; and stdin that isn't JSON. For the first three, the output
      parses as JSON and its context contains the derived directory R12 names,
      byte for byte. For the last two, the output is empty and the exit status
      is 0. (R12)
- [ ] On the current `session-start-workflows.sh`, the no-`jq` run of the
      quote-and-backslash payload fails the job. (R12)
- [ ] The implementing pull request closes koto#243. (R12)
- [ ] A guide under `docs/guides/` has a section for each of the two procedures
      in R13, and the file-shape procedure ends with the R7 file-shape command.
      (R13)
- [ ] koto's release procedure documentation contains the R14 step. (R14)
- [ ] No workflow job that reads a credential has `pull_request` or
      `pull_request_target` among its triggers. (R15)
- [ ] The credentialed job's script, run with its credential unset and again
      with it set to an empty string, exits non-zero both times with a message
      naming the credential. (R15)
- [ ] The pull-request guard and hook jobs reference no `secrets.` value, and
      each job's duration in the implementing pull request's CI run is under
      five minutes. (R16)
- [ ] The guard's and the smoke check's scripts and workflow files contain no
      reference to `niwa`, and the guard passes on a GitHub-hosted runner with
      no niwa installed. (R17)
- [ ] The documented local guard command starts at most one `cargo` process,
      and every koto it starts has `KOTO_HOME` set to a scratch directory and
      `KOTO_SESSION_ID`, `KOTO_SESSIONS_BASE` and `KOTO_ANCHOR` unset. (R18)

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
- **Platforms beyond a GitHub-hosted Linux runner** for the CI guard, and
  beyond the maintainer's own machine for the smoke check.
- **Any coupling to niwa**, in either direction.

## Known Limitations

- **A lenient reader.** Claude Code fills in defaults for missing fields and
  drops unparseable files silently. A static convention check can confirm that
  the names koto relies on are still there; it can't prove the screen still
  draws them the way it did. Only the rendered smoke check gets close to that,
  and it isn't a pull-request check.
- **No headless list.** Claude Code 2.1.316 has no non-interactive command
  that lists workflows, so the rendered smoke check needs a terminal session
  and a real login. That's why it can't run on every pull request.
- **Feature gating.** `/workflows` can be disabled by setting, environment
  variable or account policy. R10's *unavailable* outcome keeps that from
  reading as a koto failure, but it also means a run under such an account
  says nothing about koto.
- **Things the guard will see but koto doesn't handle.** Claude Code 2.1.316
  can send an empty `transcript_path` in some storage modes, and it resets
  `CLAUDE_CODE_SESSION_ID` on `/clear`. Neither is in scope to fix here; R12
  only requires the hook to stay well-formed when `transcript_path` is empty.
- **A weekly window.** A Claude Code release that breaks a convention can reach
  operators in the days before the next scheduled run.

## Decisions and Trade-offs

- **Two layers, not one.** Research found that the conventions are visible as
  string literals in the Claude Code release itself, which can be fetched
  without credentials, while seeing the actual render needs a terminal and a
  login. The requirements therefore split a credential-free guard that can run
  often (R1 to R8, R16) from a rendered smoke check that runs with credentials
  and off pull requests (R10, R15). A single live check on every pull request
  was rejected for cost, secrets exposure on fork pull requests, and
  flakiness.
- **A schedule, not only pull requests.** Claude Code changes without a koto
  commit, so a guard that runs only on koto's pull requests would see a new
  release late or never. R6 adds a weekly run against the newest release.
- **koto#243 is in.** The hook script is how koto receives the SessionStart
  input and the directory, so it is part of the session-input convention the
  guard covers. Testing it without fixing the invalid-JSON fallback would leave
  a known failure in the path the guard is meant to protect.
- **The old shape test stays.** It still catches koto changing its own output
  by accident, which the new guard doesn't; R9 keeps it and stops calling it
  the Claude Code guard.
- **The release step is advisory.** A hard gate would block a koto release on a
  Claude Code outage or an account that can't see `/workflows`. R14 makes the
  check part of the procedure without making it mechanical.
- **The guard's form and the smoke check's location are left to the design.**
  The framing deferred them; the requirements above constrain them (no
  credentials on pull requests, named failures, a demonstrated failing run, a
  schedule) without choosing.
