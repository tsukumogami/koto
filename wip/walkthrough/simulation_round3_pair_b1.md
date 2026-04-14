# Simulation — Round 3, Pair B1: Tightened gate + retry-respawn end-to-end

Design under validation: `docs/designs/DESIGN-batch-child-spawning.md`
(round-3 revisions). Round-3 closed the blocker surfaced by Pair B2
(`spawn_failed` could slide through `all_complete: true`). This round
verifies the composite fix:

- **Decision 5.3 gate vocabulary (lines 1148-1153):** `all_complete`
  now requires `pending == 0 AND blocked == 0 AND spawn_failed == 0`;
  `any_spawn_failed` is a first-class derived boolean; `needs_attention`
  folds `any_spawn_failed` in alongside `any_failed` / `any_skipped`.
- **Decision 9 Part 4 (lines 1728-1750):** R10 accepts `spawn_failed`
  children. Retry on a `spawn_failed` task is **retry-respawn** — re-
  attempt `init_state_file` using the CURRENT submission's entry for
  that name. No `Rewound` event is appended (no state file to rewind).
- **Decision 11 (lines 3264-3287):** `InvalidRetryReason` variants are
  unchanged; `ChildNotEligible` still lists current outcomes, but
  `spawn_failed` is now retryable, so it no longer triggers that
  rejection.

The scenario runs Pair B2's 5-task batch (A, B, C valid; D missing
template; E compile-fail) end-to-end through the recovery loop so the
tightened gate and retry-respawn compose on the happy-enough path. `[GAP
N: ...]` markers flag residual gaps.

Grounding: `coord.md` and `impl-issue.md` as defined in
`wip/walkthrough/walkthrough.md` lines 14-63 and 129-170. Parent has
transitions `plan_and_await → summarize` on `all_success: true` and
`plan_and_await → analyze_failures` on `needs_attention: true`.

---

## Section 1: Transcript

### Setup

Templates on disk under `template_source_dir`:

- `impl-issue.md` — valid, the canonical child template.
- `compile-fail.md` — present but YAML frontmatter has an unterminated
  string. Path resolution step (b) hits; template compilation fails.
- `does-not-exist.md` — absent from every configured base.

Parent `coord` was just initialized via
`koto init coord --template coord.md --var plan_path=PLAN.md` and is
parked at `plan_and_await`.

---

### Probe 1 — Initial submission with two bad templates

#### AGENT

```
$ koto next coord --with-data @tasks.v1.json
```

`tasks.v1.json`:

```json
{
  "tasks": [
    {"name": "A", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "B", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "C", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "D", "template": "does-not-exist.md",  "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "E", "template": "compile-fail.md",    "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

_Gloss: 5 independent roots. D's template path is unresolvable
(`TemplateNotFound`). E's file exists but fails to compile
(`TemplateCompileFailed`)._

#### KOTO

Internal order:

1. **Pre-append whole-submission validation.** R0 (non-empty), R3
   (cycles — none), R4 (dangling — none), R5 (duplicates — none),
   R6 (limits — 5 tasks, depth 1), R8 (vacuous; no on-disk children),
   R9 (name charset). **Passes.**
2. **Append `EvidenceSubmitted { tasks: [...], submitter_cwd: ... }`.**
3. **Advance loop at `plan_and_await`.** Gate reads 5 tasks from the
   batch definition; 0 children on disk → `Failed`. `all_complete:
   false` (pending = 5). Neither `all_success` nor `needs_attention`
   guards fire. Loop parks.
4. **Scheduler runs.** Classifies A, B, C, D, E as `Ready` (empty
   `waits_on`). Per-task spawn loop accumulates errors without
   halting:
   - A, B, C → `init_state_file` succeeds. `feedback.entries.{A,B,C}
     = Accepted`.
   - D → `TemplateNotFound` after exhausting `template_source_dir`
     and `submitter_cwd`. `feedback.entries.D = Errored { kind:
     "template_not_found" }`.
   - E → resolves, read succeeds, compile fails on YAML. `feedback.
     entries.E = Errored { kind: "template_compile_failed" }`.
   - Appends `SchedulerRan` event (tick non-trivial: spawned +
     errored).
5. **Response:** `gate_blocked`. Gate output now carries `spawn_failed:
   2` AND `any_spawn_failed: true` AND `needs_attention: true` (per
   round-3 vocabulary). Because `any_spawn_failed: true`,
   `reserved_actions` is synthesized. `applies_to` lists `coord.D`
   and `coord.E`.

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
      "any_spawn_failed": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {
           "task": "D",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/dan/src/tsuku/does-not-exist.md",
             "/home/dan/src/tsuku/does-not-exist.md"
           ],
           "message": "Template not found at any configured base"
         }},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {
           "task": "E",
           "kind": "template_compile_failed",
           "paths_tried": null,
           "message": "YAML parse error: unterminated string at line 3"
         }}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.A", "coord.B", "coord.C"],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working", "ready_to_drive": true},
      {"name": "coord.B", "outcome": "pending", "state": "working", "ready_to_drive": true},
      {"name": "coord.C", "outcome": "pending", "state": "working", "ready_to_drive": true}
    ],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errored": [
      {"task": "D", "kind": "template_not_found",
       "paths_tried": ["/home/dan/src/tsuku/does-not-exist.md",
                       "/home/dan/src/tsuku/does-not-exist.md"],
       "message": "Template not found at any configured base"},
      {"task": "E", "kind": "template_compile_failed",
       "paths_tried": null,
       "message": "YAML parse error: unterminated string at line 3"}
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "accepted"},
        "B": {"outcome": "accepted"},
        "C": {"outcome": "accepted"},
        "D": {"outcome": "errored", "kind": "template_not_found"},
        "E": {"outcome": "errored", "kind": "template_compile_failed"}
      },
      "orphan_candidates": []
    }
  },
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": true},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.D", "coord.E"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.D\"]}}'"
    }
  ]
}
```

