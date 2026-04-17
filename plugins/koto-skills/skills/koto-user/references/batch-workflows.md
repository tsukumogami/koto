# Running batch workflows

Batch workflows have one coordinator (the parent) and many workers (the children). The coordinator submits a task list once, the scheduler spawns children, and workers drive each child's state machine independently. The coordinator re-ticks to observe progress and fire the terminal route.

This file covers the runner surface. For the template-author view, see the `koto-author` skill.

## Partition: coordinator drives parent, workers drive children

One agent (or one thread) holds responsibility for ticking the coordinator via `koto next <parent>`. Other agents (spawned workers) hold responsibility for ticking individual children via `koto next <parent>.<task>`. The two sides do not overlap:

- The coordinator never submits evidence on behalf of a child.
- Workers never re-tick the parent. They drive a child to a terminal state and exit.
- A parent tick is serialized via an advisory flock on the parent's state file. A concurrent `koto next <parent>` returns a `concurrent_tick` error envelope; back off and retry.

The coordinator's job on each tick is: read `scheduler.materialized_children`, figure out which children are new, dispatch workers against those children, and check `blocking_conditions[0].output` for completion.

**Do not manually initialize batch children.** The scheduler owns the child lifecycle: it creates sessions using the composed name `<parent>.<task>`, registers them in the batch tracker, and monitors them for completion. A workflow initialized manually with `koto init <parent>.<task>` is invisible to the batch tracker even if the name matches — the scheduler will spawn a fresh instance on its next tick, discarding the manually-driven work. Always let the coordinator spawn children through the `tasks` submission.

## Submitting the task list

The coordinator's first real tick lands on a state with `action: "evidence_required"` and `expects.fields.tasks.type: "tasks"`. Build the task list and submit it. The `@file` prefix is standard for batch submissions since the payload tends to be large:

```bash
koto next <parent> --with-data @tasks.json
```

