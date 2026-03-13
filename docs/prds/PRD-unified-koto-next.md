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

**As an orchestrating agent in a workflow with a delegation step**, I want to receive the
delegate's output alongside the directive and have it preserved across session boundaries,
so that delegation is recoverable — if my session ends after receiving the output but before
I've acted on it, I can resume without re-invoking the delegate.

**As an orchestrating agent submitting evidence**, I want `koto next` output to tell me exactly
what fields or data to include in my submission, so that I can construct the right payload
without consulting the template definition.

**As a workflow developer authoring a branching template**, I want to declare conditions on
individual outgoing transitions so that the workflow branches based on what the agent submits,
without the agent needing to know state names.

**As a workflow developer declaring evidence requirements**, I want to specify what fields an
agent must submit for a state to advance, so that `koto next` can generate the `expects` schema
automatically and validate submissions without me writing separate validation logic.

**As a workflow developer configuring a delegation step**, I want to declare that a state
requires deep reasoning by tagging it, and have the routing to the actual delegate CLI stay
in user config, so that my template works in any environment regardless of which tools are
available.

**As a developer with a stuck workflow**, I want to identify which evidence is missing and
submit it manually so that I can unblock and resume the workflow without re-running it from
the start.

**As a CI/CD pipeline maintainer**, I want `koto next` to exit with distinct codes for
different failure categories so that my pipeline can distinguish "condition not yet satisfied,
retry later" from "workflow configuration error, operator intervention required" without
parsing error text.

**As a CI/CD pipeline maintainer**, I want koto to handle SIGTERM gracefully so that a
pipeline timeout or job cancellation leaves the workflow in a consistent, resumable state
rather than corrupting it mid-transition.

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
`koto next --with-data <file>` accepts a JSON file containing agent-supplied data. koto validates
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
After acting on the integration output, the agent submits evidence via `koto next --with-data` to
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

### Template Authoring Requirements

**R14. Per-transition condition declaration**
The template format allows conditions to be declared on individual outgoing transitions. A
transition declaration includes a target state and an optional set of conditions. When all
conditions on a transition are satisfied, that transition is eligible. Template authors who
don't need branching can continue declaring transitions as a simple list; the default (no
conditions) means the transition is eligible whenever the state's shared conditions pass.

**R15. Evidence field declaration**
Template authors can declare what evidence fields a state requires before it can advance.
Each declared field has a name and a type or constraint. koto uses these declarations to
generate the `expects` field in `koto next` output and to validate `--with-data` payloads. An
agent submitting the wrong fields or wrong types receives an `invalid_submission` error with
the specific mismatch.

**R16. Shared vs. per-transition conditions**
Template authors can declare both shared conditions (must pass before any transition is
eligible) and per-transition conditions (specific to one outgoing transition). Shared
conditions are evaluated first; per-transition conditions narrow the eligible set. This
allows common preconditions (e.g., "tests must pass") to be declared once rather than
repeated on every transition.

**R17. Template portability**
Templates must run correctly in environments with different tooling configurations. A template
authored in one environment (with delegation config, specific CLIs, or custom condition
commands available) must produce valid, runnable workflows in environments without those tools.
When optional integrations are absent, koto runs without them — the template degrades
gracefully rather than failing at load time. Templates must not assume any specific agent
runtime, tooling setup, or integration availability beyond what is explicitly declared as
required.

**R18. Template validation**
`koto template compile` validates that transition-level conditions on a branching state are
mutually exclusive — no two outgoing transitions from the same state can have conditions
that could be satisfied simultaneously by the same evidence submission. Compile-time
detection prevents ambiguous workflows from being run.

**R19. Agent-agnostic output contract**
`koto next` JSON output makes no assumptions about the consuming agent runtime. The response
schema is the complete interface contract: an agent built in any language or framework can
consume it correctly using only the JSON spec. koto does not couple its output to Claude
Code's tool-use model, Claude's prompt format, or any specific agent SDK. The directive text,
`expects` schema, and error codes are runtime-neutral.

**R20. Exit code semantics**
`koto next` exits with distinct codes for distinct failure categories, enabling CI pipelines
and scripts to branch without parsing error text:

- `0`: success — directive returned, or state advanced and new directive returned
- `1`: transient condition — gates not yet satisfied; caller may retry
- `2`: caller error — bad input, invalid submission format, precondition failed
- `3`: configuration error — template invalid, state file corrupt, integration misconfigured;
  operator intervention required