AGENT observation pipeline:

```
match response.action:
  "gate_blocked" =>
    // 3 children ready to drive
    for child in scheduler.materialized_children where ready_to_drive: true:
      dispatch(child.name)
    // 2 spawn failures surfaced via reserved_actions[0].applies_to
    //  — recovery path deferred until A/B/C make progress
```

**Probe points confirmed:**

- **(P2) `any_spawn_failed: true` in gate output alongside `any_failed`
  / `any_skipped` / `needs_attention`.** ✓
- **(P5) `needs_attention: true` WITHOUT `any_failed`/`any_skipped`.**
  Folded in via `any_spawn_failed > 0` per the round-3 definition
  (design line 1153).
- **(P6) Recovery path is discoverable from response.** `reserved_
  actions[0].invocation` gives a ready-to-run retry string, and
  `applies_to` names the two spawn_failed children. Agent does not
  need to guess the payload shape.

---

### Probe 2 — Drive A, B, C to success; re-tick parent

AGENT drives each healthy child:

```
koto next coord.A
koto next coord.A --with-data '{"status": "complete"}'
# (analogous for B and C)
```

Each returns `{"action": "done", "state": "done", "is_terminal": true}`.

Then:

```
$ koto next coord
```

KOTO:

1. Advance loop at `plan_and_await`. Gate re-evaluates: A, B, C are
   terminal-success; D, E are still absent on disk (never spawned).
   - Per-child outcomes: A/B/C = `success`; D/E = `spawn_failed`
     (the scheduler reads the batch definition from parent evidence,
     sees no state file for D or E, and re-derives `spawn_failed`
     using the recorded errors from the previous tick's feedback —
     strictly, the gate's classification of an un-spawned task with
     a past spawn error is `spawn_failed`).
   - Aggregates: `total=5, completed=3, success=3, failed=0,
     skipped=0, blocked=0, pending=0, spawn_failed=2`.
   - **Round-3 `all_complete`:** `pending(0) == 0 AND blocked(0) == 0
     AND spawn_failed(2) == 0` → **false.** (Under the pre-round-3
     definition, this would have been `true` and routed to
     `summarize`. This is exactly the blocker Pair B2 surfaced.)
   - `all_success: false`, `any_spawn_failed: true`, `needs_attention:
     true`.
