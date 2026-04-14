# Simulation Round 1 / Pair 3 — error paths and adversarial task lists

Scenario: parent workflow `coord` is initialized from the canonical
`coord.md` template (single state `plan_and_await`,
`materialize_children.from_field: tasks`, `default_template: impl-issue.md`,
`failure_policy: skip_dependents`). Parent is parked at
`plan_and_await` after `koto init`, having received the initial
`evidence_required` directive. The AGENT now starts iterating on a
task list payload, submitting adversarial / malformed variants and
observing KOTO's responses.

Throughout, KOTO's error responses must map to the `BatchError`
variants defined in Decision 1's Solution Architecture:

```
BatchError::InvalidBatchDefinition { reason }
BatchError::SpawnFailed            { task, source }
BatchError::TemplateResolveFailed  { task, paths_tried }
BatchError::BackendError           { source }
BatchError::LimitExceeded          { which, limit, actual }
```

which the design says map to `NextError::Batch { kind, message }` for
CLI response serialization. The **exact JSON envelope** is not
specified in the design — that gap is flagged inline as `[GAP]` and
documented again in Section 2.

---

## Section 1: Transcript

### Turn 1 — AGENT: evidence submission missing the `tasks` field entirely

AGENT: I'll try submitting what I've got so far. I only have notes
at this point, no task list yet.

```
$ koto next coord --with-data '{"notes": "hi"}'
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "reason": "required field missing: tasks",
  "directive": "Read the plan document at PLAN-batch-schema.md. For each issue outline in the plan:\n\n1. Extract the issue number, goal, files, and acceptance criteria\n2. Map dependencies to sibling task names (issue N -> \"issue-N\")\n3. Build a task entry: name=\"issue-N\", vars={\"ISSUE_NUMBER\": \"N\"}, waits_on=[\"issue-X\", ...] for each listed dependency\n\nSubmit the complete task list as JSON:\n`koto next coord --with-data @tasks.json`",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name":         { "type": "string", "required": true, "description": "Child workflow short name" },
          "template":     { "type": "string", "required": false, "default": "impl-issue.md" },
          "vars":         { "type": "object", "required": false },
          "waits_on":     { "type": "array",  "required": false, "default": [] },
          "trigger_rule": { "type": "string", "required": false, "default": "all_success" }
        }
      }
    }
  }
}
```

What this tells the agent: this was caught by the **advance-loop
evidence validator**, not the batch scheduler. `tasks` is declared
`required: true` in `accepts`. Because the advance loop rejects the
submission before appending, no `EvidenceSubmitted` event was added
to the log.

`[GAP: design is silent on the exact envelope when evidence
validation rejects a payload. The walkthrough happy path uses
action: "evidence_required" when no evidence has been submitted,
but there is no canonical envelope for "submitted but invalid".
Proposed: action: "evidence_rejected" with a reason field, leaving
expects intact so the agent can re-read the schema. Needs Decision
9 or an addendum to Decision 1.]`

---

### Turn 2 — AGENT: empty task list

AGENT: Let me just submit an empty list and see what happens.

```
$ koto next coord --with-data '{"tasks": []}'
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task list is empty; materialize_children requires at least one task entry"
  },
  "reason": "InvalidBatchDefinition: task list is empty; materialize_children requires at least one task entry",
  "scheduler": null
}
```

What this tells the agent: an empty array is treated as
`InvalidBatchDefinition`. Not a silently-passing "immediately
complete" case.

`[GAP: the design does NOT explicitly decide whether {"tasks": []}
is a valid "zero-task, immediately complete" batch or an
InvalidBatchDefinition. Decision 1's R1-R7 checklist does not call
it out. Implementation Approach mentions the 1000-task upper
bound but no lower bound. Interpretation matters: a zero-task
batch would pass children-complete (total=0, all_complete=true)
and advance the parent straight to the success transition, which
may surprise authors who expect "you forgot to produce any tasks".
Proposed: explicitly add R8 "task list must contain at least one
entry" and return InvalidBatchDefinition. If the author wants a
skip-the-batch escape hatch, a separate skip_batch evidence
action is cleaner than overloading empty-list semantics.]`

