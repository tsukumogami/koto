---
schema: prd/v1
status: In Progress
source_issue: 553
problem: |
  An operator driving koto sessions inside Claude Code can't tell from the
  `/workflows` screen that a session is stopped on a failing gate. koto writes
  the entry with a status Claude Code 2.1.314 doesn't know, so the list counts
  it as completed, the active step reads "Stopped", and the gate's FAIL is
  hidden. A finished session's last step is written as unfinished, so it reads
  `Done 0/1` and "Stopped" on a completed run.
goals: |
  A gate-blocked koto session's entry reads as blocked on Claude Code 2.1.314:
  it isn't counted among completed runs, its active step reads Blocked with the
  failing gate's result as its outcome, and its summary line says blocked. A
  finished session reads done all the way down. koto's own classification stays
  readable in the file, and nothing the entry already shows correctly regresses.
absorbed:
  - docs/briefs/BRIEF-koto-workflows-blocked-status.md
---

# PRD: a gate-blocked koto session reads as blocked in `/workflows`

## Status

In Progress

Absorbed [BRIEF-koto-workflows-blocked-status](docs/briefs/BRIEF-koto-workflows-blocked-status.md); carried in Absorbed Brief.

Requirements for the fix to tsukumogami/koto#553, with tsukumogami/koto#554
taken in as the same change. Every statement here about what the screen shows
refers to Claude Code 2.1.314, the version whose shipped code and live screen
these requirements were checked against.

## Absorbed Brief

An operator driving koto sessions inside Claude Code uses `/workflows` to see
which session needs them, and today a session stopped on a failing gate looks
finished: koto writes a status word Claude Code doesn't know, the list counts
the entry as completed, the active step reads Stopped and the gate's FAIL is
hidden. A finished session has the mirror fault, its last phase drawn as
unfinished. Both come from how koto marks the active phase's step and which
status it trusts the screen to understand.

The outcome the operator should get: a blocked session stands apart from the
finished ones, its step reads Blocked with the failing gate as its outcome and
its summary says blocked; once unblocked it reads running and then done all
the way down. An integration author's script still finds koto's own
classification in the file, and nothing is written where nothing was before.
The feature holds in the list bucket, summary, step display, the terminal step
of finished and failed sessions, and the docs and tests that describe them; it
pushes out durations, the stop and pause keys, live refresh, hierarchies and
non-gate waits, and any change to when koto calls a session blocked.

## Problem Statement

koto writes one `koto-<uuid>.json` per session into the hosting Claude Code
session's `/workflows` directory, and the screen renders it as a run. When a
session stops on a gate that didn't pass, koto writes the entry with top-level
`"status": "blocked"` and the failed gate as the active phase's result. On
Claude Code 2.1.314 the operator sees none of that. The list counts the session
among the completed runs and gives it no icon, the detail header has no status
word, the active step reads "Stopped", and its outcome is "The workflow stopped
before this agent finished." rather than `gate ready_flag: FAIL` (#553).

The cause is a vocabulary mismatch. Claude Code 2.1.314 knows six run
statuses: pending, running, completed, failed, killed and paused. Its list has
two buckets, running and completed, and every status other than `running`
lands in completed. Only a `running` run is live, so on any other run a step
still marked in progress is drawn as stopped. `blocked` is not one of the six.

