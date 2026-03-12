# /prd Scope: unified-koto-next

## Problem Statement

koto's state evolution currently requires two CLI commands (`koto next` and `koto transition`),
and planned features like cross-agent delegation would add a third (`koto delegate run`). Each
new capability adds a subcommand that is only valid at specific workflow points, leaving agents
responsible for knowing which command to call and when. Since the state machine already knows
what's valid at any moment, the CLI should express that as a single adaptive command: `koto next`
reads the current state, runs koto-owned integrations (CI checks, delegate CLIs), and advances
automatically when all conditions are met.

## Initial Scope

### In Scope

- `koto next` as the sole state-evolution command (replaces `koto transition` and subsumes
  planned `koto delegate run`)
- Auto-advancement: koto checks all gates, including subprocess-based checks (CI, delegates),
  and transitions when they clear — `koto next` is not idempotent by design
- Evidence submission: `koto next --submit <file>` for agent-supplied data (delegate responses,
  test output, user decisions for branching)
- Branching workflows: agent controls branch via evidence that satisfies mutually exclusive gate
  conditions on transitions — agent never names a target state
- `expects` field in `koto next` JSON output: describes what the current state accepts, enabling
  self-describing protocol (agent reads `expects`, constructs submission, calls `koto next --submit`)
- Structured JSON error model: explicit `op` field (read/write), typed error codes
  (`precondition_failed`, `invalid_input`, `gate_blocked`)
- koto-owned integrations: koto invokes known subprocesses (delegate CLIs, CI providers) as part
  of gate evaluation or state execution — agent's subprocess invocation is the fallback for unknowns
- Read-only visibility subcommand (e.g., `koto status`): shows current state and unsatisfied
  blockers; not part of the agent workflow loop; for debugging only

### Out of Scope

- Internal controller/engine implementation details
- Config system and tag vocabulary (already decided in cross-agent delegation design, issue #41)
- Non-state-evolution commands (`koto rewind`, `koto query`, `koto template compile`, etc.)
- Approval gates integration model (acknowledged but deferred — requires separate design)
- `expects` schema format (simple hint vs. full JSON Schema) — implementation detail for design doc

## Research Leads

1. **Agent workflow loop use cases**: What does the agent loop look like end-to-end for each
   use case — linear workflow, branching workflow, delegation, evidence submission? What must
   `koto next` return to make each case work without agent foreknowledge of state names?

2. **Gate model requirements for branching**: What does the template format need to express
   so that transition-level gates work for branching? What's the minimum contract between
   template authors and the engine for evidence-based branch selection?

3. **Integration ownership boundary**: What is the rule for when koto owns an integration
   vs. when the agent does? What does this mean for the delegate response flow — does koto
   store it, pass it through, or both?

4. **Error and state feedback requirements**: What information must be in `koto next` output
   for agents to handle every failure mode without ambiguity? What's the minimum set of
   error codes an agent needs to branch on?

## Coverage Notes

- The `expects` schema format is not resolved (simple type hint vs. full JSON Schema). The PRD
  should capture the requirement (agent must be able to discover what to submit) without
  prescribing the format — that belongs in the design doc.
- Approval gates are acknowledged as a future use case that doesn't fit the `--submit` model.
  The PRD should note this as a known gap without designing a solution.
- The transition-level gate model (vs. current state-level gates) is a requirement driven by
  branching support. The PRD should state the requirement; the design doc handles the gate
  syntax and engine changes.
