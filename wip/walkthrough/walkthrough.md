# Batch Child Spawning: Interactive Walkthrough

This walkthrough demonstrates the end-to-end interaction between an
agent and koto for a 3-issue batch with a diamond dependency pattern.
It shows every `koto next` call and response, explaining what happens
inside koto and how the agent decides what to do next. Every JSON
example matches the shapes on the current `docs/batch-child-spawning`
branch: `SchedulerOutcome::Scheduled`, `MaterializedChild`,
`SchedulerFeedback`, `ReservedAction`, `BatchFinalView`, and the
cloud-backend `sync_status` envelope.

## Artifacts

### Parent template: `coord.md`

```yaml
---
name: coord
version: "1.0"
description: Coordinate implementation of a plan with dependent issues
initial_state: plan_and_await
variables:
  plan_path:
    description: Path to the plan document to implement
    required: true

states:
  plan_and_await:
    transitions:
      - target: summarize
        when:
          gates.done.all_success: true
      - target: analyze_failures
        when:
          gates.done.needs_attention: true
    gates:
      done:
        type: children-complete
    accepts:
      tasks:
        type: tasks
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents
      default_template: impl-issue.md
  analyze_failures:
    transitions:
      - target: plan_and_await
        when:
          evidence.retry_failed: present
      - target: summarize
        when:
          decision: give_up
      - target: summarize
        when:
          decision: acknowledge
    accepts:
      decision:
        type: enum
        values: [give_up, acknowledge]
        required: false
  summarize:
    terminal: true
---

## plan_and_await

Read the plan document at `{{plan_path}}`. For each issue outline in
the plan:

1. Extract the issue number, goal, files, and acceptance criteria.
2. Map dependencies to sibling task names (issue N -> "issue-N").
3. Build a task entry: `name="issue-N"`, `vars={"ISSUE_NUMBER": "N"}`,
   `waits_on=["issue-X", ...]` for each listed dependency. Check
   `expects.fields.tasks.item_schema` in the response for the full
   task entry shape; `template` defaults to `impl-issue.md`.

Submit the complete task list:
`koto next coord --with-data @tasks.json`

Once submitted, drive each child listed in
`scheduler.materialized_children` with `ready_to_drive: true AND
outcome != spawn_failed`. Use the ledger as the dispatch source of
truth — `spawned_this_tick` is a per-tick observation, not a
contract. After any child completes, re-call `koto next coord` to
unblock newly-ready dependents.

<!-- details -->
Each child is an independent koto workflow named `coord.<task-name>`.
The scheduler runs on every `koto next coord` tick and creates
children whose `waits_on` dependencies are all terminal. The
`scheduler.feedback.entries` map tells you how every submitted task
was handled (`accepted`, `already_running`, `already_terminal_success`,
`already_terminal_failure`, `already_skipped`, `blocked`, `errored`,
or `respawning`). The `blocking_conditions[0].output` field carries
per-child status plus the aggregate booleans (`all_complete`,
`all_success`, `any_failed`, `any_skipped`, `any_spawn_failed`,
`needs_attention`).
<!-- /details -->

## analyze_failures

At least one child failed or was skipped. Inspect the batch view in
the response's `blocking_conditions[0].output.children` array or in
`koto status coord`. Two recovery paths:

- **Retry the failures.** Submit the `retry_failed` reserved action
  (see `reserved_actions` in the response for the ready invocation
  string). The parent re-enters `plan_and_await` and the scheduler
  respawns the named children.
- **Give up or acknowledge.** Submit `{"decision": "give_up"}` or
  `{"decision": "acknowledge"}` to route to `summarize` with the
  batch outcome as-is.

## summarize

Write a summary covering which issues succeeded, which failed, and
why. The `batch_final_view` field in this response carries the full
snapshot so you don't need a second command.
```

The parent routes on three aggregate booleans the design added in
round 1. `all_success: true` is the clean-success path.
`needs_attention: true` (which implies `all_complete` plus at least
one `failed` or `skipped`) sends the agent to `analyze_failures`,
which is the state that accepts `retry_failed`. Compile warning W4
fires if a `materialize_children` state routes only on `all_complete`
— a batch that failed outright satisfies `all_complete: true` and
would otherwise slide into `summarize` with no retry window.

### Child template: `impl-issue.md`

```yaml
---
name: impl-issue
version: "1.0"
description: Implement a single GitHub issue
initial_state: working
variables:
  ISSUE_NUMBER:
    description: The GitHub issue number to implement
    required: true

states:
  working:
    transitions:
      - target: done
        when:
          status: complete
      - target: done_blocked
        when:
          status: blocked
    accepts:
      status:
        type: enum
        values: [complete, blocked]
        required: true
    gates:
      tests:
        type: command
        command: "cargo test"
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
    default_action:
      context_assignments:
        failure_reason: "Issue {{ISSUE_NUMBER}} hit an unresolvable blocker during implementation."
  skipped_due_to_dep_failure:
    terminal: true
    skipped_marker: true
---

## working

Implement issue #{{ISSUE_NUMBER}}. Read the issue, write the code,
run the tests. When finished, submit `{"status": "complete"}`. If
you hit an unresolvable blocker, submit `{"status": "blocked"}`.

## done

Issue #{{ISSUE_NUMBER}} implemented successfully.

## done_blocked

Issue #{{ISSUE_NUMBER}} is blocked and cannot proceed.

## skipped_due_to_dep_failure

Issue #{{ISSUE_NUMBER}} was skipped because a dependency failed. No
action required — the scheduler materialized this child directly
into its terminal skip state.
```

