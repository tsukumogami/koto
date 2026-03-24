---
name: multi-state
version: "1.0"
description: Tests full workflow pattern with multiple states
initial_state: entry

states:
  entry:
    accepts:
      route:
        type: enum
        values: [setup, work]
        required: true
    transitions:
      - target: setup
        when:
          route: setup
      - target: work
        when:
          route: work
  setup:
    gates:
      config_exists:
        type: command
        command: "test -f wip/config.txt"
    accepts:
      status:
        type: enum
        values: [completed, override]
        required: true
    transitions:
      - target: work
  work:
    accepts:
      status:
        type: enum
        values: [completed]
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## entry

Choose a route: setup or work.

## setup

Verify configuration exists. Gate checks for wip/config.txt.

## work

Do the main work.

## done

Multi-state workflow complete.
