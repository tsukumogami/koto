# Simulation Round 1, Pair 2: Dynamic Task Addition Mid-Batch

Scenario: linear A -> B -> C chain, with mid-flight addition of A1 (parallel
to A) and D (tail extension after C). Probes `merge_epoch_evidence` union
semantics and several explicit edge cases.

Parent template is the same single-state fan-out pattern from
`wip/walkthrough/walkthrough.md` (state `plan_and_await`, child template
`impl-issue.md`). Parent workflow name: `coord`.

---

## Section 1: Transcript

### Turn 1 - AGENT

```
koto init coord --template coord.md --var plan_path=PLAN-linear.md
```

### Turn 2 - KOTO

```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

**What this tells the agent:** The workflow exists and is parked at the
planning state. Next step is `koto next coord` to receive the directive.

### Turn 3 - AGENT

```
koto next coord
```

### Turn 4 - KOTO

```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-linear.md. For each issue outline in the plan, build a task entry and submit the complete task list as JSON: `koto next coord --with-data @tasks.json`. After submission, children will be spawned automatically. Drive each child in `scheduler.spawned` via `koto next <child-name>`. After any child completes, re-check the parent with `koto next coord` to spawn newly-unblocked tasks.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name":    {"type": "string", "required": true},
          "template":{"type": "string", "required": false, "default": "impl-issue.md"},
          "vars":    {"type": "object", "required": false},
          "waits_on":{"type": "array",  "required": false, "default": []},
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
      "total": 0, "completed": 0, "pending": 0,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false, "children": []
    }
  }],
  "scheduler": null
}
```

**What this tells the agent:** Supply a `tasks` array; each task entry
has a schema that matches what koto accepts. No children yet.

### Turn 5 - AGENT

Reads `PLAN-linear.md` and produces an initial task list. The plan says
"A then B then C":

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]}
  ]
}
```

```
koto next coord --with-data @tasks.json
```

### Turn 6 - KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...planning directive elided...",
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

**What this tells the agent:** A is live. B and C are declared and
blocked. Go drive `coord.A`.

### Turn 7 - AGENT

```
koto next coord.A
```

### Turn 8 - KOTO

```json
{
  "action": "evidence_required",
  "state": "working",
  "directive": "Implement issue #201. Read the issue, write the code, run tests. Submit {\"status\": \"complete\"} or {\"status\": \"blocked\"}.",
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

**What this tells the agent:** Standard impl-issue directive for A.

### Turn 9 - AGENT (mid-flight realization)

Mid-implementation, the agent re-reads the plan and notices two things
it missed:

1. There's a setup task (A1) that can run in parallel with A.
2. There's a cleanup task (D) that has to run after C.

The agent leaves `coord.A` parked in `working` and returns to the parent
to resubmit an updated task list:

```
koto next coord --with-data @tasks-v2.json
```

where `tasks-v2.json` is:

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C",  "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]},
    {"name": "D",  "vars": {"ISSUE_NUMBER": "205"}, "waits_on": ["C"]}
  ]
}
```

### Turn 10 - KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...planning directive elided...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5, "completed": 0, "pending": 5,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 3,
      "all_complete": false,
      "children": [
        {"name": "coord.A",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.A1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.A1"],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": []
  }
}
```

**What this tells the agent:** The union worked. A was already running
(`already`), A1 was just spawned (`spawned`), B/C/D are blocked on the
chain. Per the design, `merge_epoch_evidence` unions tasks, so the
scheduler sees five tasks in the "current" definition. The DAG is still
valid (no cycles); runtime validation R3 + R5 + R7 pass.

[GAP: The design says "evidence resubmission unions tasks" but does not
specify the conflict-resolution rule when the same task name appears in
both the old and new submission with *different* fields. See the
`vars` change probe below.]

### Turn 11 - AGENT (probe: remove a task)

> Hold on. What if I'd gotten cold feet after Turn 9 and submitted an
> updated list that *drops* A1? A1 is already running. Does koto
> terminate it? Orphan it? Error the submission?

```
koto next coord --with-data @tasks-v3.json
```

where `tasks-v3.json` is the original 3-task list (A, B, C) again - no
A1, no D.

### Turn 12 - KOTO

[GAP: The design does not answer this. "Unions the new tasks with the
existing set" (line 93 of DESIGN-batch-child-spawning.md) strongly
implies an additive-only merge semantics, but it's not spelled out.
Three defensible interpretations:

1. **Strict union** (additive only): removal is impossible by
   construction. Every task ever named in any `EvidenceSubmitted` event
   remains in the effective task set. The agent's attempt to remove A1
   is a no-op; A1 continues running and must reach terminal for the
   batch to complete. This matches the append-only invariant and
   requires no new machinery.

2. **Last-write-wins set semantics.** The most recent task list IS the
   task set. A1 disappears from the definition but its state file still
   exists on disk. The classifier encounters an orphan child with no
   task entry - undefined behavior.

3. **Explicit removal error.** A resubmission that omits a previously-
   submitted task is rejected as `InvalidBatchDefinition`.

Proposed resolution: adopt (1) with a compiler/runtime rule that the
classifier ignores disk children whose name doesn't appear in the
current task-name union, rather than treating them as orphans. A
plausible response follows:]

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
        {"name": "coord.A",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.A1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.A", "coord.A1"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": []
  }
}
```

