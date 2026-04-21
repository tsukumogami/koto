---
name: skip-if-gate
version: "1.0"
description: Gate-backed skip_if fires when command gate passes
initial_state: check

states:
  check:
    gates:
      gate_check:
        type: command
        command: "test -f wip/flag.txt"
    skip_if:
      gates.gate_check.exit_code: 0
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Gate checks for wip/flag.txt. When it exists (exit code 0), skip_if fires
and the state auto-advances to done.

## done

Terminal state.
