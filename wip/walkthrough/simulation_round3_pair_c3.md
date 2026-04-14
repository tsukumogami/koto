# Round 3, Pair C3 — Happy-path sanity check

Scenario: 3-issue diamond (`issue-1 -> issue-2, issue-3`; 2 and 3
independent). All happy-path, no failures, no mutations, no retries,
no dynamic additions. Same templates as walkthrough.md Interactions
1-8.

Purpose of this round-3 pair: after two rounds of failure-path and
mutation-path simulations introduced `ready_to_drive`, `role`,
`subbatch_status`, `reserved_actions`, `EntryOutcome::Respawning`,
`orphan_candidates`, and assorted serde-optional fields, do SIMPLE
cases stay simple? Does the wire shape stay proportional to the
feature in the common case?

---

## Section 1: Transcript

### Turn 1 - `koto init coord --template coord.md --var plan_path=PLAN.md`

KOTO: creates session, writes header with `template_source_dir`, and
appends `WorkflowInitialized` + `Transitioned -> plan_and_await`.

```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

Gloss: 4 top-level fields. Identical to the non-batch `init`
response. No batch surface here by design -- a batch state file is
indistinguishable from any other state file until the scheduler
runs. Good.

### Turn 2 - `koto next coord`

KOTO: advisory flock acquires (state has `materialize_children`).
Advance loop parks at `plan_and_await` needing `tasks` evidence.
Gate runs, zero children, `Failed` with zero-filled output.
Scheduler runs, `NoBatch` (no `tasks` in evidence). No failure, no
skip, no spawn_failed in gate output: `reserved_actions` OMITTED.

```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN.md. For each issue outline ...",
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
      "total": 0, "completed": 0, "pending": 0, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
      "children": []
    }
  }],
  "scheduler": null
}
```

Top-level field count: 5 (`action`, `state`, `directive`, `expects`,
`blocking_conditions`) + `scheduler: null`. No `reserved_actions`
key (suppressed via `skip_serializing_if`).

Gloss: agent gets the directive, the `tasks` item_schema, and a
zero-filled gate. `scheduler: null` is an explicit signal "no batch
yet" (Decision 11 keeps the key present with a null sentinel on
pre-submission ticks so agents can rely on the key existing on
batched states). Clean.

### Turn 3 - `koto next coord --with-data @tasks.json`

AGENT submits 3 tasks. KOTO validates R0-R9, appends
`EvidenceSubmitted`, scheduler spawns `coord.issue-1`, appends
`SchedulerRan`.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 2, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
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
      {"name": "coord.issue-1", "outcome": "pending", "state": "working", "ready_to_drive": true}
    ],
    "already": [],
    "blocked": ["coord.issue-2", "coord.issue-3"],
    "skipped": [],
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "accepted"},
        "issue-2": {"outcome": "blocked", "waits_on": ["issue-1"]},
        "issue-3": {"outcome": "blocked", "waits_on": ["issue-1"]}
      }
    }
  }
}
```

Top-level: 5. `reserved_actions` absent (no failure/skip/spawn_fail
yet). Inside `scheduler`: 6 keys (`spawned_this_tick`,
`materialized_children`, `already`, `blocked`, `skipped`,
`feedback`). `errored` and `warnings` suppressed (empty vecs). Inside
`feedback`: `orphan_candidates` suppressed (empty vec). Inside each
`MaterializedChild`: 4 keys (`name`, `outcome`, `state`,
`ready_to_drive`); `role` and `subbatch_status` omitted on worker
children via `skip_serializing_if`.

Gloss: happy-path response has ZERO dead fields. Every key present
is carrying information. 3 fields the failure rounds added --
`reserved_actions`, `errored`, `warnings`, `orphan_candidates` --
all successfully omit themselves on this tick. `ready_to_drive: true`
is the one concession to failure-path semantics that lives visibly
on the happy path, and that one is load-bearing (workers read it
to decide dispatch).

### Turn 4 - `koto next coord.issue-1`

Non-batch child. Advance loop at `working`, gate passes, `status`
evidence missing.

