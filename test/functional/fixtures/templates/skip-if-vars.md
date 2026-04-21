---
name: skip-if-vars
version: "1.0"
description: skip_if fires on vars.SHARED_BRANCH when set, otherwise blocks for evidence
initial_state: start

variables:
  SHARED_BRANCH:
    description: "Branch name to use; when set, skips the start state automatically"
    required: false

states:
  start:
    gates:
      branch_check:
        type: command
        command: "test -f wip/branch.txt"
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    accepts:
      branch:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## start

When SHARED_BRANCH is set, skip_if fires and the state auto-advances to done,
bypassing the gate and evidence requirement.
When SHARED_BRANCH is not set, the gate blocks and the state requires evidence.

## done

Terminal state.
