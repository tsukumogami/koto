---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-koto-workflows-blocked-status.md
milestone: "A gate-blocked koto session reads as blocked in /workflows"
step_count: 5
---

# PLAN: a gate-blocked koto session reads as blocked in `/workflows`

## Status

Active

## Scope Summary

Make a gate-blocked koto session's `/workflows` entry read as blocked on Claude
Code 2.1.314, and make a finished or failed session's last step read done or
failed, at contract version 5. One pull request, which fixes
tsukumogami/koto#553 and tsukumogami/koto#554.

## Decomposition Strategy

Walking skeleton. Step 1 drives a real session into a failing gate through the
local backend and reads the written entry back: it crosses the new epoch-gate
helper, the projection, the version 5 contract and the materializer on one
input, and the verify script runs the same path on the `hello-koto` template's
`awakening` gate. The golden fixture and the verify script move to the new
blocked shape in that step so the suite stays green. Later steps add the done
and failed readings, the unblocking edge cases, escaping for every
template-derived string, and the docs an operator or agent reads.

## Steps

### Step 1: A gate-blocked session's entry reads as blocked in the running bucket

**Goal**: A real session driven into a failing gate through the local backend writes a version 5 entry: top-level `running`, `koto.status` `blocked`, the active step `error` with `blocked` true and `error` `gate <name>: FAIL`, and a summary ending `· blocked on gate <name>`. This is the walking skeleton: it crosses the new persistence helper, the projection, the contract and the materializer on one real input (the `hello-koto` template's `awakening` gate in the verify script, and a unit test's failing gate). It also moves the golden fixture and shape test and the verify script to the new blocked shape so the suite stays green, and bumps `CONTRACT_VERSION` to 5.

**Fixes**: https://github.com/tsukumogami/koto/issues/553

**Acceptance Criteria**:

- [ ] After a session is driven into a state whose gate fails, the written entry file has top-level `status` `running` (not `blocked`) (AC1).
- [ ] In that same file, the active phase's `workflow_agent` step has `state` `error`, `blocked` `true`, and `error` exactly `gate <name>: FAIL` naming the gate that failed (AC2).
- [ ] In that same file, `summary` begins `koto · <workflow> · <state>`, contains `blocked`, and ends `· blocked on gate <name>` (AC3).
- [ ] Every entry carries `koto.status`: it reads `blocked` for the gate-blocked session and `running` for a session with a passing gate (AC4, blocked and running cases).
- [ ] A session with a passing gate still writes top-level `running`, an active step with `state` `progress` and no `blocked` or `error` field, and a summary equal to `koto · <workflow> · <state>` (AC7).
- [ ] When the failing gate's name contains a control character, the step's `error` and the `summary` show it as a visible escape, never raw. When the name is longer than the fact cap, both are cut with the cut marker and the summary still ends with `blocked on gate` text (AC17 as it applies to the blocked shape).
- [ ] The written entry reports `koto.contractVersion` 5, and `CONTRACT_VERSION` reads 5 in a test that pins it (AC12).
- [ ] `cargo test --test native_workflows_shape` passes against the regenerated fixture, whose blocked entry has top-level `running`, an `error` step with `blocked` `true`, and `koto.status` `blocked` (AC13, blocked part).
- [ ] Running `scripts/verify-native-workflows.sh` after `cargo build` against the `hello-koto` template stopped on its `awakening` gate passes, asserting top-level `running`, `koto.status` `blocked`, the blocked step with `error` `gate spirit_greeting: FAIL`, and the blocked summary. It exits non-zero if any of those is missing (AC14, blocked parts).
- [ ] The existing tests that already pass today (`ac1_file_appears_with_current_state`, `ac2_file_updates_on_advance`, `start_time_is_stable_across_rewrites`) still pass with their assertions unchanged.

**Check**: Run `cargo test` and see the materialize blocked-gate test (reading the written file back for AC1 to AC4 and AC7) and the contract version test pass, and run `cargo test --test native_workflows_shape` and see it pass against the regenerated `enriched-shape.json`. Then run `cargo build && scripts/verify-native-workflows.sh` and see it exit 0 with `ALL CHECKS PASSED`, with the blocked-shape assertions (top-level `running`, `koto.status` `blocked`, blocked step and summary) listed as passing in its output.