```json
{
  "action": "evidence_required",
  "state": "working",
  "directive": "Implement issue #101. ... When finished, submit {\"status\": \"complete\"}.",
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

Gloss: identical shape to a v0.7.0 non-batch child response, modulo
the single `scheduler: null` sentinel. Child workflows are
untouched by the feature.

### Turn 5 - `koto next coord.issue-1 --with-data '{"status":"complete"}'`

Transitions to terminal `done`.

```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #101 implemented successfully.",
  "is_terminal": true
}
```

Gloss: 4 fields. `batch_final_view` suppressed (no `BatchFinalized`
event on child's log; the child isn't a batch parent). No
`reserved_actions`. No `scheduler`. Identical to a pre-batch
non-coordinator terminal.

### Turn 6 - `koto next coord` (re-tick)

Gate reclassifies issue-1 as terminal-success. Scheduler spawns
issue-2 and issue-3 (their `waits_on: [issue-1]` is now terminal-
success).

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 1, "pending": 2, "success": 1,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
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
      {"name": "coord.issue-1", "outcome": "success", "state": "done", "ready_to_drive": false},
      {"name": "coord.issue-2", "outcome": "pending", "state": "working", "ready_to_drive": true},
      {"name": "coord.issue-3", "outcome": "pending", "state": "working", "ready_to_drive": true}
    ],
    "already": ["coord.issue-1"],
    "blocked": [],
    "skipped": [],
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "already_terminal"},
        "issue-2": {"outcome": "accepted"},
        "issue-3": {"outcome": "accepted"}
      }
    }
  }
}
```

Gloss: 3 entries in `materialized_children`. Terminal child has
`ready_to_drive: false` -- correctly flagging "don't redispatch
this one." Two newly-spawned workers have `ready_to_drive: true`.
No `reserved_actions`, no `warnings`, no `errored`,
no `orphan_candidates`. Agent dispatches issue-2 and issue-3 in
parallel.

### Turn 7 - drive issue-2 and issue-3 (parallel workers)

Each worker runs `koto next coord.issue-N` then
`koto next coord.issue-N --with-data '{"status":"complete"}'`. Both
terminate `done` with 4-field responses identical to Turn 5.

### Turn 8 - `koto next coord` (final tick)

Gate sees 3 terminal-success children. `all_success: true` guard
fires. `plan_and_await -> summarize`. `summarize` is terminal.
Before stopping, `BatchFinalized` appended (first `all_complete:
true` on a state with `materialize_children`). `handle_next`
attaches `batch_final_view`.

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {
      "total": 3, "success": 3, "failed": 0, "skipped": 0,
      "pending": 0, "blocked": 0, "spawn_failed": 0
    },
    "tasks": [
      {"name": "issue-1", "child": "coord.issue-1", "outcome": "success"},
      {"name": "issue-2", "child": "coord.issue-2", "outcome": "success"},
      {"name": "issue-3", "child": "coord.issue-3", "outcome": "success"}
    ],
    "ready": [], "blocked": [], "skipped": [], "failed": []
  }
}
```

Gloss: 5 top-level keys. `reserved_actions` absent (no failure, no
skip). No `scheduler` (suppressed on terminal). `is_terminal: true`.
Each `BatchTaskView` is 3 keys only -- `reason`, `reason_source`,
`synthetic`, `waits_on`, `blocked_by`, `skipped_because`,
`skipped_because_chain`, `spawn_error` all suppressed by
`skip_serializing_if`.

### Turn 9 - `koto status coord` post-terminal

```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": {
    "phase": "final",
    "summary": {
      "total": 3, "success": 3, "failed": 0, "skipped": 0,
      "pending": 0, "blocked": 0, "spawn_failed": 0
    },
    "tasks": [
      {"name": "issue-1", "child": "coord.issue-1", "outcome": "success"},
      {"name": "issue-2", "child": "coord.issue-2", "outcome": "success"},
      {"name": "issue-3", "child": "coord.issue-3", "outcome": "success"}
    ]
  }
}
```

Gloss: Same snapshot as the terminal's `batch_final_view`, but with
the `ready/blocked/skipped/failed` vecs omitted (they're empty and
informational for mid-run; post-terminal they're redundant with
`summary`). 4 top-level keys.

### Cloud-mode delta

Under `CloudBackend`, every response in Turns 1-9 gains two
top-level fields at the tail: `"sync_status": "fresh"` and
`"machine_id": "machine-abc123"`. No other shape changes. Absent
under `LocalBackend`.

---

## Section 2: Findings

### F1 - Response verbosity is PROPORTIONAL to feature

- **Observation**: Happy-path non-terminal tick (Turn 6) has 5
  top-level keys: `action`, `state`, `directive`,
  `blocking_conditions`, `scheduler`. Pre-batch v0.7.0 equivalent
  had 4 (`action`, `state`, `directive`, `blocking_conditions`).
  The one addition is `scheduler`, which IS the feature. No
  gratuitous top-level surface.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F2 - Serde-optional discipline is consistent and working

- **Observation**: On the happy path, the following all
  self-omit via `skip_serializing_if`:
  - `reserved_actions` (top-level, omitted when no failure/skip)
  - `scheduler.errored` (Vec, empty)
  - `scheduler.warnings` (Vec, empty)
  - `scheduler.feedback.orphan_candidates` (Vec, empty)
  - `MaterializedChild.role` (Option, None for workers)
  - `MaterializedChild.subbatch_status` (Option, None for workers)
  - `BatchTaskView.reason`, `reason_source`, `skipped_because`,
    `skipped_because_chain`, `spawn_error`, `waits_on`, `blocked_by`,
    `synthetic` (all absent on success)
  - `done.batch_final_view` (absent on non-batch terminals)
  - `TaskSpawnError.template_source`, `compile_error` (not
    applicable here)
  The design code blocks (lines ~2889-3100, 3368-3392) uniformly
  apply `#[serde(default, skip_serializing_if = ...)]` across
  `Option`, `Vec::is_empty`, and `is_false`. No inconsistent field
  that the happy path leaves mandatory-but-null.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F3 - `MaterializedChild.role` is the right shape for flat batches