Three things the child template carries:

- `done_blocked` is terminal-with-`failure: true`. The scheduler and
  the gate both read this boolean rather than matching on state names.
- `default_action.context_assignments` writes `failure_reason` when
  the child enters `done_blocked`. Without any path writing
  `failure_reason` (accepts field, `default_action`, or
  `context_assignments` on an inbound transition), W5 fires at
  compile time and the batch view's `reason` falls back to the state
  name (`reason_source: "state_name"`). Today's W5 check only observes
  the accepts path; templates that rely on `default_action` or
  `context_assignments` may see false-positive W5 warnings until
  those surfaces are wired up, but the runtime behavior lands
  `failure_reason` correctly either way.
- `skipped_due_to_dep_failure` carries `skipped_marker: true`. F5
  requires every batch-eligible child template to declare at least
  one `skipped_marker` terminal state; the scheduler routes skip
  materializations here.

### Plan document: `PLAN-batch-schema.md`

```markdown
---
schema: plan/v1
status: Active
execution_mode: single-pr
issue_count: 3
---

# PLAN: batch-schema

## Scope Summary

Add the schema-layer changes for batch child spawning: tasks field
type, @file prefix, and the materialize_children hook.

## Issue Outlines

### 1. Add tasks field type to accepts schema

**Goal:** Extend `VALID_FIELD_TYPES` with a `tasks` variant that
accepts a structured task list payload.

**Files:** `src/template/types.rs`, `src/engine/evidence.rs`

**Acceptance criteria:**
- `type: tasks` is accepted in template `accepts` blocks.
- Evidence submission with a task list payload passes validation.
- Existing field types (enum, string, number, boolean) unchanged.

**Complexity:** simple

### 2. Add @file.json prefix to --with-data

**Goal:** When `--with-data` starts with `@`, read the remainder as
a file path and use its contents as the evidence payload.

**Files:** `src/cli/mod.rs`

**Acceptance criteria:**
- `koto next wf --with-data @file.json` reads and submits the file.
- 1 MB size cap applies.
- Missing file produces a clear error.

**Complexity:** simple
**Dependencies:** issue 1 (needs tasks type for end-to-end testing)

### 3. Add materialize_children hook to TemplateState

**Goal:** New optional field on `TemplateState` that declares batch
materialization from an accepts field.

**Files:** `src/template/types.rs`, `src/template/compile.rs`

**Acceptance criteria:**
- `materialize_children: { from_field: tasks }` compiles.
- Compiler validates E1-E10 rules.
- W1-W5 warnings fire correctly.

**Complexity:** testable
**Dependencies:** issue 1 (materialize_children.from_field must
point at a tasks-typed accepts field)
```

### Dependency graph

```
issue-1 (no deps)
   |
   +---> issue-2 (waits_on: [issue-1])
   |
   +---> issue-3 (waits_on: [issue-1])
```

Issue 1 must complete before issues 2 and 3. Issues 2 and 3 are
independent and can run in parallel.

### Task list (`tasks.json`)

```json
{
  "tasks": [
    {
      "name": "issue-1",
      "vars": {"ISSUE_NUMBER": "101"}
    },
    {
      "name": "issue-2",
      "vars": {"ISSUE_NUMBER": "102"},
      "waits_on": ["issue-1"]
    },
    {
      "name": "issue-3",
      "vars": {"ISSUE_NUMBER": "103"},
      "waits_on": ["issue-1"]
    }
  ]
}
```

`template` is omitted — the scheduler uses `default_template:
impl-issue.md` from the hook. `waits_on` is omitted from issue-1
(defaults to `[]`). `trigger_rule` is omitted everywhere (only
`all_success` is accepted in v1 per E8 decision).

---

## Walkthrough

### Interaction 1: `koto init coord --template coord.md --var plan_path=PLAN-batch-schema.md`

**What happens inside koto:**
- Creates session directory for `coord`.
- `init_state_file` writes a temp file with the header plus initial
  events, then atomically installs it with
  `renameat2(RENAME_NOREPLACE)` on Linux (or `link()` + `unlink()`
  on other Unixes). A collision returns `SpawnErrorKind::Collision`.
