# Simulation — Round 2, Pair B2: Per-task spawn failures + recovery

Design under validation: `docs/designs/DESIGN-batch-child-spawning.md`
(revised — Decisions 11 and 14).

Scenario: 5 independent tasks (no deps). Some spawn cleanly; some hit
per-task spawn errors of two different kinds. Agent drives the healthy
tasks, then attempts recovery through resubmission while probing R8,
last-write-wins, and the `spawn_failed` lifecycle.

KOTO responses are generated from the revised CD11 + CD14 shape:
`SchedulerOutcome::Scheduled.errored: Vec<TaskSpawnError>`,
`BatchTaskView.outcome: spawn_failed` with optional `spawn_error`
payload, `SchedulerFeedback.entries` map keyed by short name,
`action: "error"` envelope for whole-submission rejection.

`[GAP: ...]` markers flag places where the revised design still leaves
the `spawn_failed` lifecycle under-specified.

---

## Section 1: Transcript

### Setup — parent and template inventory

Parent workflow `coord` is parked at `plan_and_await`. The parent was
initialized via `koto init coord --template coord.md --var plan_path=PLAN.md`.

Template files on disk:
- `impl-issue.md` — valid, compiles, default-action "drive the child."
- `compile-fail.md` — **present** but YAML frontmatter has a syntax
  error (unterminated string). Passes path resolution step (b); fails
  template compilation.
- `does-not-exist.md` — **absent**. Path resolution exhausts both
  `template_source_dir` and `submitter_cwd`.

### Probe 1 — Initial submission with two bad templates

#### AGENT

```
$ koto next coord --with-data @tasks.v1.json
```

`tasks.v1.json`:

