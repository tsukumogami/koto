---
schema: design/v1
status: Accepted
problem: |
  koto's `/workflows` entry serializes its own blocked classification straight
  into the top-level `status`, a word Claude Code 2.1.314 doesn't know, and
  `build_detail` writes the active phase's step as `progress` whatever the
  run's status. On 2.1.314 that puts a gate-blocked session in the completed
  bucket with a Stopped step and hides the gate's FAIL (koto#553), and draws a
  finished session's terminal phase as unfinished (koto#554).
decision: |
  Split koto's classification from the screen's vocabulary. The top-level
  `status` becomes a screen word (`running`, `completed`, `failed`), with a
  gate-blocked session written as `running`; koto's own classification moves
  to a new `koto.status`. The active step's state follows the classification:
  `progress` when running, `error` with `blocked: true` and `error: "gate
  <name>: FAIL"` when blocked, `done` when completed, `error` with a
  cancelled-or-ended line when failed. A blocked summary ends with `· blocked
  on gate <name>`. The gate name comes from the same epoch-scoped evaluation
  the blocked predicate reads. Contract version 5.
rationale: |
  On 2.1.314 `running` is the only status the list doesn't count as completed,
  and an `error` step with `blocked` true is the only step shape that reads
  Blocked with its own text as the Outcome, so the blocked reading has one
  file shape and this is it. `paused` and the current `blocked` both land in
  the completed bucket, and putting blocked in the title leaves the step
  Stopped. Choosing the step state from the session classification fixes
  koto#554 in the same line that fixes koto#553, and keeping RenderStatus as
  koto's classification in the `koto` block costs one field and no second
  derivation.
upstream: docs/prds/PRD-koto-workflows-blocked-status.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: a gate-blocked koto session reads as blocked in `/workflows`

## Status

Accepted