- The header carries `template_source_dir` (absolute path to the
  parent template's directory).
- Initial events: `WorkflowInitialized`, `Transitioned → plan_and_await`.

**Response:**
```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

**What this tells the agent:** the coordinator workflow exists.
Drive it with `koto next coord`.

### Interaction 2: `koto next coord`

**What happens inside koto:**
- Acquires non-blocking advisory flock on `<session>/coord.lock`
  (current state has a `materialize_children` hook, so Q3 applies).
- Advance loop at `plan_and_await`. The `tasks` field is declared
  `required: true` and no evidence has been submitted yet.
- `children-complete` gate: 0 children on disk and no batch
  definition in evidence → `Failed`, with a zero-filled output.
- Advance loop stops. The scheduler sees `materialize_children` on
  the parked state but no `tasks` field → `SchedulerOutcome::NoBatch`.
- No `any_failed` or `any_skipped` → `reserved_actions` is absent.

**Response:**
```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-batch-schema.md. For each issue outline in the plan:\n\n1. Extract the issue number, goal, files, and acceptance criteria.\n2. Map dependencies to sibling task names (issue N -> \"issue-N\").\n3. Build a task entry: name=\"issue-N\", vars={\"ISSUE_NUMBER\": \"N\"}, waits_on=[\"issue-X\", ...] for each listed dependency. Check expects.fields.tasks.item_schema in the response for the full task entry shape; template defaults to impl-issue.md.\n\nSubmit the complete task list:\n`koto next coord --with-data @tasks.json`\n\nOnce submitted, drive each child in scheduler.materialized_children with `ready_to_drive: true AND outcome != spawn_failed`. After any child completes, re-call `koto next coord` to unblock newly-ready dependents.",
  "details": "Each child is an independent koto workflow named `coord.<task-name>`. The scheduler runs on every `koto next coord` tick and creates children whose `waits_on` dependencies are all terminal. The `scheduler.feedback.entries` map tells you how every submitted task was handled (accepted, already_running, already_terminal_success, already_terminal_failure, already_skipped, blocked, errored, respawning). The `blocking_conditions[0].output` field carries per-child status plus the aggregate booleans (all_complete, all_success, any_failed, any_skipped, any_spawn_failed, needs_attention).",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name": {"type": "string", "required": true, "description": "Child workflow short name"},
          "template": {"type": "string", "required": false, "default": "impl-issue.md"},
          "vars": {"type": "object", "required": false},
          "waits_on": {"type": "array", "required": false, "default": []},
          "trigger_rule": {"type": "string", "required": false, "default": "all_success"}
        }
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 0,
      "completed": 0,
      "pending": 0,
      "success": 0,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": false,
      "needs_attention": false,
      "children": []
    }
  }],
  "scheduler": null
}
```

**What this tells the agent:** "I need evidence. The directive tells
me to read the plan and build a task list. `expects.fields.tasks`
declares the shape — `item_schema` is koto-generated and carries
the `default` for `template`, so I can omit that field per task."

The agent reads `PLAN-batch-schema.md`, parses the 3 issue
outlines, maps dependencies to `waits_on`, and writes `tasks.json`.

### Interaction 3: `koto next coord --with-data @tasks.json`

**What happens inside koto:**
- Advance loop at `plan_and_await`. Evidence is pre-append validated
  against the accepts schema plus R0 (non-empty), R3 (no cycles),
  R4 (no dangling refs), R5 (unique names), R6 (hard limits),
  R8 (spawn-time immutability — no existing children yet, so vacuous),
  and R9 (names match `^[A-Za-z0-9_-]+$`, not reserved). All pass.
- Appends `EvidenceSubmitted { fields: { tasks: [...], submitter_cwd:
  "/home/dan/src/tsuku" } }`.
- Re-evaluates `children-complete`: 0 children on disk, batch
  definition of 3 tasks available → `Failed`, `total=3`, `blocked=2`.
- The `all_success: true` transition guard does not match; the
  `needs_attention: true` guard does not match. Advance loop stops
  at `plan_and_await`.
- Scheduler runs. Builds DAG, validates R1/R2 per-task. Classifies:
  `issue-1` is `Ready`; `issue-2` and `issue-3` are `BlockedByDep`.
- Spawns `coord.issue-1` via `init_state_file`. `spawn_entry`
  (template, canonical vars, sorted waits_on) is captured on the
  child's `WorkflowInitialized` event per R8.
- Non-trivial tick: appends a `SchedulerRan` event to the parent log.
- Returns `Scheduled` with per-tick observation, ledger, feedback,
  and empty errored/warnings.

**Response:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-batch-schema.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 0,
      "pending": 3,
      "success": 0,
      "failed": 0,
      "skipped": 0,
      "blocked": 2,
      "spawn_failed": 0,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-2", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-1"]},
        {"name": "coord.issue-3", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-1"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "task": "issue-1", "outcome": "pending", "state": "working", "waits_on": [], "ready_to_drive": true, "role": "worker"}
    ],
    "reclassified_this_tick": true,
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "accepted"},
        "issue-2": {"outcome": "blocked", "waits_on": ["issue-1"]},
        "issue-3": {"outcome": "blocked", "waits_on": ["issue-1"]}
      },
      "orphan_candidates": []
    },
    "errored": [],
    "warnings": []
  }
}
```

**What this tells the agent:**
- `action: gate_blocked` with `category: temporal` → "the parent
  can't advance yet, go do work on children."
- `scheduler.spawned_this_tick: ["coord.issue-1"]` is the per-tick
  observation; `materialized_children` is the ledger to key
  idempotent dispatch on.
- `scheduler.feedback.entries.issue-1.outcome: "accepted"` confirms
  the task was spawned; `issue-2` and `issue-3` report `blocked`
  with their `waits_on`. No silent no-ops.
- `reclassified_this_tick: true` confirms dispatch state shifted this
  tick — worth another ledger read.
- `role: "worker"` on `coord.issue-1` signals the child is a plain
  worker (no nested batch). Coordinators-of-sub-batches would report
  `role: "coordinator"` with a `subbatch_status` summary.
