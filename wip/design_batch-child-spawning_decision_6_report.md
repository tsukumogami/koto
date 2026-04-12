<!-- decision:start id="batch-observability-surface" status="assumed" -->
### Decision: Batch observability surface

**Context**

With batch spawning, a parent workflow has richer state than "a list of children." Observers (humans via CLI, agents like shirabe's work-on-plan consumer) need to see which tasks are declared in the batch, which have been spawned, which are blocked by dependencies, which are ready to run next, which are skipped due to a dependency failure, which are running, and which are done. Today `koto status <parent>` returns only `{name, current_state, template_path, template_hash, is_terminal}` (`src/cli/mod.rs:2373`), and `koto workflows --children <parent>` returns a flat list of `{name, state}` rows via `query_children` (`src/cli/mod.rs:2588`). Neither surface exposes the batch task graph, the ready set, or the dependency edges.

A design driver for this work is "observability through existing commands." Introducing a dedicated `koto batch status` subcommand would technically preserve existing commands but adds a third path that every consumer must discover and document. Since `run_batch_scheduler` (per Decision 6 input from the integration lead) already computes task classification on every `koto next` tick, the implementation question is where to re-expose that same classification from a read-only command path, not whether the computation is feasible.

The known downstream consumer is shirabe's work-on-plan coordinator, which submits a batch of issue-implementation tasks as evidence and polls koto for three answers on its hot path: "is the batch done?", "what's ready to work on next?", and "which tasks failed and why?". All three are aggregate properties of the batch, not of any individual child.

**Assumptions**

- Decision 5 creates real child state files for dependents of failed tasks, transitioning them to a `skipped_due_to_dep_failure` terminal state. If Decision 5 instead chooses a parent-side record, the extended `status` surface becomes the sole place to see skipped tasks — which strengthens the chosen option rather than weakening it.
- `classify_task` and `build_dag` helpers (new, in `src/engine/batch.rs` per the integration lead's section 4) are side-effect-free and can be called from a read-only path in `handle_status` without refactoring their core logic.
- Cloud backend tolerates one additional `backend.list()` plus N child state reads per `koto status <parent>` call, **only for batch-bearing parents**. Non-batch workflows see zero cost increase. Poll cadence is assumed to be on the order of seconds (human or agent), not milliseconds.
- Observers key into documented JSON fields by name. Additive extensions (new optional top-level fields) do not regress existing consumers.

**Chosen: Extend both `koto status` and `koto workflows --children` with batch metadata**

Add an optional `batch` section to `handle_status` output for the aggregate DAG view, and extend each row of `query_children` output with optional per-task fields. Both surfaces call a shared `derive_batch_view` helper that parses the batch definition from the parent's latest `EvidenceSubmitted` event and classifies each declared task using the same logic the scheduler uses during `handle_next`.

**Responsibility split.**

- **`koto status <parent>`** answers "what is the batch doing as a whole." It exposes the full declared task list (including tasks not yet spawned), the ready set, blocked set, summary counts, and failure reasons. This is the primary surface observers poll to drive batch progress.
- **`koto workflows --children <parent>`** answers "what do each of the existing children look like." It enumerates on-disk children (matching current semantics) and attaches per-row batch metadata — `task_name`, `waits_on`, `reason_code`, `reason`, `skip_reason` — so agents iterating over children don't have to re-join child session names back to task definitions.

Both extensions are additive: the new fields are emitted only when the parent's current state has a batch hook, using `#[serde(skip_serializing_if = "Option::is_none")]`. Non-batch workflows see unchanged output. Existing v0.7.0 consumers that key into `current_state`, `is_terminal`, `name`, and `state` continue to work untouched.

### `koto status <parent>` — before and after

**Before (v0.7.0, unchanged for non-batch workflows):**

```json
{
  "name": "p",
  "current_state": "awaiting_children",
  "template_path": "/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/coord.md",
  "template_hash": "abc123",
  "is_terminal": false
}
```

**After (batch parent, new `batch` section):**

```json
{
  "name": "p",
  "current_state": "awaiting_children",
  "template_path": "/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/coord.md",
  "template_hash": "abc123",
  "is_terminal": false,
  "batch": {
    "summary": {
      "total": 5,
      "done": 2,
      "running": 1,
      "ready": 1,
      "blocked": 0,
      "failed": 1,
      "skipped": 0,
      "all_complete": false
    },
    "tasks": [
      {
        "name": "t1",
        "child": "p.t1",
        "state": "done",
        "waits_on": [],
        "status": "terminal"
      },
      {
        "name": "t2",
        "child": "p.t2",
        "state": "done",
        "waits_on": [],
        "status": "terminal"
      },
      {
        "name": "t3",
        "child": "p.t3",
        "state": "review",
        "waits_on": ["t1"],
        "status": "running"
      },
      {
        "name": "t4",
        "child": "p.t4",
        "state": "failed",
        "waits_on": ["t2"],
        "status": "failed",
        "reason_code": "test_failure",
        "reason": "tests did not pass"
      },
      {
        "name": "t5",
        "child": null,
        "state": null,
        "waits_on": ["t3"],
        "status": "ready"
      }
    ],
    "ready":   ["t5"],
    "blocked": [],
    "skipped": [],
    "failed":  ["t4"]
  }
}
```

Per-task `status` is one of `terminal`, `running`, `ready`, `blocked`, `failed`, `skipped`, matching the scheduler's `classify_task` output plus the failure-routing extensions. When a task has not yet been spawned, its `child` and `state` are `null` but the row still appears — this is the critical property that `--children` cannot provide, because `--children` only lists on-disk sessions.

### `koto workflows --children <parent>` — before and after

**Before (v0.7.0, unchanged for non-batch parents):**

```json
[
  {"name": "p.t1", "state": "done"},
  {"name": "p.t2", "state": "done"},
  {"name": "p.t3", "state": "review"},
  {"name": "p.t4", "state": "failed"}
]
```

**After (batch parent, additional per-row fields):**

```json
[
  {
    "name": "p.t1",
    "state": "done",
    "task_name": "t1",
    "waits_on": []
  },
  {
    "name": "p.t2",
    "state": "done",
    "task_name": "t2",
    "waits_on": []
  },
  {
    "name": "p.t3",
    "state": "review",
    "task_name": "t3",
    "waits_on": ["t1"]
  },
  {
    "name": "p.t4",
    "state": "failed",
    "task_name": "t4",
    "waits_on": ["t2"],
    "reason_code": "test_failure",
    "reason": "tests did not pass"
  }
]
```

Note that task `t5` (un-spawned but ready) does **not** appear in the `--children` output because no child state file exists yet — this is intentional and matches `--children`'s current "enumerate on-disk sessions" semantics. Observers who need to see ready-but-un-spawned tasks use `koto status`, which is exactly the split this decision recommends.

When Decision 5 produces skipped-due-to-dep-failure children as real state files, those rows appear naturally:

```json
{
  "name": "p.t6",
  "state": "skipped_due_to_dep_failure",
  "task_name": "t6",
  "waits_on": ["t4"],
  "skip_reason": "dependency_failed",
  "skipped_dependency": "t4"
}
```

### What the shirabe work-on-plan consumer actually observes

On each poll, the consumer runs `koto status work-on-plan-42` once and reads:

1. `is_terminal` — top-level boolean, unchanged from today. "Is the batch done?"
2. `batch.ready` — list of task names whose dependencies are satisfied and which have not yet been spawned. "What's ready to work on next?" (Note: un-spawned ready tasks are auto-spawned by koto on the next `koto next` tick; this field tells the agent which tasks will spawn imminently, which is useful for UX messaging.)
3. `batch.failed` — list of task names that failed, joined with per-task `reason_code` and `reason` in `batch.tasks`. "Which tasks failed and why?"
4. `batch.summary` — aggregate counters for a single-line progress rendering like "3/5 done, 1 running, 1 failed".

When the consumer wants to drill into a specific failure, it calls `koto status p.t4` (per-child status) to get the child's full context, which already works today.

The consumer never needs to call `backend.list()` equivalents, never recomputes the DAG, and never reads child state files directly. One `koto status <parent>` call per poll covers the hot path.

**Rationale**

Four considerations pin the decision:

1. **Discoverability drives adoption.** `koto status` is the command every observer already knows. Surfacing batch state there means documentation is a one-line addition to the existing `status` entry in `koto-user` SKILL.md, not a new section about a new command. Option 3 (dedicated `koto batch status`) would require consumers to branch on "is this a batch parent?" before choosing which command to call — friction with no corresponding benefit.

2. **The ready-set must be visible, and only `status` can show un-spawned tasks.** The work-on-plan consumer's core question is "what's ready to work on next?" Option 2 (extend `--children` only) structurally cannot answer this because `--children` lists sessions, not declared tasks. Any option that omits the `status` extension fails the "don't force observers to duplicate DAG computation" constraint for un-spawned tasks.

3. **The per-child metadata on `--children` is cheap and naturally useful.** Agents that iterate over children to gather per-issue artifacts already call `--children`. Attaching `task_name` and `waits_on` to each row saves them from having to join child session names back to the batch definition themselves. The cost is one additional field lookup per row — the expensive part (`backend.list()` + N child reads) is already paid.

4. **Both extensions share one helper.** `derive_batch_view` is called from both `handle_status` and `query_children`. The logic is written once, tested once, and kept in sync automatically. There's no meaningful incremental cost to extending both surfaces versus extending one.

**Alternatives Considered**

- **Extend only `koto status <parent>` with a `batch` section.** Sufficient for the work-on-plan consumer's hot path, but misses the natural per-row extension on `--children` that agents iterating over children benefit from. Rejected in favor of the combined extension because the incremental cost of also extending `--children` is trivial (shared helper) and the usability gain is concrete.

- **Extend only `koto workflows --children <parent>` with per-row batch metadata.** Rejected. `--children` only lists sessions that exist on disk, so it cannot show un-spawned tasks — the ready set and the blocked-but-dep-satisfied set are invisible. The work-on-plan consumer's "what's ready next?" question is structurally unanswerable from this surface alone.

- **New `koto batch status <parent>` subcommand.** Rejected. Contradicts the "observability through existing commands" design driver without any offsetting benefit. The JSON payload is identical to what Option 1 returns as the `batch` field; moving it under a new subcommand only costs discoverability. New skill documentation would be required in both `koto-user` and any downstream agent skill that polls batch state.

- **Nothing new; observers compute the DAG themselves.** Rejected. Directly violates the "don't force observers to duplicate DAG computation" constraint. Every consumer would re-implement the ready/blocked classification logic that koto already has internally in `src/engine/batch.rs`. Cloud backend sync cost is also worst in this option — observers make N child reads per poll to reconstruct state koto already knows.

**Consequences**

*What changes.*

- `handle_status` in `src/cli/mod.rs:2373` gains a conditional step: after loading the compiled template, check whether the current state has a batch hook. If so, call `derive_batch_view(&backend, &compiled, &events, parent_name)` and attach the result as a `batch` field on the response. Behavior for non-batch workflows is byte-identical to today.
- `query_children` in `src/cli/mod.rs:2588` gains the same conditional step: if the parent has a batch hook, parse the batch definition from the parent's event log, join each child's session name back to its task name, and attach `task_name`, `waits_on`, `reason_code`, `reason`, `skip_reason`, and `skipped_dependency` fields to each row when applicable.
- A new module `src/engine/batch.rs` hosts `derive_batch_view` as a pure read-only function alongside `run_batch_scheduler`. `classify_task` and `build_dag` become shared helpers called from both the scheduler (write path) and the derive helper (read path). No duplicate logic.
- `src/cli/next_types.rs` gains response types for the `batch` section (`BatchView`, `BatchSummary`, `BatchTaskView`) serialized with `#[serde(skip_serializing_if = "Option::is_none")]` on every optional field, consistent with existing `DecisionSummary` conventions.

*What becomes easier.*

- Shirabe's work-on-plan consumer polls one command per cycle (`koto status`) to drive its batch-progress UI.
- Humans debugging a stuck batch run `koto status <parent>` and immediately see which task is blocking progress.
- CI dashboards that want to render a batch's DAG as a diagram get the whole structure — tasks, edges, per-task state — from one JSON call.
- Skill evals for `koto-user` can assert against a single documented response shape.

*What becomes harder.*

- `handle_status` is no longer a pure header read for batch parents; it pays `backend.list()` + N child reads. For local backends this is negligible. For cloud backends, observers polling batch parents in tight loops need to be aware of the cost. Mitigation: document the cost in `koto-user` SKILL.md and suggest human-cadence polling (seconds, not milliseconds).
- The response shape for `koto status` now has a variant (non-batch vs batch). Consumers that want to handle both cases uniformly need to check for `batch` presence. This is the standard optional-field pattern and matches how existing response types handle `alternatives_considered` and similar fields.
- `koto-user` SKILL.md needs a new section documenting the `batch` response shape and the `--children` per-row extensions. This is the "new surface" trigger from the plugin maintenance protocol — the skill update lands in the same PR as the implementation.
- `koto-author` SKILL.md needs a note that templates using a batch hook should expect observers to call `koto status` rather than enumerating children. Minor addition.
<!-- decision:end -->

---

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Extend both koto status and koto workflows --children with batch metadata"
  confidence: "high"
  rationale: >-
    koto status is the most discoverable surface and the only one that can
    expose un-spawned ready tasks; extending --children adds cheap per-row
    metadata for agents iterating over children. Both extensions share one
    derive_batch_view helper, so the incremental cost over extending only
    one is negligible while the observability gain is concrete.
  assumptions:
    - "Decision 5 creates real child state files for skipped dependents in a skipped_due_to_dep_failure terminal state. If instead a parent-side record is used, the status extension becomes the sole place to see skipped tasks, which strengthens rather than weakens this decision."
    - "classify_task and build_dag helpers in src/engine/batch.rs are side-effect-free and callable from a read-only path in handle_status without core refactoring."
    - "Cloud backend tolerates one additional backend.list() plus N child state reads per koto status call on batch parents, given human/agent poll cadence is on the order of seconds."
    - "Observers key into documented JSON fields by name; additive top-level fields do not regress existing consumers."
  rejected:
    - name: "Extend only koto status"
      reason: "Sufficient for the hot path but misses the natural per-row metadata extension on --children that agents iterating over children benefit from. The shared helper makes the incremental cost trivial."
    - name: "Extend only koto workflows --children"
      reason: "Structurally cannot show un-spawned tasks because --children only lists on-disk sessions. The work-on-plan consumer's 'what's ready next?' question is unanswerable from this surface alone."
    - name: "New koto batch status subcommand"
      reason: "Contradicts the 'observability through existing commands' design driver without offsetting benefit. Costs discoverability and requires new skill documentation in both koto-user and downstream agent skills."
    - name: "Do nothing; observers compute the DAG themselves"
      reason: "Directly violates the 'don't force observers to duplicate DAG computation' constraint. Worst cloud backend cost and maximum consumer-side complexity."
  report_file: "wip/design_batch-child-spawning_decision_6_report.md"
```
