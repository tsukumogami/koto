# Batch Child Spawning: Interactive Walkthrough

This walkthrough demonstrates the end-to-end interaction between an
agent and koto for a 3-issue batch with a diamond dependency pattern.
It shows every `koto next` call and response, explaining what happens
inside koto and how the agent decides what to do next.

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
          gates.done.all_complete: true
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
  summarize:
    terminal: true
---

## plan_and_await

Read the plan document at `{{plan_path}}`. For each issue outline in
the plan:

1. Extract the issue number, goal, files, and acceptance criteria
2. Map dependencies to sibling task names (issue N -> "issue-N")
3. Build a task entry: name="issue-N", vars={"ISSUE_NUMBER": "N"},
   waits_on=["issue-X", ...] for each listed dependency
   (check the `expects.fields.tasks.item_schema` field in the response for
   the full task entry schema; template defaults to impl-issue.md)

Submit the complete task list as JSON:
`koto next coord --with-data @tasks.json`

After submission, children will be spawned automatically. Drive each
child in `scheduler.spawned` via `koto next <child-name>`. You can
run independent children in parallel. After any child completes,
re-check the parent with `koto next coord` to spawn newly-unblocked
tasks.

<!-- details -->
Each child is an independent koto workflow named `coord.<task-name>`.
The scheduler runs on every `koto next coord` call and spawns tasks
whose `waits_on` dependencies are all terminal. You don't need to
track readiness yourself -- the `scheduler` field in the response
tells you what was just spawned. The `blocking_conditions[0].output`
field shows per-child status.
<!-- /details -->

## summarize

All issues are complete. Write a summary of what was implemented.
```

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
        run: "cargo test"
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
---

## working

Implement issue #{{ISSUE_NUMBER}}.

Read the issue, write the code, run the tests. When finished, submit
`{"status": "complete"}`. If you hit an unresolvable blocker, submit
`{"status": "blocked"}`.

## done

Issue #{{ISSUE_NUMBER}} implemented successfully.

## done_blocked

Issue #{{ISSUE_NUMBER}} is blocked and cannot proceed.
```

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
- `type: tasks` is accepted in template `accepts` blocks
- Evidence submission with a task list payload passes validation
- Existing field types (enum, string, number, boolean) unchanged

**Complexity:** simple

### 2. Add @file.json prefix to --with-data

**Goal:** When `--with-data` argument starts with `@`, read the
remainder as a file path and use its contents as the evidence payload.

**Files:** `src/cli/mod.rs`

**Acceptance criteria:**
- `koto next wf --with-data @file.json` reads and submits the file
- 1 MB size cap applies to resolved content
- Missing file produces a clear error

**Complexity:** simple
**Dependencies:** issue 1 (needs tasks type for testing)

### 3. Add materialize_children hook to TemplateState

**Goal:** New optional field on `TemplateState` that declares
batch materialization from an accepts field.

**Files:** `src/template/types.rs`, `src/template/compile.rs`

**Acceptance criteria:**
- `materialize_children: { from_field: tasks }` compiles
- Compiler validates E1-E8 rules
- W1-W2 warnings fire correctly

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

Issue 1 must complete before issues 2 and 3 can start. Issues 2 and 3
are independent of each other and can run in parallel.

### Task list (`tasks.json`)

The agent produces this from the plan document by following the
parent template's directive:

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

Note: `template` is omitted from each entry — the scheduler uses
`default_template: impl-issue.md` from the parent's
`materialize_children` hook. `waits_on` is omitted from issue-1
(defaults to `[]`).

---

## Walkthrough

### Interaction 1: `koto init coord --template coord.md --var plan_path=PLAN-batch-schema.md`

**What happens inside koto:**
- Creates session directory for `coord`
- Writes state file atomically via `init_state_file`:
  - Header: workflow=coord, template_source_dir=/home/user/repo/templates/
  - WorkflowInitialized event
  - Transitioned -> plan_and_await event

