---
name: var-substitution
version: "1.0"
description: Tests variable substitution in gate commands
initial_state: check

variables:
  MY_VAR:
    description: Variable to substitute in gate command
    required: true

states:
  check:
    gates:
      var_gate:
        type: command
        command: "test -f wip/{{MY_VAR}}.txt"
    transitions:
      - target: done
  done:
    terminal: true
---

## check

This state has a gate using {{MY_VAR}} in the command.

## done

Variable substitution test complete.