---

### Turn 3 — AGENT: cycle in waits_on

AGENT: OK, I'll build a real task list. I'll make issue-1 and
issue-2 depend on each other because my plan doc is unclear.

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}, "waits_on": ["issue-2"]},
    {"name": "issue-2", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["issue-1"]}
  ]
}
```

```
$ koto next coord --with-data @tasks.json
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "waits_on contains a cycle: issue-1 -> issue-2 -> issue-1",
    "details": {
      "check": "R3",
      "cycle": ["issue-1", "issue-2", "issue-1"]
    }
  },
  "scheduler": null
}
```

What this tells the agent: this hit runtime check R3 (`waits_on` is
a DAG). The design says R1-R7 validate at submission time and that
"cycles, dangling refs, and duplicate names fail the whole
submission with `BatchError::InvalidBatchDefinition`". The message
names which rule fired and the cycle members, so the agent can fix
the list without guessing.

`[GAP: the design enumerates R1-R7 but does NOT specify how
runtime-check identifiers surface in the error payload. An agent
pattern-matching on "cycle" in a free-form reason is brittle.
Proposed: BatchError::InvalidBatchDefinition gains a "check" field
(e.g. "R3") and a structured details payload per check. The CLI
response exposes error.details.check for programmatic recovery.]`

---

### Turn 4 — AGENT asks a clarifying question

AGENT: Before I retry, I want to understand the taxonomy. The
E-series errors are compile-time (template compile), the R-series
are runtime (task-list submission). I'm looking at the error I just
got — I see `check: "R3"`. Decision 1 lists 8 E's and 7 R's. Is
there a `koto <something>` call that returns the full R-list so I
can pattern-match on `check` codes in my retry logic?

KOTO: `[GAP: no discovery surface for R-check codes exists today.
The design assumes agents will read error.message prose. Proposed:
(a) document a stable enum of check codes {R1..R7} as part of the
public error contract, and (b) add a koto doc batch-checks
subcommand (or include check codes in the evidence_required
response alongside item_schema) so agents can enumerate rules
without scraping the design doc.]`

A provisional list, documented here for the simulation:

| Code | Check |
|------|-------|
| R1 | Child template compilable |
| R2 | Child template variables resolve |
| R3 | `waits_on` graph is a DAG (no cycles) |
| R4 | No dangling `waits_on` references |
| R5 | Task names are unique within the batch |
| R6 | Task names pass `validate_workflow_name()` |
| R7 | No collision with already-existing sibling children |

---

### Turn 5 — AGENT: dangling waits_on reference

AGENT: Retrying. I know issue-1 has no deps and issue-2 waits on
issue-1. But I typo'd the dep.

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "issue-2", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["issue-0"]}
  ]
}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task 'issue-2' has waits_on reference 'issue-0' which is not a task in this batch",
    "details": {
      "check": "R4",
      "task": "issue-2",
      "dangling": ["issue-0"],
      "known_tasks": ["issue-1", "issue-2"]
    }
  },
  "scheduler": null
}
```

What this tells the agent: R4 caught a dangling reference. The
`known_tasks` hint makes recovery easy.

---

### Turn 6 — AGENT: duplicate task names