**Response:**
```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

### Interaction 2: `koto next coord`

**What happens inside koto:**
- Reads coord's state file, derives current state: plan_and_await
- Advance loop: accepts declares `tasks: tasks, required`, no evidence yet
- children-complete gate: 0 children, no batch definition -> Failed
- Advance loop stops (gate blocked)
- Scheduler: plan_and_await has materialize_children, but evidence
  field `tasks` is absent -> NoBatch

**Response:**
```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-batch-schema.md. For each issue outline in the plan:\n\n1. Extract the issue number, goal, files, and acceptance criteria\n2. Map dependencies to sibling task names (issue N -> \"issue-N\")\n3. Build a task entry with: name=\"issue-N\", template=\"impl-issue.md\", vars={\"ISSUE_NUMBER\": \"N\"}, waits_on=[\"issue-X\", ...] for each listed dependency\n\nSubmit the complete task list as JSON:\n`koto next coord --with-data @tasks.json`\n\nAfter submission, children will be spawned automatically. Drive each child in `scheduler.spawned` via `koto next <child-name>`. You can run independent children in parallel. After any child completes, re-check the parent with `koto next coord` to spawn newly-unblocked tasks.",
  "details": "Each child is an independent koto workflow named `coord.<task-name>`. The scheduler runs on every `koto next coord` call and spawns tasks whose `waits_on` dependencies are all terminal. You don't need to track readiness yourself -- the `scheduler` field in the response tells you what was just spawned. The `blocking_conditions[0].output` field shows per-child status.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name": { "type": "string", "required": true, "description": "Child workflow short name" },
          "template": { "type": "string", "required": false, "default": "impl-issue.md" },
          "vars": { "type": "object", "required": false },
          "waits_on": { "type": "array", "required": false, "default": [] },
          "trigger_rule": { "type": "string", "required": false, "default": "all_success" }
        }
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 0, "completed": 0, "pending": 0,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false, "children": []
    }
  }],
  "scheduler": null
}
```

**What this tells the agent:** "I need evidence. The directive says
to read the plan document and build a task list. The expects block
says I need a `tasks` field of type `tasks`. No children exist yet."

The agent reads PLAN-batch-schema.md, parses the 3 issue outlines,
maps dependencies to waits_on, and writes tasks.json.

### Interaction 3: `koto next coord --with-data @tasks.json`

**What happens inside koto:**
- Advance loop at plan_and_await
- Validates evidence: tasks is tasks-typed, required, present -> OK
- Appends EvidenceSubmitted { fields: { tasks: [...3 entries...],
  submitter_cwd: "/home/user/repo" } }
- Re-evaluates children-complete gate: 0 children on disk, but
  batch definition now in evidence -> Failed with total=3, completed=0
- Gate still blocked; advance loop stays at plan_and_await
- Scheduler runs on plan_and_await, finds materialize_children hook
- Parses 3 tasks from evidence, builds DAG, validates (no cycles,
  no dangling refs, unique names)
- Classifies: issue-1 is Ready (empty waits_on), issue-2 and issue-3
  are BlockedByDep (wait on issue-1)
- Spawns coord.issue-1 via init_state_file atomically
- Returns Scheduled

**Response:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...same as above...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-2", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-1"]},
        {"name": "coord.issue-3", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-1"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-1"],
    "already": [],
    "blocked": ["coord.issue-2", "coord.issue-3"],
    "skipped": []
  }
}
```

**What this tells the agent:**
- `action: gate_blocked` with `category: temporal` -> "parent can't
  advance yet, go do work on children"
- `scheduler.spawned: ["coord.issue-1"]` -> "this child was just
  materialized and is ready to be driven"
- `scheduler.blocked: ["coord.issue-2", "coord.issue-3"]` -> "these
  are waiting on dependencies"
- The directive reminds: "drive each child in scheduler.spawned via
  koto next <child-name>"

The agent knows to start driving coord.issue-1.

### Interaction 4: `koto next coord.issue-1`

**What happens inside koto:**
- This is a completely separate workflow with its own state file
- Reads coord.issue-1's state file (created by the scheduler)
- Current state: working
- The `tests` gate (type: command, run: "cargo test") evaluates:
  runs the command, captures exit code
- If tests pass (exit_code: 0): gate passes, but the transition
  still needs status evidence -> evidence_required
- If tests fail: gate blocked

