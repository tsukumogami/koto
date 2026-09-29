---
# The `entry` state of shirabe's `/work-on` template, copied from
# tsukumogami/shirabe skills/work-on/koto-templates/work-on.md at commit
# c12968c86b52e0b444403b7ff8b4ff675e8e620e (its states, accepts, skip_if
# and transitions as written there; its long directive is shortened). The
# four targets are stubbed as terminals so the fixture stands alone.
#
# Its skip_if carries `vars.ISSUE_SOURCE: plan_outline`, a value condition
# that compiled but never matched before value routing. It is the one known
# template in use whose behavior value routing touches, and
# tests/value_routing_cli_test.rs pins what that touch is: the same target,
# logged differently.
name: work-on-entry
version: "1.0"
initial_state: entry
variables:
  ISSUE_SOURCE:
    description: Source of issue data for plan-backed mode (github or plan_outline)
    required: false
states:
  entry:
    skip_if:
      vars.ISSUE_SOURCE: plan_outline
      mode: plan_backed
    accepts:
      mode:
        type: enum
        values: [issue_backed, free_form, plan_backed, skipped]
        required: true
      issue_number:
        type: string
        description: GitHub issue number (required for issue_backed mode)
      task_description:
        type: string
        description: Task description (required for free_form mode)
      issue_source:
        type: enum
        values: [github, plan_outline]
        description: Source of issue data (plan-backed mode only)
    transitions:
      - target: context_injection
        when:
          mode: issue_backed
      - target: task_validation
        when:
          mode: free_form
      - target: plan_context_injection
        when:
          mode: plan_backed
      - target: skipped_due_to_dep_failure
        when:
          mode: skipped
  context_injection:
    terminal: true
  task_validation:
    terminal: true
  plan_context_injection:
    terminal: true
  skipped_due_to_dep_failure:
    terminal: true
---

## entry

Determine the workflow mode and provide the initial context for this task.

## context_injection

Stub.

## task_validation

Stub.

## plan_context_injection

Stub.

## skipped_due_to_dep_failure

Stub.