Design for the requirements in `docs/prds/PRD-koto-workflows-blocked-status.md`
(tsukumogami/koto#553, with tsukumogami/koto#554 taken in). It replaces the
blocked-status mapping that `DESIGN-native-workflows-phase-detail` chose in
its Fork D; everything else that design settled stands.

## Context and Problem Statement

koto renders a session in Claude Code's `/workflows` screen by writing
`koto-<uuid>.json` into the hosting session's workflows directory on every
event append (`src/workflows_surface/materialize.rs`, `try_materialize`). The
file's shape is `WorkflowFile` in `src/workflows_surface/contract.rs`; its
content comes from `derive_enriched_projection` in
`src/workflows_surface/project.rs`, which classifies the session as one of
`RenderStatus::{Running, Blocked, Completed, Failed}` and lists its phases.

Two things in that pipeline are wrong for the Claude Code that operators run
today.

The first is that koto's classification is the file's top-level `status`. A
non-terminal session whose most recent gate in the current epoch failed is
upgraded from `Running` to `Blocked`, and `Blocked` serializes as
`"status": "blocked"`. The phase-detail design picked that word on the belief
that the screen supported it. It doesn't.

The second is that `build_detail` maps the active phase to a step with
`state: "progress"` no matter what the run's status is. On a blocked run the
step reads Stopped; on a completed run the terminal phase reads `0/1` and its
step reads Stopped.

### What Claude Code 2.1.314 renders

The facts this design builds on, read from the code Claude Code 2.1.314 ships
and checked against its live screen. They describe that one version.

- **Loading.** The screen lists `*.json` in the session's workflows directory,
  parses each file as plain JSON with no schema, skips files that don't parse,
  and sorts by `startTime`, newest first. It passes `status` through as
  written; only a missing `status` gets a default. It reads the files once,
  when the screen opens.
- **Statuses.** It knows six run statuses: `pending`, `running`, `completed`,
  `failed`, `killed` and `paused`. Any other word, `blocked` included, gets no
  icon and no header word.
- **The list.** The list's summary line has two buckets, "N running" and "N
  completed". Only `running` counts as running; every other status, known or
  not, counts as completed.
- **The header.** The detail header's status word is "done" for `completed`,
  "stopped" for `killed`, "paused" for `paused` and "failed" for `failed`;
  `running`, `pending` and unknown words get none. The line under the title is
  the file's `summary`.
- **The active rule.** A run is active only when its status is `running`. On a
  run that isn't active, a step still in `start` or `progress` is drawn as
  interrupted: "Stopped", with the Outcome "The workflow stopped before this
  agent finished."
- **Step display.** A step's display comes from its `state` and two optional
  flags. `done` reads done, and its Outcome is `resultPreview`. `error` with
  `blocked` true reads Blocked, drawn as a cross in the permission color, with
  the step's `error` text as the Outcome, under any run status. `error`
  without the flag reads failed, a red cross with `error` as the Outcome.
  `start` and `progress` read running on an active run and Stopped otherwise.
- **Phases and counts.** A phase's count is its `done` steps over all its
  steps. A phase reads done when every step is done, and failed (a red cross)
  when every step is done or errored and at least one errored. The header's
  "N/M agents" counts `done` steps only.
- **Keys.** A running entry offers `x stop` and `p pause`. For an entry loaded
  from a file, with no task registered behind it, both do nothing.
- **Free text.** Nothing at the top level carries a "blocked" or "waiting"
  word. The text a file controls on screen is `summary`, `workflowName`,
  phase titles, step labels, a step's `error`, `resultPreview` and
  `promptPreview`.

### What has to change

Read against those facts, the requirements fix one file shape:

- A gate-blocked session must not be counted as completed (R1). `running` is
  the only status that isn't, so the top-level status of a blocked session has
  to be `running`, and `blocked` can't stay there.
- Its active step must read Blocked, not Stopped, Running or Done (R2), and its
  Outcome must be the failing gate and its result (R3). Only `error` with
  `blocked` true does both, and the Outcome is whatever `error` says.
- The line under the title must say the session is blocked and name the gate,
  still naming koto, the workflow and the state (R4). That line is `summary`.
- koto's own classification has to stay readable, as one of `running`,
  `blocked`, `completed` or `failed`, in every entry (R5), because the top-level
  word no longer carries it.
- A completed session's steps must all read done, so the terminal phase reads
  `1/1`, the header reads N/N, nothing reads Stopped, and the terminal phase's
  subtitle stops saying "in progress" (R6, koto#554). Its top-level status stays
  `completed`.
- A failed session (a terminal state whose name contains `failed` or `error`,
  or a cancelled one) keeps top-level `failed`, and its active step must read
  failed with a non-empty Outcome saying it was cancelled or naming the state
  it ended at (R7).
- A running session reads exactly as today (R8), and once the blocked predicate
  stops holding, the next write shows no Blocked step and no blocked summary
  (R9).
- Nothing already verified regresses (R10): `koto.currentState` and the
  summary carry the current state, a reopen shows the new state, `startTime`
  is stable, a finished session reads `completed`, and no file is written with
  rendering off or with no Claude Code session to resolve.
- The contract version goes up and says why (R11); the golden fixture and its
  test pin the new shapes (R12); the verification script and guide (R13) and
  the koto-user skill, the session-feed reference and the phase-detail design
  (R14) say what the file now does.
- New text in the file goes through the shared sanitizer and cap (R15); these
  rendering facts are written down (R16, this section); and the write stays
  best-effort (R17).

The blocked, completed and failed classifications themselves don't change. The
predicate (`latest_epoch_gate_failed` in `src/engine/persistence.rs`), the
terminal check and the failure-name heuristic stay exactly as they are; only
how the entry presents them moves.

## Decision Drivers

- **D1. The screen's vocabulary is fixed.** koto can't add a status word to
  Claude Code; it can only choose among the six it knows and the step shapes
  it draws. Every option has to be judged against what 2.1.314 renders.
- **D2. Not completed and not stopped are both required.** The owner's
  requirement accepts either the status word or the gate result as the place
  "blocked" shows, but never accepts a blocked entry counted as completed or
  drawn as stopped (R1, R2).
- **D3. One derivation of "blocked".** The gate the step names must be the
  gate the predicate decided on, read from the same events, so the text and
  the classification can't disagree (R3, R9).
- **D4. Machine readers keep an answer.** A reader that used the top-level
  word for koto's classification must find it somewhere stable (R5).
- **D5. Same-change fix for koto#554.** The terminal-phase fault is the same
  field; one mapping should fix both.
- **D6. Additive where it can be.** New fields are optional and omitted when
  empty, the version bumps, and fields earlier readers rely on keep their
  meaning, except the one value that has to change.
- **D7. Untrusted text is sanitized.** Gate names and state names are
  template content shown in another program's screen (R15).
- **D8. The write stays best-effort and cheap.** It runs on every event
  append; no new log read, no store read (R17).

## Considered Options

### Decision 1: how a gate-blocked session is presented

Resolved inline.

A blocked session today is written with top-level `blocked`, its active step
`progress`, and `summary` `koto · <workflow> · <state>`. The question is which
combination of top-level status, step shape and free text gives the reading R1
to R4 ask for on 2.1.314.

Key assumptions:

- The 2.1.314 facts above hold for the version operators run.
- The gate the step names is the most recent gate evaluated in the current
  state's current epoch, the one `latest_epoch_gate_failed` reads.

#### Chosen: running, a blocked error step, and a blocked summary

The top-level `status` is `running`. The active phase's step is written
`state: "error"`, `blocked: true`, `error: "gate <name>: FAIL"`. The summary
becomes `koto · <workflow> · <state> · blocked on gate <name>`. The phase's
`detail` and the step's `resultPreview` keep the gate line they carry today.

On 2.1.314 that puts the entry in the running bucket with the running icon,
draws the step as Blocked with `gate <name>: FAIL` as its Outcome, and puts
"blocked on gate <name>" on the first screen of the detail view. It's the
only combination that meets R1, R2, R3 and R4 at once. The cost is that the
entry carries `x stop` and `p pause` hints that do nothing, and the blocked
phase's marker is the failed-phase cross, because its one step is errored.

#### Alternatives Considered

**Top-level `paused`, with the same blocked step.** `paused` is the closest
word Claude Code has for "waiting": the header would read "· paused", the icon
is the dim circle, and with no `scriptPath` in the file no resume key appears.
It reads well on the detail view. It fails R1: the list counts `paused` among
the completed runs, so an operator scanning "N running / N completed" still
reads the stuck session as finished. D2 rules it out.

**Keep top-level `blocked`, and fix only the step.** This keeps the file's
meaning for any reader that used the top-level word, and the blocked step
alone would satisfy R2 and R3. But `blocked` is an unknown word on 2.1.314:
no icon, no header word, and counted completed. It fails R1 the same way
`paused` does, with less to show for it.

**Say blocked in the title (`workflowName`).** The title is the one string
the list shows per row, so "Fix the bug (blocked)" would be visible without
opening the entry. It changes nothing about the bucket unless the status also
changes, it leaves the step Stopped or Running (R2), and it makes the title,
which is the session's stable label from its intent, change with state. The
summary carries the same word one screen deeper without any of that.

**`running` with the step left in `progress`.** The entry would count as
running and the step would read Running, "Still running…". That's not
Stopped, but it hides the gate entirely and tells the operator the opposite of
the truth. Fails R2 and R3.

### Decision 2: where koto's own classification lives

Resolved inline.

Once the top-level word is the screen's, a reader of the file can no longer
tell blocked from running there (D4). The question is what field carries
koto's classification.

Key assumptions:

- Nothing in this repository reads a written entry's top-level `status` back;
  the only read-back is `stable_start_time` in `materialize.rs`, which reads
  `startTime`.

#### Chosen: `koto.status`, always written

`KotoBlock` gains `status`, holding `RenderStatus` as it is today (`running`,
`blocked`, `completed`, `failed`), written on every entry. The `koto` block is
koto's namespaced extension surface and already carries `currentState` and
`contractVersion`; `RenderStatus` already is koto's classification, so the
field adds no second derivation and no new enum for readers to learn.

#### Alternatives Considered

**A `koto.blocked` boolean.** Smaller, and enough for the one fact that left
the top level. But it answers one question of four: a reader would still have
to combine it with the top-level word to tell completed from failed, and the
top-level word is now chosen for a screen whose vocabulary can change. One
enum that says what koto thinks is the more stable contract.

**Keep `blocked` at the top level and add a screen field.** Claude Code reads
`status` and nothing else for the bucket; there is no second field it would
honor. This is Decision 1's second alternative under another name.

### Decision 3: the active step on completed and failed sessions

Resolved inline.

`build_detail` writes the active phase's step as `progress` whatever the run's
status. On a completed run that's koto#554; on a failed run the step reads
Stopped too. The question is how the step state is chosen, and what a failed
step's `error` says.

Key assumptions:

- A cancellation is a `WorkflowCancelled` event in the log the projection
  already reads, so a `cancelled` flag costs no extra read.

#### Chosen: the step state follows the classification

The active step's shape is a function of the session's `RenderStatus`:

| `RenderStatus` | `state` | `blocked` | `error` |
|---|---|---|---|
| `Running` | `progress` | omitted | omitted |
| `Blocked` | `error` | `true` | `gate <name>: FAIL` |
| `Completed` | `done` | omitted | omitted |
| `Failed`, cancelled | `error` | omitted | `cancelled at <state>` |
| `Failed`, failure terminal | `error` | omitted | `ended at <state>` |

For the active phase's `detail` (its subtitle), a completed phase with no gate
or evidence reads `done` instead of `in progress`, and a failed one with no
gate or evidence reads its `error` line. Done phases, upcoming phases and the
running case are unchanged.

A completed run then has every step `done`: the terminal phase reads `1/1`,
the header reads N/N, and nothing reads Stopped. A failed run's last step
reads failed, a red cross, with a line that says why. koto#554 is fixed by the
same function that fixes koto#553, which is why it's taken in here.

#### Alternatives Considered

**Fix koto#554 separately.** The two issues would each be smaller, but they
change the same match arm in `build_detail`. Landing one first leaves the
mapping half by phase status and half by session status, and the second
change reworks the first's tests.

**Drop the terminal phase's step on a finished run.** With no step, nothing
can read Stopped. But the phase would read as not started, the header count
would drop a step, and the terminal state's directive would disappear from the
entry. Writing it `done` keeps everything the entry shows today and fixes the
count.

**Write a failed run's last step as `done`.** It would read cleanly, but a
failure would then sit inside a green phase, and the operator reading the
phase list would see nothing wrong. `error` puts the failure where the
operator looks.

## Decision Outcome

The three decisions fit into one rule: koto keeps classifying the session as
it does now, and the file translates that classification into the screen's
vocabulary in two places, the top-level `status` and the active step, while
writing the classification itself into `koto.status`.

For a gate-blocked session that means top-level `running`, `koto.status`
`blocked`, an active step `error` with `blocked: true` and
`error: "gate <name>: FAIL"`, and a summary ending `· blocked on gate <name>`.
On 2.1.314 the operator sees it under "running", opens it to find the summary
saying which gate it's blocked on, and finds the step Blocked with the gate's
FAIL as its Outcome (R1 to R4). A script reads `koto.status` (R5).

For a completed session it means every step `done` and top-level `completed`
(R6). For a failed one, top-level `failed` and an `error` step saying it was
cancelled or where it ended (R7). A running session's file is unchanged except
for the new `koto.status` and the version (R8).

The gate name is read from the same events the predicate reads: a new helper
returns the most recent gate evaluated in the current epoch, with its outcome,
and `latest_epoch_gate_failed` becomes a test on that helper's answer. A gate
that failed in an earlier visit to the state can't name a step, and a later
passing evaluation, an advance, a rewind or a terminal state each clear the
blocked shape on the next write (R3, R9).

The contract goes to version 5. Everything the change adds is optional or
omitted when empty except `koto.status`, which is always written. The one
value that changes meaning is the top-level `status`, which no longer reads
`blocked`; the version bump and `koto.status` are how a reader finds out.

The entry's `0s` duration is left alone. It comes from the file having no
`durationMs`, which the screen reads as zero, and whether a session reads
blocked doesn't depend on it; the PRD puts it out of scope, and this change
writes no `durationMs`.

## Solution Architecture

### Overview

The change stays inside `src/workflows_surface/` plus one helper in
`src/engine/persistence.rs`. The projection gains two facts (the blocked
gate's name and whether the session was cancelled), the contract gains a
screen-status type and three optional fields, and `materialize.rs` maps
classification to step shape in one function.

### Components

**`src/engine/persistence.rs`**

- New `pub fn latest_epoch_gate(events: &[Event], current_state: &str) ->
  Option<(String, bool)>`: the name and passed-flag of the latest
  `GateEvaluated` in `epoch_slice(events, current_state)`, or `None` when the
  epoch has none.
- `latest_epoch_gate_failed` becomes
  `latest_epoch_gate(events, current_state).is_some_and(|(_, passed)| !passed)`.
  Its callers (the dashboard's `is_blocked`, the projection) see no change in
  behavior, and the gate a step names can't drift from the predicate.

**`src/workflows_surface/project.rs`**

- `Projection` gains `cancelled: bool`, set from the `WorkflowCancelled` scan
  `project_from_log` already does. It is read only when the status is
  `Failed`, and the classification order already decides the overlap: a
  session at a terminal state is classified by its state name first, so a
  cancelled session that also sits at a failure terminal gets the "ended at"
  line, and "cancelled at" is written only for a session that is cancelled and
  not terminal.
- `EnrichedProjection` gains `blocked_gate: Option<String>`. Where
  `derive_enriched_projection` upgrades `Running` to `Blocked`, it now calls
  `latest_epoch_gate` once and, when the gate failed, sets both the status and
  `blocked_gate`. `blocked_gate` is `Some` exactly when `base.status` is
  `Blocked`. It is a projection field rather than something `materialize.rs`
  reads off the phase list because the summary needs the gate's name even when
  the template can't be read and there is no phase list.
- The phase's `detail` and the step's `resultPreview` keep coming from
  `per_state_outcomes`, which scans the whole log. For a blocked session that
  can't disagree with `blocked_gate`: the epoch is the tail of the log since the
  session last entered its current state, it holds at least one gate
  evaluation (otherwise the session isn't blocked), and so the latest gate
  recorded for that state anywhere in the log is the epoch's latest. The two
  reads differ only on a session that isn't blocked, where no `error` is
  written.

**`src/workflows_surface/contract.rs`**

- `CONTRACT_VERSION = 5`, with a doc sentence: "Version 5 writes the top-level
  `status` in Claude Code's vocabulary (a gate-blocked session reads
  `running`), adds `koto.status` with koto's own classification, adds the
  optional `blocked` and `error` fields on a `workflow_agent` step, and
  appends `· blocked on gate <name>` to a blocked session's `summary`."
- `RenderStatus` keeps its four variants and its role as koto's
  classification; its doc comment drops the claim that the screen supports
  `blocked`. New `RenderStatus::screen_status(self) -> ScreenStatus`.
- New `ScreenStatus { Running, Completed, Failed }`, serialized lowercase, the
  type of `WorkflowFile.status`. `Blocked` maps to `Running`; the others map
  to themselves.
- `KotoBlock` gains `status: RenderStatus` (`koto.status`).
- `ProgressNode::WorkflowAgent` gains `blocked: bool`
  (`skip_serializing_if = "std::ops::Not::not"`) and `error: String`
  (`skip_serializing_if = "String::is_empty"`). Its `state` doc lists `done`,
  `progress` and `error`.
- `WorkflowFile::new` keeps its signature, writes `status.screen_status()` at
  the top level and `status` into `koto.status`.
- New `WorkflowFile::with_blocked_gate(self, gate: &str) -> Self`: rebuilds
  `summary` as `koto · <workflow> · <state> · blocked on gate <name>`. The gate
  name is cut on its own first, at a new `GATE_NAME_CAP` of 128 raw
  characters with the cut marker, and then escaped, so a cut never splits an
  escape sequence; the `koto · <workflow> · <state>` prefix is capped so
  that prefix and suffix together stay within `FACT_CAP`. A long workflow or
  state name is what gets cut, never the "blocked on gate" suffix.

**`src/workflows_surface/materialize.rs`**

- New `fn active_step(status: RenderStatus, blocked_gate: Option<&str>,
  cancelled: bool, state: &str) -> ActiveStep`, with
  `ActiveStep { state: &'static str, blocked: bool, error: String }`,
  implementing Decision 3's table. The gate or state name inside an `error`
  string is cut at `GATE_NAME_CAP` and escaped the same way before it is
  joined.
- `preview()`, which builds `promptPreview` and `resultPreview`, keeps its
  whitespace collapse and its cut at 240 characters, and then escapes the cut
  text with `sanitize_one_line`. Escaping after the cut means the cut can't
  split an escape. Today it only collapses and cuts, so a template's directive
  or gate name can reach the file with raw control characters.
- `build_detail` takes the `ActiveStep` and uses it for the `Active` phase;
  `Done` and `Upcoming` keep their shape. Every title, label and `detail` it
  writes passes through `sanitize_capped(.., FACT_CAP)`.
- `phase_detail` takes the session status for the `Active` arm: `Completed`
  with no outcome reads `done`, `Failed` with no outcome reads the step's
  `error`, everything else as today.
- `try_materialize` builds the `ActiveStep` from the projection and calls
  `.with_blocked_gate(gate)` when `blocked_gate` is `Some`, including when the
  template can't be read and there are no phases.

### Key Interfaces

The file, for the reproduction session in koto#553 at version 5, with
`koto.holdings` omitted:

```json
{
  "id": "koto-c51396b2-f1b6-4192-8a2b-b64b2dabb57e",
  "name": "Legibility live check",
  "workflowName": "Legibility live check",
  "summary": "koto · legibility · verify · blocked on gate ready_flag",
  "status": "running",
  "startTime": 1793383422984,
  "phases": [
    { "title": "Gather", "detail": "evidence: findings" },
    { "title": "Verify", "detail": "gate ready_flag: FAIL" },
    { "title": "Done" }
  ],
  "workflowProgress": [
    { "type": "workflow_phase", "index": 1, "title": "Gather" },
    { "type": "workflow_agent", "index": 1, "label": "Gather", "phaseIndex": 1,
      "phaseTitle": "Gather", "state": "done", "promptPreview": "…",
      "resultPreview": "evidence: findings" },
    { "type": "workflow_phase", "index": 2, "title": "Verify" },
    { "type": "workflow_agent", "index": 2, "label": "Verify", "phaseIndex": 2,
      "phaseTitle": "Verify", "state": "error", "promptPreview": "…",
      "resultPreview": "gate ready_flag: FAIL", "blocked": true,
      "error": "gate ready_flag: FAIL" },
    { "type": "workflow_phase", "index": 3, "title": "Done" }
  ],
  "koto": {
    "sessionId": "c51396b2-f1b6-4192-8a2b-b64b2dabb57e",
    "workflow": "legibility",
    "currentState": "verify",
    "contractVersion": 5,
    "status": "blocked"
  }
}
```

The same session finished: `summary` `koto · legibility · done`, `status`
`completed`, `koto.status` `completed`, every step `done`, and the Done
phase's `detail` `done`.

### Data Flow

Unchanged in shape: `LocalBackend::append_event` calls
`materialize_after_commit`, which resolves the target directory, reads the log
once through `derive_enriched_projection`, builds the `WorkflowFile` and writes
it atomically. The new facts (`cancelled`, `blocked_gate`) come from that same
log read; there is no new read, no store access and no remote call (D8).

## Implementation Approach

One pull request, closing koto#553 and koto#554. The phases are the order the
work is done in, not separate commits: Phase 1 can land on its own, but the
version 5 shape breaks today's contract tests, the step tests in
`materialize.rs` and the golden fixture at once, so Phases 2 to 4 land as one
commit and Phase 5 follows it.

The new fields ripple through every constructor of the changed types, all of
them inside `src/workflows_surface/` and `tests/native_workflows_shape.rs`:
`ProgressNode::WorkflowAgent` literals in `contract.rs`'s tests,
`build_detail` and the shape test; `Projection` in `project_from_log`; and
`WorkflowFile.status`'s new type wherever a test reads it. No other file in
the repository constructs these types, nothing reads a written entry's
top-level `status` back, and the repository has no JSON schema for the
entry.

### Phase 1: the predicate helper and projection facts

`latest_epoch_gate` in `persistence.rs` with `latest_epoch_gate_failed`
rewritten on it, and unit tests that it returns the latest gate's name and
outcome, `None` with no gate in the epoch, and ignores an earlier epoch.
`Projection.cancelled` and `EnrichedProjection.blocked_gate` in `project.rs`.

Deliverables: `src/engine/persistence.rs`, `src/workflows_surface/project.rs`.

### Phase 2: the version 5 contract

`ScreenStatus`, `koto.status`, the step's `blocked` and `error`,
`with_blocked_gate`, the version bump and its doc. Contract unit tests:
`status_serializes_lowercase` splits into the top-level mapping (`Blocked`
writes `running`) and `koto.status` (all four words); `contract_version_is_four`
becomes `_is_five`; a blocked summary test; and sanitizer tests with a
control character in the gate name, an over-cap gate name (cut at
`GATE_NAME_CAP` with the cut marker, which also covers the PRD's over-fact-cap
case), and an over-long workflow name that still leaves the suffix intact
(AC12, AC17).

Deliverables: `src/workflows_surface/contract.rs`.

### Phase 3: the step mapping

`active_step`, `build_detail`, `phase_detail` and `try_materialize` in
`materialize.rs`, and the tests the PRD's criteria name: the blocked shape
(AC1 to AC3), `koto.status` for all four classifications (AC4), the completed
and failed shapes (AC5, AC6), the unchanged running shape (AC7), the four
unblocking cases (AC8), two gates in one state (AC9), the no-template case
(AC10), an unwritable target (AC19), template text with a control character
reaching `promptPreview`, `resultPreview`, a phase title and a `detail`
escaped, and a failing gate's captured output reaching none of `error`,
`resultPreview` or the summary. The
existing tests named in AC11 keep
their assertions; `ac3_gate_blocked_renders_blocked` and
`terminal_renders_completed_with_phases` change to the new shapes.

Deliverables: `src/workflows_surface/materialize.rs`.

### Phase 4: the pinned shape

Regenerate `tests/fixtures/native-workflows/enriched-shape.json` with the
ignored helper in `tests/native_workflows_shape.rs`, after changing the
canonical file to the version 5 blocked shape, and add a second canonical
file and fixture assertion for a completed entry's terminal step (AC13).

Deliverables: `tests/native_workflows_shape.rs`,
`tests/fixtures/native-workflows/enriched-shape.json`.

### Phase 5: verification and docs

- `scripts/verify-native-workflows.sh`: select the active step by
  `state in (progress, error)`; AC3 checks top-level `running`,
  `koto.status` `blocked`, the step's `blocked` and `error`, and the summary;
  render #3 checks every step is `done` (AC14).
- `docs/guides/native-workflows-verification.md`: the automated list and
  manual steps 5 and 6 say what 2.1.314 shows (AC15).
- `docs/reference/session-feed.md`, "Native `/workflows` File": a version 5
  paragraph (AC16).
- `plugins/koto-skills/skills/koto-user/SKILL.md`: the status sentence in
  "Native Claude Code `/workflows` rendering" (AC16).
- `docs/designs/current/DESIGN-native-workflows-phase-detail.md`: one line in
  its Status section pointing at this design for the status mapping (AC16).
- `CHANGELOG.md`, Unreleased: a compatibility note that the top-level
  `status` no longer reads `blocked`, and where koto's classification went.

This phase runs last because the script and docs describe behavior Phases 2
and 3 produce. The koto-skills assessment the repository requires after a
`src/` change is this phase's skill edit: the `koto-user` skill is the only
one that describes the entry, and `koto-author` and `koto-adhoc` say nothing
about it.

## Security Considerations

The file koto writes is read by another program and drawn in its terminal
screen, and part of its text comes from the workflow template, which someone
other than the operator may have written. This change adds three pieces of
template-derived text: the gate name in a blocked step's `error`, the same
name in the summary's suffix, and the state name in a failed step's `error`.
Each goes through the shared `sanitize_capped` helper, which escapes control,
bidirectional and invisible formatting characters as visible `\u{XXXX}`
sequences, folds newlines and tabs to spaces, and caps the length. The gate
name is capped on its own, at 128 characters, before it is joined into the
summary, so a long workflow or state name can't push the "blocked on gate"
suffix out of the line.

Gaps that predate this change are closed by it. `preview()` collapses
whitespace and truncates but doesn't escape anything, so `promptPreview` and
`resultPreview` can carry raw escape sequences from a template's directive or
gate name today, and `build_detail` copies phase titles, step labels and
`Phase.detail` (which carries gate names and the key names of submitted
evidence) into the file unescaped. This change routes `preview()` through the
sanitizer and passes every title, label and `detail` `build_detail` writes
through `sanitize_capped(.., FACT_CAP)`, so every string in the
`workflowProgress` and `phases` arrays gets the treatment the title and summary
already get. The tests assert the escaped form for `error`, `summary`,
`promptPreview`, `resultPreview`, a phase title and a `detail` built from an
evidence key.

The `error` text is fixed at the gate's name and the word FAIL. A test gives a
failing gate captured output and asserts that none of it reaches `error`,
`resultPreview` or the summary.

Escaping stops a name from driving the terminal; it doesn't stop a template
author from choosing a misleading gate name, such as one that reads like a
second status. That was already true of every name and directive the entry
shows, and the template author is already the one deciding what the entry
says. Whether Claude Code hands entry text to its model is outside what this
design can see; the entry carried template text before this change, and the
change adds only names.

The change opens no new filesystem, network or process surface. The file is
still written by the existing temp-file-and-rename path into the directory
koto already resolves, under a name built from the session's UUID, and the
temp file is created owner-only. The new gate lookup reads events koto already
holds in memory for the same write. No dependency is added, and the only
read-back is still `startTime` from koto's own previous file.

The new data in the file is koto's four-value classification and a gate or
state name. The `error` text carries the gate's name and the word FAIL, never
the gate's output or any submitted evidence value. That boundary should hold
in later changes: a gate's output can contain paths and secrets, and this file
lands outside koto's session store, in a directory another program reads.

Two things the design leaves as they are, because they predate it and its
change doesn't touch them: the target file name is built from the session id
in the state header without checking that it is a UUID, and
`stable_start_time` reads the previous file without a size bound. Both sit
under the same user's own directories, written by koto itself; tightening them
is separate work.

What's left is an integrity effect on readers. A script that checked the
top-level `status` for `blocked` will, from version 5, see a stuck session as
`running` and draw the wrong conclusion. The CHANGELOG entry says so and points
such readers at `koto.status`, and the version bump is there for a reader to
check. Beyond that, the mapping depends on how Claude Code 2.1.314 draws the
file; the golden fixture and the guide's manual check are what catch a change
there.

## Consequences

### Positive

- An operator can tell a blocked session from a finished one on the screen
  they already use, and sees which gate to fix without leaving it.
- A finished session reads done everywhere; a failed one reads failed with a
  reason.
- A script gets koto's classification from a field koto owns instead of from
  a word chosen for another program's screen.
- The step's gate text and the blocked classification come from one read of
  the same events, so they can't disagree.

### Negative

- A blocked entry shows the running icon and offers `x stop` and `p pause`,
  which do nothing for a file-loaded entry.
- The blocked phase's marker is the failed-phase cross, since its only step is
  errored; "failed" at the phase level is a stronger word than the truth.
- A reader that used the top-level `blocked` stops seeing it.
- A blocked session whose compiled template can't be read has no phase list
  and so no Blocked step; it reads blocked only through the running bucket,
  the summary and `koto.status`.
- The mapping is tuned to one Claude Code version's rendering. A later
  version could count buckets differently or stop honoring `blocked` on a
  step.

### Mitigations

- The step itself reads Blocked with the gate, and the summary says "blocked
  on gate <name>", so the phase cross is never the only signal. The guide
  tells operators the stop and pause keys don't act on koto entries.
- `koto.status`, the version 5 bump and the CHANGELOG note tell a reader where
  the classification went.
- The rendering facts are written down above, the golden fixture pins the
  shape, and the guide's manual check is the step that catches a Claude Code
  change.
