---
name: pooled-ordinary
version: "1.0"
description: An ordinary action on the test-suite pool, for the previous release to run unthrottled
initial_state: test
states:
  test:
    default_action:
      command: "echo suite ran"
      pool: test-suite
      capture_stdout_as: SUITE_LOG
    transitions:
      - target: done
  done:
    terminal: true
---

## test

Run the suite.

## done

Done.
