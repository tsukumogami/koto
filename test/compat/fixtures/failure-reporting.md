---
name: compat-failure-reporting
version: "1.0"
description: Walks a session through a failing and then passing command gate, a failing and then succeeding default action, and a context-exists gate, for the failure-reporting compatibility check
initial_state: build
states:
  build:
    gates:
      tests:
        type: command
        command: "echo build-stdout-line; echo build-stderr-line >&2; test -f ready"
    transitions:
      - target: setup
  setup:
    default_action:
      command: "echo setup-stdout-line; echo setup-stderr-line >&2; test -f setup-ok"
    transitions:
      - target: review
  review:
    gates:
      note:
        type: context-exists
        key: review_note
    transitions:
      - target: wrapup
  wrapup:
    accepts:
      verdict:
        type: enum
        required: true
        values: [ship, hold]
    transitions:
      - target: done
        when:
          verdict: ship
      - target: held
        when:
          verdict: hold
  done:
    terminal: true
  held:
    terminal: true
---

## build

Build the change and run the tests.

## setup

Prepare the workspace.

## review

Wait for the review note.

## wrapup

Decide whether to ship.

## done

Shipped.

## held

Held back.