AGENT: I'll copy-paste an entry and change the vars.

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "102"}}
  ]
}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "duplicate task name: 'issue-1' appears 2 times in the task list",
    "details": {
      "check": "R5",
      "duplicates": {"issue-1": 2}
    }
  },
  "scheduler": null
}
```

What this tells the agent: R5 (unique names) caught it. The
payload tells the agent exactly which names collide and how many
times.

---

### Turn 7 — AGENT: reserved / invalid task name

AGENT: The Decision 1 table says the child name is `<parent>.<task.name>`
and the task name has to pass `validate_workflow_name`. What if I
pick names that collide with reserved top-level evidence keys?
Like `retry_failed`?

```json
{
  "tasks": [
    {"name": "retry_failed", "vars": {"ISSUE_NUMBER": "101"}}
  ]
}
```

KOTO:

```json
{
  "action": "scheduled",
  "state": "plan_and_await",
  "scheduler": {
    "spawned": ["coord.retry_failed"],
    "already": [],
    "blocked": [],
    "skipped": []
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 1,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.retry_failed", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }]
}
```

`[GAP: the design reserves retry_failed only as a PARENT evidence
top-level key (Decision 5, section 5.4). Nothing prevents a TASK
from being named "retry_failed", producing a child workflow
coord.retry_failed. This is legal by validate_workflow_name (dot
and underscore permitted) but it is surprising: the parent's
retry_failed submission operates on a different namespace (parent
evidence) than the child of that name, so there is no actual
conflict at the engine level, but the naming collision will
confuse readers of koto workflows --children output and skill
docs. Proposed: add R8 "task name does not collide with reserved
parent evidence keys" (retry_failed, gates, plus whatever future
reserved keys). Better still, scope the check to "reserved names
that would produce an ambiguous child workflow name under <parent>.".]`

What about an empty string?

```json
{"tasks": [{"name": "", "vars": {"ISSUE_NUMBER": "101"}}]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task name is empty; validate_workflow_name rejected ''",
    "details": {"check": "R6", "task_index": 0}
  }
}
```

And `".."`?

```json
{"tasks": [{"name": "..", "vars": {"ISSUE_NUMBER": "101"}}]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task name '..' produces child workflow name 'coord...' which validate_workflow_name rejects",
    "details": {"check": "R6", "task_index": 0, "computed_child_name": "coord..."}
  }
}
```

`[GAP: validate_workflow_name's current rules are not reproduced
in the design doc, so it's unclear whether R6 runs on task.name
alone or on the computed child name coord.<name>. Decision E4
mentions "dots are legal" so coord... may or may not pass. The
scheduler must validate the computed child name, not just the
task name, to prevent path traversal and event-log corner cases.
Proposed: R6 explicitly says "validate_workflow_name is applied
to the computed <parent>.<task.name> string, not task.name
alone".]`

---

### Turn 8 — AGENT: invalid task entry shape

AGENT: What if I forget the name field, or use the wrong type for
waits_on?

```json
{"tasks": [{"vars": {"ISSUE_NUMBER": "101"}}]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "tasks[0]: missing required field 'name'",
    "details": {"task_index": 0, "missing": ["name"]}
  }
}
```

```json
{"tasks": [{"name": "issue-1", "waits_on": "issue-0"}]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "tasks[0].waits_on: expected array of string, got string",
    "details": {"task_index": 0, "field": "waits_on", "expected": "array", "got": "string"}
  }
}
```

```json
{"tasks": [{"name": "issue-1", "vars": ["ISSUE_NUMBER=101"]}]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "tasks[0].vars: expected object (string -> string), got array",
    "details": {"task_index": 0, "field": "vars", "expected": "object", "got": "array"}
  }
}
```

What this tells the agent: item-schema type violations all surface
as `InvalidBatchDefinition`. Position-indexed (task_index) because
names may be missing.

`[GAP: unclear whether shape validation (required fields, type
mismatches on the item entry) is handled by the type: tasks
evidence validator in the advance loop (pre-append, like Turn 1)
or by the scheduler (post-append, with a persisted broken event).
The design's item_schema is koto-generated, not template-authored
(Decision 8), but nothing specifies where the VALIDATION of that
schema runs. Proposed: type: tasks runs the item_schema check in
the evidence validator BEFORE appending, sharing code with the
scheduler's runtime checks. This keeps bad submissions out of the
event log.]`

---

### Turn 9 — AGENT asks: does a failed submission land in the event log?

AGENT: Serious question. I've now sent seven broken submissions.
If I fix my JSON and succeed on the eighth try, is my event log
full of rejected EvidenceSubmitted events? Does the parent state
advance at all when a submission is rejected?

KOTO:

The advance loop's contract is: evidence is validated BEFORE
append. An evidence_rejected response means no EvidenceSubmitted
event was appended, and the parent's state file is unchanged on
disk. This is the same pattern the engine uses today for existing
field types (enum, string, number, boolean).

For the scheduler-level checks (R1-R7), the design is more subtle.
From Decision 1 plus the Implementation Approach Phase 3
"Submission-time hard limit enforcement" block: limits are
enforced AT evidence submission, which implies BEFORE append. But
the Data Flow section (Step 4) says:

> validates the evidence against the accepts schema
> appends EvidenceSubmitted { fields: { tasks: [...], submitter_cwd } }
> ... run_batch_scheduler ... builds DAG, runs runtime validation (R1-R7)

which implies the append happens FIRST and then R1-R7 run. If R3
(cycle detection) fires after the append, the rejected task list
is now persistently in the event log.

`[GAP: this is the biggest ambiguity surfaced by this simulation.
The design gives two incompatible answers:
  - Implementation Approach Phase 3: limits checked at submission
    time (pre-append).
  - Data Flow Step 4: append happens, then scheduler runs R1-R7.
The difference matters for:
  (a) audit-trail cleanliness: do rejected submissions show up in
      koto query output?
  (b) resubmit semantics: does the agent need to submit a
      correcting event, or just a fresh payload?
  (c) retry/rewind interaction: if a bad event was appended, does
      a subsequent successful submission automatically shadow it
      via merge_epoch_evidence, or does the parent need a rewind?
Proposed: ALL batch-definition validation (item_schema + R1-R7 +
LimitExceeded) runs pre-append in the evidence validator. The
scheduler's classification is guaranteed to operate on
already-validated evidence, which simplifies the scheduler and
keeps the event log clean. Decision 1 or a new addendum should
state this explicitly.]`

---

### Turn 10 — AGENT: per-task template override pointing at a missing file

AGENT: What if I override the template on one task and point at
something that doesn't exist?

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "issue-2", "template": "does-not-exist.md", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["issue-1"]}
  ]
}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "TemplateResolveFailed",
    "message": "task 'issue-2': template 'does-not-exist.md' could not be resolved",
    "details": {
      "task": "issue-2",
      "paths_tried": [
        "/home/user/repo/templates/does-not-exist.md",
        "/home/user/repo/does-not-exist.md"
      ]
    }
  },
  "scheduler": null
}
```

What this tells the agent: Decision 4's resolution order is
visible — (a) `template_source_dir` first, (b) submitter_cwd
fallback. The agent can fix the template path without guessing the
base directory.

`[GAP: the design says "errors listing both attempts if still not
found" (Decision 4) but does NOT specify WHEN this error fires —
at submission time (R1 "child template compilable") or at
scheduler spawn time (TemplateResolveFailed in the BatchError
enum). Since only task 'issue-2' is affected and 'issue-1' has
no override, a strict reading of failure_policy: skip_dependents
might suggest issue-1 spawns and issue-2 skips. But both live in
the SAME submitted batch definition, so atomic rejection is the
safer policy. Proposed: R1 is a compile-the-template check that
runs at submission, for ALL tasks, for whichever template each
task will use (default or override). Any TemplateResolveFailed
rejects the entire submission. failure_policy applies only to
runtime child failures, not to malformed definitions.]`

---

### Turn 11 — AGENT: limit violations

AGENT: I'm doing a big refactor and have 1,500 issues in my plan.

```json
{"tasks": [/* 1500 entries */]}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "LimitExceeded",
    "message": "batch exceeds hard limit: 1500 tasks > 1000 max",
    "details": {"which": "task_count", "limit": 1000, "actual": 1500}
  },
  "scheduler": null
}
```

AGENT: OK, and one task that fans out too much?

```json
{
  "tasks": [
    {"name": "sink", "waits_on": ["a","b","c","d","e","f","g","h","i","j","k"]},
    ...
  ]
}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "LimitExceeded",
    "message": "task 'sink': waits_on has 11 entries, limit is 10",
    "details": {"which": "waits_on_per_task", "limit": 10, "actual": 11, "task": "sink"}
  }
}
```

AGENT: And a very deep DAG?

```json
{
  "tasks": [
    {"name": "t1"},
    {"name": "t2", "waits_on": ["t1"]},
    {"name": "t3", "waits_on": ["t2"]},
    ... (51 tasks, each waiting on the previous)
  ]
}
```

KOTO:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "LimitExceeded",
    "message": "DAG depth 51 exceeds limit of 50",
    "details": {"which": "dag_depth", "limit": 50, "actual": 51}
  }
}
```