**What this tells the agent:** The removal was ignored. Interpretation
(1) applies. The agent would have to pivot strategy — e.g., submit
cancellation evidence to the child directly, or drive A1 to
`done_blocked`. This is a real rough edge: agents will try to "edit"
the plan and discover they can't.

### Turn 13 - AGENT (probe: modify an already-spawned task's waits_on)

> OK, keep A1 in. But I think I got A's dependencies wrong - what if A
> actually should wait on A1? Let me re-submit with A's `waits_on:
> ["A1"]`. A is already running, though.

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}, "waits_on": ["A1"]},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C",  "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]},
    {"name": "D",  "vars": {"ISSUE_NUMBER": "205"}, "waits_on": ["C"]}
  ]
}
```

### Turn 14 - KOTO

[GAP: The design does not specify whether conflicting fields in the
latest submission override prior submissions or are ignored. Given the
"union" framing and the statement that "already-spawned children from
earlier submissions are untouched" (line 1868), the safe reading is
that `waits_on` on already-spawned tasks has no retroactive effect: A
is already running because its pre-merge `waits_on` was empty, and the
merged DAG is used only for future scheduling decisions (e.g., B's
dependency on A still resolves correctly because A either reaches
terminal or doesn't).

But this creates a silent anomaly: the new DAG says A waits on A1, yet
A has already been spawned. In a strict DAG interpretation this is a
lie. The classifier sees A as `Running` regardless of whether its
upstream is satisfied.

Proposed resolution: reject mutation of `waits_on` on a task whose
child already exists on disk, with `BatchError::InvalidBatchDefinition
{ reason: "task A has waits_on changed from [] to [A1] but is already
spawned" }`. Alternatively, document explicitly that `waits_on` changes
on already-spawned tasks are silently ignored. Either way, the current
design is silent and the safer response is to error:]

```json
{
  "action": "batch_error",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task 'A' has waits_on changed from [] to [\"A1\"] but coord.A is already spawned; edits to waits_on on spawned tasks are not permitted"
  },
  "scheduler": null
}
```

**What this tells the agent:** Resubmit with A's `waits_on` restored
to its spawned-time value, or use `retry_failed` to re-queue A with new
inputs.

### Turn 15 - AGENT (probe: cycle introduced by merge)

> Fine. Let me try adding a task E that depends on D, and have C depend
> on E. That loops C -> E -> D -> C.

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C",  "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B", "E"]},
    {"name": "D",  "vars": {"ISSUE_NUMBER": "205"}, "waits_on": ["C"]},
    {"name": "E",  "vars": {"ISSUE_NUMBER": "206"}, "waits_on": ["D"]}
  ]
}
```

### Turn 16 - KOTO

Per line 1862-1868 of the design, cycles discovered only after the
merge cause `BatchError::InvalidBatchDefinition`; the resubmission is
rejected and already-spawned children are untouched.

```json
{
  "action": "batch_error",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "cycle detected in post-merge task DAG: C -> E -> D -> C"
  },
  "scheduler": null
}
```

**What this tells the agent:** The merged DAG is validated. The agent
must submit a cycle-free list.

[GAP: Turn 14's "waits_on was also modified on an already-spawned task"
case and this cycle case both surface at validation time, but the
design lumps them both under `InvalidBatchDefinition` without
distinguishing. Agents would benefit from a sub-kind discriminator
(`CycleDetected`, `SpawnedTaskMutated`, `DanglingRef`) so they can
react programmatically. Proposed resolution: add a `sub_kind` field to
the error payload enumerating R1-R7 outcomes.]

### Turn 17 - AGENT (probe: identical resubmission)

> Let me just re-send the exact same Turn 9 payload again. Is that a
> no-op or an error?

```
koto next coord --with-data @tasks-v2.json   # same bytes as Turn 9
```

### Turn 18 - KOTO

[GAP: The design doesn't explicitly address identical resubmission. The
advance loop validates the evidence against the `accepts` schema and
appends an `EvidenceSubmitted` event unconditionally. There's no
de-duplication layer.]

Best-available reading: the evidence is appended as a new event. The
scheduler re-derives its working set; since nothing changed, the
classification is identical to Turn 10's, so `scheduler.spawned` is
empty and `scheduler.already` carries everything currently on disk:

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
        {"name": "coord.A",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.A1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.A", "coord.A1"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": []
  }
}
```