2. Transition `plan_and_await → analyze_failures` fires on
   `needs_attention: true`. First pass with `all_complete: true`
   hasn't been reached, so **no `BatchFinalized` event is appended
   yet** (design line 1391: appended "the moment
   `children-complete` first evaluates `all_complete: true`").
3. Scheduler runs at `analyze_failures`; no `materialize_children`
   hook → `NoBatch`. No additional spawn attempt for D/E this
   tick. `[GAP R3-1: The scheduler at `analyze_failures` has no
   hook, so even though the parent is now at the state where
   recovery happens, D and E are not re-attempted automatically.
   This is by design — retry is agent-driven — but worth noting
   because an agent that naively re-ticks in a loop will NOT
   converge. The agent must submit `retry_failed` to make progress.
   The `reserved_actions` + directive surface this clearly, so the
   gap is purely ergonomic, not a correctness issue.]`
4. `reserved_actions` re-synthesized because `any_spawn_failed:
   true`. `applies_to` is now just `["coord.D", "coord.E"]`.

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
      "total": 5,
      "completed": 3,
      "pending": 0,
      "success": 3,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 2,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {"task": "D", "kind": "template_not_found", "paths_tried": [...], "message": "..."}},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {"task": "E", "kind": "template_compile_failed", "paths_tried": null, "message": "..."}}
      ]
    }
  }],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": true},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.D", "coord.E"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.D\"]}}'"
    }
  ],
  "scheduler": null
}
```

**Probe points confirmed:**

- **(P1) Tightened `all_complete`.** With A/B/C at `success` and D/E
  stuck at `spawn_failed`, `all_complete` is **false**. The parent
  does NOT slide into `summarize`.
- **(P5) `needs_attention` routing.** With `spawn_failed = 2` and
  neither `failed` nor `skipped`, the parent transitioned to
  `analyze_failures`. The reference `coord.md` transition
  `when: gates.done.needs_attention: true` matches because
  `needs_attention` folds `any_spawn_failed` in.

---

### Probe 3 — First recovery attempt: retry without fixing the template

AGENT, having read `reserved_actions[0].invocation`, tries the
obvious thing first — retry D (maybe it was transient?).

```
$ koto next coord --with-data '{"retry_failed": {"children": ["coord.D"]}}'
```

#### KOTO

1. `handle_retry_failed` intercepts before `advance_until_stop`.
2. **R10 validation.** `children: ["coord.D"]` non-empty. `coord.D`
   exists in the declared task set (evidence has it). `coord.D` does
   NOT exist on disk — it's a `spawn_failed` row. Round-3 R10 accepts
   `failure`, `skipped`, AND `spawn_failed` (design line 1731,
   "each named child exists on disk with outcome `failure`, `skipped`,
   or `spawn_failed`"). **`[GAP R3-2: The design text says "exists on
   disk with outcome spawn_failed". But a `spawn_failed` child by
   definition has NO state file on disk — that is exactly why the
   spawn errored. The "exists on disk" phrasing from the
   failure/skipped case does not transfer cleanly. The validator
   must instead check "exists in the declared task set AND the gate's
   current row has outcome spawn_failed", which is derived from
   `parent_events` + `backend.list()`, not from a child state file.
   The design text needs this carve-out to not contradict itself.]`**
3. **Retry-respawn mechanism** (Decision 9 Part 4):
   - Append `EvidenceSubmitted { retry_failed: {"children":
     ["coord.D"]} }` to parent log.
   - Append the clearing `EvidenceSubmitted { retry_failed: null }`.
   - **No `Rewound` event.** There is no child state file to rewind.
     The retry's effect is to flag D for re-spawn on the next tick.
   - Control returns to advance loop. `evidence.retry_failed: present`
     matcher fires (evaluated on un-merged submission payload);
     parent transitions `analyze_failures → plan_and_await`.
4. **Scheduler re-runs at `plan_and_await`.**
   - A, B, C: terminal-success → `already_terminal`.
   - D: classified `NotYetSpawned` + flagged for re-spawn. Per-task
     spawn loop re-attempts `init_state_file` using the CURRENT
     submission's entry for D (from the un-modified `tasks.v1.json`
     submission — D still points at `does-not-exist.md`).
   - `init_state_file` fails again with the same
     `TemplateNotFound`. New `TaskSpawnError` appended to the tick's
     `errored` vector; `feedback.entries.D = Errored { kind:
     "template_not_found" }`.
   - E: not named in the retry set. Its row stays `spawn_failed`.
     Feedback for E is `AlreadyTerminal`? No — E has no state file
     either. `[GAP R3-3: `SchedulerFeedback::EntryOutcome` doesn't
     have a variant for "submitted task, still spawn_failed from a
     prior tick, not named in this retry". The enum variants
     `Accepted, AlreadyRunning, AlreadySkipped, AlreadyTerminal,
     Blocked, Errored, Respawning` don't cover a persistent
     un-retried spawn_failed. The cleanest fit is to re-emit
     `Errored` each tick with the cached/last error, but that
     re-floods `scheduler.errored` and log. The design needs an
     explicit answer: does E appear in this tick's feedback at all,
     or only in the gate output?]`
   - `SchedulerRan` appended (errored vec non-empty).
5. Advance loop re-evaluates gate at `plan_and_await`.
   `all_success: false`, `needs_attention: true`. Transitions back
   to `analyze_failures`. This mirrors Pair B2's GAP 5 observation:
   repeated ticks on a persistent spawn failure flood the log with
   SchedulerRan + Transitioned events.

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5,
      "completed": 3,
      "pending": 0,
      "success": 3,
      "failed": 0,
      "skipped": 0,
      "blocked": 0,
      "spawn_failed": 2,
      "all_complete": false,
      "all_success": false,
      "any_failed": false,
      "any_skipped": false,
      "any_spawn_failed": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {"task": "D", "kind": "template_not_found",
                         "paths_tried": [...],
                         "message": "Template not found at any configured base"}},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {"task": "E", "kind": "template_compile_failed",
                         "paths_tried": null,
                         "message": "YAML parse error: unterminated string at line 3"}}
      ]
    }
  }],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": true},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.D", "coord.E"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.D\"]}}'"
    }
  ],
  "scheduler": null
}
```

