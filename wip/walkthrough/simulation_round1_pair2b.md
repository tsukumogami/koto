# Simulation — Round 1, Pair 2b: Dynamic additions, mutation semantics

Design under validation: `docs/designs/DESIGN-batch-child-spawning.md`
Scenario: linear chain A -> B -> C. A spawns, starts running. Probe
mutation edge cases through successive resubmissions.

The design says `merge_epoch_evidence` **unions** the new tasks with
the existing set (Abstract bullet and Decision 1 commentary). It does
NOT say what a "union" means when the **same task name** appears in
both the prior and current submission with **different entry fields**
(`vars`, `waits_on`, `template`, etc.). Most of this simulation
exercises that exact gap.

KOTO responses are generated from the design rules (Decision 1, R1-R7,
Decision 4, Decision 5). `[GAP: ...]` markers flag places where the
design is silent or under-specified.

---

## Section 1: Transcript

### Setup — baseline batch

Parent workflow `coord` has been initialized via
`koto init coord --template coord.md --var plan_path=PLAN.md`.
The parent is parked at `plan_and_await`.

#### AGENT

```
$ koto next coord --with-data @tasks.json
```

`tasks.json`:

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

_Gloss: linear chain A -> B -> C. A has no deps; B waits on A; C waits on B._

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.A"],
    "already": [],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

_Gloss: A spawned with ISSUE_NUMBER=101. A is Running. B, C are
BlockedByDep. Parent is parked on plan_and_await._

---

### Probe 1: `vars` mutation on already-spawned task

#### AGENT

Resubmit with A's vars changed from `{"ISSUE_NUMBER": "101"}` to
`{"ISSUE_NUMBER": "999"}`. A is already Running.

```
$ koto next coord --with-data @tasks.v2.json
```

`tasks.v2.json`:

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "999"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

`[GAP: Decision 1 says vars are "forwarded to child's resolve_variables()"
at spawn. Decision E2 makes spawn idempotent via backend.exists() (the
"free idempotency check"). Result: coord.A already exists, scheduler
sees it as Running, never re-spawns, so the new vars are silently
ignored. The event log records the new EvidenceSubmitted event with
vars=999, but the child was initialized with 101. The design documents
NEITHER of: (a) a warning on mutation of a spawned task's vars, (b)
rejection of the submission, (c) any agent-visible signal that the
resubmission contained a diff that could not be applied. Response
looks identical to the prior tick, which is actively misleading.]`

_Gloss: The scheduler silently ignored the vars mutation. Child A is
still running with ISSUE_NUMBER=101. Agent has no way to know its
resubmission was partially ineffective._

---

### Probe 2: `waits_on` mutation on already-spawned task (retroactive dep)

#### AGENT

Resubmit with A's `waits_on` changed from `[]` to `["B"]`. A is
already Running.

```
$ koto next coord --with-data @tasks.v3.json
```

`tasks.v3.json`:

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}, "waits_on": ["B"]},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "code": "InvalidBatchDefinition",
    "message": "cycle detected in waits_on DAG: A -> B -> A",
    "reason": "R3 (waits_on is a DAG)",
    "attempted_tasks": ["A", "B", "C"]
  },
  "scheduler": null
}
```

_Gloss: R3 catches the cycle. Design line 1862-1868: "Cycles,
dangling refs, and duplicate names fail the whole submission with
BatchError::InvalidBatchDefinition ... already-spawned children from
earlier submissions are untouched." coord.A keeps running at
ISSUE_NUMBER=101. This probe is handled._

`[GAP: Consider a non-cyclic retroactive dep. Replace tasks.v3.json's
A with {"name": "A", "waits_on": ["C"]} and remove C's waits_on. The
merged DAG is acyclic (A depends on C, B depends on A, C has no
deps), but coord.A was ALREADY SPAWNED when it had no dependencies.
The design does not say whether this resubmission succeeds. If R3
passes (no cycle on paper) and R1-R7 don't forbid "adding a
dependency to an already-spawned task", the scheduler accepts the
mutation, but the child is already running — the new dep is never
enforced. Propose a new runtime rule: reject any submission where a
task whose child already exists on disk has a waits_on that differs
from the original submission.]`

Let me retry that variant explicitly.

#### AGENT (variant)

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}, "waits_on": ["C"]},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}}
  ]
}
```