**Response (assuming tests pass because no code has changed yet):**
```json
{
  "action": "evidence_required",
  "state": "working",
  "directive": "Implement issue #101.\n\nRead the issue, write the code, run the tests. When finished, submit {\"status\": \"complete\"}. If you hit an unresolvable blocker, submit {\"status\": \"blocked\"}.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": { "type": "enum", "values": ["complete", "blocked"], "required": true }
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**What this tells the agent:** "You're working on issue #101. The
directive says to implement it, run tests, and submit status evidence.
No gates are blocking."

The agent now does the actual implementation work (writes code, runs
tests, etc.). This is the "real work" phase.

### Interaction 5: Agent decides what to do with issue-1

**[DECISION POINT]** The agent has finished implementing issue #101.
Tests pass. The next step is to submit evidence to advance the child
workflow.

(Walkthrough continues interactively from here. The agent can:
submit "complete", submit "blocked", or do something unexpected.)

### Interaction 5a: `koto next coord.issue-1 --with-data '{"status": "complete"}'`

**What happens inside koto:**
- Advance loop at working state
- Validates evidence: status is enum, value "complete" is in allowed set
- Appends EvidenceSubmitted { fields: { status: "complete" } }
- Re-evaluates tests gate: cargo test runs, exit_code 0 -> passes
- Transition routing: status == "complete" AND tests gate passes
  -> transition to done
- Appends Transitioned { state: done }
- done is terminal -> advance loop stops

**Response:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #101 implemented successfully.",
  "is_terminal": true
}
```

**What this tells the agent:** "This child is done. The directive
confirms success." The agent knows to return to the parent.

### Interaction 6: `koto next coord` (re-tick the parent)

**What happens inside koto:**
- Advance loop at plan_and_await
- children-complete gate re-evaluates:
  - coord.issue-1 exists, state=done, terminal=true, failure=false
    -> outcome: success
  - coord.issue-2 not yet spawned -> outcome: blocked
  - coord.issue-3 not yet spawned -> outcome: blocked
  - total=3, completed=1, pending=2 -> Failed (not all_complete)
- Advance loop stays at plan_and_await
- Scheduler runs:
  - Re-classifies all 3 tasks
  - issue-1: Terminal (child exists, done state)
  - issue-2: Ready (waits_on=[issue-1], and issue-1 is Terminal)
  - issue-3: Ready (waits_on=[issue-1], and issue-1 is Terminal)
  - Spawns coord.issue-2 and coord.issue-3 via init_state_file

**Response:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...same as above...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 1, "pending": 2,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-2", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-3", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-2", "coord.issue-3"],
    "already": ["coord.issue-1"],
    "blocked": [],
    "skipped": []
  }
}
```

**What this tells the agent:**
- issue-1 is done (outcome: success)
- issue-2 and issue-3 were JUST spawned (scheduler.spawned) and are
  ready to be driven
- No more tasks are blocked (scheduler.blocked is empty)
- The agent can drive issue-2 and issue-3 in parallel since they
  have no dependency on each other

### Interaction 7a and 7b: Drive issue-2 and issue-3 in parallel

The agent spawns two sub-agents (or drives them sequentially).
Each calls `koto next coord.issue-2` / `koto next coord.issue-3`,
gets the directive, implements the code, and submits status evidence.

(Walkthrough continues interactively. If one fails, the other
continues. If both succeed, the parent completes.)

### Interaction 8: Both children complete, re-tick parent

`koto next coord`:

**Response:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete. Write a summary of what was implemented.",
  "is_terminal": true
}
```

**What happens:** children-complete gate saw all 3 terminal +
success -> all_complete: true -> transition to summarize -> terminal.
Batch is done.

---

## Failure scenario: issue-2 fails

If the agent submits `{"status": "blocked"}` for coord.issue-2:

### Failed child response

```json
{
  "action": "done",
  "state": "done_blocked",
  "directive": "Issue #102 is blocked and cannot proceed.",
  "is_terminal": true
}
```

### Parent re-tick after failure

`koto next coord`:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 3, "pending": 0,
      "success": 2, "failed": 1, "skipped": 0, "blocked": 0,
      "all_complete": true,
      "children": [
        {"name": "coord.issue-1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-2", "state": "done_blocked", "complete": true, "outcome": "failure", "failure_mode": true},
        {"name": "coord.issue-3", "state": "done", "complete": true, "outcome": "success"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.issue-1", "coord.issue-2", "coord.issue-3"],
    "blocked": [],
    "skipped": []
  }
}
```

Note: all_complete is true (all children reached terminal states),
but 1 is failed. The transition `when: { gates.done.all_complete:
true }` fires and the parent transitions to `summarize`. The parent
template's `summarize` state should handle the partial-success case
in its directive, or the template could add a separate transition
routing on `gates.done.failed > 0` to an `analyze_failures` state.

### Retry after failure

If the template routes to an analysis state instead of summarize,
the agent can submit retry evidence:

`koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-2"], "include_skipped": true}}'`

This would rewind coord.issue-2 back to its initial state (or delete
and respawn if it was a skipped marker), and the parent transitions
back to plan_and_await for another round of the scheduler loop.