**What this tells the agent:** Identical resubmission is effectively a
no-op from a spawning perspective but does append a fresh
`EvidenceSubmitted` event, bloating the log.

[GAP: Event-log bloat is not dangerous, but it's surprising. Proposed
resolution: in `merge_epoch_evidence`, detect byte-identical task-list
submissions and still append the event (for auditability) but make
this a documented behavior so agents don't poll-resubmit. Alternative:
`handle_next` short-circuits if the submitted task list is
byte-identical to the most recent merged view.]

### Turn 19 - AGENT (probe: per-task template override)

> The `materialize_children` hook's `default_template` was set at
> compile time. Can a task in my submission override that via a
> per-task `template` field pointing at a totally different template?
> What about swapping `default_template` globally in a re-submit?

Two sub-probes. First, per-task override:

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}, "template": "impl-fast-issue.md"},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    ...
  ]
}
```

### Turn 20 - KOTO

Per Decision 1, the per-task `template` field is "Per-task override
wins" (line 617). This only matters for un-spawned tasks; A1 is already
spawned with the default template, so the new per-task `template` value
on A1 is either ignored (similar to `waits_on`) or errors.

[GAP: Same class of ambiguity as Turn 14. The design says per-task
override wins at initial spawn, but it's silent on whether mid-flight
template changes to already-spawned tasks are honored, ignored, or
rejected. Consistency argument says: reject per Turn 14's proposed
resolution, since the child's template is captured in its
`StateFileHeader` at init time and can't retroactively change.

Second sub-probe: can an agent submit a task list with a different
`default_template` in the evidence? The design explicitly does not
allow this - `default_template` lives on the hook, which is
template-authored and compile-validated (E9). Evidence cannot mutate
the hook. So the answer is clear: no. But the related question is what
happens if the per-task `template` points to a path that only exists
relative to the submitter's cwd (Decision 4 fallback).]

Continuing with the per-task override case, the response (assuming
the template is un-spawned, e.g., if A1 hadn't been spawned yet):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": { "...": "..." }
  }],
  "scheduler": {
    "spawned": ["coord.A1"],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": []
  }
}
```

With the additional caveat that if `impl-fast-issue.md` does not resolve
against `template_source_dir` OR `submitter_cwd`, the scheduler returns:

```json
{
  "action": "batch_error",
  "state": "plan_and_await",
  "error": {
    "kind": "TemplateResolveFailed",
    "task": "A1",
    "paths_tried": [
      "/home/user/repo/templates/impl-fast-issue.md",
      "/home/user/repo/impl-fast-issue.md"
    ]
  }
}
```

**What this tells the agent:** Template resolution errors list both
bases from Decision 4.

### Turn 21 - AGENT (probe: vars change on already-spawned task)

> I realize I put the wrong ISSUE_NUMBER on A. Let me change its vars
> from `{"ISSUE_NUMBER": "201"}` to `{"ISSUE_NUMBER": "999"}` and
> resubmit.

### Turn 22 - KOTO

[GAP: Same family as Turn 14. Per the general principle that "already-
spawned children from earlier submissions are untouched" (line 1868),
vars changes on a live child must not take effect — the child's
`resolve_variables()` ran at `init_state_file` time and the resolved
values live in its header/directives. But the design doesn't spell this
out. Proposed resolution: same as Turn 14 — reject with a
`SpawnedTaskMutated` sub-kind, OR document explicitly that post-spawn
mutations to `vars`, `waits_on`, and `template` are silently ignored
(never retroactively applied). The silent-ignore path is agent-hostile;
erroring is more honest.]

Assuming the "reject" resolution:

```json
{
  "action": "batch_error",
  "state": "plan_and_await",
  "error": {
    "kind": "InvalidBatchDefinition",
    "message": "task 'A' vars changed after spawn; edits to vars on spawned tasks are not permitted"
  }
}
```