#### KOTO (variant)

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.C"],
    "already": ["coord.A"],
    "blocked": ["coord.B"],
    "skipped": []
  }
}
```

`[GAP: The response is internally inconsistent. The DAG says A waits
on C, so A should be BlockedByDep. But A is already Running — the
scheduler cannot un-spawn it. The output reports A as "pending"
(in progress) while C is also "pending" running in parallel, which
contradicts the declared waits_on. No error, no warning. The design
never addresses this case.]`

---

### Probe 3: `template` mutation on already-spawned task

#### AGENT

Resubmit with A's template changed from the default to `impl-issue-v2.md`.

```json
{
  "tasks": [
    {"name": "A", "template": "impl-issue-v2.md", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

`[GAP: The child state file's header already captured the original
template path in WorkflowInitialized. A is halfway through an event
log against impl-issue.md. The scheduler's classify step queries
backend.exists("coord.A") and returns "already". The template diff
is silently dropped. Worse: on retry_failed of A, internal_rewind_to_initial
appends a Rewound event. Decision 5 says prior evidence becomes
invisible, but the state file's template pointer is unchanged. The
rewound child runs impl-issue.md v1, NOT v2. Template mutation
intent never reaches the running child. Same severity as Probe 1.]`

---

### Probe 4: Task removal (A absent from resubmission)

#### AGENT

Resubmit task list without A.

```json
{
  "tasks": [
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

_Note: B's `waits_on` was changed to `[]` to keep this submission
self-consistent; otherwise B would be dangling-ref against the
now-removed A._

#### KOTO

Option X (strict union per design "unions the new tasks with the
existing set"): A remains in the merged definition, response is a
no-op.

Option Y (replace semantics): A is removed from the definition, but
coord.A exists on disk. What is its status?

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "code": "InvalidBatchDefinition",
    "message": "[GAP: unspecified]",
    "reason": "[GAP: design does not say]"
  },
  "scheduler": null
}
```

`[GAP: The Abstract (line 93) and line 1864-1868 both say the merge
UNIONS the new tasks with the existing set. Under union semantics,
removing A is impossible — A persists in the effective task list
forever because it was submitted in epoch 1. The design does not
provide a "remove task" mechanism at all. This means:
(a) agent has no way to cancel a bad task entry short of retry_failed
    (which requires the child to fail first, not to be cancelled);
(b) if the agent builds an incorrect first submission and tries to
    amend by resubmitting a corrected list, the original wrong
    entries stay in force.
The design owes one of: (1) explicit "tasks are immutable once
submitted, resubmission can only add" with clear error messaging,
(2) a "cancel task" primitive separate from retry_failed, or
(3) replace-semantics with explicit handling of orphaned already-spawned
    children. Currently, koto status coord would NOT show A as a
    "leftover" because the union still includes A. coord.A keeps
    running indefinitely because nothing re-classifies it as removed.]`

---

### Probe 5: Rename task

#### AGENT

Rename A to A-renamed while keeping vars and waits_on.

```json
{
  "tasks": [
    {"name": "A-renamed", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A-renamed"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 0, "pending": 4,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.A-renamed", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A-renamed"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.A-renamed"],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

`[GAP: Rename is indistinguishable from "add a new task + keep the
original". Under union semantics, BOTH coord.A (from epoch 1) and
coord.A-renamed (from epoch 2) end up in the task definition. The
agent ran the same implementation twice — once against ISSUE_NUMBER=101
under name A, and once under name A-renamed (same ISSUE_NUMBER). Two
worker processes will race on the same issue. The children-complete
gate total jumps from 3 to 4. This is the worst failure mode in this
pair because it silently duplicates work. Proposed resolution:
runtime validation R8 "no two submitted tasks may have identical
non-`name` fields; warn if such pairs exist".]`

---

### Probe 6: Adding duplicate name

#### AGENT

Submit a second entry named A alongside the original.

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "A", "vars": {"ISSUE_NUMBER": "999"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "code": "InvalidBatchDefinition",
    "message": "duplicate task name: A appears 2 times",
    "reason": "R5 (task names unique)"
  },
  "scheduler": null
}
```

_Gloss: R5 catches in-submission duplicates. Decision 1 line 640:
"task names unique". Clean rejection. No gap here._

`[GAP (adjacent): R5 operates on a single submission. If epoch-1
submission had one A and epoch-2 submission has another A (with a
different vars), merge_epoch_evidence unions them. Does the union
dedupe by name? The design does not say. If dedupe-last-wins: epoch-2
overrides, but the child from epoch-1 is already running — return to
Probe 1 silently-ignored pathology. If dedupe-first-wins: epoch-2
submission is a no-op (silent). If no dedupe: two entries with the
same name pass through to R5, which would then reject the merged
definition retroactively, invalidating every subsequent tick
including already-spawned children.]`

---

### Probe 7: Identical resubmission (no diff)

#### AGENT

Submit tasks.json exactly as originally sent.

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

`[GAP: Event log grows by one EvidenceSubmitted event that is
semantically identical to the previous one. The scheduler re-classifies
(harmless — pure function), R1-R7 re-run (harmless — all pass), and
no-op spawn. Cost: one extra write, one extra cloud-sync push, one
additional event replayed on every future advance_until_stop call.
Across a long-running batch, identical resubmissions pollute the log
linearly. The design does not propose deduplication. Proposal:
`run_batch_scheduler` could compare the just-submitted evidence
payload against the previous tasks-field evidence and short-circuit
the Evidence append if byte-identical.]`

---

### Probe 8: Resubmission during retry cycle

#### AGENT

Suppose A has run to terminal failure. Agent submits retry_failed:

```
$ koto next coord --with-data '{"retry_failed": {"children": ["coord.A"], "include_skipped": false}}'
```

Before the retry finishes (i.e. agent has not yet called
`koto next coord.A`), agent resubmits a modified task list:

```
$ koto next coord --with-data @tasks.v2.json
```

where v2 adds a fourth task D.

#### KOTO (after retry_failed)

```json
{
  "action": "scheduler_processed",
  "state": "plan_and_await",
  "retry": {
    "rewound": ["coord.A"],
    "include_skipped_closure": []
  },
  "scheduler": {
    "spawned": [],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

_Gloss: Per Decision 5 line 1042-1049, retry_failed is intercepted
BEFORE advance_until_stop. A Rewound event is appended to coord.A
(new epoch), and a {"retry_failed": null} clearing event is appended
to the parent. The advance loop then runs on the post-rewind state.
coord.A is now in its initial state, re-classified as Running (not
NotYetSpawned), so no fresh spawn._

#### KOTO (after v2 submission with added D)

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 0, "pending": 4,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.D"],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": []
  }
}
```

_Gloss: retry_failed was already consumed by the null-clearing event,
so the new EvidenceSubmitted does not re-trigger a retry. Merge
unions the v2 tasks with the prior task set, adds D, spawns D._

`[GAP: The interaction between retry_failed's null-clearing event
(Decision 5) and merge_epoch_evidence's null-as-unset semantics is
load-bearing but lightly documented. What if v2's submission itself
contains `retry_failed: null` in its payload? merge_epoch_evidence
already treats the previous value as null-cleared, so this is a
no-op. What if v2 contains a NEW non-null retry_failed (agent tries
to trigger retry_failed for coord.B WHILE resubmitting the task list
in the same call)? The design's interception logic (handle_retry_failed
runs before advance_until_stop) suggests the retry fires and the task
list is appended in the same tick, but the ordering of these two
effects is not spelled out. Does the rewind use the pre-append or
post-append task list? Propose explicit ordering: (1) apply retry_failed
rewinds against the PRE-append task set, (2) append the new
EvidenceSubmitted, (3) advance, (4) schedule. Document this.]`

---

### Probe 9: Forward reference within the same submission

#### AGENT

Submit a task D that waits on E, plus a new task E, all in the same
submission. A is already running.

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]},
    {"name": "D", "vars": {"ISSUE_NUMBER": "104"}, "waits_on": ["E"]},
    {"name": "E", "vars": {"ISSUE_NUMBER": "105"}}
  ]
}
```

#### KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5, "completed": 0, "pending": 5,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 3,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.E"]},
        {"name": "coord.E", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.E"],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": []
  }
}
```

_Gloss: R4 (dangling refs) operates on the MERGED task list as a
whole — forward references within the same submission resolve
because both D and E are present. R3 (waits_on is a DAG) constructs
a graph over all tasks before checking cycles. This probe is
handled correctly by the design._

---

## Section 2: Findings

### Finding 1: vars mutation on already-spawned task is silently ignored
- **Observation**: Resubmission changes vars for task A. coord.A
  already exists on disk. Scheduler skips via `backend.exists()`
  idempotency check. New EvidenceSubmitted is persisted but never
  applied. Response is indistinguishable from a no-op resubmission.
- **Location in design**: Decision 1 "vars forwarded to child's
  resolve_variables()" (line 618), Decision E2 "backend.exists()
  idempotency check" (line 247), Abstract bullet on union semantics
  (line 93). None address the case where fields OTHER than `name`
  differ across submissions.
- **Severity**: High. Silent divergence between submitted evidence
  and observable behavior is a correctness hazard — the agent
  reasonably assumes its resubmission took effect.
- **Proposed resolution**: Add a runtime rule (call it R8): "for
  each task in the merged definition whose child already exists on
  disk, all fields (`template`, `vars`, `waits_on`) must match the
  task entry under which the child was originally spawned. Mismatch
  is `BatchError::InvalidBatchDefinition` with diff detail." Persist
  the spawned-at-epoch snapshot in the child's header, OR hash the
  effective entry and store the hash in the child's
  WorkflowInitialized event, so the check can compare back.
  Alternatively, explicitly document in Decision 1 that "task entries
  are frozen at spawn; subsequent resubmission of a changed entry is
  silently ignored for spawned children" and emit a WARNING in the
  scheduler outcome surface when this is detected.

### Finding 2: Retroactive waits_on mutations produce an inconsistent DAG
- **Observation**: Changing an already-spawned task's `waits_on`
  to reference tasks not yet complete creates a DAG where the
  "blocked" task is actually running. R3 only catches cycles, not
  the "spawned before dep" anomaly.
- **Location in design**: Decision 1 rule R3 (line 640, "waits_on
  is a DAG"), line 1864-1868 on dynamic additions.
- **Severity**: Medium-High. The gate output becomes internally
  inconsistent; agents reading blocked_by to decide parallelism
  may make wrong decisions.
- **Proposed resolution**: Same R8 as Finding 1 covers `waits_on`
  mutation. Additionally, document that `waits_on` on an
  already-spawned task is immutable.

### Finding 3: Template mutation on already-spawned task is dropped
- **Observation**: Template-path change is ignored at spawn time
  (already exists) and persists as "dropped intent" across
  retry_failed rewinds, which preserve the original template path
  in the header.
- **Location in design**: Decision 4 path resolution (lines 831-856)
  and Decision 5 retry mechanics (lines 997-1040).
- **Severity**: Medium. Less common than vars mutation but harder
  to detect. Especially dangerous during a template-migration
  workflow where the agent actively tries to swap in a v2 template.
- **Proposed resolution**: Covered by R8. Additionally, Decision 5
  section 5.4 should state explicitly that retry_failed re-runs
  the child against the originally-spawned template, not a newly
  submitted template.

### Finding 4: Task removal is impossible under union semantics
- **Observation**: Resubmitting a task list without A still keeps
  A in the effective definition because merge UNIONS. There is no
  primitive to remove or cancel a task. Agents cannot correct a
  bad initial submission.
- **Location in design**: Abstract line 93 ("unions the new tasks
  with the existing set"), Decision 5 (only retry_failed is defined
  as a task-level mutation primitive).
- **Severity**: High. Blocks practical recovery from authoring
  errors.
- **Proposed resolution**: Either (a) define explicit "cancel task"
  semantics — a new reserved `cancel_tasks: [...names]` evidence
  action paralleling `retry_failed`, which marks a task as
  abandoned in the gate output and deletes its state file if
  spawned; or (b) document that task sets are strictly additive and
  recommend `koto workflows --terminate coord.<task>` followed by
  some new parent-level "accept partial batch" escape hatch.
  Preference (a) — keeps the mental model symmetric with retry_failed.

### Finding 5: Rename silently duplicates work
- **Observation**: Renaming A to A-renamed creates a new task entry
  while leaving coord.A alive. Two workers perform the same work
  against the same `ISSUE_NUMBER`. total jumps from 3 to 4.
- **Location in design**: No rule covers semantic duplication.
  R5 (name uniqueness) operates on names, not payloads.
- **Severity**: High. Silent duplicate execution of side-effectful
  work (especially dangerous for GitHub-issue use case where two
  PRs may open against the same issue).
- **Proposed resolution**: R9: "warn (or reject, per policy) when
  any task in a new submission has a vars-and-waits-on signature
  byte-identical to an existing spawned task under a different
  name." Alternatively, lean on R8 + explicit task-remove primitive
  from Finding 4 — if renaming is expressed as "remove A, add
  A-renamed", then R8 catches the spawned-A-cannot-be-removed
  condition.

### Finding 6: Cross-epoch duplicate-name resolution is unspecified
- **Observation**: R5 catches duplicates within a single submission,
  but the union across epochs is silent. If epoch 1 has task A and
  epoch 2 has a second entry also named A with different fields,
  the design does not specify: first-wins, last-wins, dedupe-fail,
  or post-union R5 re-check.
- **Location in design**: Decision 1 R5 (line 640), line 1862-1868.
- **Severity**: Medium. Sits behind Findings 1-3 — whichever
  resolution is chosen for this influences the severity of those
  findings.
- **Proposed resolution**: Document "first-wins by name across
  epochs". Combine with R8 to ensure subsequent submissions can
  only ADD new names, never redefine existing ones. This resolves
  Findings 1-3 in a single policy.

### Finding 7: Identical resubmissions pollute the event log
- **Observation**: A byte-identical resubmission still writes a new
  EvidenceSubmitted. Event log grows linearly across no-op ticks.
- **Location in design**: Not covered.
- **Severity**: Low. Performance/storage, not correctness.
- **Proposed resolution**: Short-circuit when the submitted evidence
  payload equals the previous tasks-field evidence byte-for-byte.
  Skip the append; still run the scheduler tick.

### Finding 8: retry_failed + concurrent task-list resubmission has unspecified ordering
- **Observation**: When both effects are requested in a single tick
  (retry_failed interception happens pre-advance; task list append
  happens in-advance), the design does not pin the ordering of
  rewind-targets computation vs task-list merge.
- **Location in design**: Decision 5 section 5.4 (lines 1042-1049),
  Abstract bullet on union (line 93).
- **Severity**: Low-Medium. Edge case (agent submitting both
  primitives together), but load-bearing for correctness when it
  happens.
- **Proposed resolution**: Explicit ordering: (1) intercept
  retry_failed; (2) compute transitive closure against PRE-append
  task set; (3) apply rewinds; (4) append clearing event; (5) let
  advance_until_stop append the new tasks EvidenceSubmitted;
  (6) run scheduler against merged task set. Document this in
  Decision 5.

### Finding 9: Resubmission has no success/failure feedback for the mutation portion
- **Observation**: Across all probes 1-7, the response JSON for
  accepted-but-no-op-merge looks identical to the response for
  meaningful-merge. `scheduler.spawned` only reports new children;
  `scheduler.already` reports persisting children without
  distinguishing "already, and your resubmission changed this
  entry" from "already, and nothing changed". No diff, no applied/
  ignored breakdown.
- **Location in design**: Decision 6 status surface (lines 1097-1148)
  covers observation of batch state, not feedback on submission.
- **Severity**: Medium. Closely related to Findings 1-3; the fix
  may be the same mechanism.
- **Proposed resolution**: Extend `SchedulerOutcome` with a new
  field (e.g. `ignored_mutations: [{task, fields, reason}]`) that
  lists entries whose delta from the prior submission could not
  be applied because the child was already spawned. This is the
  minimum agent-visible signal even if R8 / frozen-entry policy
  rejects such submissions outright.