What this tells the agent: LimitExceeded is consistent across
three axes. The `which` discriminator (`task_count`,
`waits_on_per_task`, `dag_depth`) is stable and enumerable.

`[GAP: the BatchError::LimitExceeded variant defines only {which,
limit, actual}. The design does not formalize that 'which' is a
closed enum of three values. Agents cannot rely on it for
programmatic handling without a documented contract. Proposed:
document {task_count, waits_on_per_task, dag_depth} as the
exhaustive v1 set, and reserve the right to add more in future
versions (with a forward-compat "other" branch in consumer code).]`

---

### Turn 12 — AGENT asks: template: null vs omitted

AGENT: The `expects.item_schema` says `template` has
`default: impl-issue.md`. What if I explicitly pass `template:
null`? Is that the same as omitting it, or is it an error?

```json
{"tasks": [{"name": "issue-1", "template": null, "vars": {"ISSUE_NUMBER": "101"}}]}
```

KOTO: `[GAP: the design does not specify. JSON semantics allow
three states: field absent, field: null, field: "impl-issue.md".
item_schema does not distinguish "nullable" in its current form.
Two plausible behaviors:
  (a) null is strictly rejected with InvalidBatchDefinition
      "tasks[0].template: expected string, got null";
  (b) null is normalized to absent and the default applies.
Same question applies to vars: null and waits_on: null. Proposed:
null is strictly rejected for v1 (behavior (a)); authors who
want the default simply omit the field. This is clean,
defensible in docs, and avoids the JSON-semantics debate. Add an
explicit note to the item_schema response and in koto-author
skill.]`

