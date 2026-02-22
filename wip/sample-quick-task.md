---
name: quick-task
version: "1.0"
description: A focused task workflow for small, well-defined changes
initial_state: assess

variables:
  TASK:
    description: What to build or fix
    required: true
  REPO:
    description: Repository path
    default: "."

states:
  assess:
    transitions: [plan, escalate]
    gates:
      task_defined:
        type: field_not_empty
        field: TASK
  plan:
    transitions: [implement]
  implement:
    transitions: [verify]
    gates:
      tests_pass:
        type: command
        command: go test ./...
  verify:
    transitions: [done, implement]
    gates:
      ci_green:
        type: field_equals
        field: CI_STATUS
        value: passed
  escalate:
    terminal: true
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

Review the codebase in {{REPO}} and determine:

- What files need to change
- How complex the change is (small fix vs multi-file refactor)
- Whether tests exist for the affected code
- Any risks or edge cases

If the task is too large or unclear, transition to **escalate** with a note about why.

### Decision Criteria

| Signal | Action |
|--------|--------|
| Clear scope, < 5 files | Transition to **plan** |
| Ambiguous requirements | Transition to **escalate** |
| Needs design discussion | Transition to **escalate** |

## plan

Create an implementation plan for: {{TASK}}

Based on the assessment, write a concrete plan covering:

1. Files to modify (with specific functions/sections)
2. New files to create (if any)
3. Tests to write or update
4. Order of operations (what to change first)

Keep the plan focused -- this is a quick task, not a full design doc.

## implement

Execute the plan. Write code and tests for: {{TASK}}

Follow these guidelines:

- Make small, atomic commits
- Run tests after each significant change
- If you discover the plan was wrong, go back to **plan** rather than improvising

When you believe implementation is complete, run the full test suite before transitioning.

## verify

Verify the implementation is complete and CI is green.

Check:
- [ ] All tests pass locally
- [ ] No lint warnings introduced
- [ ] Changes match the original task description
- [ ] No unrelated changes included

If CI fails or something is wrong, transition back to **implement** to fix it.

## escalate

This task could not be completed in the quick-task workflow.

Reason for escalation should be documented in the evidence. The task may need:
- A design discussion
- Breaking into smaller pieces
- Input from a human reviewer

## done

Task complete. {{TASK}} has been implemented and verified.