**R21. Signal handling**
On SIGTERM or SIGINT, koto completes any in-progress atomic write before exiting, ensuring
the state file is never left in a partially-written state. If a transition commit is in
progress, it either completes or rolls back fully. The workflow is always resumable after
a signal-induced exit.

**R22. Completed workflow state preservation**
After a workflow reaches a terminal state, its state file is preserved and queryable. The
record includes each transition taken, the timestamp it occurred, and the evidence submitted
that satisfied the conditions. This record persists until explicitly deleted, allowing
post-completion audit of agent behavior independent of the agent session that ran the
workflow.

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
- [ ] `koto next --with-data <file>` validates the submission, stores it, re-evaluates conditions,
      and advances if they now pass
- [ ] `koto next --with-data <file>` returns `invalid_submission` error when the file doesn't
      match the `expects` schema
- [ ] `koto next --with-data <file>` returns `precondition_failed` error when the current state
      doesn't accept submissions
- [ ] When conditions are unsatisfied, the response lists each blocking condition by name,
      type (evidence / integration), and what it requires
- [ ] A branching template with two outgoing transitions can be advanced to the correct target
      by submitting evidence that satisfies that transition's conditions, without the agent
      naming the target state
- [ ] When a processing integration (delegate) is configured for the current state, `koto next`
      invokes it and includes the response in the output; a subsequent `koto next --with-data` call
      is required to record the agent's interpretation and advance state
- [ ] When a processing integration is unavailable, `koto next` returns the directive with
      `delegation.available: false` instead of failing
- [ ] All error responses include a typed `code` field
- [ ] `koto next` output always includes an `advanced` field
- [ ] A read-only subcommand (e.g., `koto status`) returns current state and unsatisfied
      conditions without modifying workflow state
- [ ] A template with per-transition conditions compiles successfully and produces the
      correct `expects.options` in `koto next` output at the branching state
- [ ] A template with declared evidence fields produces a matching `expects.fields` schema
      in `koto next` output; submitting wrong fields returns `invalid_submission`
- [ ] A template with both shared and per-transition conditions evaluates shared conditions
      first; a submission that fails shared conditions does not advance regardless of
      per-transition condition satisfaction
- [ ] `koto template compile` rejects a template where two outgoing transitions from the
      same state can be satisfied by the same evidence submission
- [ ] A template with delegation tags runs without error in an environment with no delegation
      config; `koto next` returns the directive without a delegate output field
- [ ] `koto next` exits 0 when returning a directive (no advancement or successful advancement)
- [ ] `koto next` exits 1 when conditions are not yet satisfied (gates blocked)
- [ ] `koto next` exits 2 when the caller provides bad input (invalid submission, precondition failed)
- [ ] `koto next` exits 3 when configuration is invalid (corrupt state file, bad template)
- [ ] Sending SIGTERM while `koto next` is committing a transition results in either a
      complete commit or a clean rollback — the state file is never partially written
- [ ] After a workflow reaches a terminal state, `koto query` returns the full transition
      history including timestamps and evidence submitted at each step
- [ ] A developer can submit missing evidence manually via `koto next --with-data` to unblock
      a stuck workflow without re-initializing it

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
- **Multi-workflow aggregate visibility**: monitoring running workflows across a project or
  team (which are stuck, for how long, owned by whom) is a separate capability; koto's
  state model is per-workflow and local
- **Parameterized workflow invocation**: triggering a workflow with external inputs (e.g.,
  "run this template on PR #847") relates to `koto init`, not `koto next`; separate PRD
- **Integration extensibility contract**: the interface for building new koto-native
  integrations (plugin manifest, registration mechanism, contract specification) is a
  separate design concern from the consumer-facing `koto next` interface

## Known Limitations

- Transition-level conditions are a breaking change to the template format. Existing templates
  that use state-level gates will require migration. This is acceptable given koto has no
  production users, but template authors should be aware.
- The `koto transition` command is removed. Workflows that relied on agents explicitly naming
  target states must be redesigned to use evidence-based branching instead.
- Processing integrations run synchronously during `koto next`. Long-running integrations
  (large codebase delegation, slow CI) will block the call for the duration of their timeout.
