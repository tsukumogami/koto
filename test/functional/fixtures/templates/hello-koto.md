---
name: hello-koto
version: "1.0"
description: Minimal greet-a-spirit workflow used as a smoke-test fixture
initial_state: awakening

variables:
  SPIRIT_NAME:
    description: Name of the spirit to greet
    required: true

states:
  awakening:
    gates:
      spirit_greeting:
        type: command
        command: "test -f wip/spirit-greeting.txt"
    accepts:
      acknowledgement:
        type: string
        required: false
    transitions:
      - target: eternal
        when:
          gates.spirit_greeting.exit_code: 0
  eternal:
    terminal: true
---

## awakening

Greet the spirit {{SPIRIT_NAME}} by writing a greeting to `wip/spirit-greeting.txt`. When the file exists, the workflow advances to its terminal state.

## eternal

The spirit {{SPIRIT_NAME}} has been greeted. The workflow is complete.