**Probe points confirmed:**

- **(P3) Retry-respawn semantics.** Retry on a `spawn_failed` child
  re-attempted `init_state_file` using the CURRENT submission entry
  (which was still bad → same error). **No `Rewound` event appended
  to D** (it has no state file). The error re-appeared in
  `scheduler.errored` and D's gate row kept `outcome: spawn_failed`.

**Residual observation:** `[GAP R3-4: The response does not tell the
agent "this is the second time this spawn has failed" — there's no
retry counter on `TaskSpawnError` and no `retry_count` field on the
gate row. An agent that retried without fixing the template gets a
response identical to Probe 2 (modulo the now-extra SchedulerRan
events in the log). The agent cannot distinguish "nothing happened"
from "retry ran and failed again with the same error". `retry_count`
or an epoch on the gate row would close this.]`

---

### Probe 4 — Second recovery attempt: agent tries to update D's template first

AGENT reasons: "The retry didn't help because I didn't fix D's entry.
I need to submit an updated task list with D pointing at a valid
template." Agent assembles `tasks.v2.json`:

```json
{
  "tasks": [
    {"name": "A", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "B", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "2"}},
    {"name": "C", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "3"}},
    {"name": "D", "template": "impl-issue.md",      "vars": {"ISSUE_NUMBER": "4"}},
    {"name": "E", "template": "compile-fail.md",    "vars": {"ISSUE_NUMBER": "5"}}
  ]
}
```

Agent submits:

```
$ koto next coord --with-data @tasks.v2.json
```

#### KOTO

Parent is parked at `analyze_failures`. The `analyze_failures` state's
`accepts` block only declares `decision` (enum). The `tasks` field is
declared on `plan_and_await`, not on `analyze_failures`.

Per the advance loop's evidence validator (pre-append):

- Evidence payload has key `tasks`.
- `analyze_failures.accepts` has no `tasks` field.
- `deny_unknown_fields` → rejection with `invalid_submission`.

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Field 'tasks' is not accepted at state 'analyze_failures'. Accepted fields: decision.",
    "details": [{"field": "tasks", "reason": "unknown_field"}]
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Probe point (P4) confirmed:** the agent **cannot** update D's
spawn_failed entry directly from `analyze_failures`. The walkthrough
reading guide (lines 1242-1247) is explicit: "`tasks` evidence is not
submittable at `analyze_failures`. The parent template's `accepts`
schema for `tasks` lives on `plan_and_await`. Agents cannot submit a
new `tasks` list while parked at `analyze_failures`; they must first
retry (routing back to `plan_and_await`) or submit a `decision` to
route to `summarize`. Dynamic task additions require returning to the
batched state first."

This is **Observation R3-A** (the "crux" the prompt called out):

> The agent's recovery flow for a `spawn_failed` child that needs a
> corrected task entry is: (1) submit any `retry_failed` on the
> spawn_failed child to route the parent back to `plan_and_await`;
> (2) submit the updated `tasks` list (last-write-wins on un-spawned
> D); (3) re-tick the parent, which will spawn D with the corrected
> entry.