- `needs_attention` is false, so no `reserved_actions` block.

The agent starts driving `coord.issue-1`.

### Interaction 4: `koto next coord.issue-1`

**What happens inside koto:**
- Reads `coord.issue-1`'s state file (created by the scheduler).
  Current state: `working`. No advisory lock — child workflows don't
  carry a `materialize_children` hook.
- Advance loop at `working`. `status` evidence is missing; the
  `tests` gate (`command: cargo test`) runs and passes.
- Gate passes, but the transition still needs `status` evidence.

**Response:**
```json
{
  "action": "evidence_required",
  "state": "working",
  "directive": "Implement issue #101. Read the issue, write the code, run the tests. When finished, submit {\"status\": \"complete\"}. If you hit an unresolvable blocker, submit {\"status\": \"blocked\"}.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": {"type": "enum", "values": ["complete", "blocked"], "required": true}
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**What this tells the agent:** "I'm implementing issue #101. No
gates block me. When I'm done, submit `status` evidence."

### Interaction 5: `koto next coord.issue-1 --with-data '{"status": "complete"}'`

**What happens inside koto:**
- Advance loop at `working`. Validates `status` as an enum value
  in `[complete, blocked]` → passes.
- Appends `EvidenceSubmitted { fields: { status: "complete" } }`.
- `tests` gate re-runs and passes. Transition matches on
  `status: complete` → `done`. `done` is terminal.

**Response:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #101 implemented successfully.",
  "is_terminal": true
}
```

**What this tells the agent:** "Child is finished. Return to the
parent." No `batch_final_view` on this response — `coord.issue-1`
is not a batch parent.

### Interaction 6: `koto next coord` (re-tick parent)

**What happens inside koto:**
- Advisory flock re-acquired on `coord.lock`. Tempfile sweep (Q7)
  removes any `.koto-*.tmp` files older than 60 s under the parent's
  session directory.
- Advance loop at `plan_and_await`. Gate re-evaluates: `coord.issue-1`
  is terminal with `failure: false` → outcome `success`. `issue-2`
  and `issue-3` have no state file on disk yet → outcome `blocked`.
  `all_complete: false`. Neither aggregate guard fires.
- Scheduler re-classifies. `issue-1` is `Terminal`; `issue-2` and
  `issue-3` are `Ready` (their `waits_on` dependency is terminal).
- Spawns `coord.issue-2` and `coord.issue-3`. `SchedulerRan` event
  appended.

**Response:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-batch-schema.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 1,
      "pending": 2,
      "success": 1,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-2", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-3", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-2", "coord.issue-3"],
    "materialized_children": [
      {"name": "coord.issue-1", "task": "issue-1", "outcome": "success", "state": "done", "waits_on": [], "ready_to_drive": false, "role": "worker"},
      {"name": "coord.issue-2", "task": "issue-2", "outcome": "pending", "state": "working", "waits_on": ["issue-1"], "ready_to_drive": true, "role": "worker"},
      {"name": "coord.issue-3", "task": "issue-3", "outcome": "pending", "state": "working", "waits_on": ["issue-1"], "ready_to_drive": true, "role": "worker"}
    ],
    "reclassified_this_tick": true,
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "already_terminal_success"},
        "issue-2": {"outcome": "accepted"},
        "issue-3": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    },
    "errored": [],
    "warnings": []
  }
}
```

**What this tells the agent:**
- `issue-1` is done (`outcome: success`).
- `spawned_this_tick: ["coord.issue-2", "coord.issue-3"]` — both
  just materialized. They have no dependency on each other, so the
  agent can dispatch them in parallel.
- `feedback.entries.issue-1.outcome: "already_terminal_success"` confirms the
  resubmission is a no-op on that row (issue-1 already finished successfully).

The agent dispatches two workers for the new children.

### Interaction 7: Drive issue-2 and issue-3 (happy path)

Each worker runs its own drive loop:

```
koto next coord.issue-2
koto next coord.issue-2 --with-data '{"status": "complete"}'
```

and

```
koto next coord.issue-3
koto next coord.issue-3 --with-data '{"status": "complete"}'
```

Each exits with a `done` response analogous to Interaction 5.

### Interaction 8: `koto next coord` (final tick, happy path)

**What happens inside koto:**
- Advance loop at `plan_and_await`. Gate sees 3 terminal success
  children. `all_complete: true`, `all_success: true`. The
  transition guarded on `gates.done.all_success: true` fires →
  `summarize`. `summarize` is terminal.
- Before stopping, the advance loop appends `BatchFinalized` with
  the final `BatchView` snapshot (first `all_complete: true` on a
  state with `materialize_children`).
- Scheduler is called on `summarize`, finds no hook → `NoBatch`.
- `handle_next` detects `BatchFinalized` in the log and attaches
  `batch_final_view` to the terminal response.

**Response:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch": {"phase": "final"},
  "batch_final_view": {
    "total": 3,
    "completed": 3,
    "pending": 0,
    "success": 3,
    "failed": 0,
    "skipped": 0,
    "blocked": 0,
    "spawn_failed": 0,
    "all_complete": true,
    "all_success": true,
    "any_failed": false,
    "any_skipped": false,
    "any_spawn_failed": false,
    "needs_attention": false,
    "children": [
      {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
      {"name": "coord.issue-2", "state": "done", "complete": true, "outcome": "success"},
      {"name": "coord.issue-3", "state": "done", "complete": true, "outcome": "success"}
    ]
  }
}
```

