---
name: batch-worker
version: "1.0"
description: Implement a single task spawned by a batch coordinator
initial_state: working

variables:
  ISSUE_NUMBER:
    description: Identifier for the task this worker implements
    required: true

states:
  working:
    accepts:
      status:
        type: enum
        values: [complete, blocked, skipped_by_scheduler]
        required: true
      failure_reason:
        type: string
        required: false
    transitions:
      - target: done
        when:
          status: complete
      # The assignment stores the reason where the parent's batch view
      # reads it. It sits on the edge out of the state that takes the
      # evidence: evidence doesn't reach a later state.
      - target: done_blocked
        when:
          status: blocked
        context_assignments:
          failure_reason: "${evidence.failure_reason}"
      # Synthetic edge to satisfy F5 reachability. The scheduler
      # materializes skip markers directly — agents never submit
      # `skipped_by_scheduler`.
      - target: skipped_due_to_dep_failure
        when:
          status: skipped_by_scheduler
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
  skipped_due_to_dep_failure:
    terminal: true
    skipped_marker: true
---

## working

Implement task #{{ISSUE_NUMBER}}. When finished, submit `{"status": "complete"}`. If you hit an unresolvable blocker, submit `{"status": "blocked", "failure_reason": "<one-line cause>"}`.

## done

Task #{{ISSUE_NUMBER}} completed.

## done_blocked

Task #{{ISSUE_NUMBER}} is blocked. The `failure_reason` stored on the way here is the parent's batch-view `reason` for this task, with `reason_source: "failure_reason"`. Had the worker submitted no `failure_reason`, the parent would see the state name instead (`reason_source: "state_name"`).

## skipped_due_to_dep_failure

This task was skipped because dependency `{{skipped_because}}` did not succeed. No action required — the scheduler materialized this child directly into its terminal skip state.
</content>
</invoke>