- **Observation**: The round-3 design makes `role` an
  `Option<ChildRole>` with `skip_serializing_if = "Option::is_none"`
  where `None` is equivalent to `Worker`. On the happy path here
  (all workers), `role` is ABSENT from every `MaterializedChild`.
  Good: we get the pre-round-3 shape back for flat batches, and
  intermediate coordinators become self-advertising (presence of
  `role: "coordinator"` on exactly the nested-batch children).
  Same story for `subbatch_status`.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change. Prior round 2 pair-b note
  suggested inverting to `absent-when-worker` and that's what the
  revised design did.

### F4 - Happy-path gate output carries 6 aggregate booleans; 4 are always false here

- **Observation**: The gate `output` block carries `all_complete`,
  `all_success`, `any_failed`, `any_skipped`, `any_spawn_failed`,
  `needs_attention`. On a pure-success run, four of those (`any_*`
  + `needs_attention`) are always `false`. They ARE serialized
  (they're plain `bool`, not `Option`). An agent running through
  the walkthrough reads six booleans where two carry routing
  signal.
- **Severity**: nice-to-have. This is a legibility concern, not a
  correctness one. The design explicitly argues (Decision 5.2)
  that the booleans should always be present so template routing
  is uniform.
- **Proposed resolution**: no code change. Consider a one-line
  note in the Reading guide: "The `any_*` and `needs_attention`
  fields are always present; on successful runs they are
  uniformly `false`. Route on `all_success` for the happy path;
  route on `needs_attention` for the failure path." Actually,
  lines 1232-1237 of walkthrough.md already cover this. No
  change needed.

### F5 - `feedback.entries` on happy path duplicates
`spawned_this_tick` + `already` + `blocked` + gate output

- **Observation**: On Turn 3, an agent can learn that issue-1 is
  accepted from:
  1. `scheduler.spawned_this_tick[0]` (observation)
  2. `scheduler.materialized_children[0]` (ledger)
  3. `scheduler.feedback.entries["issue-1"].outcome == "accepted"`
  4. `blocking_conditions[0].output.children[0]` (gate row)
  Similarly, issue-2 being blocked is derivable from
  `scheduler.blocked`, `scheduler.feedback.entries["issue-2"]`, and
  the gate's `children` row. Three-to-four ways to get the same
  fact.
- **Severity**: should-document. The design intends each view for
  a different audience (observation / ledger / per-submission
  feedback / gate verdict), but the walkthrough doesn't name the
  canonical source for a given question.
- **Proposed resolution**: Add a short "which signal answers which
  question" table to walkthrough.md's Reading guide:
  | Question | Canonical source |
  |---|---|
  | "What should I dispatch next?" | `scheduler.materialized_children` filtered on `ready_to_drive` |
  | "What happened to MY submission this tick?" | `scheduler.feedback.entries` |
  | "What just changed this tick?" | `scheduler.spawned_this_tick` |
  | "What is the aggregate batch verdict?" | `blocking_conditions[0].output` (booleans) |
  | "Per-child outcome and reason?" | `blocking_conditions[0].output.children[i]` |
  Prevents agents from picking the wrong source and getting
  stale-ish data during retries.

### F6 - Happy-path walkthrough remains readable WITHOUT reading
failure-path decisions

- **Observation**: A fresh reader of the design doc can work
  through Interactions 1-8 of walkthrough.md and understand the
  feature without touching Decisions 9, 10, 11, 12, 13, or 14
  (the retry/mutation/synthesis/supersede decisions). The only
  round-3 fields visible on the happy path are
  `ready_to_drive` and `scheduler.feedback.entries`, and both are
  adequately introduced by walkthrough.md's Interaction 3 gloss
  (lines 489-499) and Reading guide (lines 1225-1231).
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change. The "simple cases stay
  simple" property holds.

### F7 - `scheduler: null` vs `scheduler` omitted is inconsistent
on terminal responses

- **Observation**: Turn 2, Turn 3, and Turn 6 all carry
  `"scheduler": null` (Turn 2) or a populated `scheduler` block
  (Turns 3, 6). The `done` terminal response in Turn 8 carries NO
  `scheduler` key at all. The design's `handle_next` logic
  (lines 3125-3150) emits `scheduler: null` on non-batch/error
  responses and a populated block on scheduled ticks, but the
  terminal case silently drops the field. Agents parsing for
  `response.scheduler !== undefined` will see a discontinuity at
  the terminal.
- **Severity**: should-fix (minor spec gap). Either always emit
  `"scheduler": null` on terminals, or spec that terminal
  responses omit it and agents should not read it.
- **Proposed resolution**: one-line amendment to Decision 11's
  response envelope section: "Terminal `done` responses omit
  `scheduler` entirely; `batch_final_view` is the canonical
  post-batch surface." Alternative: always emit with
  `scheduler: null` on terminals for uniformity. The former is
  cheaper and already what the implementation would naturally
  do; the latter is slightly more consistent for agent parsers.

### F8 - Terminal `done` response is minimal and well-shaped

- **Observation**: Turn 8's terminal `done` has 5 keys: `action`,
  `state`, `directive`, `is_terminal`, `batch_final_view`. A
  non-batch terminal (Turn 5) has 4 keys: drops
  `batch_final_view`. Both shapes are minimal. `reserved_actions`
  correctly suppressed on a pure-success terminal (no `any_failed`
  / `any_skipped` in the final gate).
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F9 - `koto status` post-terminal: `batch.phase: "final"` is
the only signal distinguishing mid-run from post-terminal

- **Observation**: `koto status coord` returns the same shape
  mid-run and post-terminal, with `phase` as the sole
  discriminator (`"active"` vs `"final"`). On the post-terminal
  response (Turn 9), the `ready/blocked/skipped/failed` name-vec
  fields are omitted (they are empty and would be informational
  only mid-run). This is correct behavior but undocumented in
  walkthrough.md.
- **Severity**: should-document.
- **Proposed resolution**: one-paragraph addition to the Reading
  guide: "On a terminal batch, `koto status` returns
  `batch.phase: \"final\"` and drops the `ready/blocked/skipped/
  failed` name vectors (which only carry meaning mid-run); the
  `summary` counters and `tasks` array remain. The same shape is
  emitted via `batch_final_view` on the terminal `koto next`
  response, so a second call is not required."

### F10 - Cloud-off mode: no shape delta except the two fields

- **Observation**: Turns 1-9 under `LocalBackend` produce the
  same JSON shape as under `CloudBackend` minus `sync_status` and
  `machine_id`. No other fields are gated on backend. Verified by
  inspection of Decisions 11 and 13 code blocks (lines 2889-3100,
  3368-3392); `machine_id` appears only inside
  `SchedulerWarning::StaleTemplateSourceDir`, which is already
  guarded by `skip_serializing_if` per-field.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F11 - Over-engineering audit per round-3 addition

| Field | Happy-path appearance | Justified? |
|---|---|---|
| `ready_to_drive` | Every `MaterializedChild` (true / false) | Yes. Load-bearing for worker filtering. |
| `role` | Absent (suppressed on workers) | Yes. Zero cost on flat batches. |
| `subbatch_status` | Absent | Yes. Zero cost on flat batches. |
| `reserved_actions` | Absent (no failure/skip) | Yes. Failure-path-only by design. |
| `EntryOutcome::Respawning` | Never appears | Yes. Retry-race-only. |
| `scheduler.errored` | Absent (empty vec, suppressed) | Yes. |
| `scheduler.warnings` | Absent (empty vec, suppressed) | Yes. |
| `orphan_candidates` | Absent (empty vec, suppressed) | Yes. |
| `scheduler.feedback.entries` | Populated every submission | Yes. Agent reads per-submission outcome here. Overlaps with other views (see F5) but is the canonical per-submission source. |
| `synthetic: true` on BatchTaskView | Absent on successes (skip_serializing_if = is_false) | Yes. |

No dead fields. No mandatory-but-useless field that never fires on
the happy path yet stays present. Round-3's introduction of the
failure-path surface successfully lives behind serde optionality,
and the happy-path wire shape is effectively the pre-round-3
shape plus `ready_to_drive` (one bool per child) and
`feedback.entries` (one small map per submission).

---

## Summary note

The happy-path wire shape is clean and proportional. The round-3
additions (`ready_to_drive`, `role`, `subbatch_status`,
`reserved_actions`, `Respawning`, `orphan_candidates`) are uniformly
serde-optional and all correctly self-omit on a pure-success run
except `ready_to_drive`, which is load-bearing. The one legibility
concern (F5: three ways to learn the same fact) can be closed with
a one-table addition to the Reading guide naming the canonical
source per question. F7 (terminal responses drop `scheduler`
silently) and F9 (post-terminal `koto status` has an undocumented
field-set delta) are minor documentation/spec gaps. No blockers,
no over-engineering, no dead fields. "Simple cases stay simple"
holds for the revised design.
