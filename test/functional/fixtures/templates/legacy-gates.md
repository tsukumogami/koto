---
name: legacy-gates
version: "1.0"
description: Template using legacy boolean gate behavior
initial_state: verify

states:
  verify:
    gates:
      ci_check:
        type: command
        command: "true"
    accepts:
      status:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: complete
        when:
          status: done
      - target: complete
  complete:
    terminal: true
---

## verify

Legacy gate: boolean pass/block only. No gates.* routing.

## complete

Done.
