# Advocate: Declarative Workflow Language

## Approach Description

The template format is the authoritative specification for all workflow semantics. Template
YAML frontmatter declares evidence field requirements, per-transition conditions, integration
tags, and validation constraints. The state model is derived from what templates declare —
evidence fields create schema requirements, per-transition conditions define branching behavior.
The CLI output schema is also derived — the `expects` field mirrors template evidence
declarations, and transition options mirror template condition sets.

This creates one source of truth: the template is the specification; the state machine and
CLI output are generated from it.

**Example template state declaration (new format):**

```yaml
states:
  analyze_results:
    directive: |
      Review the test output and determine whether to proceed or escalate.
    evidence:
      decision:
        type: enum
        values: [proceed, escalate]
        description: "Your assessment of the results"
      rationale:
        type: string
        required: true
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: escalate_review
        when:
          decision: escalate
    gates:
      tests_passed:
        type: command
        command: ./check-ci-status.sh
```

The `evidence` block declares what fields the agent must submit. The `when` clauses on
transitions define which transition fires based on submitted values. The `expects` field in
`koto next` output is computed directly from the `evidence` block.

## Investigation

The current template format uses YAML frontmatter with a `states` map. Each state has
`transitions: []string` and `gates: map[string]GateDecl`. The compilation pipeline
(source YAML → compiled JSON → engine Machine) is clean and well-separated.

The current `Directive` struct in `pkg/controller` is flat: `Action`, `State`, `Directive`
string, `Message`. There is no `expects` field, no schema, no structured output. The CLI
formats this as text or basic JSON.

Under declarative-language-first, the compilation pipeline gains a schema extraction step:
compiled templates carry both the execution model (for the engine) and the declaration model
(for computing `koto next` output). The engine evaluates `when` clauses against submitted
evidence. The CLI output derives `expects` from the compiled evidence declarations.

Sub-design boundaries are clean: template format gets its own design (what can be declared,
YAML syntax, compilation), state model gets its own design (what's persisted, evidence
scoping), CLI contract gets its own design (output schema, error codes). All three derive
from the template declarations.

## Strengths

- **Single source of truth**: template is the specification; CLI output and state schema
  are computed from it, not maintained separately
- **Self-describing output is automatic**: the `expects` field derives directly from
  template `evidence` declarations — no separate schema registry needed
- **Template authors have full control**: developers can read one file and understand
  everything the workflow does — evidence requirements, branching conditions, integrations
- **Compilation catches errors early**: invalid evidence references, conflicting conditions,
  or missing transition targets are caught at template compile time, not at runtime
- **Clean sub-design boundaries**: template format, state model, and CLI contract are
  each independently specifiable once the declaration model is chosen

## Weaknesses

- **Template format becomes complex**: expressing evidence schemas, per-transition
  conditions, integration tags, and validation constraints in YAML frontmatter is verbose;
  templates for complex workflows become hard to read
- **Compilation pipeline grows**: any change to template semantics requires updates to
  the compiler and all downstream consumers; the indirection adds maintenance overhead
- **Evidence schema expressiveness is limited**: YAML-declared schemas work well for
  flat key-value evidence (enums, strings, booleans) but become awkward for nested objects
  or complex type constraints without embedding JSON Schema or a schema DSL
- **Breaking change scope is largest**: the template format change is more extensive than
  other approaches because it adds new top-level blocks (`evidence`, `when` clauses) on
  top of the transition model change

## Deal-Breaker Risks

- **None identified** for simple evidence models. The key risk is expressiveness: if
  evidence schemas need to go beyond flat key-value pairs (e.g., nested objects, array
  values, cross-field validation), the YAML declaration approach breaks down and either
  requires an embedded DSL or falls back to runtime validation — at which point the
  "declared in template" premise no longer holds fully.

## Sub-Design Boundaries

1. **Template format design**: YAML syntax for evidence declarations, `when` clauses,
   integration tags; compilation pipeline changes; format version bump
2. **State model design**: evidence scoping (per-state clearing), history structure,
   state file schema versioning
3. **CLI contract design**: `koto next` output schema (`expects` derivation, error
   codes, `advanced` flag, integration output)
4. **Engine execution design**: evaluating `when` clauses against evidence, multi-transition
   selection, cycle detection, advancement loop

## Implementation Complexity

- Scope: **Large**
- CLI: add `expects` derivation from compiled template declarations; add structured output
- State model: add per-state evidence scoping; add evidence schema validation on submission
- Template format: add `evidence` block, `when` clauses on transitions, integration tags;
  update compilation pipeline; bump format version

## Summary

The declarative language first approach makes the template the single source of truth for
all workflow semantics: evidence requirements, branching conditions, and integration config
are all declared in the template and derived from it by the state machine and CLI output.
This satisfies self-describing output without a separate schema registry, but at the cost
of a more complex template format and compilation pipeline. It works well for workflows
with flat evidence schemas and simple conditions; complex evidence types expose its limits.