**Dependencies**: None

**Files**: `src/engine/persistence.rs`, `src/workflows_surface/project.rs`, `src/workflows_surface/contract.rs`, `src/workflows_surface/materialize.rs`, `tests/native_workflows_shape.rs`, `tests/fixtures/native-workflows/enriched-shape.json`, `scripts/verify-native-workflows.sh`

### Step 2: A finished session reads done and a failed one reads failed

**Goal**: A completed koto session's `/workflows` entry shows every step `done`, so the terminal phase reads 1/1, the header reads N/N and nothing reads Stopped; a cancelled session and a session that ended at a failure-named terminal state show their last step as failed with a line saying why. Each entry's `koto.status` reads `completed` or `failed`, and the top-level `status` stays `completed` or `failed`.

**Fixes**: https://github.com/tsukumogami/koto/issues/554

**Acceptance Criteria**:

- [ ] After a three-phase session is driven to a terminal non-failure state, the written file has the top-level `status` `completed` and `koto.status` `completed`. Every `workflow_agent` step has `state` `done`, so the count of `done` steps equals the count of steps, and no step has `progress` as its `state`.
- [ ] In that completed file, the terminal phase's entry in `phases` has a `detail` that is not `in progress` (it reads `done` when the phase has no gate or evidence line), and the terminal step carries no `blocked` or `error` field.
- [ ] After a session is cancelled at a non-terminal state, the written file has the top-level `status` `failed` and `koto.status` `failed`. The active phase's step has `state` `error`, no `blocked` field, and an `error` of `cancelled at <state>` naming the state the session was in.
- [ ] After a session is driven to a terminal state named `failed`, the written file has the top-level `status` `failed` and `koto.status` `failed`. The active phase's step has `state` `error`, no `blocked` field, and an `error` of `ended at failed`.
- [ ] A session that was cancelled while sitting at a failure-named terminal state gets the `ended at <state>` line, not `cancelled at <state>`.
- [ ] In the failed cases, the active phase's `detail` shows the step's `error` line when the phase has no gate or evidence line, so the failure text shows in the phase list too.
- [ ] A running session's file is unchanged by this step: top-level `status` `running`, the active step `state` `progress` with no `blocked` or `error` field, and a summary equal to `koto · <workflow> · <state>`.
- [ ] The state name inside a failed step's `error` is escaped and cut at the shared gate-name cap, so a state name holding a control character appears as a visible `\u{XXXX}` sequence, not raw.
- [ ] Running `scripts/verify-native-workflows.sh` against the built binary exits 0, and its render #3 (the finished session) fails the script if any `workflow_agent` step is not `done` or any step is `progress`.

**Check**: Run `cargo test workflows_surface` and see the completed, cancelled and failure-terminal tests pass, with the pinned-shape test `cargo test --test native_workflows_shape` also passing. Then run `cargo build && scripts/verify-native-workflows.sh` and see exit code 0 with render #3 reporting every `workflow_agent` step `done` and none `progress`.

**Dependencies**: Step 1

**Files**: `src/workflows_surface/project.rs`, `src/workflows_surface/materialize.rs`, `tests/native_workflows_shape.rs`, `scripts/verify-native-workflows.sh`

### Step 3: Unblocking and edge cases clear or keep the blocked reading correctly

A passing re-evaluation, an advance, a rewind or a re-entry without a new evaluation clears the blocked step and summary, two gates in one state name the last-evaluated one, and a session with no readable template still reads blocked through its status and summary.

### Step 4: Every template-derived string in the entry reaches the screen escaped

Control characters in a directive, gate name, state name, phase title or evidence key show up escaped in every field of the entry, a long gate name is cut without losing the blocked suffix, and no gate output reaches the file.

### Step 5: Operators and agents reading the docs see what the version 5 entry does

The verification guide, the session-feed reference, the koto-user skill, the phase-detail design pointer and the CHANGELOG describe the version 5 entry and what Claude Code 2.1.314 shows for it.

## Implementation Sequence

The steps run in order in one pull request. Once step 1 is in, steps 3 and 4
can run beside step 2, since each builds only on the blocked reading step 1
adds; step 5 follows step 2, because the docs describe the done and failed
readings too.
