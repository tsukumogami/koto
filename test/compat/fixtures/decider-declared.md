---
name: decider-compat
version: "1.0"
description: Enum and boolean decider declarations for the koto v0.12.2 compatibility check
initial_state: triage
variables:
  CHANGE_NOTE:
    description: One-line summary of the change under triage
    default: "Rename the --dry-run flag to --plan in the sync command."
states:
  triage:
    accepts:
      route:
        type: enum
        values: [auto, manual, drop]
        required: true
        description: How should this change be handled?
        decider:
          answers:
            auto: {description: "A mechanical change that needs no human review.", mode: auto, threshold: 0.9}
            manual: {description: "Needs a human to look at it first."}
            drop: {description: "Out of scope or already done.", mode: never}
          escape: {value: zq-escape-7kd2, description: "The note is missing or can't be judged."}
          inputs:
            - {var: CHANGE_NOTE, label: change_note, max_bytes: 2048}
    transitions:
      - target: build
        when:
          route: auto
      - target: review
        when:
          route: manual
      - target: dropped
        when:
          route: drop
  review:
    accepts:
      approved:
        type: boolean
        required: true
        description: Did a human approve the change?
    transitions:
      - target: build
        when:
          approved: true
      - target: dropped
        when:
          approved: false
  build:
    accepts:
      passed:
        type: boolean
        required: true
        description: Did the build and the tests pass?
        decider:
          answers:
            true: {description: "Every check passed."}
            false: {description: "At least one check failed."}
          inputs:
            - {var: CHANGE_NOTE, label: change_note}
    transitions:
      - target: done
        when:
          passed: true
      - target: triage
        when:
          passed: false
  done:
    terminal: true
  dropped:
    terminal: true
---

## triage

Decide how to handle the change: {{CHANGE_NOTE}}

## review

Ask a human to review the change.

## build

Build the change and run the tests.

## done

Done.

## dropped

Dropped.
