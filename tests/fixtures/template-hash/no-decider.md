---
name: no-decider
version: "1.0"
description: A template with no decider block, pinned to its template_hash
initial_state: gather
variables:
  PLAN_DOC:
    description: Path to the plan document
    required: true
states:
  gather:
    default_action:
      command: "echo gathered"
      capture_stdout_as: GATHERED
    gates:
      notes:
        type: context-exists
        key: notes.md
    transitions:
      - target: review
        when:
          gates.notes.exists: true
      - target: gather
        when:
          gates.notes.exists: false
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Is the plan outline item clear enough to implement?
      ready:
        type: boolean
        required: false
        description: Is the branch ready?
      rationale:
        type: string
        required: false
        description: Why this verdict
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
  done:
    terminal: true
  stopped:
    terminal: true
---

## gather

Gather notes for {{PLAN_DOC}}.

## review

Review the plan item. Captured: {{GATHERED}}.

## done

Done.

## stopped

Stopped.
