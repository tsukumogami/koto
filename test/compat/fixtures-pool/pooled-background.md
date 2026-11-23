---
name: pooled-background
version: "1.0"
description: A background action on the test-suite pool that runs until its release file appears
initial_state: test
variables:
  RELEASE:
    description: The file whose appearance ends the command
    required: true
states:
  test:
    default_action:
      command: "while [ ! -f {{RELEASE}} ]; do sleep 0.1; done; echo suite ran"
      pool: test-suite
      background: true
      timeout_secs: 120
    transitions:
      - target: done
  done:
    terminal: true
---

## test

Run the suite in the background.

## done

Done.