Task entry shape (the response's `expects.fields.tasks.item_schema` spells it out):

```json
{
  "tasks": [
    {"name": "task-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "task-2", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["task-1"]},
    {"name": "task-3", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["task-1"]}
  ]
}
```

Short names only; the scheduler composes `<parent>.<name>` for the child workflow. `template` per entry is optional (inherits `default_template` from the hook). `waits_on` is optional (defaults to `[]`).

## `materialized_children`: the dispatch ledger

Every response from a batch-scoped parent tick carries a `scheduler.materialized_children` array. This is the authoritative per-child ledger — derived fresh from disk on every tick:

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.task-1"],
    "materialized_children": [
      {"name": "coord.task-1", "task": "task-1", "outcome": "running", "state": "working", "waits_on": [], "ready_to_drive": true},
      {"name": "coord.task-2", "task": "task-2", "outcome": "blocked", "waits_on": ["task-1"], "ready_to_drive": false},
      {"name": "coord.task-3", "task": "task-3", "outcome": "blocked", "waits_on": ["task-1"], "ready_to_drive": false}
    ]
  }
}
```

**Key: dispatch on the ledger, not on `spawned_this_tick`.** `spawned_this_tick` is a per-tick observation — under concurrent ticks it can report the same child more than once, and on resume it silently drops children spawned in prior process lifetimes. `materialized_children` is recomputed from disk each tick, so it is safe to key idempotent dispatch on.

The canonical worker-dispatch filter:

> Dispatch a worker for every entry in `materialized_children` where `ready_to_drive == true AND outcome != "spawn_failed"`, excluding any child already dispatched this session.

`ready_to_drive: true` requires three conditions together: the child has no spawn error, its `outcome` is `running` (state file exists, non-terminal), and every `waits_on` entry has reached a terminal classification (`success`, `failure`, or `skipped` — any terminal counts). Children with `outcome` of `pending`, `blocked`, or any terminal outcome are never `ready_to_drive`. During retries the scheduler may respawn dependents before their ancestors re-run; those land with `ready_to_drive: false` until the next tick reclassifies them.

`TaskOutcome` values in `materialized_children[*].outcome`:

| Value | Meaning | `ready_to_drive` |
|---|---|---|
| `pending` | Task entry exists but no state file has been written yet (deps satisfied, scheduler is about to spawn or just spawned this tick). | always `false` — no child file for a worker to drive |
| `running` | Child state file exists and is non-terminal. | `true` once every `waits_on` entry is terminal; else `false` |
| `blocked` | No state file; at least one `waits_on` dependency is non-terminal. | always `false` |
| `success` | Child terminal; template does not flag `failure: true` or `skipped_marker: true`. | always `false` (terminal) |
| `failure` | Child terminal with `failure: true`. | always `false` (terminal) |
| `skipped` | Child terminal with `skipped_marker: true` (dependency failed). | always `false` (terminal) |
| `spawn_failed` | Scheduler couldn't create the state file (compile error, collision, I/O). | always `false` |

## `feedback.entries`: what happened to each submitted task

`scheduler.feedback.entries` is keyed by short task name and carries exactly one outcome per submitted entry:

```json
{
  "feedback": {
    "entries": {
      "task-1": {"outcome": "accepted"},
      "task-2": {"outcome": "blocked", "waits_on": ["task-1"]},
      "task-3": {"outcome": "already_terminal_success"}
    },
    "orphan_candidates": []
  }
}
```

Entry outcomes:

| Outcome | Meaning |
|---|---|
| `accepted` | Scheduler spawned (or had already spawned) a matching child. |
| `already_running` | Child state file exists on disk, current state is non-terminal. Not a liveness probe — only a disk assertion. |
| `already_terminal_success` | Child terminal in a success state. |
| `already_terminal_failure` | Child terminal in a state flagged `failure: true`. |
| `already_skipped` | Child terminal in a state flagged `skipped_marker: true`. |
| `blocked` | Deferred — one or more `waits_on` dependencies are non-terminal. Carries a `waits_on` list. |
| `errored` | Spawn failed. Carries a `kind` mirroring `TaskSpawnError.kind`. |
| `respawning` | Target child is mid-respawn this tick (retry path). R8 comparison is vacuous during this window. |

`orphan_candidates` lists children on disk whose short names are NOT in the current submission. Informational — acknowledging or cleaning them up is the agent's responsibility.

## Gate output: the batch aggregate view

`blocking_conditions[0].output` on a `children-complete` gate carries the batch aggregate. The booleans are the load-bearing route signals for the coordinator template:

| Boolean | True when | Use for |
|---|---|---|
| `all_complete` | `pending == 0 AND blocked == 0 AND spawn_failed == 0` | Gate pass signal only. Don't route on this alone — failures satisfy it too. |
| `all_success` | Every child in terminal-success | Clean-completion branch. |
| `needs_attention` | `any_failed OR any_skipped OR any_spawn_failed` | Retry / analyze branch. |
| `any_failed` | ≥ 1 failure | Fine-grained routing. |
| `any_skipped` | ≥ 1 skipped | Fine-grained routing. |
| `any_spawn_failed` | ≥ 1 spawn failure | Fine-grained routing; folded into `needs_attention`. |

Per-child entries in `output.children[]` mirror the data in `materialized_children` but from the gate-observer's perspective. Failed children carry a `reason` string and `reason_source` (one of `failure_reason`, `state_name`, `skipped`, `not_spawned`) so agents can tell where the reason came from.

## `reserved_actions`: the retry-discovery surface

When the aggregate shows `any_failed`, `any_skipped`, or `any_spawn_failed`, the response carries a top-level `reserved_actions` array. Every entry is a ready-to-run retry plan:

```json
{
  "reserved_actions": [
    {
      "action": "retry_failed",
      "label": "Retry failed children",
      "description": "Re-run children whose outcome is failure, skipped, or spawn_failed.",
      "applies_to": ["task-2"],
      "invocation": "koto next 'coord' --with-data '{\"retry_failed\":{\"children\":[\"task-2\"]}}'"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `action` | Canonical action name (`retry_failed` in v1). |
| `label` | Short human-readable label. |
| `description` | One-line summary. |
| `applies_to` | Short task names currently eligible for retry (outcome is `failure`, `skipped`, or `spawn_failed`). |
| `invocation` | POSIX-safe ready-to-run command string. Copy and run as-is. |

Reserved-action evidence bypasses the state's `accepts` validator — it's not `expects.fields` content. Read `reserved_actions` and submit the `invocation` directly; don't try to cram `retry_failed` into a normal evidence submission alongside other keys.

## `retry_failed` mechanics

The `retry_failed` payload shape:

```json
{"retry_failed": {"children": ["task-2", "task-5"], "include_skipped": true}}
```

| Field | Default | Meaning |
|---|---|---|
| `children` | required | Short task names to retry. Each must exist on disk and have outcome `failure`, `skipped`, or `spawn_failed`. |
| `include_skipped` | `true` | When `true`, propagates the retry to skipped dependents automatically. Set `false` to retry only the named children. |

What happens:

1. koto intercepts the submission before the advance loop runs.
2. R10 validates the payload (children exist, are eligible, not batch parents, no mixed evidence).
3. For each named failure, koto appends `Rewound` to the child's log so the next tick re-runs it from the initial state.
4. For each skipped dependent (if `include_skipped`), the scheduler deletes the skip-marker state file and respawns a fresh child against the current task entry.
5. The parent's advance loop resumes and follows the `evidence.retry_failed: present` transition back to the batched state.

Rejection precedence (R10): `UnknownChildren → ChildIsBatchParent → ChildNotEligible → MixedWithOtherEvidence`. `MixedWithOtherEvidence` fires when the submission includes any non-`retry_failed` keys — retry is its own evidence channel.

**Cross-level retry is rejected.** Naming a coordinator child (a child whose template declares a `materialize_children` hook) in `retry_failed.children` returns `InvalidRetryReason::ChildIsBatchParent`. Retry at the level where the failure occurred, then bubble up.

## `sync_status` under the cloud backend

`sync_status` and `machine_id` are **not** emitted on regular `koto next` batch responses. They appear only on `koto session resolve` output when the workflow uses the cloud backend:

```bash
koto session resolve <parent> --children=auto
```

```json
{
  "name": "<parent>",
  "sync_status": "fresh",
  "machine_id": "machine-abc123",
  "children": [ ... ]
}
```

`sync_status` values (as reported by `koto session resolve`):

| Value | Meaning | Action |
|---|---|---|
| `fresh` | Local and remote agree. | Proceed. |
| `stale` | Remote is ahead; local needs to pull before writing. | Pull or let koto resolve on the next call. |
| `local_only` | No remote version exists yet. | Proceed; the first write establishes remote. |
| `diverged` | Local and remote have independent writes since the last common ancestor. | Run `koto session resolve <parent> --children=auto` to reconcile. |

Under the local backend these fields are absent from `koto session resolve` output as well. `koto session resolve` reconciles both the parent log and the per-child state files in one call; run it when a batch is driven across multiple machines and you need to check or reconcile divergence.

## Skip-marker children: `synthetic: true`

When `failure_policy: skip_dependents` fires, the scheduler materializes the dependent child directly into a `skipped_marker: true` state — no worker runs. Calling `koto next <skip-marker-child>` or `koto status <skip-marker-child>` returns a response with `synthetic: true`:

```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "This task was skipped because dependency 'coord.task-1' did not succeed. No action required.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.task-1",
  "skipped_because_chain": ["coord.task-1"]
}
```

`synthetic: true` is computed from the state's `skipped_marker: true` flag — no sidecar file, no template hash. The synthetic directive interpolates `{{skipped_because}}` as the direct upstream blocker; `skipped_because_chain[-1]` is the root cause when skips chain through multiple levels.

**Delete-and-respawn silently drops uncommitted work.** When a `retry_failed` propagates to a skipped dependent, the scheduler deletes the skip-marker state file and respawns the child from scratch. If a worker was mid-driving that child and had not yet committed its evidence, that work is lost without warning. In practice this is not a hazard for skip markers (they have no worker), but the same delete-and-respawn pattern under unusual race conditions can silently drop an in-flight child's uncommitted events. Avoid retrying children whose workers are still actively writing.

## `batch_final_view` on terminal responses

When the parent reaches a terminal state, the response carries `batch_final_view` with the full frozen snapshot:

```json
{
  "action": "done",
  "state": "summarize",
  "is_terminal": true,
  "batch_final_view": {
    "summary": {"total": 3, "success": 3, "failed": 0, "skipped": 0, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "coord.task-1", "task_name": "task-1", "outcome": "success"},
      {"name": "coord.task-2", "task_name": "task-2", "outcome": "success"},
      {"name": "coord.task-3", "task_name": "task-3", "outcome": "success"}
    ]
  }
}
```

The view is frozen the first time the gate reported `all_complete: true` on a `materialize_children` state. Agents writing a summary directive read `batch_final_view` directly — no second `koto status` call.

## Canonical source per question

Multiple surfaces expose batch state. Use the right one for the question you're asking:

| Question | Canonical source |
|---|---|
| "Which children do I dispatch workers against?" | `scheduler.materialized_children` filtered by `ready_to_drive == true AND outcome != "spawn_failed"`. |
| "What happened to each submitted task this tick?" | `scheduler.feedback.entries` (keyed by short task name). |
| "Did this task's spawn fail, and why?" | `scheduler.errored[]` (typed per-task errors) and `materialized_children[*].outcome == "spawn_failed"`. |
| "Did the gate pass / should the parent advance?" | `blocking_conditions[0].output.all_complete` (and aggregate booleans for routing). |
| "What reason should I render for a failed child?" | `blocking_conditions[0].output.children[*].reason` with `reason_source` as the provenance tag. |
| "Which children are eligible for retry, and how do I invoke it?" | `reserved_actions[0].applies_to` and `reserved_actions[0].invocation`. |
| "Is this the final batch outcome?" | `batch_final_view.summary` on the terminal `done` response. |
| "Are there children on disk I forgot about?" | `scheduler.feedback.orphan_candidates`. |
| "Is this child itself a sub-batch coordinator?" | `materialized_children[*].role == "coordinator"` with `subbatch_status` for inner-batch counts. |

`spawned_this_tick`, `already`, `blocked`, `skipped`, `errored` under `scheduler` are per-tick observations, useful for logging and diagnostics. Don't key dispatch or completion logic on them.
</content>
</invoke>