For the simulation, picking behavior (a):

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "tasks[0].template: expected string, got null; omit the field to use default_template",
    "details": {"task_index": 0, "field": "template", "expected": "string", "got": "null"}
  }
}
```

---

### Turn 13 — AGENT: premature retry_failed

AGENT: Haven't submitted a task list yet. What if I try to retry
something that doesn't exist?

```
$ koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-1"]}}'
```

KOTO:

`[GAP: this scenario is barely specified. Decision 5 section 5.4
says retry_failed is intercepted in handle_next BEFORE
advance_until_stop runs. The behavior when there's nothing to
retry is undefined. Three candidate behaviors:
  1. evidence_rejected: parent has never materialized a batch.
     retry_failed has no anchor.
  2. evidence_rejected: named child 'coord.issue-1' does not
     exist on disk (backend.exists returns false).
  3. no-op: treat as already-cleared and let advance loop proceed
     as usual.
Proposed: return evidence_rejected with
InvalidBatchDefinition-ish kind "InvalidRetryRequest" listing
which named children were unknown (from backend.list). The check
runs at submission time BEFORE the handle_retry_failed path
appends anything, keeping retry submissions symmetric with task
list submissions: pre-validated, no half-applied state.
Alternative: promote this to a sixth BatchError variant,
InvalidRetryRequest { unknown_children: Vec<String>, parent_has_batch: bool }.]`

Provisional response:

```json
{
  "action": "evidence_rejected",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidRetryRequest",
    "message": "retry_failed cannot execute: parent has no materialized batch (no EvidenceSubmitted with tasks)",
    "details": {
      "requested_children": ["coord.issue-1"],
      "unknown_children": ["coord.issue-1"],
      "parent_has_batch": false
    }
  },
  "scheduler": null
}
```

What this tells the agent: the scheduler rejects retry of nothing.
The agent must submit the original task list first.

---

### Turn 14 — AGENT: successful resubmission after fixing everything

AGENT: OK, let me assemble a clean payload now.

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "issue-2", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["issue-1"]},
    {"name": "issue-3", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["issue-1"]}
  ]
}
```

KOTO (identical to walkthrough.md Interaction 3):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...",
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

AGENT: Final question — the event log. None of my 10 rejected
submissions left traces, right? If I `koto query coord`, I see
exactly one EvidenceSubmitted (the good one), plus the initial
WorkflowInitialized and Transitioned events?

