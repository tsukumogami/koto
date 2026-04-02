---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  After Features 1 and 2, the template compiler accepts gate contract declarations
  without validating them. override_default values on gates are accepted as any JSON
  regardless of whether the keys and types match the gate type's schema. Transition
  when clauses that reference gates.* fields don't have their gate names or field names
  checked against the template's declared gates or the gate type's schema. No check
  verifies that override defaults can actually satisfy at least one transition, so a
  template with an unreachable override path compiles and deploys silently.
decision: |
  TBD after decision investigation.
rationale: |
  TBD after decision investigation.
---

# DESIGN: Gate contract compiler validation

## Status

Proposed

## Context and Problem Statement

Features 1 and 2 of the gate-transition contract roadmap added structured gate output
and the override mechanism. Templates can now declare `override_default` values on
gates and reference `gates.*` fields in `when` clauses. The compiler accepts both
without validating them against the gate type's schema.

Three concrete validation gaps remain:

**Gap 1: override_default is unchecked.** A gate can declare
`override_default: {bad_field: 999}` for a `command` gate (which should produce
`{exit_code: number, error: string}`) and the compiler accepts it. At runtime, the
override substitutes `{bad_field: 999}` into the evidence map. Any `when` condition
checking `gates.ci_check.exit_code` would silently fail to match.

**Gap 2: gates.* when clause references are unchecked.** A transition `when`
clause can reference `gates.nonexistent_gate.exit_code` or
`gates.ci_check.phantom_field`, and the compiler accepts it. At runtime, the resolver
finds nothing at the dot-path and the condition silently fails — or silently passes
if no override is in effect and the gate produces a different schema.

**Gap 3: No reachability check.** A template could declare override defaults that
don't satisfy any transition's `when` conditions. When an agent overrides all gates,
no transition fires — a dead end that the template author didn't intend.

This is Feature 3 of the gate-transition contract roadmap (#118). It implements
PRD R9: compiler validation of the full gate/transition/override contract.

## Decision Drivers

- **No circular dependencies.** `template/types.rs` must not import from `gate.rs`
  — gate.rs already imports GATE_TYPE_* constants and the Gate struct from types.rs.
  Gate schema information must live in types.rs or a shared location.
- **Actionable error messages.** Each error must name the specific state, gate, and
  field that's wrong, not just report a generic schema violation.
- **No false positives on valid templates.** The reachability check must not reject
  valid templates that mix gate output and agent evidence in `when` clauses — those
  states can't be resolved from gate overrides alone, and that's expected.
- **Extensible to future gate types.** Adding a new gate type (jira, http,
  json-command) should require minimal changes to the validation code. The schema
  registry should be the only place to update.
- **Build on existing infrastructure.** The existing `validate_evidence_routing()`,
  gate type constants, and transition resolver should be used where possible rather
  than duplicated.
