# Advocate: Minimal Extension

## Approach Description

Extend the existing model with the minimum changes required to satisfy the PRD. The current
template format, state model, and CLI are sound — they need targeted additions, not a redesign.
Add per-transition conditions alongside existing state-level gates (optional, additive), add
per-state evidence scoping (clear evidence map on each transition), and add `expects` and
`advanced` fields to the CLI output derived from optional field declarations in the template.

The key principle: backward compatibility where possible. Templates that don't declare
per-transition conditions or evidence fields continue to work. New capabilities are opt-in.

**Before/after template comparison:**

```yaml
# Current format (continues to work)
states:
  gather_info:
    transitions:
      - analyze
    gates:
      has_data:
        type: field_not_empty
        field: input_file

# Extended format (new capabilities, opt-in)
states:
  analyze_results:
    transitions:
      - target: deploy
        when:
          decision: proceed        # per-transition condition (new)
      - target: escalate_review
        when:
          decision: escalate
    expects:                       # evidence field declarations (new)
      decision:
        type: enum
        values: [proceed, escalate]
      rationale:
        required: true
    gates:                         # state-level gates remain as-is
      tests_passed:
        type: command
        command: ./check-ci.sh
```

## Investigation

The current codebase is well-structured for this approach. `MachineState` already has
`Transitions []string` and `Gates map[string]*GateDecl`. The engine's `Transition()` handles
gate evaluation, history entry construction, and atomic persist. `State.Evidence` is a flat
map. The template compilation pipeline (source YAML → compiled JSON → engine types) is clean.

Minimal changes required:
- `MachineState.Transitions` changes from `[]string` to `[]TransitionDecl{Target, When, ...}`
  (the `When` conditions replace the state-level gate map for branching states; state-level
  gates remain for non-branching conditions)
- `State.Evidence` clearing: add archive-to-history and reset before each `persist()` call
- Template source gains optional `expects` block and `when` clauses on transitions
- CLI output gains `expects` (derived from template `expects` declarations), `advanced` flag,
  and structured error codes

Sub-design boundaries are the same as other approaches but each sub-design is smaller in
scope because less changes.

## Strengths

- **Lowest migration burden**: existing templates that don't use per-transition conditions
  or `expects` declarations continue to compile and run without changes; migration is opt-in
- **Proven foundation**: the engine, state model, and template compilation pipeline are
  battle-tested; changes are additive rather than redesigns
- **Incremental delivery**: each sub-design is independent and shippable — template format
  changes don't block state model changes; CLI output changes don't block engine changes
- **Lower risk**: the blast radius of each change is contained; a bug in per-transition
  condition evaluation doesn't affect states using state-level gates only
- **Familiar authoring model**: workflow developers learn one addition at a time rather than
  a completely new template language

## Weaknesses

- **Two syntaxes coexist**: backward compatibility means both `transitions: [string]` (old)
  and `transitions: [{target, when}]` (new) are valid, creating two ways to express
  transitions; template authors face a choice, and documentation must cover both
- **`expects` derivation is limited**: the `expects` block in templates can declare field
  names and simple constraints, but complex validation (conditional requirements, nested
  objects, cross-field constraints) isn't expressible without a schema DSL; `expects` in
  the output is a hint, not a contract
- **Mutual exclusivity is hard to enforce**: the PRD requires that per-transition conditions
  be mutually exclusive (only one transition satisfiable at a time). Detecting violations
  statically requires analyzing condition combinations at compile time — non-trivial
  compiler logic for an optional feature
- **Technical debt from compatibility**: maintaining two transition syntaxes indefinitely
  creates ongoing documentation, testing, and maintenance burden

## Deal-Breaker Risks

- **Per-transition mutual exclusivity enforcement**: if two transitions from the same
  state can both be satisfied simultaneously, the workflow is non-deterministic. Enforcing
  this at template compile time is complex; deferring it to runtime requires clear error
  handling when it triggers. Neither is solved by the minimal extension approach — it's a
  problem regardless of approach, but the minimal approach is less likely to define a clean
  model for it.
- **Evidence scoping during auto-advancement**: when a state is auto-advanced through
  (conditions already satisfied, no agent submission), its evidence is archived but was never
  submitted. Downstream states cannot access it. This is a semantic constraint template
  authors must understand — the engine won't catch template designs that implicitly depend
  on evidence from auto-advanced states.

## Sub-Design Boundaries

1. **Template format syntax**: per-transition `when` conditions, `expects` field
   declarations, integration tags; backward-compatible compilation pipeline
2. **State model mutations**: evidence scoping lifecycle (archive on transition, clear for
   new state); state file schema version bump
3. **CLI output contract**: `Directive` struct additions (`expects`, `advanced`, error
   codes); `--with-data` and `--to` flag handling
4. **Auto-advancement engine**: transition chain loop, cycle detection, stopping conditions,
   integration invocation

## Implementation Complexity

- Scope: **Medium-Small** (smallest of the four approaches)
- CLI: add `expects` derivation from optional template declarations; add `advanced` and
  structured error codes; add `--with-data`/`--to` flags
- State model: evidence archive-and-clear on each `Transition()` call; state file version bump
- Template format: add optional `when` clauses and `expects` block; update compilation;
  maintain backward compatibility with existing `transitions: []string` syntax

## Summary

Minimal extension is the pragmatic choice: extend what's proven, add what's missing, and
let the scope of change be proportional to the problem. The approach ships a complete
unified CLI with the least churn to template authors and the smallest implementation risk.
The cost is two coexisting syntaxes and limited `expects` schema expressiveness. The mutual
exclusivity problem and evidence scoping constraints are real but exist in all approaches —
minimal extension just doesn't pretend to solve them more elegantly.
