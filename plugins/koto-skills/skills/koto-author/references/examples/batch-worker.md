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
        values: [complete, blocked]
        required: true
    gates:
      tests:
        type: command
        command: "echo skip"
    transitions:
      - target: done
        when:
          status: complete
      - target: done_blocked
        when:
          status: blocked
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
    accepts:
      failure_reason:
        type: string
        required: true
  skipped_due_to_dep_failure:
    terminal: true
    skipped_marker: true
---

## working

Implement task #{{ISSUE_NUMBER}}. When finished, submit `{"status": "complete"}`. If you hit an unresolvable blocker, submit `{"status": "blocked", "failure_reason": "<one-line cause>"}`.

## done

Task #{{ISSUE_NUMBER}} completed.

## done_blocked

Task #{{ISSUE_NUMBER}} is blocked. The accepted `failure_reason` surfaces to the parent's batch view via `reason_source: "failure_reason"`. Without it, W5 warns at compile time and the parent sees the state name as the reason (`reason_source: "state_name"`).

## skipped_due_to_dep_failure

This task was skipped because dependency `{{skipped_because}}` did not succeed. No action required — the scheduler materialized this child directly into its terminal skip state.
</content>
</invoke>