```json
{
  "tasks": [
    {"name": "x1", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "x2", "template": "does-not-exist.md", "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "x3", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "x4", "template": "compile-fail.md",  "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "x5", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

_Gloss: 5 roots, no deps. x2 has a missing template path. x4 has a
template that exists but fails to compile._

#### KOTO

Internal order, per the revised design (Step 4 and Decisions 11 / 14):

1. **Pre-append validation (whole-submission).** R0 (non-empty), R3
   (cycles — none; it's an empty-edge DAG), R4 (dangling — none),
   R5 (duplicates — none), R6 (limits — 5 tasks, depth 1, waits_on
   all empty), R8 (spawn-time immutability — vacuous; no children on
   disk), R9 (names — all match `^[A-Za-z0-9_-]+$`). **Passes.**
2. **`EvidenceSubmitted` appended.** Task list + `submitter_cwd`
   recorded.
3. **Advance loop** re-evaluates `done` (children-complete) gate.
   Batch definition read from evidence: `total: 5, pending: 5,
   ready: 5, blocked: 0`. Gate `Failed`; `all_complete: false`.
   Loop stops at `plan_and_await`.
4. **Scheduler runs.** `run_batch_scheduler(final_state =
   "plan_and_await")`:
   - Classifies all 5 as `NotYetSpawned` / `Ready`.
   - Per-task spawn loop (per CD11: accumulate, never halt):
     - x1 → resolves `impl-issue.md`, compiles, `init_state_file`
       succeeds. Added to `spawned_this_tick`, `feedback.entries.x1 =
       Accepted`.
     - x2 → path resolution exhausts (a) absolute (relative), (b)
       `template_source_dir`, (c) `submitter_cwd`. Emits
       `TaskSpawnError { task: "x2", kind: TemplateNotFound,
       paths_tried: [...], message: ... }`. No `init_state_file`
       call. `feedback.entries.x2 = Errored { kind:
       "template_not_found" }`.
     - x3 → spawns cleanly.
     - x4 → path resolves (step b hits), read succeeds, compile
       fails with YAML error. Emits `TaskSpawnError { task: "x4",
       kind: TemplateCompileFailed, paths_tried: None, message: ...
       }`. `feedback.entries.x4 = Errored { kind:
       "template_compile_failed" }`. **Note: the revised
       `SpawnErrorKind::TemplateCompileFailed` has no `paths_tried`
       semantically, but `TaskSpawnError.paths_tried` is still
       `Option<Vec<String>>`. Design sets it to `None` for compile
       failures — the path is elsewhere (compile_error fields live
       on `BatchError::TemplateCompileFailed`, not on
       `TaskSpawnError`). `[GAP 1: The compile path and the
       `compile_error` string surface on `BatchError::
       TemplateCompileFailed` but the per-tick scheduler emits
       `TaskSpawnError`, which only carries `kind` + `message` +
       optional `paths_tried`. Agents get a generic message string
       with no structured compile_error / path to surface. Consider
       adding `compile_path: Option<String>` and `compile_error:
       Option<String>` to `TaskSpawnError` for the
       TemplateCompileFailed kind.]`**
     - x5 → spawns cleanly.
   - `SchedulerRan` event appended (tick is non-trivial:
     `spawned_this_tick` and `errored` both non-empty).
5. **Response assembled.** Outer envelope `gate_blocked` (the gate
   failed), with scheduler outcome attached and the gate output
   enumerating all 5 tasks by name.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5,
      "completed": 0,
      "pending": 3,
      "success": 0,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 2,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.x1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.x2", "state": null, "complete": false, "outcome": "spawn_failed",
         "spawn_error": {
           "task": "x2",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/dan/src/tsuku/does-not-exist.md",
             "/home/dan/src/tsuku/does-not-exist.md"
           ],
           "message": "Template not found at any configured base"
         }},
        {"name": "coord.x3", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.x4", "state": null, "complete": false, "outcome": "spawn_failed",
         "spawn_error": {
           "task": "x4",
           "kind": "template_compile_failed",
           "paths_tried": null,
           "message": "YAML frontmatter error: unterminated string at line 3"
         }},
        {"name": "coord.x5", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.x1", "coord.x3", "coord.x5"],
    "materialized_children": [
      {"name": "coord.x1", "outcome": "pending", "state": "working"},
      {"name": "coord.x3", "outcome": "pending", "state": "working"},
      {"name": "coord.x5", "outcome": "pending", "state": "working"}
    ],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errored": [
      {
        "task": "x2",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/dan/src/tsuku/does-not-exist.md",
          "/home/dan/src/tsuku/does-not-exist.md"
        ],
        "message": "Template not found at any configured base"
      },
      {
        "task": "x4",
        "kind": "template_compile_failed",
        "paths_tried": null,
        "message": "YAML frontmatter error: unterminated string at line 3"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "x1": {"outcome": "accepted"},
        "x2": {"outcome": "errored", "kind": "template_not_found"},
        "x3": {"outcome": "accepted"},
        "x4": {"outcome": "errored", "kind": "template_compile_failed"},
        "x5": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: 3 of 5 live; 2 are parked in `spawn_failed` limbo with no state
file on disk. `materialized_children` only lists the 3 actual children.
The gate output lists all 5 because it reads the batch definition, not
just disk._

**Observations.**
- `spawn_failed: 2` in the summary. `pending: 3` refers to the live
  children still working (x1, x3, x5). Revised design example
  (walkthrough.md:1067-1083) is consistent: spawn_failed is its own
  bucket, not rolled into pending.
- `all_complete: false`. Per `pending == 0 AND blocked == 0`, we're
  not complete. But **note that if the pending children all terminate
  cleanly, `pending` would drop to 0, `blocked` is already 0, and
  `all_complete` would go true — while x2 and x4 still have no state
  files.** This is probed below in Probe 3.
- `needs_attention: false` because `failed == 0 AND skipped == 0`.
  The revised booleans do NOT count `spawn_failed` as a failure for
  the attention flag. **`[GAP 2: `needs_attention = all_complete AND
  (failed > 0 OR skipped > 0)` by current definition. A batch with
  all-pending-children-terminal-success and `spawn_failed > 0` would
  satisfy `all_complete: true` AND `all_success: false` (because
  `all_success` requires `failed == 0 AND skipped == 0`, and
  `spawn_failed` is implicitly not counted) — but `needs_attention`
  stays `false`. Templates routing on `needs_attention` would
  transition to a "done_happy" state on a batch with unresolved
  spawn failures. Either `all_complete` should tighten to also
  require `spawn_failed == 0`, or `needs_attention` should fire on
  `spawn_failed > 0`, or a new `any_spawn_failed` boolean should
  join the set.]`**
- `reserved_actions` is absent (gate output did not report
  `any_failed` or `any_skipped`; the criterion for emitting
  `reserved_actions` per Decision 9 is `any_failed || any_skipped`,
  which does not consider `spawn_failed`). **`[GAP 3: `retry_failed`
  does not accept `spawn_failed` children per its R10 check
  ("outcome: failure or skipped"). Recovery for `spawn_failed` is
  therefore not via the documented retry path — it's via
  resubmission of the corrected task entry under last-write-wins.
  The revised walkthrough and reference template don't walk through
  this path. Agents will look for a `reserved_actions.retry_failed`
  slot on this response and find none.]`**

---

### Probe 2 — Agent drives x1, x3, x5 to completion

Agent drives each child via `koto next coord.x1`, `koto next coord.x3`,
`koto next coord.x5` in parallel. Each terminates successfully (reaches
a `done` state with `failure: false`). Standard child-lifecycle; no
scheduler interaction.

### Probe 3 — Re-tick parent after healthy children complete

#### AGENT

```
$ koto next coord
```

(no evidence; just a refresh).

#### KOTO

Internal order:

1. **Pre-append validation.** No evidence submitted; nothing to
   validate.
2. **No `EvidenceSubmitted` append.**
3. **Advance loop at `plan_and_await`.** `done` gate re-evaluates:
   - reads batch definition from latest-epoch `EvidenceSubmitted`
     (still the v1 payload)
   - disk scan: x1, x3, x5 are terminal non-failure
   - x2, x4 have no state file at all (never spawned)
   - classifier outcomes:
     - x1, x3, x5 → `success`
     - x2, x4 → `spawn_failed` (computed from prior `SchedulerRan`
       events? or re-derived per tick?)
   - summary: `total: 5, completed: 3, pending: 0, success: 3,
     failed: 0, skipped: 0, blocked: 0, spawn_failed: 2`
   - `all_complete` = `pending == 0 AND blocked == 0` = **TRUE**
   - `all_success` = `all_complete AND failed == 0 AND skipped == 0`
     = **TRUE** (because `spawn_failed` is not counted)
4. **Transition fires.** If the template has the default
   "coord.md"-style routing, the `all_complete: true` edge matches
   (or `all_success: true`, same outcome). Parent transitions to
   `summarize` (terminal).
5. **Scheduler runs on `summarize`.** No `materialize_children` hook
   → `NoBatch`. Returns.
6. **Response is `action: "done"`.** `batch_final_view` attached
   (first `all_complete: true` tick appends `BatchFinalized`).

**This is the bug.** The parent walks off to "success" carrying two
tasks that never had state files, never ran, never produced work.
The operator-facing summary would report "5 of 5 tasks, 3 succeeded,
2 spawn_failed" but the routing treats the batch as done.

**`[GAP 4: The `spawn_failed` lifecycle in the gate predicate.
Options:**
- **Tighten `all_complete` to `pending == 0 AND blocked == 0 AND
  spawn_failed == 0`. Templates would stick at `plan_and_await`
  until the agent resubmits with corrections. Concern: breaks the
  walkthrough's language that `all_complete` means "nothing left to
  do."
- **Add `any_spawn_failed` boolean; add W6 compile warning for
  materialize_children states routing only on `all_complete`
  without an `any_spawn_failed` branch. Consistent with W4's
  approach to `any_failed`.
- **Make the scheduler re-classify `spawn_failed` as `pending` on
  every tick, since the entry still exists in the batch definition
  and still has no terminal outcome.** This would match the
  disk-derived model: pending means "no terminal state file," which
  spawn_failed is.
- **Treat `spawn_failed` like `failed` for aggregate booleans.**
  Adds clarity at the cost of conflating two distinct error modes
  (spawn-time vs run-time).

The design as written does not pick. Concrete behavior of this tick
— whether the parent advances or stays parked — is unspecified.]`**

For the rest of this simulation we assume the **buggy-by-default**
current spec (parent advances to summarize). The agent is now in
recovery mode after the fact.

---

### Probe 4 — Recovery attempt 1: fix x2, leave x4 broken

Before the bug from Probe 3 manifests (imagine the agent catches the
`spawn_failed: 2` in Probe 1 and acts before re-ticking), agent
submits a corrected task list with x2's template fixed. x4 is left
with the broken template on purpose.

#### AGENT

```
$ koto next coord --with-data @tasks.v2.json
```

`tasks.v2.json`:

```json
{
  "tasks": [
    {"name": "x1", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "x2", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "x3", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "x4", "template": "compile-fail.md",  "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "x5", "template": "impl-issue.md",    "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

#### KOTO

Internal order:

1. **Pre-append validation (whole-submission).**
   - R0: non-empty, OK.
   - R3/R4/R5: graph OK, no cycles, no duplicates.
   - R6: under limits.
   - **R8 — Spawn-time immutability.** For each entry whose
     `<parent>.<task.name>` child exists on disk with a `spawn_entry`
     snapshot:
     - x1 → `coord.x1` exists on disk (spawned in Probe 1). Submitted
       entry `{template: "impl-issue.md", vars: {ISSUE_NUMBER: "1"},
       waits_on: []}` matches the `spawn_entry` snapshot
       field-for-field. **Passes.**
     - x2 → `coord.x2` does NOT exist on disk (never spawned).
       Entry is un-spawned; last-write-wins applies; R8 vacuous.
       **Passes.**
     - x3 → matches. **Passes.**
     - x4 → `coord.x4` does NOT exist on disk. Un-spawned; R8
       vacuous. Entry unchanged from v1 but that's fine; R8 doesn't
       care about "unchanged," only "spawned and now changed."
       **Passes.**
     - x5 → matches. **Passes.**
   - R9: names OK.
2. **`EvidenceSubmitted` appended** (v2 task list recorded).
3. **Advance loop at `plan_and_await`.** Gate re-evaluates with
   merged batch definition (union-by-name; last-write-wins for
   un-spawned, R8-locked for spawned — in this case no field
   actually changed for x1/x3/x5, and x2/x4 overwrite their
   un-spawned entries).
4. **Scheduler runs.**
   - Per-task loop:
     - x1 → `coord.x1` already terminal success. Classified
       `already` (or `Terminal` non-failure). `feedback.entries.x1
       = Already`.
     - x2 → `NotYetSpawned`, `Ready` (no deps). Try to spawn with
       the *new* template path `impl-issue.md`. Path resolves,
       compiles, `init_state_file` succeeds. Added to
       `spawned_this_tick`. `feedback.entries.x2 = Accepted`.
     - x3 → `already`.
     - x4 → `NotYetSpawned`, `Ready`. Try to spawn with
       `compile-fail.md`. Path resolves (step b). Compile fails
       again. Emits `TaskSpawnError { task: "x4", kind:
       TemplateCompileFailed, ... }`. Same shape as Probe 1.
       `feedback.entries.x4 = Errored { kind:
       "template_compile_failed" }`.
     - x5 → `already`.
   - `SchedulerRan` appended.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5,
      "completed": 3,
      "pending": 1,
      "success": 3,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 1,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.x1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.x2", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.x3", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.x4", "state": null, "complete": false, "outcome": "spawn_failed",
         "spawn_error": {
           "task": "x4",
           "kind": "template_compile_failed",
           "paths_tried": null,
           "message": "YAML frontmatter error: unterminated string at line 3"
         }},
        {"name": "coord.x5", "state": "done", "complete": true, "outcome": "success"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.x2"],
    "materialized_children": [
      {"name": "coord.x1", "outcome": "success", "state": "done"},
      {"name": "coord.x2", "outcome": "pending", "state": "working"},
      {"name": "coord.x3", "outcome": "success", "state": "done"},
      {"name": "coord.x5", "outcome": "success", "state": "done"}
    ],
    "already": ["coord.x1", "coord.x3", "coord.x5"],
    "blocked": [],
    "skipped": [],
    "errored": [
      {
        "task": "x4",
        "kind": "template_compile_failed",
        "paths_tried": null,
        "message": "YAML frontmatter error: unterminated string at line 3"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "x1": {"outcome": "already"},
        "x2": {"outcome": "accepted"},
        "x3": {"outcome": "already"},
        "x4": {"outcome": "errored", "kind": "template_compile_failed"},
        "x5": {"outcome": "already"}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: Recovery for x2 works cleanly. Last-write-wins allowed the
corrected template path to flow through to `init_state_file`. x4
failed again — same kind, same shape — because its template is still
broken on disk._

**Observations.**
- **x2 recovered.** This confirms the design's recovery story for
  `spawn_failed` children: resubmit with a corrected entry, R8
  doesn't block (no spawned child to lock against), the scheduler's
  per-task loop tries again.
- **x4 failed again with the same payload.** Predictable: the
  scheduler re-attempts spawn every tick for `NotYetSpawned` `Ready`
  tasks. Every tick where the template is still broken will emit a
  fresh `TaskSpawnError` and a fresh `SchedulerRan` event. **`[GAP
  5: Repeated spawn_failed tasks flood `SchedulerRan` with near-
  identical events. The design's "no-op ticks skip the append" rule
  (errored non-empty ⇒ append) means every retry tick for a
  persistently-broken template adds a log entry. If the agent polls
  every second for 10 minutes awaiting children to terminate, the
  parent log grows by 600 near-duplicate entries. Not a correctness
  bug, but an observability-noise gap. Consider deduplicating by
  `(task, kind, message)` within a tick window, or appending a
  single "still broken" entry per value-change.]`**
- `feedback.entries` covers all 5 entries in this tick's submission.
  **This answers the explicit probe question: yes, the feedback map
  is re-computed every tick for the current submission, and every
  entry gets an outcome (no silent cases per CD10).**
- `already` gets populated with the full child name
  (`coord.x1`), while `feedback.entries` is keyed on short name
  (`x1`). **Consistent with the revised schema (CD10: "Keyed by the
  agent-submitted short name").**

---

### Probe 5 — Recovery attempt 2: change x4's path, still broken

x4 remains unsalvageable; agent tries a different broken template
(`compile-fail-v2.md` — different file, same kind of YAML error).

#### AGENT

```
$ koto next coord --with-data @tasks.v3.json
```

`tasks.v3.json`:

```json
{
  "tasks": [
    {"name": "x1", "template": "impl-issue.md",       "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "x2", "template": "impl-issue.md",       "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "x3", "template": "impl-issue.md",       "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "x4", "template": "compile-fail-v2.md",  "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "x5", "template": "impl-issue.md",       "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

#### KOTO

R8 check for each entry:
- x1, x3, x5 → match spawned snapshots. Pass.
- x2 → `coord.x2` now exists on disk (spawned in Probe 4) with
  `spawn_entry.template = "impl-issue.md"`, `vars = {ISSUE_NUMBER:
  "2"}`, `waits_on = []`. Submitted entry matches. **Pass.**
- x4 → `coord.x4` does NOT exist on disk. Un-spawned; R8 vacuous.
  Last-write-wins: v3's entry supersedes v1/v2. **Pass.**

`EvidenceSubmitted` appended. Scheduler runs:
- x1, x2, x3, x5 → `already`.
- x4 → `NotYetSpawned`, `Ready`. Resolves `compile-fail-v2.md`,
  reads, compile fails. Emits `TaskSpawnError { task: "x4", kind:
  TemplateCompileFailed, message: "<new error>" }`.

Response shape is the same as Probe 4, minus the x2 spawn (x2 is
now `already`). `spawn_error.message` differs to reflect the new
compile error from the new file. **`[GAP 6: Nothing in the gate
output or `TaskSpawnError` ties the previous spawn failure to the
new one. There's no "retry history" per task. Agents see a fresh
`spawn_error` every tick but have no signal that x4 has been stuck
for, say, 5 ticks running. Diagnostic richness is limited to the
current tick's error.]`**

The answer to the explicit probe question: **yes, the scheduler
simply tries again with the new template. Same failure kind
(`template_compile_failed`), different `message`, possibly different
`paths_tried`.**

---

### Probe 6 — R8 rejection: mutation mixed with recovery

x1 is spawned-and-terminal. Agent submits a payload that mutates
x1's `vars` (rejected per R8) AND contains corrections for x2 and
x4. Question: does the R8 rejection abort the whole submission, or
are x2/x4 still processed?

#### AGENT

```
$ koto next coord --with-data @tasks.v4.json
```

`tasks.v4.json`:

```json
{
  "tasks": [
    {"name": "x1", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "999"}},
    {"name": "x2", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "x3", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "x4", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "x5", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

_Gloss: x1's `ISSUE_NUMBER` changed from "1" to "999" (mutation
attempt). x4's template switched to a valid one. x2, x3, x5
unchanged from their `spawn_entry` snapshots._

#### KOTO

Internal order:
1. **Pre-append validation (whole-submission).**
   - R0 OK, R3 OK, R4 OK, R5 OK, R6 OK.
   - **R8** — iterate spawned children:
     - x1 → `coord.x1` exists; `spawn_entry.vars = {ISSUE_NUMBER:
       "1"}`, submitted `{ISSUE_NUMBER: "999"}`. **Mismatch.**
       Reject.

Design text (CD10): "One mismatched entry rejects the whole
submission." R8 fires `InvalidBatchReason::SpawnedTaskMutated {
task: "x1", changed_fields: [{ field: "vars.ISSUE_NUMBER",
spawned_value: "1", submitted_value: "999" }] }`.

2. **Rejection is pre-append.** No `EvidenceSubmitted` written. No
   `SchedulerRan`. Parent state file byte-identical.
3. **Response is `action: "error"`.**

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: spawned task mutated",
    "details": [{"field": "tasks[0].vars.ISSUE_NUMBER", "reason": "spawned_task_mutated"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "x1",
      "changed_fields": [
        {"field": "vars.ISSUE_NUMBER", "spawned_value": "1", "submitted_value": "999"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Observations.**
- **The whole submission is rejected atomically.** x2's correction
  and x4's fix do NOT flow through. The agent must resubmit without
  the x1 mutation.
- The atomicity is correct by design but **poorly signposted**.
  **`[GAP 7: The `SpawnedTaskMutated` error surfaces only the one
  offending task. Nothing in the response tells the agent "your
  other 4 entries were otherwise-valid; resubmit with the spawned
  task's fields corrected." Agents reading only the error envelope
  may not realize x2 and x4 recovery attempts are in limbo. A
  `error.batch.discarded_entries: [...]` or a hint in `message`
  would clarify.]`**
- **Matches the explicit probe answer: the atomic unit of a
  submission is the whole `tasks` array. Any R8 (or R3/R4/R5/R6/R9)
  rejection drops everything.** `EvidenceSubmitted` is atomic too —
  all fields or none.

Agent's correct move: resubmit `tasks.v3.json` (without the x1
mutation). x2 is already spawned from Probe 4, so it'll come back
as `already`. x4 will emit another `spawn_failed`. No progress
on x4 until the template file is fixed on disk.

---

### Probe 7 — Terminal question: can the batch ever complete if x4 stays broken?

Suppose x4 remains unfixable (template author bug, can't be patched
without a code change; no retry action applies to `spawn_failed`).
Can the batch reach a clean terminal state?

**Options per revised design:**

1. **Advance through the `all_complete` footgun.** With x1, x2, x3,
   x5 all terminal success and x4 in `spawn_failed`, `pending == 0`
   (spawn_failed is not pending), `blocked == 0`. `all_complete`
   fires true. Parent routes through the success path. **This is
   wrong-by-construction — the design routes a batch with an
   unresolved `spawn_failed` to "done" as if nothing went wrong.**
   (GAP 4 above.)
2. **Cancel x4.** `cancel_tasks` is deferred to v1.1 (CD10). No v1
   primitive.
3. **Retry x4.** `retry_failed` requires `outcome == failure ||
   outcome == skipped` per R10. `spawn_failed` is neither. R10
   rejects with `ChildNotEligible`.
4. **Delete the parent state file and start over.** Nuclear; loses
   the work done on x1, x2, x3, x5.
5. **Manually delete child's state file.** No child state file
   exists — x4 was never spawned. Nothing to delete.

**The only actual escape in v1 is Option 1 (walk through the
footgun) or Option 4 (nuclear).** Neither is graceful.

**`[GAP 8 (compound): The `spawn_failed` state lacks a clean
terminal path in v1. Decision 9's retry machinery doesn't cover it.
Decision 10's mutation semantics don't cover it. Decision 13's
skip-markers don't cover it (skip markers are scheduler-authored
for dependency-failure cascade, not for spawn errors). Options to
close:**
- **Let `retry_failed` accept `spawn_failed` children** (loosen R10).
  The "retry" is a re-spawn attempt with the possibly-corrected
  latest-epoch entry. Semantically clean.
- **Add `cancel_tasks` to v1** for explicit skipping of
  spawn_failed tasks. CD10 rejects this as v1 scope, but the
  `spawn_failed` lifecycle is arguably a strong argument to
  revisit.
- **Treat `spawn_failed` as `failed` for retry eligibility and for
  `needs_attention`.** Simplest fix. Adds an aggregate-boolean nudge
  and a retry route without new primitives.**]`**

---

## Section 2: Findings

### What worked as designed

1. **Per-task accumulation is real.** The scheduler spawned 3 of 5
   tasks cleanly on the first submission and accumulated 2 typed
   per-task errors without halting. Both the `SchedulerOutcome.
   errored` vector and the per-child `BatchTaskView.outcome:
   spawn_failed` carry structured payloads.
2. **Last-write-wins recovery for un-spawned tasks works.** Probe 4
   confirmed that resubmitting with a corrected `template` for
   x2 (un-spawned, `spawn_failed`) flows cleanly through R8 and
   gets the child spawned on the same tick.
3. **R8 enforcement is atomic at the whole-submission level.**
   Probe 6 confirmed that a single mutation rejection aborts the
   whole submission; no partial commit, no `EvidenceSubmitted` side
   effect.
4. **Feedback map covers all entries.** Every submitted task gets
   an `EntryOutcome` (`Accepted`, `Already`, `Blocked`, `Errored`)
   keyed on short name. No silent cases. Probes 4 and 5 showed the
   map contents vary per tick based on current disk state.
5. **Error split (CD14) differentiates cleanly.** `TemplateNotFound`
   vs `TemplateCompileFailed` surface as separate `SpawnErrorKind`
   variants with different payload shapes.

### Gaps in the revised design

**GAP 1 — `TaskSpawnError` carries less structure than
`BatchError::TemplateCompileFailed`.** The per-tick per-task error
type has `paths_tried: Option<Vec<String>>` and `message: String`,
but for `TemplateCompileFailed` the structured `compile_path` and
`compile_error` live on the (unused-here) `BatchError` variant. The
agent has to parse `message` strings. **Recommendation:** extend
`TaskSpawnError` to carry `compile_path: Option<String>` and
`compile_error: Option<String>` for the
`TemplateCompileFailed` kind.

**GAP 2 — `needs_attention` and `all_success` ignore
`spawn_failed`.** The aggregate booleans were designed to catch
`failed` and `skipped` but `spawn_failed` is a third failure mode
introduced by CD11 without updating the aggregate-boolean
definitions. A batch with all-terminal-children plus
`spawn_failed > 0` routes as if nothing is wrong. **Recommendation:**
either tighten `all_complete` to require `spawn_failed == 0`, or add
`any_spawn_failed` to the boolean set and extend W4's compile warning
to catch templates that route without guarding it.

**GAP 3 — `retry_failed` excludes `spawn_failed`.** R10's "outcome:
failure or skipped" excludes `spawn_failed`. There is no
`reserved_actions` hint for recovering from a spawn failure, so
agents looking for a machine-readable recovery signal find nothing.
**Recommendation:** let `retry_failed` accept `spawn_failed`
children; the semantics are identical (re-run the per-task spawn
with the latest-epoch entry).

**GAP 4 — Parent can advance to success with unresolved
`spawn_failed`.** Combined effect of GAP 2 and GAP 3. This is the
highest-severity finding in the round. A 5-task batch where 2 never
spawned routes to `action: "done"` as if it succeeded. **Concrete
recommendation:** tighten `all_complete` in CD11's schema definition
to `pending == 0 AND blocked == 0 AND spawn_failed == 0`. The
walkthrough's stated semantics ("nothing left to do") match this
tighter definition naturally.

**GAP 5 — `SchedulerRan` log noise on persistent spawn
failures.** Every `koto next` tick where a spawn-failed task
remains stuck appends another `SchedulerRan` event with the same
error payload. For agents polling, this floods the parent log.
**Recommendation:** deduplicate `SchedulerRan` appends by
`(errored tasks fingerprint, spawned_this_tick fingerprint)` against
the previous `SchedulerRan` within the same epoch.

**GAP 6 — No per-task retry history on `TaskSpawnError`.** Each
tick emits a fresh error with no indication that the same task has
been failing for N ticks. Diagnostic value is limited. **Lower
severity** — the log trail is recoverable via event replay.

**GAP 7 — R8 rejection response does not flag discarded
sibling entries.** When R8 rejects the submission, x2/x4 corrections
are silently dropped. The error envelope only names the offending
task. **Recommendation:** add `error.batch.discarded_entries` or a
hint in `error.message` telling the agent "N other entries in this
submission were not applied; resubmit without the spawn-time
mutation."

**GAP 8 (compound) — `spawn_failed` has no graceful v1 terminal
path.** GAP 2, 3, and 4 interact to produce the condition:
`spawn_failed` is recoverable only by resubmission, but if the
agent cannot fix the underlying template, there is no graceful
way to walk the parent off the stuck state besides the
wrong-by-construction `all_complete` footgun. This is the
architectural gap the revised design inherits from incomplete
treatment of the spawn_failed lifecycle. The cleanest combined
fix is GAP 4 tightening + GAP 3 loosening of R10 to let the retry
primitive cover spawn failures too.

### Points the design got exactly right under probing

- **Atomicity of submission.** Probe 6 confirmed that whole-submission
  validation is pure and pre-append. Rejection leaves zero state.
  R8's "one mismatch rejects all" is the right default for agent
  mental models — partial commits of a submitted array would be
  actively harmful.
- **Feedback keyed on short name, child set keyed on full name.**
  The naming convention (`coord.x1` for children, `x1` for feedback
  entries) is consistent across the probes and matches the design
  text.
- **Per-task errors don't pollute whole-submission errors.**
  Probes 1 and 4 confirmed: x2's `TemplateNotFound` and x4's
  `TemplateCompileFailed` stayed per-task; the top-level
  `NextError` stayed `null`. The scheduler response carried the
  detail. CD11's pledge holds.
- **`materialized_children` is the ledger, `spawned_this_tick` is
  the observation.** Probe 4's response shows `materialized_children`
  listing 4 live children (x1, x2, x3, x5) while `spawned_this_tick`
  only names `coord.x2`. Agents idempotently dispatching based on
  `materialized_children` would not double-spawn.

### Summary of recommended changes to the revised design

1. **Tighten `all_complete`** to include `spawn_failed == 0`.
2. **Extend retry_failed eligibility** to accept `spawn_failed`
   children.
3. **Add `any_spawn_failed` aggregate boolean** and extend W4 to
   warn when templates don't guard it.
4. **Enrich `TaskSpawnError`** with `compile_path` and
   `compile_error` for the `TemplateCompileFailed` kind.
5. **Flag discarded entries in R8 rejection responses.**
6. **Deduplicate `SchedulerRan` appends** for identical
   persistently-failing ticks.

Changes 1-3 together close GAP 8's architectural hole. Changes 4-6
are polish but meaningfully improve the recovery story for
real-world agents.