The key insight: **step (1) is load-bearing. The retry itself
doesn't need to succeed — it only needs to fire the template
transition `evidence.retry_failed: present` back to `plan_and_await`
so that step (2)'s `tasks` submission lands on a state that accepts
it.**

`[GAP R3-5: This recovery sequence is non-obvious. The walkthrough
reading guide documents it in one paragraph, but `reserved_actions`
does not explain it. An agent reading only the response sees
`applies_to: ["coord.D", "coord.E"]` and a retry invocation, but no
hint that "retry first, then update the task list, then re-tick" is
the recipe for a template-correction case. Closing this gap: either
(a) expand `reserved_actions[0].description` to mention the pattern,
or (b) accept `tasks` evidence at `analyze_failures` for un-spawned
tasks only (narrow exception), or (c) introduce a reserved
`update_tasks` action for spawn_failed children. Simplest is (a).]`

---

### Probe 5 — Agent executes the recovery recipe

AGENT reverts to:

```
$ koto next coord --with-data '{"retry_failed": {"children": ["coord.D"]}}'
```

This is **the same submission as Probe 3**. It re-attempts spawn with
the still-bad D entry, fails again with `TemplateNotFound`, re-routes
parent back to `analyze_failures` via `needs_attention: true`. No
progress on D. Response shape identical to Probe 3.

AGENT sees response and reasons: "Retry fired, but my updated task
list wasn't recorded because I was at `analyze_failures` when I tried
earlier. Let me retry again, then submit the tasks list while I'm
briefly at `plan_and_await` — but the advance loop transitions back
to `analyze_failures` atomically, so I can never observe the
`plan_and_await` state between the retry and the re-tick."

This is **Observation R3-B** (a subtle sequencing issue):

> `retry_failed` on `analyze_failures` fires the transition back to
> `plan_and_await`, but the advance loop immediately re-runs the
> scheduler, gate, and re-evaluates transitions. Because
> `needs_attention` is still true (D still `spawn_failed`, now with
> a fresh error), the parent transitions right back to
> `analyze_failures` **in the same tick**. The agent never gets a
> response from `plan_and_await` to submit an updated `tasks`
> payload against.

Two sub-options for the agent:

**Option 5a — Submit retry + updated tasks in sequence, relying on
advance-loop semantics.** The agent cannot batch these: per Decision
9 Part 4, mixed `retry_failed` + `tasks` submissions reject with
`InvalidRetryReason::MixedWithOtherEvidence`. So they must be serial.

But if the retry always returns to `analyze_failures`, the agent
can't submit `tasks` to a `plan_and_await` state that's only
transiently visited. **Unless** the advance loop is somehow paused,
or the `tasks` submission is valid at the moment `plan_and_await`
is entered during the retry handler (before the advance loop's next
pass). This is a design ambiguity.

`[GAP R3-6: The advance loop's re-entry after `handle_retry_failed`
appears to run in the same `koto next` invocation. If so, the agent
has no "window" to submit `tasks` at `plan_and_await` between the
retry-induced transition and the subsequent re-evaluation. The
recovery recipe in the walkthrough reading guide (line 1244) says
"submit a decision to route to summarize" OR "retry". It does NOT
describe a path to submit a corrected `tasks` list for a
spawn_failed child. The ONLY way the agent can land a `tasks`
correction for D is if the advance loop, after firing the
`evidence.retry_failed: present` transition, stops at `plan_and_
await` WITHOUT immediately transitioning back to `analyze_failures`
on the same tick. This requires the scheduler's re-spawn attempt
for D to either (a) succeed on this tick (reading the existing
task list — which has the bad D entry), which it won't, or (b) be
deferred to the next tick. The design as written seems to have a
gap here.]`

**Option 5b — Submit a `decision` evidence to route to `summarize`,
accepting the failure.** `summarize` is terminal; the batch ends
with D and E unspawned. The agent has to abandon the two tasks that
can't spawn. This is the graceful-but-incomplete exit.

**Option 5c (the one that actually works) — Two separate `koto next`
calls.** After the retry fires in Probe 5, the response puts the
parent at `analyze_failures`. The agent then realizes: the retry
caused the parent's log to append the `evidence.retry_failed`
transition BACK to `plan_and_await`, but `needs_attention` made it
immediately transition forward again. The agent needs the scheduler
to re-attempt D's spawn from the UPDATED entry — which requires the
entry to be updated BEFORE the retry-triggered respawn runs.

