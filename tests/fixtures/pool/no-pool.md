---
name: no-pool
version: "1.0"
description: A default_action, a polled gate and evidence routing, with none of pool, background or timeout_secs
initial_state: build
variables:
  TARGET:
    description: What to build
    required: false
    default: all
states:
  build:
    default_action:
      command: "make {{TARGET}}"
      working_dir: "."
      capture_stdout_as: BUILD_LOG
    gates:
      built:
        type: context-exists
        key: build.md
    transitions:
      - target: test
        when:
          gates.built.exists: true
  test:
    default_action:
      command: "make test"
    gates:
      suite:
        type: command
        command: "test -f test-report.xml"
        poll:
          interval_secs: 5
          timeout_secs: 600
    accepts:
      outcome:
        type: enum
        values: [passed, failed]
        required: true
    transitions:
      - target: done
        when:
          outcome: passed
          gates.suite.exit_code: 0
      - target: build
        when:
          outcome: failed
  done:
    terminal: true
---

## build

Build {{TARGET}}.

## test

Run the suite and report the outcome.

## done

Done.
