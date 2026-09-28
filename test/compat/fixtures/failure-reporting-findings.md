---
name: compat-failure-reporting-findings
version: "1.0"
description: Walks a session through a command gate that reports a finding and fails twice before passing, a default action that reports a warning and fails before succeeding, and a context-exists gate, so the log holds every check-event field the failure-reporting compatibility check reads
initial_state: build
states:
  build:
    gates:
      tests:
        type: command
        command: "printf '%s\\n' '::koto-finding::{\"rule_id\":\"E501\",\"level\":\"error\",\"message\":\"line too long\",\"path\":\"src/app.py\",\"line\":12}'; echo build-stdout-line; echo build-stderr-line >&2; test -f ready"
    transitions:
      - target: setup
  setup:
    default_action:
      command: "printf '%s\\n' '::koto-finding::{\"rule_id\":\"W291\",\"level\":\"warning\",\"message\":\"trailing whitespace\"}'; echo setup-stdout-line; echo setup-stderr-line >&2; test -f setup-ok"
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