KOTO: Under the proposed resolution in Turn 9 (pre-append
validation for the batch-definition layer), yes — the rejected
submissions never touched the event log. The file on disk is
byte-identical to a timeline where the agent got the task list
right on the first try.

---

## Section 2: Findings

Numbered in rough severity order.

### 1. Evidence-rejection response envelope is unspecified

- **Observation**: The design mentions `NextError::Batch { kind,
  message }` as the CLI serialization target but never shows the
  full outer JSON shape. The walkthrough only models happy-path
  envelopes (`evidence_required`, `gate_blocked`, `done`). Agents
  have no canonical shape for "you submitted something and it was
  rejected".
- **Location in design**: Decision 1 Solution Architecture
  (`BatchError` enum and the comment `BatchError maps to
  NextError::Batch { kind, message } for CLI response
  serialization`). Also Implementation Approach Phase 3
  (`NextError::Batch` variant mention without JSON schema).
- **Severity**: blocker. Agents can't write resilient code against
  an unspecified error shape.
- **Proposed resolution**: Add an "Error response envelope"
  sub-section under Decision 1 with a concrete JSON example:

  ```json
  {
    "action": "evidence_rejected",
    "state": "<current_state>",
    "error": {
      "kind": "<BatchError variant>",
      "message": "<human-readable>",
      "details": { ...variant-specific fields... }
    },
    "scheduler": null
  }
  ```

  Explicitly document that `action: "evidence_rejected"` is the
  new top-level `NextResponse` variant used for both
  advance-loop evidence validation failures AND scheduler-level
  batch validation failures. Keep `expects` in the response
  when possible so the agent can re-read the schema.

### 2. Pre-append vs post-append validation is ambiguous

- **Observation**: "Submission-time hard limit enforcement"
  (Implementation Approach Phase 3) says limits are enforced at
  submission time (pre-append). The Data Flow Step 4 walkthrough
  shows `EvidenceSubmitted` being appended BEFORE the scheduler
  runs R1-R7. These contradict each other for the non-limit
  checks.
- **Location in design**: Decision 1 (R1-R7 runtime checks) and
  Solution Architecture Data Flow Step 4 (ordering).
- **Severity**: blocker. Downstream semantics — event log
  cleanliness, resubmit ergonomics, rewind/retry behavior — all
  hinge on this.
- **Proposed resolution**: State explicitly that
  `InvalidBatchDefinition`, `TemplateResolveFailed` (R1), and
  `LimitExceeded` ALL run pre-append, in the evidence validator
  that today handles `type: enum` / `type: string` etc. The
  scheduler's classification path is guaranteed to operate on
  validated task lists. Document in Data Flow Step 4 by moving
  the "builds DAG, runs runtime validation" language above the
  "appends `EvidenceSubmitted`" step.

### 3. R-check codes are not part of the public contract

- **Observation**: Agents surfacing user-facing retry logic need
  to pattern-match on which rule fired. The design names R1-R7
  internally but doesn't expose them in response payloads.
- **Location in design**: Decision 1 compiler-validation table.
- **Severity**: should-fix. Workable without it (agents can scan
  message prose), but fragile.
- **Proposed resolution**: (a) `BatchError::InvalidBatchDefinition`
  gains an optional `check: &'static str` field ("R1".."R7");
  (b) document the code set as stable public API; (c) optionally
  expose via a new `koto doc batch-checks` subcommand or inline
  in the `evidence_required` response so agents can enumerate
  rules.

### 4. Empty-task-list semantics unspecified

- **Observation**: `{"tasks": []}` has no defined behavior. An
  immediately-complete batch (total=0, all_complete=true) may
  surprise authors; a silent success-transition obscures
  "you didn't produce any tasks".
- **Location in design**: Decision 1 task schema and R1-R7 list.
- **Severity**: should-fix.
- **Proposed resolution**: Add R8 "task list is non-empty" and
  return `InvalidBatchDefinition`. If someone later wants a
  zero-task escape hatch, introduce a distinct `skip_batch`
  evidence action rather than overloading the empty list.

### 5. `template: null` vs omitted is undefined

- **Observation**: JSON allows three states for optional string
  fields (absent, null, value). The `item_schema` format doesn't
  distinguish them. Same issue for `vars: null`, `waits_on: null`.