**What this tells the agent:** "Batch is finished, everything
succeeded. `batch_final_view` has the final snapshot for the
summary directive." No `reserved_actions` — nothing to retry.

---

## Failure scenario: issue-2 submits `{"status": "blocked"}`

The agent runs
`koto next coord.issue-2 --with-data '{"status": "blocked"}'`:

```json
{
  "action": "done",
  "state": "done_blocked",
  "directive": "Issue #102 is blocked and cannot proceed.",
  "is_terminal": true
}
```

The child's `default_action.context_assignments` writes
`failure_reason` on entry to `done_blocked`, so the batch view's
`reason` for this child will carry a readable message instead of
falling back to the state name.

### Interaction F1: `koto next coord` after issue-2 fails

**What happens inside koto:**
- Advance loop at `plan_and_await`. Gate re-evaluates:
  - `coord.issue-1`: terminal, success
  - `coord.issue-2`: terminal, `failure: true` → outcome `failure`
  - `coord.issue-3`: terminal, success (finished in parallel)
- Aggregates: `total=3`, `completed=3`, `failed=1`,
  `all_complete: true`, `all_success: false`, `any_failed: true`,
  `needs_attention: true`.
- The `all_success` guard does not fire. The `needs_attention`
  guard does → transition to `analyze_failures`. On the first
  `all_complete: true` pass, the advance loop appends a
  `BatchFinalized` event.
- Scheduler is called on `analyze_failures`, finds no hook →
  `NoBatch`.
- `reserved_actions` is synthesized because `any_failed: true`.

**Response:**
```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. Inspect the batch view in the response's blocking_conditions[0].output.children array or in `koto status coord`. Two recovery paths:\n\n- Retry the failures. Submit the retry_failed reserved action (see reserved_actions in the response for the ready invocation string). The parent re-enters plan_and_await and the scheduler respawns the named children.\n- Give up or acknowledge. Submit {\"decision\": \"give_up\"} or {\"decision\": \"acknowledge\"} to route to summarize with the batch outcome as-is.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {
        "type": "enum",
        "values": ["give_up", "acknowledge"],
        "required": false
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 3,
      "pending": 0,
      "success": 2,
      "failed": 1,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": true,
      "all_success": false,
      "any_failed": true,
      "any_skipped": false,
      "any_spawn_failed": false,
      "needs_attention": true,
      "children": [
        {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-2", "state": "done_blocked", "complete": true, "outcome": "failure", "failure_mode": "done_blocked:failure_reason", "reason": "Issue 102 hit an unresolvable blocker during implementation.", "reason_source": "failure_reason"},
        {"name": "coord.issue-3", "state": "done", "complete": true, "outcome": "success"}
      ]
    }
  }],
  "reserved_actions": [
    {
      "action": "retry_failed",
      "label": "Retry failed children",
      "description": "Re-run children whose outcome is failure, skipped, or spawn_failed.",
      "applies_to": ["issue-2"],
      "invocation": "koto next 'coord' --with-data '{\"retry_failed\":{\"children\":[\"issue-2\"]}}'"
    }
  ],
  "scheduler": null
}
```

**What this tells the agent:**
- The parent advanced to `analyze_failures`. `needs_attention: true`
  on the gate aggregates was the signal.
- `reserved_actions[0]` gives a ready `invocation` string that the
  agent can run without having memorized the `retry_failed` schema.
- `reason_source: "failure_reason"` on `coord.issue-2` confirms the
  reason came from the child's context key, not from the state
  name.

### Interaction F2: `koto next coord --with-data '{"retry_failed":{"children":["issue-2"]}}'`

**What happens inside koto:**
- Advisory flock acquired. `handle_retry_failed` intercepts in
  `handle_next` *before* `advance_until_stop` runs.
- R10 validates the payload: non-empty `children` (short task names,
  not composed `<parent>.<task>` names), each exists on disk, each
  has outcome `failure` or `skipped`. `include_skipped` defaults to
  `true`. `issue-2` qualifies. Mixed payloads (retry_failed plus
  other keys) would reject with
  `InvalidRetryReason::MixedWithOtherEvidence`.
