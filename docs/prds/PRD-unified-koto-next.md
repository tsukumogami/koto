---
status: Draft
problem: |
  koto's state evolution requires multiple CLI commands: koto next to read the current
  directive, koto transition to advance state, and planned features like cross-agent
  delegation would add koto delegate run. Each command is only valid at specific workflow
  points, leaving the orchestrating agent responsible for knowing which command to call and
  when. This creates two problems: agents can call commands out of order and get confusing
  errors, and the CLI surface grows with every new capability. Since the state machine already
  knows what's valid at any moment, the CLI should express that — a single koto next command
  that checks conditions, runs integrations, and advances state when ready.
goals: |
  Establish koto next as the single command for all state evolution. Agents call koto next
  in a loop: koto checks conditions, chains through states automatically until it reaches one
  requiring agent action, and returns that state's directive. Agents submit evidence when the
  current state requires it, branch by satisfying transition-specific conditions rather than
  naming a target state, and receive integration outputs (like delegate responses) in the
  response to interpret before submitting. The CLI surface stays constant as new capabilities
  are added.
source_issue: 43
---

# PRD: Unified koto next Command

## Status

Draft

## Problem Statement

koto is a workflow engine for AI coding agents. Agents run workflows by calling `koto next`
to get the current directive, executing it, then calling `koto transition <target>` to advance
state. Planned features add more commands: `koto delegate run` for cross-agent delegation,
and future capabilities would add more still.

This command-per-action model has two compounding problems. First, agents must know which
command to call and when — nothing prevents calling `koto transition` when the state machine
expects an evidence submission, or calling `koto delegate run` when no delegation is active.
Out-of-order calls produce confusing errors rather than clear guidance. Second, the CLI surface
grows with every new capability, making the agent integration contract harder to specify and
maintain over time.

The state machine already knows what's valid at any moment. The CLI should reflect that:
at any point in a workflow, there is exactly one thing koto needs from the orchestrating
agent. A single `koto next` command can express this — adapting to what the current state
requires rather than requiring agents to track it themselves.

## Goals

- Agents use a single command (`koto next`) for all state evolution in a workflow
- Agents don't need foreknowledge of state names, valid transitions, or command sequencing
- `koto next` output is self-describing: an agent that has never seen the template can
  determine its next action from the response alone
- The CLI surface stays constant as new capabilities (evidence submission, delegation, future
  integrations) are added
- Branching workflows are supported without agents naming target states

## User Stories

**As an orchestrating agent running a linear workflow**, I want to call `koto next` in a loop
and receive a directive each time, so that I don't need to know state names or call a separate
command to advance state.

**As an orchestrating agent in a branching workflow**, I want to submit evidence that reflects
my decision, and have koto advance to the correct next state automatically, so that I don't
need to know what the next state is called or which transitions are defined.

**As an orchestrating agent in a workflow with a delegation step**, I want to call `koto next`
and receive both the directive and the delegate's response, so that I can interpret the
response and decide what to do with it before submitting evidence to advance state.

**As an orchestrating agent submitting evidence**, I want `koto next` output to tell me exactly
what fields or data to include in my submission, so that I can construct the right payload
without consulting the template definition.

**As a developer debugging a stuck workflow**, I want a read-only command that shows the current
state and which conditions are blocking advancement, so that I can diagnose problems without
affecting workflow state.

## Requirements

### Functional Requirements

**R1. Single state-evolution command**
`koto next` is the only command for state evolution. `koto transition` is removed. No other
state-evolution subcommands are added as new capabilities are introduced.

**R2. Auto-advancement**
When `koto next` is called, koto evaluates all conditions for the current state. If conditions
are satisfied, koto advances to the next state and re-evaluates conditions there. koto continues
advancing through states until it reaches one that requires agent action — a state with unsatisfied
conditions or a state whose directive the agent must execute. The response reflects the final
stopping state, not any intermediate states passed through.

`koto next` is not idempotent by design — calling it may trigger one or more state transitions if
conditions have become satisfied since the last call (e.g., CI check has passed).

**R3. Evidence submission**
`koto next --submit <file>` accepts a JSON file containing agent-supplied data. koto validates
the submission against the current state's requirements, stores the data, re-evaluates
conditions, and advances if they are now satisfied.

**R4. Self-describing output**
Every `koto next` response includes an `expects` field describing what the current state
accepts. When the state accepts no submission (all conditions are koto-verified), `expects`
is absent or null. An agent that has never seen the workflow template can determine its next
action from the response alone.

**R5. Advancement signal**
Every `koto next` response includes an `advanced` field indicating whether state changed
during this call. Agents must not need to compare state names between calls to detect
advancement.

**R6. Transition-level conditions**
Workflow templates can declare conditions on individual outgoing transitions, not only on
the state as a whole. For branching states, each transition has its own set of conditions.
The agent satisfies one transition's conditions through evidence submission; koto advances
to that transition's target automatically. Conditions on different outgoing transitions from
the same state must be mutually exclusive — only one transition's conditions should be
satisfiable at a time.

**R7. koto-owned integrations**
koto runs integrations it knows the contract for as part of `koto next` processing. Integrations
fall into two categories:

- **Condition integrations**: deterministic checks that block or allow advancement (e.g., CI
  status check, test runner). Declared in the template. koto runs these when evaluating
  conditions, similar to how command gates work today.
- **Processing integrations**: external tools that process the directive and return output
  (e.g., delegate CLIs for deep reasoning). Declared in user config, invoked by koto during
  `koto next` when the current state has matching configuration.

The orchestrating agent's subprocess invocation is the fallback for integrations koto doesn't
have a built-in contract for.

**R8. Integration output in response**
When koto runs a processing integration (e.g., delegate CLI) during a `koto next` call, the
integration's output is included in the response. The agent receives it as context for executing
the directive and is responsible for interpreting the output. koto does not interpret integration
responses — it cannot assess whether a delegate's findings are actionable, complete, or correct.
After acting on the integration output, the agent submits evidence via `koto next --submit` to
record its assessment and trigger advancement. The delegation flow is therefore two calls: one
to receive the directive and delegate output, one to submit the agent's interpretation and advance.

**R9. Structured error model**
`koto next` errors are machine-parseable with typed error codes. Agents branch on error code,
not error message text. Required error codes:

- `gate_blocked`: conditions not yet satisfied; includes per-condition detail
- `precondition_failed`: submission provided but current state doesn't accept one
- `invalid_submission`: submission format doesn't match what the state expects
- `integration_unavailable`: a required koto-owned integration is not accessible; includes
  fallback guidance

**R10. Advancement with gate failure detail**
When conditions are not satisfied, the response includes structured detail for each unsatisfied
condition: condition name, what it requires, and whether the agent can satisfy it (evidence
gate) or koto will verify it independently (integration gate). Agents use this to determine
whether to submit evidence or to wait and call `koto next` again.

### Non-functional Requirements

**R11. Output backward compatibility scope**
koto has no production users. No backward compatibility with the existing `koto transition`
command is required. The template format change for transition-level conditions is a breaking
change; existing templates require migration.

**R12. Integration availability fallback**
For processing integrations (delegate CLIs), if the configured tool is not accessible, `koto
next` returns the directive without integration output, and includes a `delegation.available:
false` field so the agent can handle the directive directly. Condition integrations (CI checks,
command gates) fail the condition if unavailable — they don't silently pass.

**R13. Response completeness**
A `koto next` response must be fully self-contained. It must not require the agent to reference
prior responses or maintain session context to understand what to do. The directive text has
all variables interpolated; the `expects` field fully describes any required submission.

## Acceptance Criteria

- [ ] `koto next` with no arguments returns the current state's directive and `expects` field
- [ ] `koto next` with no arguments advances through all states whose conditions are
      immediately satisfied, stopping at the first state that requires agent action, and
      returns that state's directive with `advanced: true`
- [ ] `koto next --submit <file>` validates the submission, stores it, re-evaluates conditions,
      and advances if they now pass
- [ ] `koto next --submit <file>` returns `invalid_submission` error when the file doesn't
      match the `expects` schema
- [ ] `koto next --submit <file>` returns `precondition_failed` error when the current state
      doesn't accept submissions
- [ ] When conditions are unsatisfied, the response lists each blocking condition by name,
      type (evidence / integration), and what it requires
- [ ] A branching template with two outgoing transitions can be advanced to the correct target
      by submitting evidence that satisfies that transition's conditions, without the agent
      naming the target state
- [ ] When a processing integration (delegate) is configured for the current state, `koto next`
      invokes it and includes the response in the output; a subsequent `koto next --submit` call
      is required to record the agent's interpretation and advance state
- [ ] When a processing integration is unavailable, `koto next` returns the directive with
      `delegation.available: false` instead of failing
- [ ] All error responses include a typed `code` field
- [ ] `koto next` output always includes an `advanced` field
- [ ] A read-only subcommand (e.g., `koto status`) returns current state and unsatisfied
      conditions without modifying workflow state

## Out of Scope

- **Internal implementation details**: gate evaluation engine changes, `Directive` struct
  modifications, `MachineState` data model — these belong in the design doc
- **Template format syntax**: the YAML syntax for transition-level conditions, the format
  version strategy for the breaking change — design doc
- **Config system and delegation tag vocabulary**: already decided in the cross-agent
  delegation design (issue #41)
- **`expects` schema format**: whether `expects` is a simple type hint or a full JSON Schema
  fragment — design doc
- **Approval gates**: workflows that pause for human or external-system approval via an
  out-of-band channel. Acknowledged as a future use case; the input model (how approval
  reaches koto) is deferred
- **Non-state-evolution commands**: `koto rewind`, `koto query`, `koto template compile`,
  `koto init` — not in scope
- **Streaming integration responses**: delegate responses are captured synchronously and
  returned in full; streaming is deferred

## Known Limitations

- Transition-level conditions are a breaking change to the template format. Existing templates
  that use state-level gates will require migration. This is acceptable given koto has no
  production users, but template authors should be aware.
- The `koto transition` command is removed. Workflows that relied on agents explicitly naming
  target states must be redesigned to use evidence-based branching instead.
- Processing integrations run synchronously during `koto next`. Long-running integrations
  (large codebase delegation, slow CI) will block the call for the duration of their timeout.
