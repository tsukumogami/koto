---
name: coord
version: "1.0"
description: Coordinate implementation of a plan with dependent tasks
initial_state: plan_and_await

variables:
  plan_path:
    description: Path to the plan document to implement
    required: true

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
          gates.done.all_complete: true
          gates.done.needs_attention: false
      - target: analyze_failures
        when:
          gates.done.all_complete: true
          gates.done.needs_attention: true
  analyze_failures:
    accepts:
      decision:
        type: enum
        values: [give_up, acknowledge]
        required: false
    transitions:
      - target: plan_and_await
        when:
          evidence.retry_failed: present
      - target: summarize
  summarize:
    terminal: true
---

## plan_and_await

Read the plan at `{{plan_path}}`. For each task in the plan, build a task entry with `name`, `vars`, and optional `waits_on`. The scheduler uses `default_template: batch-worker.md` when an entry omits `template`.

Submit the task list: `koto next {{SESSION_NAME}} --with-data @tasks.json`.

Then drive every entry the response reports as `materialized_children[*]` with `ready_to_drive: true AND outcome != spawn_failed`. Re-tick the parent after any child completes so the scheduler picks up newly-ready dependents.

The `children-complete` gate holds in `gate_blocked` (`temporal`) until every non-skipped child's result is in. While `output.outstanding` names children, keep re-ticking `koto next {{SESSION_NAME}}`. When `output.results_in` is `true` the gate passes and each `output.children[]` entry carries a `result`. Read each child's outcome inline — for `coord.task-1` that is `output.children[0].result.status` (e.g. `success`) and `output.children[0].result.summary` — without ticking or querying the child. That converged read feeds the summary you write in `summarize`.

<!-- details -->

The `scheduler.feedback.entries` map tells you exactly how every submitted task was handled (`accepted`, `already_running`, `already_terminal_success`, `already_terminal_failure`, `already_skipped`, `blocked`, `errored`, `respawning`). The children-complete gate output routes the parent. Both branches carry `all_complete: true` and split on `needs_attention`: `false` advances to `summarize`, `true` advances to `analyze_failures`. Without the `needs_attention` conjunct, a failed batch would satisfy `all_complete: true` and slide past the retry window (compile warning W4 catches that footgun); the repeated `all_complete` conjunct is what makes the two branches mutually exclusive at compile time. Those two transitions are the whole routing table — there is no third one for "children still running". While the batch is in flight `all_complete` is `false`, so neither guard matches and the tick stops without advancing, which is exactly the polling behavior described above. Adding an `all_complete: false` self-loop to force polling breaks it: the engine takes the transition, re-evaluates the same gate, revisits the same state within the tick, and `koto next` exits 3 with `template_error`, "cycle detected".

## analyze_failures

At least one child failed or was skipped. Two recovery paths:

- Retry: copy the `invocation` from `reserved_actions[0]` and run it. That submits the reserved `retry_failed` key, which the `evidence.retry_failed: present` branch routes back to `plan_and_await`, and the scheduler respawns the named children.
- Give up or acknowledge: submit `{"decision": "give_up"}` or `{"decision": "acknowledge"}`. Neither matches the retry branch, so the unconditional transition carries the workflow to `summarize` with the batch outcome as-is.

The retry branch is the state's only conditional transition, paired with an
unconditional fallback. That matters: mutual exclusivity is only checked between
*conditional* transitions, and `evidence.retry_failed: present` shares no field
with a `decision` value, so a second conditional branch here would not compile.

## summarize

Write a summary covering which tasks succeeded, which failed, and why. The `batch_final_view` field on this response carries the full snapshot — no second command needed.
</content>
</invoke>