The only construction that composes is:

1. Submit `retry_failed` on a sentinel task (any non-spawn_failed
   retryable child). But there are none — A, B, C are all success
   (ChildNotEligible), no failures, no skips. **The retry primitive
   has no eligible sentinel to ride.**

2. Submit a `decision` to route to `summarize`. Terminal. D and E
   are abandoned with `outcome: spawn_failed`. `batch_final_view`
   carries the final snapshot showing 3 successes and 2
   spawn_failures.

**The architectural conclusion of Probe 5:** in a pure-spawn_failed
scenario (no failures, no skips), there is no v1 primitive for the
agent to update the spawn_failed task's entry. The agent can retry
(same outcome), or acknowledge/give_up (abandon D and E).

This is **GAP R3-7**, an architectural gap that round-3 did NOT
fully close despite loosening R10:

`[GAP R3-7 (architectural): Round-3's R10 loosening accepts
spawn_failed for retry_failed, making retry-respawn valid. But
retry-respawn uses the CURRENT submission entry — and the design
provides no path for the agent to UPDATE that entry when the parent
is parked at `analyze_failures`. Recovery from a spawn_failed task
requiring a template change is blocked in v1 unless:
(a) `analyze_failures` accepts `tasks` evidence for un-spawned
    tasks only (the walkthrough explicitly forbids this); OR
(b) the retry primitive transitions to `plan_and_await` and then
    STOPS before re-evaluating the gate, letting the agent submit a
    fresh `tasks` payload; OR
(c) a new `update_tasks` reserved action is introduced that mutates
    un-spawned entries without routing state.
Round-3's narrow fix (tighten all_complete + allow retry on
spawn_failed) closes the "slide through to success" bug but does
NOT close the "agent cannot repair a spawn_failed child" problem.]`

---

### Probe 6 — The fictional happy path (assume agent somehow lands at `plan_and_await`)

For completeness, assume by some mechanism the agent DID get a
`plan_and_await` response. AGENT submits `tasks.v2.json` (D now
pointing at `impl-issue.md`, E still bad):

```
$ koto next coord --with-data @tasks.v2.json
```

#### KOTO

1. **Pre-append validation.**
   - R3/R4/R5/R6/R9: pass.
   - **R8:** Iterate over submitted entries. Check for each task:
     if child already exists on disk, `spawn_entry` must match.
     - A, B, C: exist on disk. `spawn_entry` on each WorkflowInitialized
       event matches the submitted entry → pass.
     - D: NO on-disk child (last tick's retry-respawn failed again,
       no state file). R8 comparison is **vacuous** for D; the
       entry is subject to last-write-wins.
     - E: NO on-disk child. Same logic — vacuous, last-write-wins.
   - R0: pass.
   - **Validation passes.**
2. Append `EvidenceSubmitted { tasks: [...] }`. The batch definition
   is now the union: A, B, C (spawned, immutable), D (updated entry,
   impl-issue.md), E (unchanged entry, compile-fail.md).
3. Gate re-eval. D and E still `spawn_failed` from prior tick — the
   gate reads current disk state, which still shows no state file
   for either. `all_complete: false`. Parks.
4. Scheduler runs. D is classified `NotYetSpawned` (no on-disk
   child) + has spawn_error history → attempts re-spawn. The entry
   now points at `impl-issue.md` → resolves, compiles,
   `init_state_file` succeeds. `feedback.entries.D = Accepted`.
   `spawned_this_tick: ["coord.D"]`.
   E: re-attempted with the UNCHANGED bad template. Fails again.
   `feedback.entries.E = Errored { kind: "template_compile_failed" }`.
   `scheduler.errored` carries E.
