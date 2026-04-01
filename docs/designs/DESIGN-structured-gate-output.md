---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Gate evaluation returns a boolean GateResult enum (Passed/Failed/TimedOut/
  Error) that carries no structured data. The advance loop uses a single
  gate_failed boolean to decide whether to block or advance. Transition
  routing can't use gate output because there isn't any -- the resolver only
  matches agent-submitted evidence. This is Feature 1 of the gate-transition
  contract roadmap: the foundation that all other features build on.
decision: |
  Placeholder -- will be filled after decision execution phases.
rationale: |
  Placeholder -- will be filled after decision execution phases.
---

# DESIGN: Structured gate output

## Status

Proposed

## Context and problem statement

Three components need to change to make gate output available for transition
routing:

1. **Gate evaluation must produce structured data.** The `GateResult` enum in
   `src/gate.rs` has four variants (Passed, Failed{exit_code}, TimedOut,
   Error{message}) with no structured output. Each gate type already captures
   data it throws away: command gates have exit codes and stdout, context-exists
   gates have a boolean result, context-matches gates have a match result.
   The evaluation functions need to return structured data matching each gate
   type's documented schema (R1): command -> `{exit_code, error}`,
   context-exists -> `{exists, error}`, context-matches -> `{matches, error}`.

2. **Gate output must enter the transition resolver.** `resolve_transition` in
   `src/engine/advance.rs` takes `&BTreeMap<String, serde_json::Value>` of
   evidence and matches it against `when` conditions using exact JSON equality.
   Gate output needs to be injected into this map under the `gates.*` namespace
   as a nested JSON structure. The resolver currently does flat key matching
   (`evidence.get("field")`), but `when` clauses like
   `gates.ci_check.exit_code: 0` require dot-path traversal into nested maps.

3. **The advance loop must use gate output for routing.** Today the advance
   loop at `src/engine/advance.rs:295-316` evaluates gates, checks a boolean
   `any_failed`, and either returns `GateBlocked` or falls through to
   transition resolution. With structured output, the advance loop needs to:
   merge gate output into the evidence map, evaluate pass conditions to
   determine if the state should auto-advance or stop, and report structured
   gate data in the response when the state stops.

This design scopes to Feature 1 (issue #116) of the gate-transition contract
roadmap. It covers R1 (gate type schemas), R2 (structured evaluation), R3
(gate output in routing), R4a (response format), and R11 (event ordering).
Override mechanism (Feature 2), compiler validation (Feature 3), and backward
compatibility details (Feature 4) are separate designs.

## Decision drivers

- **Minimal resolver changes**: dot-path traversal is the biggest code change.
  Prefer an approach that minimizes changes to `resolve_transition` while
  supporting nested gate data.
- **Gate type extensibility**: new gate types (json-command, http, jira) will
  register schemas and parsing logic. The design should make adding a new
  gate type straightforward without modifying core engine code.
- **Backward compatibility**: existing templates without `gates.*` in `when`
  clauses must work identically. The advance loop must detect whether a state
  uses structured gate output and fall back to legacy behavior if not.
- **Consistent error handling**: timeout and spawn errors should produce the
  same schema shape as normal output (e.g., `{exit_code: -1, error:
  "timed_out"}` for command gates), so `when` clauses can route on errors
  without special-casing.
- **Pass condition as data, not control flow**: the pass condition evaluates
  against structured output, not a boolean flag. The `gate_failed` boolean
  should be derived from pass condition evaluation, not set independently.
- **Event ordering**: gate output events (if needed) must have deterministic
  sequence numbers relative to other events in the same invocation.
