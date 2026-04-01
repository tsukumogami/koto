---
name: context-gate
version: "1.0"
description: Demonstrates context-exists gate with structured output in blocking_conditions
initial_state: check

states:
  check:
    gates:
      ctx_check:
        type: context-exists
        key: required_key
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check that required_key exists in the context. Gate blocks if the key is absent.

## done

Context gate workflow complete.