### Turn 23 - AGENT (happy path resumes)

The agent reverts the bad probes and goes back to the Turn 10 state.
It drives `coord.A` and `coord.A1` in parallel.

```
koto next coord.A --with-data '{"status": "complete"}'
```

### Turn 24 - KOTO

```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #201 implemented successfully.",
  "is_terminal": true
}
```

### Turn 25 - AGENT

```
koto next coord.A1 --with-data '{"status": "complete"}'
```

### Turn 26 - KOTO

```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #204 implemented successfully.",
  "is_terminal": true
}
```

### Turn 27 - AGENT

```
koto next coord
```

### Turn 28 - KOTO

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5, "completed": 2, "pending": 3,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A",  "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.A1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.B"],
    "already": ["coord.A", "coord.A1"],
    "blocked": ["coord.C", "coord.D"],
    "skipped": []
  }
}
```

**What this tells the agent:** A and A1 both succeeded. B was just
spawned. C and D are still blocked.

### Turns 29-34 - B then C then D (elided)

Agent drives B to `done`, re-ticks parent, C spawns. Drives C to
`done`, re-ticks parent, D spawns. Drives D to `done`, re-ticks parent.

### Turn 35 - KOTO (final)

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete. Write a summary of what was implemented.",
  "is_terminal": true
}
```

Batch complete with all 5 children successful.

---

## Section 2: Findings

### Finding 1: Task removal semantics are unspecified

- **Observation:** The design says `merge_epoch_evidence` "unions" new
  tasks with existing ones (line 93) but is silent on whether the next
  submission can *remove* a previously-submitted task — especially one
  that's already been spawned. Agents will try this.
- **Location in design:** Executive Summary line 92-95; Decision 1 task
  schema; Step 4 walkthrough lines 1859-1861 (scheduler reads "latest
  epoch's EvidenceSubmitted event", singular, which contradicts the
  union framing).
- **Severity:** should-fix.
- **Proposed resolution:** Add an explicit subsection "Task set
  monotonicity" to Decision 1 stating that the effective task set is
  the union of all `EvidenceSubmitted.tasks` across the epoch, keyed by
  task name. Omitting a previously-submitted task name from a later
  submission has no effect — the task remains in the definition.
  Provide a `cancel_tasks` evidence action as the documented escape
  hatch (future work) if the user story requires it.

### Finding 2: Field mutation on already-spawned tasks is undefined

