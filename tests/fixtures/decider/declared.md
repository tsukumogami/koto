---
name: decider-declared
version: "1.0"
description: Enum and boolean decider declarations on accepts fields
initial_state: gather
variables:
  PLAN_DOC:
    description: Path to the plan document
    default: docs/plans/PLAN-example.md
states:
  gather:
    default_action:
      command: "printf 'Add a --dry-run flag to the sync command.' | koto context add {{SESSION_NAME}} outline.md"
    gates:
      outline:
        type: context-exists
        key: outline.md
    transitions:
      - target: measure
        when:
          gates.outline.exists: true
  measure:
    default_action:
      command: "echo src/sync.rs"
      capture_stdout_as: CHANGED_FILES
    transitions:
      - target: review
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Is the plan outline item clear and scoped enough to implement?
        decider:
          answers:
            proceed: {description: "Names a concrete change with checkable criteria.", threshold: 0.92}
            exit: {description: "Vague, contradictory, or needs design first.", mode: never}
          escape: {value: unclear, description: "Missing, truncated, or unjudgeable."}
          inputs:
            - {context: outline.md, label: outline_item, max_bytes: 12000}
            - {var: PLAN_DOC, label: plan_path}
      rationale:
        type: string
        required: false
        description: Why this verdict
    transitions:
      - target: confirm
        when:
          verdict: proceed
      - target: stopped
        when:
          verdict: exit
  confirm:
    accepts:
      ready:
        type: boolean
        required: true
        description: Does the change touch only the files the outline item names?
        decider:
          answers:
            true: {description: "Only the named files changed."}
            false: {description: "Other files changed too.", mode: auto}
          inputs:
            - {var: CHANGED_FILES, label: changed_files, max_bytes: 4096}
    transitions:
      - target: done
        when:
          ready: true
      - target: review
        when:
          ready: false
  done:
    terminal: true
  stopped:
    terminal: true
---

## gather

Write the outline item for {{PLAN_DOC}} to the `outline.md` context key.

## measure

Record the changed files.

## review

Decide whether the outline item is clear enough to implement.

## confirm

Changed files: {{CHANGED_FILES}}. Confirm the change stays within the outline item.

## done

Done.

## stopped

Stopped.
