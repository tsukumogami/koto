# Authoring batch workflows

Batch workflows let a parent template fan out a dynamic list of children, wait for them, and route on aggregate outcomes. The shape is:

- a `tasks` accepts field on the coordinator state,
- a `materialize_children` hook binding that field to a child template, and
- a `children-complete` gate routing on aggregate booleans.

This file covers the template-author surface. For the agent-runner view, see the `koto-user` skill.

## The `materialize_children` hook

Declare it on the coordinator state that accepts the task list:

```yaml
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: batch-worker.md
      failure_policy: skip_dependents
    transitions:
      - target: summarize
        when:
          gates.done.all_success: true
      - target: analyze_failures
        when:
          gates.done.needs_attention: true
```

| Field | Required | Purpose |
|---|---|---|
| `from_field` | Yes | Name of an accepts field on the same state. Must have `type: tasks` and `required: true`. |
| `default_template` | Yes | Path to the child template. Relative paths resolve against the parent template's directory. |
| `failure_policy` | No | `skip_dependents` (default) materializes skip markers for dependents of a failed task. `continue` lets dependents run anyway. |

The task list submitted under `from_field` is an array of entries with this shape:

```json
{
  "tasks": [
    {"name": "task-1", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "task-2", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["task-1"]},
    {"name": "task-3", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["task-1"], "template": "other-worker.md"}
  ]
}
```

The agent submits short names (`task-1`). Children get composed names (`<parent>.<task>`), e.g. `coord.task-1`. `waits_on` references short names. `template` per entry overrides `default_template`.

## Compile rules (E1-E10) and warnings (W1-W5, F5)

The compiler validates `materialize_children` with ten errors and six warnings:

| Rule | Check |
|---|---|
| E1 | `from_field` is non-empty. |
| E2 | `from_field` names a declared accepts field. |
| E3 | Referenced field has `type: tasks`. |
| E4 | Referenced field has `required: true`. |
| E5 | Declaring state is not terminal. |
| E6 | `failure_policy` is `skip_dependents` or `continue`. |
| E7 | State has at least one outgoing transition. |
| E8 | No two states reference the same `from_field`. |
| E9 | `default_template` is non-empty and resolves to a compilable template. |
| E10 | State with `materialize_children` declares a `children-complete` gate. |
| W1 | `children-complete` gate has no routing when clause. |
| W2 | `children-complete.name_filter` does not end with `.`. |
| W3 | Terminal state whose name matches /block\|fail\|error/ lacks `failure: true`. |
| W4 | `materialize_children` state routes only on `all_complete`/`all_success` without handling failures. Route on `needs_attention` for the retry branch. |
| W5 | Terminal state with `failure: true` has no declared path writing the `failure_reason` context key. |
| F5 | Child template referenced by `default_template` has no reachable `skipped_marker: true` terminal state. |

W4 and F5 are the two rules most likely to bite first-time batch authors. W4 prevents silently skipping the retry window; F5 prevents the scheduler from failing to materialize skip markers.

## The `failure_reason` convention (W5)

When a worker enters a terminal state with `failure: true`, the parent's batch view exposes a per-child `reason`. That reason comes from one of three places, in priority order:

1. The `failure_reason` context key written by the terminal state's `accepts` block (this check is what W5 verifies today).
2. A `default_action` that writes `failure_reason`.
3. A `context_assignments` entry on a transition into the terminal state.

Without any of these, the reason falls back to the state name, and the parent sees `reason_source: "state_name"` instead of `reason_source: "failure_reason"`. W5 warns at compile time when no path writing `failure_reason` is declared, so the author notices before the first failed run.

The simplest satisfying pattern is to accept `failure_reason` as an evidence field on the failure state:

```yaml
done_blocked:
  terminal: true
  failure: true
  accepts:
    failure_reason:
      type: string
      required: true
```

The worker submits `{"status": "blocked", "failure_reason": "API quota exhausted"}`, and the parent's batch view renders `reason: "API quota exhausted"` with `reason_source: "failure_reason"`.

## F5: child templates need a `skipped_marker` state

Every child template referenced by a batch coordinator must declare at least one terminal state with `skipped_marker: true`. When a dependency fails and `failure_policy: skip_dependents` (the default) applies, the scheduler materializes the dependent child directly into a skip-marker state — no worker is ever dispatched. F5 enforces that the skip target exists.

Convention: name the state `skipped_due_to_dep_failure` and document the `{{skipped_because}}` variable the synthetic directive interpolates. The directive rendered for a skip-marker child is:

> This task was skipped because dependency `<skipped_because>` did not succeed. No action required.

Agents that read the directive for context see a readable explanation. Agents that want the full blame chain read `skipped_because_chain[-1]` for the root cause.

## Routing on aggregate booleans (W4)

