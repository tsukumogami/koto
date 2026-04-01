---
name: structured-gates
version: "1.0"
description: Demonstrates structured gate output in blocking_conditions; gate always fails
initial_state: check

states:
  check:
    gates:
      ci_check:
        type: command
        command: "exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Run the ci_check gate (always exits 1). Observe the structured output in blocking_conditions.

## done

Structured gate output workflow complete.
