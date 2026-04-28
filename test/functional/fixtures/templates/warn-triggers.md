---
name: warn-triggers
version: "1.0"
description: Template with a terminal state name that triggers the W3 warning
initial_state: start

states:
  start:
    accepts:
      status:
        type: enum
        values: [ok, fail]
        required: true
    transitions:
      - target: done
        when:
          status: ok
      - target: failed_result
        when:
          status: fail
  done:
    terminal: true
  failed_result:
    terminal: true
---

## start

Choose an outcome.

## done

Completed successfully.

## failed_result

Terminal failure state. Name contains "fail" without `failure: true` — triggers W3.
