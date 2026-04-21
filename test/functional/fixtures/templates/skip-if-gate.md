---
name: skip-if-gate
version: "1.0"
description: Gate-backed skip_if fires when context-exists gate passes
initial_state: check

states:
  check:
    gates:
      ctx_check:
        type: context-exists
        key: ctx_flag
    skip_if:
      gates.ctx_check.exists: true
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Gate checks for ctx_flag in the context store. When it exists, skip_if fires
and the state auto-advances to done.

## done

Terminal state.
