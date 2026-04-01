---
name: mixed-routing
version: "1.0"
description: Routes based on both gates.* output and agent evidence in the same when clause
initial_state: check

states:
  check:
    gates:
      lint:
        type: command
        command: "exit 0"
    accepts:
      decision:
        type: enum
        values: [approve, reject]
        required: true
    transitions:
      - target: approved
        when:
          gates.lint.exit_code: 0
          decision: approve
      - target: rejected
        when:
          decision: reject
      - target: done
  approved:
    terminal: true
  rejected:
    terminal: true
  done:
    terminal: true
---

## check

Run lint gate and wait for agent decision. Both must match for the approved route.

## approved

Lint passed and decision approved.

## rejected

Decision rejected.

## done

Fallback complete.