The `children-complete` gate surfaces fifteen output fields: eight counts (`total`, `completed`, `pending`, `success`, `failed`, `skipped`, `blocked`, `spawn_failed`), six aggregate booleans, and the per-child `children[]` array. The booleans route the parent:

| Boolean | Derived from | When to route on it |
|---|---|---|
| `all_complete` | `pending == 0 AND blocked == 0 AND spawn_failed == 0` | "The batch stopped moving." Every branch's first conjunct — but never a route on its own, which is what W4 catches. |
| `needs_attention` | `any_failed OR any_skipped OR any_spawn_failed` | "And here's how it went." The second conjunct that splits clean from dirty. |
| `all_success` | Every child in terminal-success | Equivalent to `all_complete AND NOT needs_attention`. Readable, but see the compile-time constraint below before reaching for it. |
| `any_failed` | At least one failure | Fine-grained routing when `any_skipped` and `any_failed` need different states. |
| `any_skipped` | At least one skipped | Same. |
| `any_spawn_failed` | At least one `spawn_failed` outcome | Fine-grained routing; folded into `needs_attention`. |

The safe default is a two-branch coordinator where **both branches carry both conjuncts**:

```yaml
transitions:
  - target: summarize
    when:
      gates.done.all_complete: true
      gates.done.needs_attention: false
  - target: analyze_failures
    when:
      gates.done.all_complete: true
      gates.done.needs_attention: true
```

Routing only on `all_complete: true` fires W4 — an outright failure still satisfies `all_complete`, so the parent would slide past the retry window into the clean-completion branch. The `needs_attention` conjunct is what silences it.

**Why not the more readable `all_success: true` / `needs_attention: true` pair?**
Because it doesn't compile. Mutual exclusivity is checked syntactically: two
conditional transitions must share at least one `when` field whose values differ.
`all_success` and `needs_attention` share no field, so the compiler rejects the
state with "transitions ... are not mutually exclusive: transitions share no
fields". Repeating `all_complete: true` on both branches supplies the shared
field, and `needs_attention` differing across them supplies the disjoint value.

**Two branches cover every exit, including "still running."** Those two guards
look like they leave a hole: while children are in flight nothing matches. That's
the point. `all_complete` is `false` until the batch stops moving, so the first
conjunct fails on both branches, the tick doesn't advance, and the agent ticks
again. On a coordinator state that accepts a task list, that stop surfaces as
`evidence_required` carrying the gate output; on a gate-only fan-out state with
no `accepts` block, it surfaces as `gate_blocked` with `category: "temporal"`.
The waiting case is that stop, so it doesn't get a transition.

**Never guard a branch on a bare `false`.** While children are pending,
`all_complete`, `all_success`, and `needs_attention` are *all* `false`. A branch
guarded by `needs_attention: false` alone therefore fires while the batch is
still running and summarizes results that don't exist yet. The
`all_complete: true` conjunct is what holds it back.

**Don't fill the apparent hole with a self-loop.** A
`gates.<gate>.all_complete: false` transition back to the coordinator state gets
taken by the engine: it records a same-state move, re-evaluates the same gate on
the next lap of the advance loop, and lands on a state it already visited this
tick — so `koto next` exits 3 with `template_error`, "cycle detected", on every
poll while the children run. Two branches, no third.

## Two-hat coordinators (coordinator-as-child)

A child template can itself declare a `materialize_children` hook. The child then plays two roles at once:

- **Worker** to its parent — its outcome (success / failure / skipped) flows up to the outer batch.
- **Coordinator** of its own sub-batch — its state file contains a `materialize_children` hook on its current state, so `koto next <child>` runs its own scheduler tick.

The outer scheduler marks such children with `role: "coordinator"` in `materialized_children`, and attaches a `subbatch_status` summary (success / failed / skipped / pending counts) so outer-level observers can see inner-batch progress without descending into the child's own `koto status` output.

`batch_final_view` on the outer parent does NOT recursively embed the child's `batch_final_view`. Nested batches stay separate artifacts.

Cross-level retry is rejected in v1. Naming a coordinator child in `retry_failed.children` returns `InvalidRetryReason::ChildIsBatchParent`. Retry at the level where the failure happened, then bubble up.

## Reference templates

The `examples/` directory carries a minimal runnable pair:

- `batch-coordinator.md` — parent with `plan_and_await` / `analyze_failures` / `summarize`. Demonstrates `materialize_children`, routing on aggregate booleans, and the retry path.
- `batch-worker.md` — child with `working` / `done` / `done_blocked` / `skipped_due_to_dep_failure`. Demonstrates `failure: true`, the `failure_reason` accepts path for W5, and the F5 skip-marker state.

Both compile as-is. Use them as a starting skeleton when you add a new batch workflow.
</content>
</invoke>