A finished session has the mirror fault (#554). koto writes the active phase's
step as in progress whatever the run's status, so on a completed run the
terminal phase reads `Done 0/1`, the header counts one step short, and the
terminal step reads "Stopped". Both faults sit in one field: how koto marks the
active phase's step.

Operators are the ones affected: anyone running koto-backed workflows inside
Claude Code who uses `/workflows` to see which session needs them. Today a
session waiting on them looks finished, which is the opposite of what the
screen is for.

## Goals

An operator opens `/workflows` and can tell which koto session needs them. A
session stopped on a failing gate isn't counted among the completed runs, its
active step reads Blocked rather than Stopped, the step's outcome names the
gate that failed, and the line under the title says the session is blocked.
When the operator fixes the condition and the session moves on, a reopen shows
it running, and when it finishes it reads done everywhere.

Nothing that reads correctly today gets worse (R10). A script that reads the
file for koto's own view of the session can still find that it is blocked (R5).

## User Stories

- As an operator with several koto sessions in one Claude Code session, I want
  a session stopped on a failing gate to stand apart from the finished ones in
  `/workflows`, so that I know which one needs me and which gate it is waiting
  on.
- As an operator who has just fixed a gate's condition, I want a reopen of
  `/workflows` to show the session running and then done, with no phase left
  looking unfinished, so that I can trust the screen once the session is
  through.
- As an integration author whose script reads the entry file, I want koto's own
  classification of the session (running, blocked, completed, failed) in the
  file, so that my script doesn't have to reverse-engineer it from fields shaped
  for Claude Code's screen.
- As an operator who has turned rendering off, or who runs koto outside Claude
  Code, I want no entry file written, so that the change costs me nothing.

## Requirements

A session is **gate-blocked** when koto's existing predicate holds: it isn't at
a terminal state, it wasn't cancelled, and the most recent gate evaluated in its
current state in the current epoch didn't pass. With two gates in one state,
the one evaluated last decides. A session is **completed** when it is at a
terminal state whose name koto's existing failure heuristic doesn't flag, and
**failed** when it is at a terminal state the heuristic flags (a name containing
`failed` or `error`) or when it was cancelled. A session that is none of these
is **running**. This PRD changes none of these classifications; it changes how
the entry presents them. The exact wording of new text in the file is the
design's call, within what the requirements below fix.

### Functional

- **R1. Not counted as completed.** On Claude Code 2.1.314, a gate-blocked
  session's entry is counted in the list's running bucket, not its completed
  bucket.
- **R2. The active step reads Blocked.** A gate-blocked session's active-phase
  step displays as Blocked in the detail view and the agent view, never as
  Stopped, Running or Done. When the entry has no phases (its compiled template
  can't be read), there is no step, and R1, R4 and R5 carry the blocked reading
  alone.
- **R3. The failing gate is visible.** The Blocked step's Outcome in the agent
  view names the gate the blocked predicate rests on (the most recent gate
  evaluated in the current state's current epoch) and its result, in the form
  `gate <name>: FAIL`. A gate failure from an earlier visit to the state never
  supplies this text.
- **R4. The summary line says blocked.** A gate-blocked session's `summary`
  (the line under the title) says the session is blocked and names the gate,
  while still naming koto, the workflow and the current state.
- **R5. koto's own classification stays readable.** Every entry koto writes
  carries `koto.status`, holding koto's classification of the session: exactly
  one of `running`, `blocked`, `completed` or `failed`.
- **R6. A completed session reads done everywhere.** For a completed session,
  every step in the file is done: the terminal phase reads `1/1`, the header's
  agent count reads N/N with N the number of steps, and no step reads Stopped
  (#554). The terminal phase's subtitle no longer reads "in progress". The
  top-level status stays `completed`.
- **R7. A failed session's last step reads failed.** For a failed session, the
  top-level status stays `failed`, and the active phase's step displays as
  failed, not Stopped, not Done and not Blocked. Its Outcome is a non-empty
  line saying why: that the session was cancelled, or that it ended at the
  named failure state.
- **R8. A running session is unchanged.** A session that is neither
  gate-blocked nor terminal reads as it does today: running bucket, active step
  Running, summary `koto · <workflow> · <state>`.
- **R9. Unblocking clears the blocked display.** Once the session's current
  state no longer satisfies the gate-blocked predicate (the gate passes on
  re-evaluation, or the session advances, rewinds or reaches a terminal state),
  the next write shows no Blocked step and no blocked summary.
- **R10. No regression.** Every entry still carries the session's current state
  in `koto.currentState` and in its summary;
  a reopen after an advance shows the new state; `startTime` stays stable across
  rewrites; a finished session's top-level status is still `completed`; and no
  entry file is written when `workflows.native = false` with no published
  location or `KOTO_WORKFLOWS_DIR`, or when no Claude Code session id is
  available to resolve a directory.
- **R11. The contract is versioned.** The file's contract version is bumped,
  and the contract's version comment says what the new version changed.
- **R12. The pinned shape follows the change.**
  `tests/fixtures/native-workflows/enriched-shape.json` and
  `tests/native_workflows_shape.rs` pin the new gate-blocked shape, and pin a
  completed session's terminal step as done.
- **R13. The verification stays true.** `scripts/verify-native-workflows.sh`
  asserts the new gate-blocked shape and the done terminal step, and
  `docs/guides/native-workflows-verification.md` describes what an operator
  sees on Claude Code 2.1.314 at each manual step.
- **R14. Living docs stay true.** The koto-skills text that describes the
  `/workflows` entry's status (`plugins/koto-skills/skills/koto-user/SKILL.md`)
  and the reference description of the file (`docs/reference/session-feed.md`,
  "Native `/workflows` File") are updated to match. The earlier phase-detail
  design, whose blocked-status decision this change replaces, says where the
  replacement is recorded.

### Non-functional

- **R15. Untrusted text stays sanitized.** Any text this change adds to the
  file (the gate name in the step's outcome and in the summary) passes the same
  sanitizer and cap every other printed fact in the file passes.
- **R16. The rendering facts are written down.** The design records the
  Claude Code 2.1.314 behavior these requirements rely on, described rather
  than quoted from Claude Code's code, so a later Claude Code upgrade can be
  checked against it.
- **R17. Best-effort stays best-effort.** The write still never fails the
  commit it follows.

## Acceptance Criteria

Unit tests below live beside the existing ones in
`src/workflows_surface/materialize.rs` and drive a real session through the
local backend, reading the written file back.

- [ ] **AC1.** A test drives a session into a state whose gate fails and
  asserts the written file's top-level `status` is `running` (R1).
- [ ] **AC2.** The same test asserts the active phase's `workflow_agent` step
  has `state` `error`, `blocked` `true`, and an `error` of
  `gate <name>: FAIL` (R2, R3).
- [ ] **AC3.** The same test asserts the `summary` contains `blocked` and the
  gate's name, and still begins `koto · <workflow> · <state>` (R4).
- [ ] **AC4.** Tests assert `koto.status` is present and reads `blocked` for the
  gate-blocked session, `running` for a running one, `completed` for a
  completed one, `failed` for a cancelled one, and `failed` for one at a
  terminal state named `failed` (R5).
- [ ] **AC5.** A test drives a three-phase session to a terminal non-failure
  state and asserts every `workflow_agent` step in the file has `state` `done`
  (so the count of done steps equals the count of steps), the terminal phase's
  `detail` is not `in progress`, and the top-level `status` is `completed`
  (R6).
- [ ] **AC6.** A test cancels a session and asserts the active phase's step has
  `state` `error`, no `blocked` field, a non-empty `error` containing
  `cancelled`, and a top-level `status` of `failed`. A second test drives a
  session to a terminal state named `failed` and asserts the same shape with an
  `error` containing that state's name (R7).
- [ ] **AC7.** A test with a passing gate asserts top-level `status` `running`,
  the active step `state` `progress`, no `blocked` or `error` field on it, and a
  summary equal to `koto · <workflow> · <state>` (R8).
- [ ] **AC8.** Four tests start from a gate-blocked write and assert the next
  write has no step with `blocked` `true` and no `blocked` in the summary:
  after a passing evaluation of the same gate; after an advance to the next
  state; after a rewind to an earlier state; and after leaving the state and
  re-entering it with no new gate evaluation (where the re-entered step also
  has no `error`) (R3, R9).
- [ ] **AC9.** A test records two gates in one state: gate A fails, then gate B
  passes, and the write is not blocked; then B fails after A passes, and the
  step's `error` names B (R3).
- [ ] **AC10.** A test drives a gate-blocked session whose compiled template
  can't be read, and asserts top-level `status` `running`, `koto.status`
  `blocked`, a summary containing `blocked`, and no `workflowProgress` (R2).
- [ ] **AC11.** These existing tests pass with their current assertions intact:
  `ac1_file_appears_with_current_state`, `ac2_file_updates_on_advance`,
  `start_time_is_stable_across_rewrites`, `native_gate_default_on_and_respects_explicit_optout`,
  `ac4_no_write_when_headless_and_unpublished`, and the `status` assertion in
  `terminal_renders_completed_with_phases` (R10).
- [ ] **AC12.** `CONTRACT_VERSION` reads 5 (it was 4), a test pins 5, and the
  constant's doc comment has a sentence beginning "Version 5" (R11).
- [ ] **AC13.** `cargo test --test native_workflows_shape` passes against a
  fixture whose blocked entry has top-level `running`, an `error` step with
  `blocked` `true`, and `koto.status` `blocked`, and the test pins a completed
  entry's terminal step as `done` (R12).
- [ ] **AC14.** `scripts/verify-native-workflows.sh` passes after `cargo build`,
  and it fails if any of these is missing: the blocked step, the blocked
  summary, `koto.status` `blocked`, and the done terminal step (R13).
- [ ] **AC15.** In `docs/guides/native-workflows-verification.md`, the blocked
  manual step says the entry is counted under "running", its active step reads
  Blocked with the `gate <name>: FAIL` Outcome, and the summary says blocked;
  the completed manual step says the terminal phase reads `1/1` and no step
  reads Stopped; and both name Claude Code 2.1.314 (R13).
- [ ] **AC16.** `plugins/koto-skills/skills/koto-user/SKILL.md` and
  `docs/reference/session-feed.md` say a gate-blocked entry's active step reads
  Blocked and that `koto.status` carries koto's classification, and neither
  says the top-level status reads `blocked`; the reference names contract
  version 5; and `docs/designs/current/DESIGN-native-workflows-phase-detail.md`
  links this change's design for the status mapping (R14).
- [ ] **AC17.** A test with a gate name carrying a control character asserts
  the step's `error` and the `summary` carry it escaped, not raw; a test with a
  gate name longer than the fact cap asserts both are cut at the cap with the
  cut marker (R15).
- [ ] **AC18.** The design document for this change has a section that states
  the six 2.1.314 run statuses, the two list buckets, that only `running` makes
  a run active, and that an `error` step with `blocked` true reads Blocked with
  its `error` as the Outcome, citing Claude Code 2.1.314 (R16).
- [ ] **AC19.** A test publishes a target path that can't be written (an
  existing regular file in place of the directory), drives a session into a
  failing gate, and asserts the event append still succeeds (R17).
- [ ] **AC20.** Manual, on Claude Code 2.1.314: the `hello-koto` template
  (`test/functional/fixtures/templates/hello-koto.md`) stopped on its
  `awakening` gate shows the entry under "running", its Awakening step as
  Blocked with `gate spirit_greeting: FAIL` as its Outcome, and its summary
  saying blocked; driven to its end, it shows the done check mark, `· done`, every phase
  complete (`1/1`) and no Stopped step.

## Out of Scope

- **The `0s` durations.** Every koto entry reads `0s` because the file has no
  `durationMs`. Whether a session reads blocked doesn't depend on it, so it's a
  separate fix.
- **The stop and pause keys.** A `running` entry offers `x stop` and `p pause`,
  and Claude Code ignores both for a file it didn't start. koto can't change
  that from the file.
- **Live refresh.** The screen reads the files once per opening.
- **koto's other surfaces.** `koto next`, `koto status`, the dashboard and the
  `koto workflows` listing keep their own blocked reporting unchanged; only the
  `/workflows` entry file changes.
- **Hierarchies and other waits.** Parent and child sessions rendering together,
  and sessions waiting on evidence, a decision or a child rather than a failed
  gate.
- **The blocked predicate.** When koto considers a session blocked stays as it
  is.
- **Other Claude Code versions.** The requirements are checked against 2.1.314
  only; guarding against a later version's changes is separate work.
- **Stale gate text on a done or re-entered phase.** A phase's `detail` and
  `resultPreview` show the latest gate evaluated in that state across the whole
  log, so a phase left or re-entered after a failed gate can still show that
  FAIL as text. This change only guarantees that such text never makes a step
  read Blocked (R3, R9); it doesn't change what the text says.
- **A session with no recorded transition.** A session that never advanced has
  no active phase today; this change gives it none.

## Known Limitations

- A gate-blocked entry carries the running-run hints (`x stop`, `p pause`),
  which do nothing for a koto entry. That's the cost of being counted as not
  completed: running is the only bucket that isn't.
- On 2.1.314 a phase whose steps have all finished or errored, with at least
  one errored, is drawn with the failed-phase cross. The blocked phase has one
  step, so its phase marker is that cross, even though the step itself reads
  Blocked. The operator sees "needs attention" at the phase level and the exact
  reason at the step.
- A reader that relied on the top-level `status` reading `blocked` stops seeing
  it there. koto's classification in the `koto` block (R5) and the contract
  version bump (R11) are how such a reader finds out.
- The rendering facts come from one Claude Code version. A later version may
  render the same file differently.

## Decisions and Trade-offs

- **#554 is part of this change.** The terminal-phase fault lives in the same
  field as the blocked fault, the active phase's step state, which koto writes
  as in progress whatever the run's status. Fixing one without the other would
  touch the same lines twice. Alternative: a separate change for #554, rejected
  because it would leave the field half-right in between.
- **"Not counted as completed" is a hard requirement.** The owner's requirement
  allows the status word or the gate result to carry "blocked", but not being
  counted completed and not reading Stopped are both required. On 2.1.314 only
  `running` keeps an entry out of the completed bucket, so R1 is phrased against
  the bucket, and which fields achieve it is the design's call.
- **The summary says blocked, too.** The gate result in the step is the
  requirement's minimum, but it is two levels deep (detail view, then agent
  view). The summary is on the detail view's first screen, so R4 puts blocked
  where the operator looks first.
- **The criteria name file fields.** AC1, AC2 and AC5 to AC8 assert the step's
  `state`, `blocked` and `error` fields rather than only the screen. On 2.1.314
  a step reads Blocked only when it is `error` with `blocked` true, and reads
  done only when it is `done`, so the screen behavior R1 to R7 ask for has
  exactly one file shape, and a test can check it where CI can't drive the
  screen. AC20 checks the screen itself.
- **Durations stay out.** Requested by nobody on this milestone's path and
  unrelated to the blocked reading; leaving them out keeps the change to one
  field and its tests.