5. Response: `gate_blocked`. `children` now shows A/B/C success, D
   pending/working, E still spawn_failed. `spawn_failed: 1`,
   `pending: 1`, `any_spawn_failed: true`, `needs_attention: true`.
   Parent routes back through the analyze_failures loop when the
   agent re-ticks.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
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
      "any_spawn_failed": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.D", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "spawn_failed",
         "reason_source": "not_spawned",
         "spawn_error": {"task": "E", "kind": "template_compile_failed", "paths_tried": null,
                         "message": "YAML parse error: unterminated string at line 3"}}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.D"],
    "materialized_children": [
      {"name": "coord.A", "outcome": "success", "state": "done", "ready_to_drive": false},
      {"name": "coord.B", "outcome": "success", "state": "done", "ready_to_drive": false},
      {"name": "coord.C", "outcome": "success", "state": "done", "ready_to_drive": false},
      {"name": "coord.D", "outcome": "pending", "state": "working", "ready_to_drive": true}
    ],
    "already": ["coord.A", "coord.B", "coord.C"],
    "blocked": [],
    "skipped": [],
    "errored": [
      {"task": "E", "kind": "template_compile_failed", "paths_tried": null,
       "message": "YAML parse error: unterminated string at line 3"}
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "already_terminal"},
        "B": {"outcome": "already_terminal"},
        "C": {"outcome": "already_terminal"},
        "D": {"outcome": "accepted"},
        "E": {"outcome": "errored", "kind": "template_compile_failed"}
      },
      "orphan_candidates": []
    }
  }
}
```

**This probe's contribution:** confirms that IF the agent can update
D's entry at `plan_and_await`, last-write-wins + retry-respawn
together produce the intended recovery. The tightened `all_complete`
continues to hold the parent back from terminal until E is also
resolved. The design composes correctly **when the agent can reach
`plan_and_await` with a pending `tasks` submission**, which is the
sticking point from Probe 5.

---

## Section 2: Findings

### What round-3 fixed (confirmed)

**(P1) Tightened `all_complete`.** Probe 2 confirmed: a 5-task batch
with 3 successes and 2 spawn_failures yields `all_complete: false`.
The parent does NOT slide into `summarize`. This closes Pair B2's
primary blocker (GAP 4 in that round).

**(P2) `any_spawn_failed` boolean.** Every probe's gate output
carries `any_spawn_failed` alongside `any_failed`, `any_skipped`,
`needs_attention`. Design line 1152 is honored.

**(P3) Retry-respawn semantics.** Probe 3 confirmed: retry on a
`spawn_failed` child re-attempts `init_state_file` using the current
submission entry, appends NO `Rewound` event, and re-surfaces the
same `TaskSpawnError` in `scheduler.errored` when the underlying
template is still broken.

**(P5) `needs_attention` routing.** Probe 2 confirmed: with
`spawn_failed > 0` and no failures/skips, the parent transitions to
`analyze_failures` via `needs_attention: true`. The reference
`coord.md` guard matches the round-3 gate vocabulary.

### What round-3 did NOT fix (residual gaps)

**GAP R3-1 (ergonomic).** An agent re-ticking at `analyze_failures`
will NOT see D or E re-spawned automatically. Retry is
agent-initiated. Clear from the directive + reserved_actions,
so it's a convention gap, not a correctness one.

**GAP R3-2 (spec text).** Design line 1731 says retry_failed requires
the child to "exist on disk with outcome spawn_failed". A
spawn_failed child has no state file. The check is actually against
the parent's batch definition + gate row, not on-disk state. Text
needs tightening.

**GAP R3-3 (feedback enum).** `SchedulerFeedback::EntryOutcome` has
no variant for "persistent un-retried spawn_failed". E in Probe 3
has no clean feedback slot. Options: (a) re-emit `Errored` each tick
with cached error (floods log), (b) introduce
`PersistentSpawnFailed` variant, (c) omit from feedback entirely.

**GAP R3-4 (diagnostic).** No retry counter on `TaskSpawnError` or
the gate row. An agent that retries without fixing the underlying
cause sees a response that looks identical to the prior tick. A
per-row `retry_count` or `attempt_epoch` would close this.

**GAP R3-5 (discoverability).** `reserved_actions[0].description`
does not explain the recipe for correcting a spawn_failed entry's
template. The walkthrough reading guide documents it (lines 1242-
1247), but the response alone is insufficient. Agents without the
reading guide will try the obvious retry, fail, and have no hint
about the full path.

**GAP R3-6 (sequencing).** The advance loop's behavior after
`handle_retry_failed` is load-bearing for the recovery flow. If the
loop immediately re-evaluates the gate after the retry-induced
transition to `plan_and_await`, `needs_attention: true` will fire
and bounce the parent right back to `analyze_failures` — WITHIN
THE SAME TICK. The agent never gets a response from `plan_and_
await` to submit an updated `tasks` payload against. The design
does not specify whether a stop-at-plan_and_await window exists.

**GAP R3-7 (architectural, compound).** In a pure-spawn_failed
scenario (no `failure`, no `skipped` children — only spawn errors),
the agent **cannot update** the spawn_failed task's template. The
retry primitive uses the current submission entry; the `tasks`
accepts declaration is only on `plan_and_await`; and the advance
loop atomically re-routes to `analyze_failures` on each tick. The
only v1 terminal paths are:
(a) retry repeatedly against the same bad entry — never converges;
(b) submit `decision: give_up` or `decision: acknowledge` — routes
    to `summarize` with D and E permanently spawn_failed in
    `batch_final_view`.

The tighter `all_complete` ensures the batch doesn't falsely report
success, but it also ensures the batch can NEVER complete cleanly
when a spawn_failed template requires a task-entry correction.
This is a tradeoff not explicitly documented in Decision 9 Part 4's
alternatives discussion.

### Observations

**Observation R3-A — Recovery recipe is a three-step dance.** For a
spawn_failed child requiring a template change, the agent would need:
(1) retry to route parent to `plan_and_await`, (2) submit updated
`tasks` at `plan_and_await`, (3) re-tick. Step (2) is where the
design's gap lives (see R3-6, R3-7).

**Observation R3-B — The retry primitive is binary, not
idempotent-with-mutation.** `retry_failed` accepts a children list
only. It cannot carry an updated task entry. Decision 9 Part 4 was
explicit about this (mixed payloads reject). The design's separation
of concerns is intentional; the cost is the R3-7 recovery gap.

**Observation R3-C — `reserved_actions[0].applies_to` matches.** At
both Probe 1 (before any child terminal) and Probe 2 (after A/B/C
success), `applies_to` correctly lists only `coord.D` and `coord.E`.
Successful children are excluded. This is correct per design line 1737
("Retries on running or successful children reject with ChildNotEligible").

### Points the round-3 design got exactly right under probing

- **Agent UX on the initial response (P6 confirmed).** Probe 1's
  response gives the agent everything needed to triage: `children`
  array surfaces the 3 pending + 2 spawn_failed cleanly; `spawn_error`
  per-row carries structured `kind` and `paths_tried`;
  `reserved_actions[0]` hands over the retry invocation. The agent
  does not need to guess that `retry_failed` applies to
  spawn_failures — `applies_to` names them.
- **Tightened `all_complete` is the minimum necessary semantic fix.**
  Any looser definition of "complete" would either (a) hide
  spawn_failed behind a success route (Pair B2's bug), or
  (b) require a dedicated `batch_terminal` state on every parent
  template. The tightening composes without template changes.
- **`needs_attention` folding in `any_spawn_failed` is the minimum
  necessary routing fix.** Existing templates routing on
  `needs_attention: true` automatically pick up spawn-failure
  coverage. No template author migration required.

### Recommended follow-ups

These are the candidate round-4 items, in priority order:

1. **Close GAP R3-7** (architectural). Pick one of:
   (a) Allow `tasks` evidence at `analyze_failures`, restricted to
       un-spawned task entries (spawn_failed or new names).
   (b) Introduce an `update_tasks` reserved action for un-spawned
       entries.
   (c) Document `decision: give_up` as the only v1 exit for
       unfixable spawn_failed, and accept the limitation.
2. **Close GAP R3-6** (sequencing). Specify the advance-loop
   behavior after a retry-induced transition: either "stop at
   plan_and_await for one tick" or "continue if needs_attention
   fires." Document in Decision 9 Part 2.
3. **Close GAP R3-5** (discoverability). Expand
   `reserved_actions[0].description` when any of the retryable
   children carry `spawn_error` with a task-entry cause (wrong
   template path, bad vars). Mention the "retry first, then fix
   entry" recipe.
4. **Close GAP R3-4** (diagnostic). Add `retry_count` or
   `attempt_epoch` to the per-row gate output for spawn_failed
   children.
5. **Close GAPs R3-2 and R3-3** (spec text + feedback enum).
   Editorial.

Round-3's composite fix closes the hole Pair B2 surfaced (`all_
complete` no longer fires on spawn_failed) and provides a retry path
for spawn_failed children. It does NOT provide a template-correction
path for spawn_failed children that requires a new task-list
submission. That is the remaining architectural surface for round 4.