- Validation passes → canonical retry sequence:
  1. Append `EvidenceSubmitted { retry_failed: {...} }` to coord.
  2. Append the clearing `EvidenceSubmitted { retry_failed: null }`
     to coord (under `CloudBackend`, `sync_push_state` here; Decision
     12 Q6's push-parent-first eliminates phantom child epochs).
  3. For each child in the downward closure of `issue-2`:
     `coord.issue-2` has outcome `failure` → append `Rewound`
     targeting `working` (the initial state). No skipped dependents,
     so nothing else changes.
- Control returns to `advance_until_stop`. The advance loop is now
  at `analyze_failures`. The transition guarded on
  `evidence.retry_failed: present` matches (the un-merged
  submission payload). Advance loop transitions back to
  `plan_and_await`.
- Scheduler runs on `plan_and_await`. `coord.issue-2` is now
  `Running` (state file exists, not terminal). Nothing to spawn.
  `SchedulerRan` not appended (no spawn/skip/error).

**Response:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-batch-schema.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 2,
      "pending": 1,
      "success": 2,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-2", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-3", "state": "done", "complete": true, "outcome": "success"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.issue-1", "task": "issue-1", "outcome": "success", "state": "done", "waits_on": [], "ready_to_drive": false, "role": "worker"},
      {"name": "coord.issue-2", "task": "issue-2", "outcome": "pending", "state": "working", "waits_on": ["issue-1"], "ready_to_drive": true, "role": "worker"},
      {"name": "coord.issue-3", "task": "issue-3", "outcome": "success", "state": "done", "waits_on": ["issue-1"], "ready_to_drive": false, "role": "worker"}
    ],
    "reclassified_this_tick": true,
    "feedback": {"entries": {}, "orphan_candidates": []},
    "errored": [],
    "warnings": []
  }
}
```

**What this tells the agent:** "The retry worked. Parent routed
back to `plan_and_await`. `coord.issue-2` is `pending` again on
its initial `working` state. Drive it." `feedback.entries` is
empty because no `tasks` payload was submitted this tick — the
retry path does not update the task set.

The agent drives `coord.issue-2` through to `done`, then ticks the
parent again. The gate then evaluates to `all_success: true`; the
advance loop appends a fresh `BatchFinalized` (superseding the
prior one) and transitions to `summarize`. Terminal response shape
is identical to Interaction 8 with `batch_final_view.success: 3`.

---

## Skip scenario: issue-1 fails; issue-2 and issue-3 are skipped

If `coord.issue-1` reaches `done_blocked` before `issue-2` and
`issue-3` ever run, the scheduler materializes them as skip markers
on the next parent tick. Both children land directly in
`skipped_due_to_dep_failure` (marked `skipped_marker: true`). Their
context carries `skipped_because: coord.issue-1`.

### Parent response after the skip tick

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. ...",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "values": ["give_up", "acknowledge"], "required": false}
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 3,
      "pending": 0,
      "success": 0,
      "failed": 1,
      "skipped": 2,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": true,
      "all_success": false,
      "any_failed": true,
      "any_skipped": true,
      "any_spawn_failed": false,
      "needs_attention": true,
      "children": [
        {"name": "coord.issue-1", "state": "done_blocked", "complete": true, "outcome": "failure", "failure_mode": "done_blocked:failure_reason", "reason": "Issue 101 hit an unresolvable blocker during implementation.", "reason_source": "failure_reason"},
        {"name": "coord.issue-2", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-1", "skipped_because_chain": ["coord.issue-1"], "reason_source": "skipped"},
        {"name": "coord.issue-3", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-1", "skipped_because_chain": ["coord.issue-1"], "reason_source": "skipped"}
      ]
    }
  }],
  "reserved_actions": [
    {
      "action": "retry_failed",
      "label": "Retry failed children",
      "description": "Re-run children whose outcome is failure, skipped, or spawn_failed.",
      "applies_to": ["issue-1", "issue-2", "issue-3"],
      "invocation": "koto next 'coord' --with-data '{\"retry_failed\":{\"children\":[\"issue-1\"]}}'"
    }
  ],
  "scheduler": null
}
```

**What this tells the agent:**
- `any_failed` and `any_skipped` are both true.
- `skipped_because_chain: ["coord.issue-1"]` on the skipped
  children points the agent at the root cause directly.
- `reserved_actions[0].applies_to` lists all retryable children by
  short task name. Naming just `issue-1` in the submission propagates
  the retry downward to `issue-2` and `issue-3` via
  `include_skipped: true` (the default).

### `koto status coord.issue-2` on a skip marker

```json
{
  "workflow": "coord.issue-2",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-1",
  "skipped_because_chain": ["coord.issue-1"],
  "reason_source": "skipped"
}
```

And `koto next coord.issue-2` on the same skip marker returns:

```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "This task was skipped because dependency 'coord.issue-1' did not succeed. No action required.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-1",
  "skipped_because_chain": ["coord.issue-1"]
}
```

`synthetic: true` is computed from `skipped_marker: true` on the
child's current state — no template hash, no sidecar flag. If the
agent later submits `retry_failed` naming `coord.issue-1`, the
scheduler deletes-and-respawns the skip markers for `issue-2` and
`issue-3` as rewind targets of the original `impl-issue.md`
template (runtime reclassification, Decision 9 Part 5).

---

## Error scenario: submission with a cycle

Suppose the agent submits a broken task list:

```json
{"tasks": [
  {"name": "issue-A", "waits_on": ["issue-B"]},
  {"name": "issue-B", "waits_on": ["issue-A"]}
]}
```

**What happens inside koto:**
- Pre-append validation runs R0, R3, R4, R5, R6, R8, R9. R3 detects
  the cycle.
- Rejection is pre-append: no `EvidenceSubmitted` is written, no
  `SchedulerRan` is written, parent state file is byte-identical
  to the pre-call state.
- Response is the `action: "error"` envelope from Decision 11.
  `error.batch` carries the typed discriminator.

