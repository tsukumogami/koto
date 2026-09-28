---
name: compat-failure-reporting-context
version: "1.0"
description: Assigns the published-location key on a transition, so a later write of the same key by koto itself shows whether an older koto reading the log restores the stale assigned value over it
initial_state: start
states:
  start:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: hold
        when:
          step: go
        context_assignments:
          workflows/publish-location: stale-from-transition
  hold:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          step: go
  done:
    terminal: true
---

## start

Start.

## hold

Hold until told to go on.

## done

Done.
