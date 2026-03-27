---
name: simple-gates
version: "1.0"
description: Tests gate-with-evidence-fallback behavior
initial_state: start

states:
  start:
    gates:
      check_file:
        type: command
        command: "test -f wip/check.txt"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        required: false
    transitions:
      - target: done
        when:
          status: completed
      - target: done
        when:
          status: override
      - target: done
  done:
    terminal: true
---

## start

Check whether wip/check.txt exists. If the gate passes, auto-advance. If it fails, provide evidence to proceed.

## done

Gate test complete.
