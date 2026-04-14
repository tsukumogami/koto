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
          gates.done.all_success: true
      - target: analyze_failures
        when:
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
        when:
          decision: give_up
      - target: summarize
        when:
          decision: acknowledge
  summarize:
    terminal: true
---

## plan_and_await

Read the plan at `{{plan_path}}`. For each task in the plan, build a task entry with `name`, `vars`, and optional `waits_on`. The scheduler uses `default_template: batch-worker.md` when an entry omits `template`.

Submit the task list: `koto next {{SESSION_NAME}} --with-data @tasks.json`.

Then drive every entry the response reports as `materialized_children[*]` with `ready_to_drive: true AND outcome != spawn_failed`. Re-tick the parent after any child completes so the scheduler picks up newly-ready dependents.

<!-- details -->

The `scheduler.feedback.entries` map tells you exactly how every submitted task was handled (`accepted`, `already_running`, `already_terminal_success`, `already_terminal_failure`, `already_skipped`, `blocked`, `errored`, `respawning`). The children-complete gate output routes the parent: `all_success: true` advances to `summarize`, `needs_attention: true` advances to `analyze_failures`. Without a `needs_attention` branch, a failed batch would satisfy `all_complete: true` and slide past the retry window (compile warning W4 catches that footgun).

## analyze_failures

At least one child failed or was skipped. Two recovery paths:

- Retry: copy the `invocation` from `reserved_actions[0]` and run it. The parent re-enters `plan_and_await` and the scheduler respawns the named children.
- Give up or acknowledge: submit `{"decision": "give_up"}` or `{"decision": "acknowledge"}` to route to `summarize` with the batch outcome as-is.

## summarize

Write a summary covering which tasks succeeded, which failed, and why. The `batch_final_view` field on this response carries the full snapshot — no second command needed.
</content>
</invoke>