**Response:**
```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: cycle in waits_on graph",
    "details": [{"field": "tasks", "reason": "cycle"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "cycle",
      "cycle": ["issue-A", "issue-B", "issue-A"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**What this tells the agent:** "Submission rejected before any
state change. Fix the DAG and resubmit." Other pre-append rejections
use the same envelope with different `error.batch.reason` tags:
`empty_task_list`, `dangling_refs`, `duplicate_names`,
`invalid_name`, `reserved_name_collision`, `spawned_task_mutated`,
`limit_exceeded_tasks`, `limit_exceeded_waits_on`,
`limit_exceeded_depth`.

### Per-task spawn failure (sibling error)

If one task points at an unresolvable template but the graph is
valid, whole-submission validation passes; `init_state_file` is
attempted per task and the bad one surfaces in `scheduler.errored`
without aborting the others:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3,
      "completed": 0,
      "pending": 2,
      "success": 0,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 1,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.issue-1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-2", "state": null, "complete": false, "outcome": "spawn_failed", "reason_source": "not_spawned", "spawn_error": {"task": "issue-2", "kind": "template_not_found", "template_source": "override", "paths_tried": ["/home/dan/src/tsuku/impl-missing.md"], "message": "Template not found at any configured base"}},
        {"name": "coord.issue-3", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1", "coord.issue-3"],
    "materialized_children": [
      {"name": "coord.issue-1", "task": "issue-1", "outcome": "pending", "state": "working", "waits_on": [], "ready_to_drive": true, "role": "worker"},
      {"name": "coord.issue-2", "task": "issue-2", "outcome": "spawn_failed", "waits_on": [], "ready_to_drive": false},
      {"name": "coord.issue-3", "task": "issue-3", "outcome": "pending", "state": "working", "waits_on": [], "ready_to_drive": true, "role": "worker"}
    ],
    "reclassified_this_tick": true,
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "accepted"},
        "issue-2": {"outcome": "errored", "kind": "template_not_found"},
        "issue-3": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    },
    "errored": [
      {
        "task": "issue-2",
        "kind": "template_not_found",
        "message": "Template not found at any configured base",
        "paths_tried": ["/home/dan/src/tsuku/impl-missing.md"],
        "template_source": "override"
      }
    ],
    "warnings": []
  }
}
```

**What this tells the agent:** "Two of three siblings spawned.
`issue-2`'s template isn't where the scheduler looked. `paths_tried`
lists the absolute paths attempted." Decision 14 splits
`template_not_found` (path wrong) from `template_compile_failed`
(path right, template invalid), so the agent can render targeted
recovery.

### Scheduler warning: absent `template_source_dir`

