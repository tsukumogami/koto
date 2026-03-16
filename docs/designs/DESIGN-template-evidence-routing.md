---
status: Planned
problem: |
  koto's template format currently uses flat transition lists (transitions: [target1,
  target2]) with no way to express evidence-driven routing or processing integrations.
  As the event log format (#46) adds typed events like evidence_submitted, the template
  format must declare what evidence each state accepts, how submitted values route to
  outgoing transitions, and which states invoke external processing tools. Without these
  declarations, agents can't know what data to submit, and the advancement engine can't
  route transitions based on evidence values.
decision: |
  Add three constructs to the template format: accepts blocks (per-state evidence field
  schema with types and required flags), when conditions on transitions (field-value
  equality matching for routing), and an integration field (string tag for processing
  tool routing). Remove field gates (field_not_empty, field_equals) entirely since
  accepts/when replaces them. Only command gates survive. When conditions use AND
  semantics (all fields must match). The compiler validates pairwise mutual exclusivity
  of when conditions at compile time and rejects non-deterministic templates. The
  format_version remains 1.
rationale: |
  In the event-sourced model, evidence only enters through koto next --with-data and is
  scoped to the current state via the epoch boundary rule. Field gates that check
  agent-submitted evidence are redundant with accepts/when, which handles the same use
  cases more expressively. Removing them yields two orthogonal concepts (command gates
  for environment, accepts/when for agent evidence) with no overlap or interaction rules
  to define. koto has no users, so this is a clean break with no migration concerns.
  The format_version stays at 1 because koto hasn't shipped yet -- there's no released
  v1 to distinguish from.
---

# DESIGN: Template Format -- Evidence Routing

## Status

Planned

## Upstream Design Reference

Strategic design: `docs/designs/DESIGN-unified-koto-next.md` (status: Planned)

This tactical design implements Phase 2 (Template Format) from the strategic design.
Relevant sections: Template Format, Sub-Design Boundaries, Implementation Approach Phase 2.

## Context and Problem Statement

koto's template format defines workflows as states with flat transition lists
and koto-verifiable gates. A state declares its outgoing transitions as a list of
target state names (`transitions: [deploy, escalate_review]`) and optional gates
that block advancement until conditions are met.

This works for linear workflows where states have a single outgoing path. But the
event-sourced state model from #46 introduces `evidence_submitted` events where
agents provide structured data, and the advancement engine needs to route to
different transitions based on that data. The current format has no way to express:

- What fields an agent should submit at a given state (evidence schema)
- Which transition fires when specific evidence values are provided (conditional routing)
- Which states should invoke external processing tools before accepting evidence

The strategic design (`DESIGN-unified-koto-next.md`) defines the high-level shape:
`accepts` blocks for evidence schema, `when` conditions for routing, and
`integration` tags for processing tools. This tactical design specifies the exact
YAML syntax, compiled JSON schema, compiler validation rules, and Rust types.

koto has no released users, so these changes go directly into format_version 1
with no migration concerns.

## Decision Drivers

- **Self-describing templates**: The compiled JSON must contain enough information
  for `koto next` to generate an `expects` field telling agents what to submit,
  without the CLI needing to understand template-specific logic
- **Compile-time safety**: Non-deterministic templates (where two transitions could
  fire for the same evidence) must be caught by the compiler, not at runtime
- **Clean separation of concerns**: Gates check environmental conditions (CI passed,
  file exists). Evidence routing checks agent-submitted data. These shouldn't overlap.
- **Minimal complexity**: Only add what's needed for the advancement engine. No
  operator extensibility, no complex condition DSL
- **Existing gate compatibility**: Command gates remain useful for environmental
  checks and shouldn't be removed

## Considered Options

### Decision: How to handle the overlap between field gates and accepts/when

The current code has three gate types: `field_not_empty`, `field_equals`, and
`command`. The first two check agent-submitted evidence (does a field exist? does
it equal a value?). The new `accepts`/`when` system also checks agent-submitted
evidence, but with more expressiveness (typed schemas, conditional routing, mutual
exclusivity validation).

In the event-sourced model, evidence only enters the system through explicit agent
submission (`koto next --with-data`), and it's scoped to the current state by the
epoch boundary rule. This means field gates and `accepts`/`when` operate on the
same data through different mechanisms. The question is whether to keep both, restrict
their interaction, or remove the redundant one.

#### Chosen: Unified Model (remove field gates)

Remove `field_not_empty` and `field_equals` gate types entirely. Only `command`
gates survive. Everything field gates expressed is now expressed through
`accepts`/`when`:

- `field_not_empty: decision` becomes `accepts: {decision: {type: string, required: true}}`
- `field_equals: decision = proceed` becomes `when: {decision: proceed}` on a transition

This leaves two orthogonal concepts in the template format:
- **Command gates**: check the environment (CI passed, file exists). Koto evaluates
  these without agent involvement.
- **Accepts/when**: handle agent-submitted evidence. Agents submit data via
  `--with-data`, and `when` conditions route to the matching transition.

No interaction rules are needed because the two concepts don't overlap. A state can
have command gates (environmental prerequisites) and `accepts`/`when` (evidence
routing) without ambiguity: gates evaluate first, then `when` conditions match
against submitted evidence.

koto has no users, so removing field gates is a clean break with no migration cost.

#### Alternatives Considered

**Strict Separation**: Keep field gates but forbid them on states with `accepts`
blocks (compiler error). This eliminates the overlap ambiguity but keeps dead code
around. If field gates only work on states without `accepts`, they're checking
evidence on states that have no evidence schema, which is contradictory in the
event-sourced model where evidence is scoped to the current state.

**Coexistence with Precedence**: Allow field gates and `accepts` on the same state,
with gates evaluating first as prerequisites. Rejected because it creates a complex
mental model (two evaluation phases for the same data), semantic ambiguities (field
required by gate but optional in accepts?), and degrades the self-describing
principle (agents see an `expects` field they can't submit to while gates block).

### Decision: What `when` matching semantics to use

A `when` condition is a map of field names to expected values. When the map has
multiple fields, the matching rule determines whether a transition fires.

#### Chosen: AND semantics (all fields must match)

A transition's `when` condition matches only if every field in the map equals its
expected value in the submitted evidence. `when: {decision: proceed, priority: high}`
matches only when the agent submits both `decision=proceed` AND `priority=high`.

This is the standard semantics for key-value condition maps. SQL WHERE clauses,
Kubernetes label selectors, and GitHub Actions `on` filters all use AND for
multiple conditions. OR semantics would require a different syntax (e.g., a list
of conditions) to avoid surprising template authors.

#### Alternatives Considered

**OR semantics (any field matches)**: A transition fires if any field matches. This
would make `when: {decision: proceed, priority: high}` fire when either field
matches, which is unintuitive for a map structure. It also makes mutual exclusivity
harder to reason about -- two conditions that share no fields would both fire for
any evidence submission. Rejected because the syntax suggests AND and OR would
require a different structure.

### Decision: What scope of mutual exclusivity validation

When two transitions from the same state have `when` conditions, the compiler needs
to detect conflicts (two transitions could match the same evidence). With AND
semantics, the question is how far the compiler goes in checking this.

#### Chosen: Pairwise field-level exclusivity

The compiler checks all pairs of transitions from the same state. Two transitions
are provably exclusive if they share at least one field with disjoint values. Because
`when` uses AND semantics, a single field disagreement is enough to guarantee the
conditions can't both match.

For example:
- `{decision: proceed, priority: high}` vs `{decision: proceed, priority: low}` --
  exclusive on `priority` (different values), even though `decision` overlaps.
- `{decision: proceed}` vs `{decision: escalate}` -- exclusive on `decision`.
- `{decision: proceed}` vs `{priority: high}` -- NOT provably exclusive. No shared
  field, so both could match if the agent submits `{decision: proceed, priority: high}`.

The algorithm: for each pair of transitions with `when` conditions, find shared
fields. If any shared field has different values, the pair is exclusive. If no shared
field exists, or all shared fields have the same value, the compiler rejects the
template as potentially non-deterministic.

This catches both single-field and multi-field conflicts without combinatorial
explosion. The check is O(n^2) in the number of transitions per state, which is
fine since states rarely have more than a handful of transitions.

#### Alternatives Considered

**Single-field only**: Only validate transitions whose `when` has exactly one field.
Multi-field conditions are left to the template author. Simpler to implement, but
misses real conflicts. Two transitions with `{decision: proceed, priority: high}` and
`{decision: proceed, priority: high}` (identical multi-field conditions) would pass
the compiler unchecked. The pairwise check is only marginally more complex and catches
strictly more errors.

**Full satisfiability analysis**: Model conditions as logical formulas and check
whether any assignment satisfies both. Correct but overkill -- the pairwise
field-level check covers every practical case. Full SAT analysis adds complexity
with no real benefit for equality-only conditions.

**Skip validation entirely**: Detect conflicts at runtime only. Rejected because
compile-time validation catches real mistakes cheaply. Non-deterministic routing
errors at runtime are harder to debug than compiler errors.

### Decision: How `koto next` output changes in issue #47

With this change, `transitions` changes from `Vec<String>` to `Vec<Transition>`.
Serializing the structured type directly would change the `koto next` JSON output
from `["target1", "target2"]` to `[{"target": "target1"}, {"target": "target2"}]`.

#### Chosen: Keep current output, defer contract change to #48

`koto next` maps structured transitions to target names
(`transitions.iter().map(|t| &t.target)`) and outputs the same flat string array.
The `accepts` and `when` data are loaded internally but don't appear in output.
Issue #48 designs the full output contract (adding `expects`, `integration.available`)
as a separate change.

This keeps issue boundaries clean. #47's job is the template format, not the CLI
output contract. If #47 changed the output shape, #48 would have to work around an
interim format rather than designing the output from scratch.

#### Alternatives Considered

**Output structured transitions**: Serialize `Transition` objects directly in
`koto next` output. Rejected because this preempts #48's output contract design.
The `when` conditions in CLI output have no consumer until the advancement engine
(#49) exists, so exposing them early adds complexity with no benefit.

**Flat list plus accepts**: Extract targets as flat strings but also add the `accepts`
block to output so agents can see what evidence to submit. A reasonable middle ground,
but still preempts #48's `expects` field design. Better to let #48 design the full
agent-facing output schema in one pass.

### Decision: How the `integration` field compiles

States can declare an `integration` tag naming a processing tool. The question is
whether the compiler validates integration names against project configuration.

#### Chosen: Verbatim passthrough, no compile-time validation

The compiler stores `integration` as `Option<String>`, passing the string through
from YAML to compiled JSON with no name validation. The only check is that the
string is non-empty if present.

The strategic design explicitly says missing integration config is not a template
load-time error. Integration names resolve at runtime from project configuration.
The compiler can't know what integrations are installed, and templates should be
portable across projects with different integration setups.

#### Alternatives Considered

**Validate against config**: Check integration names against a project config file
at compile time. Rejected because it couples template compilation to project-specific
configuration, breaking portability. A template compiled in one project would fail
in another that has the same integration under a different name. The integration
runner (#49) handles resolution at runtime where project config is available.

## Decision Outcome

### Summary

The template format gains three new constructs and drops field gates. The
`format_version` stays at 1 since koto hasn't shipped yet.

Each state can declare an `accepts` block: a map of field names to schemas. Each
field has a `type` (enum, string), a `required` flag, and for enums a `values` list
of allowed values. This block is the source of truth for what evidence an agent
should submit at this state.

Transitions change from plain strings to structured objects. Each transition has a
`target` state and an optional `when` condition: a map of field names to expected
values. `when` uses AND semantics -- all fields must match for the transition to
fire. When an agent submits evidence via `--with-data`, the advancement engine
matches the submitted values against each transition's `when` conditions and routes
to the first match. The compiler validates that `when` conditions across a state's
transitions are mutually exclusive: for each pair of transitions, at least one
shared field must have disjoint values. If two transitions share no fields or agree
on all shared fields, the compiler rejects the template as non-deterministic.

States can declare an `integration` field: a string tag naming a processing tool.
The compiler stores it verbatim. The integration runner (#49) resolves the tag to an
actual command at runtime through project configuration. Missing config is not a
compile-time error.

`field_not_empty` and `field_equals` gate types are removed. `command` gates remain
unchanged.

### Rationale

The unified model produces the simplest design because it follows from the
event-sourced architecture. Evidence enters through typed events and is scoped by
the epoch boundary. Field gates were checking the same data that `accepts`/`when`
now handles with better expressiveness and compile-time validation. Keeping both
would require defining interaction rules for no practical benefit. Removing field
gates yields a net code reduction and fewer concepts for template authors.

## Solution Architecture

### Overview

This design adds three constructs to the template schema and removes two gate types.
The changes touch three files: type definitions (`src/template/types.rs`), the
compiler (`src/template/compile.rs`), and the CLI (`src/cli/mod.rs`). The compiled
JSON output gains new fields but keeps its flat structure. `format_version` remains 1.

### YAML Source Format

A template with evidence routing looks like this:

```yaml
---
name: review-workflow
version: "1.0"
initial_state: analyze_results
states:
  analyze_results:
    integration: delegate_review
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
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
        command: ./check-ci.sh
  deploy:
    transitions:
      - target: complete
  escalate_review:
    transitions:
      - target: complete
  complete:
    terminal: true
---
## analyze_results
Review the test output and decide the next step.
## deploy
Deploy to production.
## escalate_review
Escalate to senior review team.
## complete
Review workflow complete.
```

States without `accepts` or `when` use the same syntax as before but with structured
transition objects instead of plain strings.

### Compiled Types

Four Rust types define the compiled schema:

**TemplateState** gains three fields:
- `accepts: Option<BTreeMap<String, FieldSchema>>` -- evidence field schema
- `transitions: Vec<Transition>` -- replaces `Vec<String>`
- `integration: Option<String>` -- processing tool tag

**Transition** (new): holds `target: String` and `when: Option<BTreeMap<String, serde_json::Value>>`.
Unconditional transitions omit `when`. Conditional transitions map field names to
expected values.

**FieldSchema** (new): holds `field_type: String` (enum, string, number, boolean),
`required: bool`, optional `values: Vec<String>` for enums, and optional `description`.

**Gate**: `GATE_TYPE_FIELD_NOT_EMPTY` and `GATE_TYPE_FIELD_EQUALS` constants are
removed; only `GATE_TYPE_COMMAND` remains. The `field` and `value` fields on the
struct are removed since command gates don't use them. The compiler rejects field
gate types with an error message pointing to `accepts`/`when` as the replacement.

### Compiler Validation

The compiler adds six validation rules and removes two:

**Added:**
1. `when` fields must reference fields declared in the state's `accepts` block
2. `when` values for enum fields must appear in the field's `values` list
3. Empty `when` blocks are rejected
4. Pairwise mutual exclusivity: for each pair of transitions with `when` conditions,
   at least one shared field must have disjoint values. Transitions with no shared
   fields are rejected as potentially non-deterministic.
5. `when` conditions require the state to have an `accepts` block
6. `when` condition values must be JSON scalars (strings, numbers, booleans); arrays
   and objects are rejected

**Removed:**
1. `field_not_empty` gate validation
2. `field_equals` gate validation

The mutual exclusivity check works pairwise across all transitions with `when`
conditions on the same state. Because `when` uses AND semantics (all fields must
match), two conditions are provably exclusive if any shared field has different
values. The check is O(n^2) in transitions per state, which is fine in practice.

### CLI Impact

`koto next` currently serializes `transitions: Vec<String>` directly. After this
change, transitions become `Vec<Transition>` (structured objects with `target` and
`when`). Serializing these directly would break the current output contract. For
issue #47 scope, `koto next` maps structured transitions to target names
(`transitions.iter().map(|t| &t.target)`) to preserve the current flat string array
output. The full output contract change (adding `expects`, `integration.available`)
is #48's responsibility.

`koto template compile` and `koto template validate` work unchanged once the types
and compiler support the new constructs.

### Data Flow

```
Template YAML  -->  compile()  -->  Compiled JSON (format_version: 1)
                       |
                  Validation:
                  - when/accepts consistency
                  - mutual exclusivity
                  - gate type restriction
                       |
                       v
                 CompiledTemplate
                       |
          koto next reads compiled template
          extracts targets from transitions
          outputs current format (until #48)
```

## Implementation Approach

### Phase 1: Type Definitions

Update `src/template/types.rs`:
- Add `Transition` and `FieldSchema` structs
- Add `accepts`, `integration` fields to `TemplateState`
- Change `transitions` from `Vec<String>` to `Vec<Transition>`
- Remove `GATE_TYPE_FIELD_NOT_EMPTY` and `GATE_TYPE_FIELD_EQUALS` constants
- Clean up dead `field` and `value` fields on `Gate` struct (only `command` and
  `timeout` are needed for command gates)
- Update `validate()` to add new rules (when/accepts consistency, mutual exclusivity,
  scalar value check, gate type restriction)

Deliverables:
- Updated `src/template/types.rs`
- Unit tests for new validation rules

### Phase 2: Compiler

Update `src/template/compile.rs`:
- Add `SourceTransition` and `SourceFieldSchema` deserialization types
- Update `SourceState` with `accepts`, `integration`, structured `transitions`
- Update `compile_gate()` to only accept command gates
- Transform source types to compiled types (HashMap to BTreeMap, source to compiled)

Deliverables:
- Updated `src/template/compile.rs`
- Compiler tests for templates with evidence routing (valid and invalid)

### Phase 3: CLI and Tests

Update `src/cli/mod.rs`:
- Adjust `koto next` to extract target names from structured transitions
  (preserving current output format for #48)

Update integration tests:
- Convert all test templates to use structured transitions (including
  `minimal_template()` and any embedded template fixtures in `compile.rs` tests)
- Update hello-koto template transitions from `[done]` to `[{target: done}]`
- Add test cases for new validation errors

Note: all three phases land in a single PR. Phase 1 changes break compilation
until Phase 2 updates the compiler, so these aren't independently mergeable.

Deliverables:
- Updated `src/cli/mod.rs`
- Updated `tests/integration_test.rs`
- Updated plugin templates

## Security Considerations

### Download verification

Not applicable. This change doesn't affect how binaries or templates are downloaded.
Templates are local markdown files read from disk.

### Execution isolation

Command gates execute shell commands with the same permissions as the koto process.
This is unchanged. The new constructs don't add execution vectors: `accepts` and
`when` are declarative schema, and `integration` is a string tag that doesn't
execute anything at compile time. The integration runner (#49) handles execution
with its own isolation model.

### Supply chain risks

Not applicable. Templates are authored locally, not fetched from a registry. The
compiler reads local files only. No new external dependencies are introduced.

### User data exposure

`accepts` schemas describe what fields agents should submit but don't transmit data
themselves. Evidence data flows through `koto next --with-data`, which is an existing
CLI path. The `integration` tag is stored in compiled JSON on disk. No new data
exposure vectors.

## Consequences

### Positive

- Two orthogonal concepts (command gates for environment, accepts/when for evidence)
  with no overlap or interaction rules
- Compile-time detection of non-deterministic templates through mutual exclusivity
  validation
- Self-describing templates: the `accepts` block tells agents exactly what to submit
  without external documentation
- Net code reduction from removing field gate types and their validation logic
- Clean foundation for #48 (output contract) and #49 (integration runner) to build on

### Negative

- `accepts` syntax is more verbose than field gates for simple required-field checks:
  `accepts: {decision: {type: string, required: true}}` vs `field_not_empty: decision`
- Transitions with `when` conditions on disjoint fields (no shared field names) can't
  be proven exclusive at compile time and are rejected, even if they might be safe in
  practice. Template authors must add a shared discriminator field.
- The `serde_json::Value` type for `when` condition values is loosely typed; the
  compiler validates enum values but other types are checked only at runtime

### Mitigations

- The verbosity cost is minimal in practice because `accepts` carries more information
  (type, description) that field gates didn't support. Template authors write `accepts`
  once per state, not per transition.
- Rejecting disjoint-field conditions is the safe default. If two transitions test
  completely different fields, both could match any evidence that includes both fields.
  Requiring a shared discriminator makes the routing logic explicit and deterministic.
  This is a compile-time constraint that pushes template authors toward clearer designs.
- The `serde_json::Value` flexibility is intentional: it allows future type extensions
  (numeric comparisons, boolean flags) without schema changes. Current templates use
  string equality matching, which the compiler does validate for enum fields.