- **Location in design**: Decision 8 `item_schema` generation.
- **Severity**: should-fix.
- **Proposed resolution**: State that explicit `null` is
  rejected (`InvalidBatchDefinition`, expected string|object|array
  got null). Agents who want the default must omit the field.
  Add a sentence to the koto-author and koto-user skills.

### 6. Reserved-name collision with `retry_failed` / `gates`

- **Observation**: A task named `retry_failed` produces a child
  `coord.retry_failed`. The engine-level behavior is unambiguous
  (parent evidence and child workflow names live in separate
  namespaces), but the resulting `koto workflows --children`
  listing is confusing and operator-hostile.
- **Location in design**: Decision 1 R6 (task names pass
  `validate_workflow_name`) and Decision 5 section 5.4
  (`retry_failed` reserved).
- **Severity**: nice-to-have.
- **Proposed resolution**: Add to R6 (or as R6b) "task name
  must not equal a reserved parent-evidence key: {`retry_failed`,
  `gates`, plus any future additions}".

### 7. `validate_workflow_name` scope is ambiguous

- **Observation**: Does R6 run on `task.name` alone or on the
  computed `<parent>.<task.name>`? Names like `..` produce
  `coord...` which may or may not pass.
- **Location in design**: Decision 1 task entry schema row,
  Decision E4 naming decision.
- **Severity**: should-fix. Edge case but a path-traversal /
  session-file-collision risk if permissive.
- **Proposed resolution**: R6 is explicitly defined as
  "`validate_workflow_name` is applied to the concatenated
  `<parent>.<task.name>` string, not to `task.name` alone".

### 8. Per-task `template` resolution failure timing

- **Observation**: A per-task template override that doesn't
  resolve — does it fail the whole batch at submission, or only
  skip that task (with `failure_policy: skip_dependents`) at
  spawn time?
- **Location in design**: Decision 4 (path resolution) and
  Decision 1 R1 (child template compilable).
- **Severity**: should-fix.
- **Proposed resolution**: R1 runs at submission time, for
  whichever template each task will use (default or override).
  Any `TemplateResolveFailed` is atomic: the entire submission
  is rejected. `failure_policy` applies to RUNTIME child failures
  only, not to malformed task definitions.

### 9. Premature `retry_failed` behavior is undefined

- **Observation**: Submitting `retry_failed` before any batch
  has been materialized has no specified handling.
- **Location in design**: Decision 5 section 5.4.
- **Severity**: should-fix.
- **Proposed resolution**: Add a new `BatchError::InvalidRetryRequest
  { unknown_children: Vec<String>, parent_has_batch: bool }`
  variant. `handle_retry_failed` validates at the top: if
  `!parent_has_batch` or any requested child isn't in
  `backend.list()`, reject pre-append. Surface via the same
  `action: "evidence_rejected"` envelope.

### 10. `LimitExceeded.which` is not typed as a closed enum

- **Observation**: The variant uses `which: &'static str`,
  implying an open string set.
- **Location in design**: `BatchError::LimitExceeded` definition.
- **Severity**: nice-to-have.
- **Proposed resolution**: Document the v1 value set as closed:
  `{task_count, waits_on_per_task, dag_depth}`. Note that
  consumers should default-branch for forward compatibility.
  Optionally promote to an enum type in Rust.

### 11. No single-source listing of per-task item-shape error messages

- **Observation**: Missing `name`, wrong type for `waits_on`,
  wrong type for `vars` — each surfaced with slightly different
  shapes in this simulation because the design is silent on
  item-schema validation error format.
- **Location in design**: Decision 8 (`item_schema` generation)
  and Decision 1 R1-R7 (but item-shape checks aren't in R1-R7,
  they're a tier below).
- **Severity**: should-fix.
- **Proposed resolution**: Document that `type: tasks` evidence
  validation produces errors shaped
  `{kind: "InvalidBatchDefinition", details: {task_index, field?,
  expected?, got?, missing?}}`. Share the validator code between
  the advance-loop evidence check and scheduler startup so both
  paths emit identical errors.