For pre-Decision-4 state files (upgraded mid-workflow), the header
has no `template_source_dir`. The scheduler emits a warning on the
tick but still attempts `submitter_cwd`:

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "task": "issue-1", "outcome": "pending", "state": "working", "waits_on": [], "ready_to_drive": true, "role": "worker"}
    ],
    "reclassified_this_tick": true,
    "feedback": {
      "entries": {"issue-1": {"outcome": "accepted"}},
      "orphan_candidates": []
    },
    "errored": [],
    "warnings": [
      {"kind": "missing_template_source_dir"}
    ]
  }
}
```

A `stale_template_source_dir` warning carries the failed path:

```json
{"kind": "stale_template_source_dir", "path": "/Users/dan/src/tsuku"}
```

---

## Cloud-mode response additions

When the workflow uses `CloudBackend`, every response gains two
top-level fields:

```json
{
  "sync_status": "fresh",
  "machine_id": "machine-abc123"
}
```

`sync_status` values: `fresh`, `stale`, `local_only`, `diverged`.
Observers on other machines see `diverged` and run
`koto session resolve coord --children=auto` to reconcile the
parent log and the per-child state files. These fields are absent
under `LocalBackend`.

## Concurrency error: concurrent parent tick

If two agents try to drive the parent at the same time, the second
one hits the advisory flock and gets a retryable error:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "integration_unavailable",
    "message": "Another tick is in progress for workflow 'coord'. Retry shortly.",
    "details": [{"field": "workflow", "reason": "concurrent_tick"}],
    "batch": {
      "kind": "concurrent_tick",
      "holder_pid": 48211
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**What this tells the agent:** "Back off and retry." The lock is
held only for the duration of the other tick; normal behavior
resumes once the holder exits. This applies to batch parents only
(states carrying `materialize_children` or parents with a prior
`SchedulerRan` / `BatchFinalized` event in the log). Non-batch
workflows and child ticks are unlocked.

---

## Canonical source per question

Every batch tick exposes the same state through several surfaces.
The table below pins which surface is authoritative for which
question — route agent logic off the canonical source, not an
incidental field that happens to carry the same data.

| Question | Canonical source | Why |
|---|---|---|
| "Which children should a worker dispatch next?" | `scheduler.materialized_children` filtered by `ready_to_drive == true AND outcome != "spawn_failed"` | Re-derived from disk every tick; `spawned_this_tick` is a per-tick observation that double-reports across concurrent ticks and silently drops on resume. |
| "How did the scheduler handle each submitted task?" | `scheduler.feedback.entries` (keyed by short task name) | Every submitted entry gets exactly one outcome per tick; no silent no-ops. |
| "Did this task's spawn fail, and why?" | `scheduler.errored[]` (typed `TaskSpawnError`) and `materialized_children[*].outcome == "spawn_failed"` | Structured per-task detail lives under `errored`; the ledger mirrors the failure so the dispatch filter can see it. |
| "Is the gate passing? Should the parent advance?" | `blocking_conditions[0].output.all_complete` (gate pass) and the aggregate booleans for routing | Gate output is what the transition guards read; the engine's routing decision reads these fields. |
| "What reason should I render for a failed child?" | `blocking_conditions[0].output.children[*].reason` with `reason_source` as provenance | `reason_source` tells the agent whether `reason` came from `failure_reason`, the state name, a skip marker, or a never-spawned task. |
| "Which children are eligible for retry, and how do I invoke it?" | `reserved_actions[0].applies_to` (short names) and `reserved_actions[0].invocation` | `reserved_actions` is the discovery surface; `invocation` is a ready-to-run POSIX-safe command. |
| "Is this the final outcome of the batch?" | `batch_final_view` on the terminal `done` response | Frozen at finalization time; survives past the active phase so summary states can read it without a second `koto status` call. |
| "Are there children on disk I forgot about?" | `scheduler.feedback.orphan_candidates` | Flags disk children whose short name is not in the current submission — detects accidental rename or drop. |
| "Is this child itself a sub-batch coordinator?" | `materialized_children[*].role == "coordinator"` with `subbatch_status` for inner-batch counts | Sticky once a `SchedulerRan` observes the role; gives outer agents nested-batch visibility without recursion. |

Non-canonical surfaces useful for logging only: `spawned_this_tick`,
`reclassified_this_tick`, and the `warnings` array. Don't key
dispatch or completion logic on them.

## Reading guide

- **Per-tick observation vs ledger.** `spawned_this_tick` can
  legitimately report the same child across concurrent ticks; it's
  an observation, not a contract. `materialized_children` is the
  authoritative ledger — use it for idempotent dispatch.
- **Dispatch on `ready_to_drive`, not on ledger presence.** Every
  entry in `materialized_children` carries a `ready_to_drive` flag.
  During retries the scheduler may respawn dependents whose
  `waits_on` ancestors are not yet terminal-success; those children
  land in the ledger with `ready_to_drive: false`. Workers MUST skip
  entries with `ready_to_drive: false` or risk starting a child
  against stale upstream state.
- **Aggregate booleans over `all_complete`.** Route templates on
  `all_success`, `needs_attention`, `any_failed`, `any_skipped`,
  `any_spawn_failed`. `needs_attention` folds in
  `any_spawn_failed`, so the existing routing already covers
  submissions where one task failed to spawn. Compile warning W4
  catches the "all_complete alone" footgun.
- **`reserved_actions` is not `expects`.** Reserved evidence bypasses
  the accepts validator. Read `reserved_actions` for the ready
  invocation string and submit it directly.
- **`tasks` evidence is not submittable at `analyze_failures`.** The
  parent template's `accepts` schema for `tasks` lives on
  `plan_and_await`. Agents cannot submit a new `tasks` list while
  parked at `analyze_failures`; they must first retry (routing back
  to `plan_and_await`) or submit a `decision` to route to
  `summarize`. Dynamic task additions require returning to the
  batched state first.
- **Synthetic child directive interpolates `skipped_because`.** The
  synthetic directive rendered by `koto next <skip-marker-child>`
  is `"This task was skipped because dependency '<skipped_because>'
  did not succeed. No action required."` The `{{skipped_because}}`
  placeholder is the direct upstream blocker (singular). Agents
  reading for the root cause use `skipped_because_chain[-1]`.
- **`synthetic: true` is a state-level computation.** The child's
  current state carries `skipped_marker: true`; the scheduler deletes
  and respawns that child the moment dependency outcomes change. No
  sidecar file, no template hash.
- **Errors are pre-append.** Rejected submissions do not touch the
  event log. Resubmitting an identical-but-fixed payload is always
  safe.
- **Per-task failures don't halt siblings.** One bad template
  surfaces in `scheduler.errored` and `outcome: spawn_failed` on the
  gate row; other tasks keep spawning. `needs_attention` becomes
  true, so the parent routes to `analyze_failures` exactly as if a
  real failure occurred. Retry on a `spawn_failed` task
  re-attempts `init_state_file` using the current submission's
  entry (retry-respawn).
- **Cross-level retry is rejected in v1.** If any child named in
  `retry_failed.children` is itself a batch parent (a nested
  coordinator), the submission rejects with
  `InvalidRetryReason::ChildIsBatchParent`. Retry at the level where
  the failure happened, then bubble up.
- **Delete-and-respawn silently drops uncommitted work.** When
  `retry_failed` propagates to a skipped dependent, the scheduler
  deletes the skip-marker state file and respawns the child from
  scratch. Skip markers never have an active worker, so the write
  loss is theoretical there. The hazard applies to any delete-and-
  respawn path: if a worker was mid-driving a child and had not yet
  committed evidence, that work is lost without warning. Avoid
  retrying children whose workers are still actively writing.
- **Two-hat intermediate children.** When `MaterializedChild.role`
  reports `"coordinator"` on a child, the child is itself running a
  sub-batch. Its `subbatch_status` summarizes the inner batch state;
  drive the child like any other coordinator to advance its inner
  batch. `batch_final_view` on the outer parent does not recursively
  embed the child's `batch_final_view`.