- **Observation:** When a resubmission changes `vars`, `waits_on`, or
  `template` for a task whose child is already on disk, the design does
  not say whether the change is retroactively applied, silently
  ignored, or rejected. Line 1868 ("already-spawned children from
  earlier submissions are untouched") hints at "ignored" but is
  embedded in a cycle-rejection paragraph, not a general rule.
- **Location in design:** Decision 1 task schema; Step 4 walkthrough
  §4 line 1862-1868.
- **Severity:** blocker (behavior is load-bearing for agent mental
  model and for retry correctness).
- **Proposed resolution:** Add a rule R8 to Decision 1's runtime
  validation table: "If a task name appears in the current submission
  and its child already exists on disk, any of `vars`, `waits_on`,
  `template` that differ from the spawn-time values cause
  `InvalidBatchDefinition` with sub-kind `SpawnedTaskMutated`." Allow
  agents to mutate un-spawned tasks freely.

### Finding 3: `InvalidBatchDefinition` lacks a sub-kind discriminator

- **Observation:** R1-R7 failures, cycles from merges, mutation
  rejections, and dangling refs all surface as
  `BatchError::InvalidBatchDefinition { reason: String }`. Agents can
  only react via fragile string parsing.
- **Location in design:** Decision 1 rules R1-R7; `BatchError` enum
  declaration around line 1597.
- **Severity:** should-fix.
- **Proposed resolution:** Promote `InvalidBatchDefinition` to carry a
  `sub_kind: BatchDefinitionErrorKind` enum with variants
  `CycleDetected`, `DanglingRef`, `DuplicateName`, `NameValidation`,
  `ChildCollision`, `SpawnedTaskMutated` (from Finding 2),
  `UnknownField`. Surface it in the CLI response JSON.

### Finding 4: Identical resubmission is not de-duplicated

- **Observation:** Re-posting byte-identical evidence appends a new
  `EvidenceSubmitted` event and runs a full scheduler pass. Harmless
  today, but polling agents could bloat event logs noticeably over
  long-running batches.
- **Location in design:** Step 4 walkthrough §2, §3.
- **Severity:** nice-to-have.
- **Proposed resolution:** Document the behavior explicitly in
  Decision 1 ("Identical resubmission is permitted and is a no-op for
  scheduling; an event is still appended for audit"). Optionally,
  short-circuit in `merge_epoch_evidence` when the submitted list
  equals the current merged view.

### Finding 5: The `scheduler.spawned`/`already`/`blocked` lists don't
distinguish "spawned this tick because of new task added" vs "spawned
this tick because dep unblocked"

- **Observation:** When an agent submits a dynamic addition (Turn 9)
  and also happens to unblock something, the response conflates both
  spawn reasons under `scheduler.spawned`. Debuggability suffers.
- **Location in design:** `SchedulerOutcome::Scheduled` definition
  around line 1577.
- **Severity:** nice-to-have.
- **Proposed resolution:** Add a per-task `reason` enum field:
  `spawned: Vec<{name: String, reason: "deps_resolved" | "newly_added"}>`.

### Finding 6: Per-task `template` resolution error does not say which
base was used

- **Observation:** The proposed `TemplateResolveFailed` error variant
  (line 1603) carries `paths_tried: Vec<String>` but the naming doesn't
  signal which path came from `template_source_dir` (primary) vs
  `submitter_cwd` (fallback), making Decision 4 debugging harder.
- **Location in design:** Decision 4 scheduler resolution order (line
  852); `BatchError::TemplateResolveFailed` around line 1603.
- **Severity:** nice-to-have.
- **Proposed resolution:** Change `paths_tried: Vec<String>` to
  `attempts: Vec<{base: "template_source_dir" | "submitter_cwd",
  resolved: String}>`.

### Finding 7: Dynamic-addition + retry interaction is not worked through

- **Observation:** Decision 5 describes retry mechanics assuming a
  static batch. It doesn't address: after a retry of failed child X,
  can the agent *also* dynamically add new siblings in the same
  submission? The `retry_failed` handler runs pre-advance-loop; the
  advance loop then sees a merged task list plus a rewind. Interaction
  order (retry first, then scheduler merge, then spawn) is implied but
  not stated. Edge case: if the same submission includes both
  `retry_failed` and `tasks`, which takes precedence?
- **Location in design:** Decision 5 §5.4; line 1042 "Interception
  point".
- **Severity:** should-fix.
- **Proposed resolution:** In Decision 5 §5.4, add: "A single evidence
  submission may carry both a `tasks` field (adding to the task set)
  and a `retry_failed` field. `handle_retry_failed` runs first on the
  current task set; the new tasks are then merged and validated
  against the post-retry child state. Cycles introduced by added tasks
  reject the whole submission including the retry." Alternatively:
  reject mixed submissions with a clear error.

### Finding 8: Scheduler reads "latest epoch's EvidenceSubmitted event"
is single-event wording, contradicting "union" narrative

- **Observation:** Line 1859-1861 says "Extracts the task list from
  the latest epoch's `EvidenceSubmitted` event via `derive_evidence +
  merge_epoch_evidence`." This is singular, but `merge_epoch_evidence`
  by name (and by behavior per line 93) merges multiple events within
  an epoch. The single-event phrasing is misleading.
- **Location in design:** Step 4 implementation plan §4.
- **Severity:** nice-to-have (documentation).
- **Proposed resolution:** Rewrite as: "Extracts the task list by
  folding `merge_epoch_evidence` over all `EvidenceSubmitted` events
  in the current epoch; the resulting `tasks` array is the effective
  batch definition."

### Finding 9: No guidance on how agents should discover that a resubmit
was needed

- **Observation:** The scenario (agent realizes mid-flight that tasks
  are missing) is the motivating use case per line 134-137, but
  nothing in the response contract surfaces "you should re-plan". The
  agent has to know out-of-band. This is more a PRD concern than a
  design blocker, but worth flagging.
- **Location in design:** Decision Drivers "Dynamic additions are
  required"; Step 4 walkthrough.
- **Severity:** nice-to-have.
- **Proposed resolution:** In the walkthrough and the `koto-user`
  skill, add a brief guidance note: "If new information arrives during
  execution that changes your task list, submit an updated tasks
  array via the same `koto next <parent> --with-data` command.
  Already-spawned tasks are preserved; newly-added tasks are spawned
  on the next scheduler tick." No design-doc change strictly needed.
