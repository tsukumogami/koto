---
status: Accepted
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Features 1 and 2 of the gate-transition contract landed structured gate output
  and the override mechanism, but left three validation gaps in the template compiler.
  override_default values are accepted as any JSON without schema checking. Transition
  when clauses can reference gate names that don't exist in the state or field names
  that aren't in the gate type's schema. No check verifies that override defaults can
  satisfy at least one transition, so a template with an unreachable override path
  compiles silently and fails at runtime.
decision: |
  Add a GateSchemaFieldType enum and gate_type_schema() function to template/types.rs
  alongside the existing GATE_TYPE_* constants. Use this registry in validate() for
  two new checks: exact-match validation of override_default (all fields present, no
  extras, correct types), and gates.* when clause validation (gate name exists in
  state, field name in gate type schema, malformed paths are errors). Add a reachability
  check that applies override defaults to all gates and verifies at least one pure-gate
  transition can fire, using a local resolve_gates_path helper to avoid a circular
  dependency with engine/advance.rs.
rationale: |
  Placing the schema registry in template/types.rs alongside existing GATE_TYPE_*
  constants is the only option that avoids the circular dependency (gate.rs imports
  from types.rs). Exact match for override_default catches partial overrides that
  subset validation would miss, since a missing field silently breaks any when clause
  checking it. Scoping reachability to pure-gate transitions avoids false positives
  on mixed states where agent evidence is also required. The local resolve_gates_path
  helper (5 lines) keeps the matching algorithm identical to the engine without
  introducing a cross-module dependency.
---

# DESIGN: Gate contract compiler validation

## Status

Accepted

## Upstream Design Reference

This is Feature 3 of the gate-transition contract roadmap. Feature 1 design:
[DESIGN-structured-gate-output](current/DESIGN-structured-gate-output.md).
Feature 2 design: [DESIGN-gate-override-mechanism](current/DESIGN-gate-override-mechanism.md).

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
checking `gates.ci_check.exit_code` silently fails to match.

**Gap 2: gates.* when clause references are unchecked.** A transition `when`
clause can reference `gates.nonexistent_gate.exit_code` or
`gates.ci_check.phantom_field`, and the compiler accepts it. At runtime, the resolver
finds nothing at the dot-path and the condition silently fails.

**Gap 3: No reachability check.** A template can declare override defaults that
don't satisfy any transition's `when` conditions. When an agent overrides all gates,
no transition fires — a dead end that the template author didn't intend and has no
way to detect before deployment.

This is Feature 3 of the gate-transition contract roadmap (#118). It implements
PRD R9: compiler validation of the full gate/transition/override contract.

## Decision Drivers

- **No circular dependencies.** `template/types.rs` must not import from `gate.rs`
  — gate.rs already imports the Gate struct and GATE_TYPE_* constants from types.rs.
  Gate schema information must live in types.rs or a shared location.
- **Actionable error messages.** Each error must name the specific state, gate, and
  field involved, not just report a generic schema violation.
- **No false positives on valid templates.** The reachability check must not reject
  valid templates that mix gate output and agent evidence in `when` clauses — those
  states require agent evidence in addition to gate output, and the compiler can't
  know what evidence the agent will provide.
- **Extensible to future gate types.** Adding a new gate type (jira, http,
  json-command) should require updating only one place. The schema registry is that
  place.
- **Build on existing infrastructure.** The existing `validate_evidence_routing()`,
  gate type constants, and transition matching algorithm should be used where
  possible rather than duplicated.

## Considered Options

### Decision 1: Gate type schema location

The compiler's `validate()` method in `src/template/types.rs` needs to know each
gate type's output field names and value types to validate `override_default` values
and `when` clause field references. Gate type constants (`GATE_TYPE_COMMAND`, etc.)
already live in `template/types.rs`. The schemas are currently implicit in `gate.rs`'s
`built_in_default()` function (`{"exit_code": 0, "error": ""}` encodes that `command`
gates have a number field `exit_code` and a string field `error`).

The key constraint: `gate.rs` imports from `template/types.rs`. Any reverse import
would create a circular dependency. The schema must live in `types.rs` or be
derivable without importing `gate.rs`.

#### Chosen: Static GateTypeSchema registry in template/types.rs

Add a `GateSchemaFieldType` enum (`Number`, `String`, `Boolean`) and a
`gate_type_schema(gate_type: &str) -> Option<&'static [(&'static str, GateSchemaFieldType)]>`
function alongside the existing `GATE_TYPE_*` constants. No new module boundaries,
no circular imports. `validate()` calls `gate_type_schema()` directly.

`template/types.rs` already owns the GATE_TYPE_* constants and the full `validate()`
method — it's the natural home for compile-time gate contract knowledge. The
`GateSchemaFieldType` enum provides typed exhaustive matching rather than string
comparisons, catching omissions at compile time when new gate types are added.

#### Alternatives Considered

**Derive schema from built_in_default() output shape**: move `built_in_default()`
from `gate.rs` to `template/types.rs` and derive field types from JSON value shapes.
Rejected because it separates `built_in_default()` from the evaluators it supports
in `gate.rs`, creating a maintenance gap where a new gate type must be updated in
`gate.rs` (evaluator) and `template/types.rs` (defaults) with no compiler enforcement
linking them. Deriving types from `serde_json::Value` shapes also requires runtime
inspection for values that could be declared statically.

**Inline schema in validate() match arms**: hardcode field names and types directly
in each match arm of `validate()`. Rejected because the same schema is needed in
multiple validation passes (override_default and when clause validation are separate),
and no shared abstraction means surgical edits in multiple places for each new gate
type. Loses exhaustive match coverage that a typed enum provides.

---

### Decision 2: override_default validation strictness

When a gate declares `override_default: <value>`, the compiler needs to validate it
against the gate type's output schema. At runtime (Feature 2), `override_applied`
(sourced from `override_default` or the built-in default) is injected as the gate's
entire output into the `gates.*` evidence map — there's no merging with actual gate
output. Whatever object lands in `override_applied` is the complete, authoritative
output for that gate.

This runtime contract has a direct implication: if `override_default` is partial,
missing fields are simply absent from the evidence map, causing any `when` condition
checking them to silently fail. The question is whether to enforce completeness at
compile time or rely on the reachability check (Decision 4) to catch dead ends.

#### Chosen: Exact match

`override_default` validation requires:
1. The value must be a JSON object (not null, array, or scalar)
2. All schema fields must be present (no missing fields)
3. No extra fields beyond the schema
4. Each field's JSON value type must match the schema type (Number/String/Boolean)

This closes the contract at compile time. The built-in defaults (`{exit_code: 0, error: ""}`,
`{exists: true, error: ""}`, `{matches: true, error: ""}`) all satisfy exact match —
custom `override_default` values should meet the same contract.

Error messages name state, gate, field, and expected type:
```
state "verify" gate "ci_check": override_default missing required field "error"
  (command schema requires: exit_code: number, error: string)

state "verify" gate "ci_check": override_default has unknown field "status"
  (command schema: exit_code, error)

state "verify" gate "ci_check": override_default field "exit_code" has wrong type
  expected: number, found: string
```

#### Alternatives Considered

**Subset match**: present fields must have correct types, extra fields rejected, missing
fields allowed. Rejected because the reachability check only fires when no transition
fires — if override_default has `{exit_code: 0}` (missing `error`), and some `when`
clause checks only `exit_code`, that transition resolves and reachability passes. But
a `when` clause checking `gates.ci_check.error: ""` on another transition silently
fails at runtime. Subset passes D4 but leaves a class of runtime dead ends undetected.

**Type-check only**: extra fields accepted, missing fields accepted, wrong types
rejected. Rejected because it provides minimal compile-time value: gate schemas are
small (2-3 fields each) and fully documented. Accepting unknown fields silently
discards data the author thought was meaningful.

**Warn only**: compilation succeeds; warnings emitted for type mismatches. Rejected
because it directly contradicts PRD acceptance criteria ("compiler rejects"). The
failure mode is a silent dead end at runtime — the agent overrides a gate, nothing
transitions, the workflow stalls. Early rejection at compile time is the correct
enforcement point.

---

### Decision 3: gates.* when clause validation depth

`validate_evidence_routing()` in `types.rs` already splits `when` clause fields into
`gates.*` keys and agent evidence keys. For `gates.*` keys, it currently only checks
that the comparison value is a JSON scalar. It doesn't check that the gate name exists
in the state's `gates:` block or that the field name is valid for the gate type.

PRD R9: "Transition when clauses that reference gates.* fields reference valid gate
names and fields from the gate type's schema."

#### Chosen: Gate + field existence

For each `gates.*` key in a `when` clause, validate:
1. The path has exactly 3 dot-separated segments (`gates.<gate_name>.<field_name>`)
2. `gate_name` names a gate declared in the state's `gates:` block
3. `field_name` is a valid field in that gate type's schema (from `gate_type_schema()`)

Malformed paths (2 segments or 4+ segments) are compile errors — the `gates.` prefix
signals intent to reference gate output, so any unexpected shape is an authoring mistake
that would produce a when condition that can never match.

This is the minimum that fully satisfies PRD R9. The implementation cost is low:
two lookups per `gates.*` key against the state's gates map and the schema registry.

#### Alternatives Considered

**Gate existence only**: verify gate name exists, skip field name validation. Rejected
because it leaves field-name typos (e.g., `exitt_code`) silently producing when
conditions that can never match at runtime. Adds complexity without completing PRD R9.

**Gate + field existence + value type compatibility**: additionally verify the
comparison value's JSON type matches the schema field type. Deferred, not permanently
rejected — type mismatch produces the same observable outcome as a field name typo
(condition silently never fires), but type checking couples the validator to the JSON
type system before the usage patterns are understood. It's a natural extension once
gate/field validation is in place.

---

### Decision 4: Reachability check scope and implementation

PRD R9 requires: when override defaults are applied to all gates, at least one
transition must resolve. This is a compile-time check that catches templates where
an agent could override all gates and still have no path forward.

The key challenge: states can have mixed transitions that require BOTH gate output
AND agent evidence. Applying override defaults to all gates but having no agent
evidence means those transitions can never match at compile time — but this is not
a dead end in practice (the agent still needs to submit evidence). The check must
not flag these states as dead ends.

#### Chosen: Pure-gate transitions only, inline resolver call

**Scope**: check only states that have at least one transition whose `when` clause
contains exclusively `gates.*` fields (no agent evidence keys). For such states,
gate override defaults are the only input required. States where every transition
requires agent evidence are exempt — they're not dead ends caused by bad override
defaults.

**Implementation**: add a local `fn resolve_gates_path<'a>(evidence: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value>` helper to `types.rs` (5 lines, identical algorithm to `resolve_value` in `advance.rs`). For each state with pure-gate transitions:
1. Build a `serde_json::Value` evidence map using each gate's `override_default`
   (if declared) or the built-in default from `gate_type_builtin_default()` (a new
   companion function to `gate_type_schema()` in `types.rs` — see Decision 5)
2. For each pure-gate transition, check if all `gates.*` when conditions match
3. If no pure-gate transition matches: compile error

`resolve_gates_path` is not imported from `advance.rs` because `types.rs` can't
import from `advance.rs` (circular dep). The 5-line implementation is not the
"analytical pattern matching" rejected below — it's the exact same algorithm
(split on `.`, walk the map, compare values). The risk of divergence is minimal
because both use exact JSON equality, which is stable.

#### Alternatives Considered

**All gated states**: check every state with gates, regardless of transition structure.
Rejected because it produces false positives for states with mixed transitions —
applying override defaults with no agent evidence yields `NeedsEvidence`, which is
not a dead end. The existing `validate_evidence_routing()` code already separates
gate fields from agent fields; ignoring that separation here would be inconsistent.

**Skip reachability entirely**: treat as best-effort per PRD Known Limitations.
Rejected because the implementation cost for the pure-gate scope is low, and the
check provides genuine value: without it, a template author with an unreachable
override path has no compile-time feedback and no indication that `koto overrides record`
would leave the workflow stalled.

**Analytical pattern matching without resolve_gates_path**: walk when conditions
analytically, comparing against override default values using custom logic. Rejected
because it duplicates comparison logic from `resolve_value` in `advance.rs`, creating
risk of subtle divergence between compiler and engine behavior for edge cases.

---

### Decision 5: Compile-time built-in default values

The reachability check needs built-in default values for gates that don't declare
`override_default` (e.g., a command gate with no custom override default uses
`{exit_code: 0, error: ""}`). `built_in_default()` in `gate.rs` has these values,
but `types.rs` can't import from `gate.rs`.

#### Chosen: Companion function gate_type_builtin_default() in types.rs

Add `fn gate_type_builtin_default(gate_type: &str) -> Option<serde_json::Value>` to
`template/types.rs` alongside `gate_type_schema()`. The values mirror
`gate.rs::built_in_default()`:
- `command`: `{"exit_code": 0, "error": ""}`
- `context-exists`: `{"exists": true, "error": ""}`
- `context-matches`: `{"matches": true, "error": ""}`

This is a minor duplication of static constant values. The duplication is acceptable
because the values are stable (tied to the gate type's pass condition) and any change
to them requires updating both functions in tandem — a natural, visible coupling.

#### Alternatives Considered

**Move built_in_default() to types.rs and have gate.rs call it**: reverses the
current dependency direction. Rejected because it separates `built_in_default()` from
the evaluator functions in `gate.rs` that use it during override recording, making the
override execution model harder to follow in one place.

**Create a shared constants module** (e.g., `src/template/gate_defaults.rs`):
both `gate.rs` and `types.rs` import from it. Rejected as disproportionate complexity
for three small JSON objects. The duplication is small and bounded.

## Decision Outcome

### Summary

The compiler gains a `GateSchemaFieldType` enum (`Number`, `String`, `Boolean`) and
two new functions in `template/types.rs`: `gate_type_schema()` returning a static
field list per gate type, and `gate_type_builtin_default()` returning the default
override value per gate type. These functions live alongside the existing `GATE_TYPE_*`
constants, making them the single authoritative source for gate type contracts at
compile time.

The `validate()` method gains three new validation passes, all within the existing
state-iteration loop:

**override_default validation** (D2): for each gate with a declared `override_default`,
the value must be a JSON object with exactly the schema fields (`exit_code`+`error`
for command, `exists`+`error` for context-exists, `matches`+`error` for
context-matches) and compatible value types. Missing fields, extra fields, and type
mismatches are hard errors with messages that name state, gate, field, and expected type.

**gates.* when clause validation** (D3): added to `validate_evidence_routing()`.
Each `gates.*` key must have exactly 3 dot-separated segments. Segment 2 (gate name)
must exist in the state's `gates:` block. Segment 3 (field name) must be in
`gate_type_schema()` for that gate type. Two-segment and four-segment paths are errors.

**Gate reachability check** (D4): after the other validations pass, for each state
with at least one pure-gate-only transition (a `when` clause with only `gates.*` fields),
build a `serde_json::Value` evidence map using each gate's `override_default` (or
`gate_type_builtin_default()` if none declared), then check if at least one pure-gate
transition's `when` conditions are all satisfied by the map. If not, compile error.
A `resolve_gates_path` helper (5 lines) handles dot-path traversal without importing
`advance.rs`.

States without any pure-gate-only transition are exempt from the reachability check.
Mixed-evidence states (where all `when` clauses require agent evidence in addition to
gate fields) are valid — the compiler doesn't know what evidence the agent will provide.

The reachability check runs after D2 and D3 pass, so it can assume override defaults
are well-formed and when clause gate references are valid.

### Rationale

The five decisions fit together without friction. D1's schema registry provides the
field and type information that D2 and D3 need. D2's exact match ensures D4 operates
on complete, well-typed override defaults — no missing fields to handle as special
cases during reachability. D3's gates.* path validation means D4 can trust that all
`gates.*` when conditions reference real gates and real fields. D5's
`gate_type_builtin_default()` companion function keeps the circular dep avoided by D1
while giving D4 the built-in values it needs.

The no-false-positives constraint on the reachability check is satisfied by the pure-gate
scope. Mixed-evidence states require agent input that the compiler can't predict, so
exempting them is the only sound policy. The local `resolve_gates_path` helper is
minor: 5 lines of trivial code, same algorithm as `resolve_value` in `advance.rs`,
negligible divergence risk.

## Solution Architecture

### Overview

All new code lives in `src/template/types.rs`. Three new functions are added at the
module level. The `validate()` method gains three new validation passes inside the
existing state-iteration loop. No new files, no new module dependencies.

### Components

**`GateSchemaFieldType` enum** (`src/template/types.rs`)
```
pub enum GateSchemaFieldType { Number, String, Boolean }
```
Represents the JSON value type of a gate output field. Used by `gate_type_schema()`,
the override_default validation, and the reachability check.

**`gate_type_schema(gate_type: &str) -> Option<&'static [(&'static str, GateSchemaFieldType)]>`**
Returns the output field schema for known gate types:
- `command`: `[("exit_code", Number), ("error", String)]`
- `context-exists`: `[("exists", Boolean), ("error", String)]`
- `context-matches`: `[("matches", Boolean), ("error", String)]`
- Unknown type: `None`

**`gate_type_builtin_default(gate_type: &str) -> Option<serde_json::Value>`**
Returns the built-in default override value for known gate types:
- `command`: `{"exit_code": 0, "error": ""}`
- `context-exists`: `{"exists": true, "error": ""}`
- `context-matches`: `{"matches": true, "error": ""}`
- Unknown type: `None`

**`resolve_gates_path(evidence: &Value, path: &str) -> Option<&Value>`** (private)
Five-line helper for dot-path traversal: split path on `.`, walk nested JSON maps.
Used only by the reachability check. Same algorithm as `resolve_value` in
`src/engine/advance.rs`.

**`validate()` additions**
Three new passes inside the existing `for (state_name, state) in &self.states` loop:
1. `validate_override_defaults(state_name, state)` — D2 check
2. Extended `validate_evidence_routing(state_name, state)` — D3 additions to existing function
3. `validate_gate_reachability(state_name, state)` — D4 check

### Key interfaces

**gate_type_schema output shape:**
```rust
// Example: gate_type_schema(GATE_TYPE_COMMAND) returns:
Some(&[("exit_code", GateSchemaFieldType::Number), ("error", GateSchemaFieldType::String)])
```

**override_default validation call site** (inside validate()):
```rust
for (gate_name, gate) in &state.gates {
    if let Some(override_val) = &gate.override_default {
        // validate override_val against gate_type_schema(gate.gate_type)
    }
}
```

**Reachability check evidence map shape:**
```json
{
  "gates": {
    "ci_check": {"exit_code": 0, "error": ""},
    "schema_check": {"exists": true, "error": ""}
  }
}
```

**Pure-gate transition identification:**
A transition is pure-gate if all its `when` conditions start with `"gates."`.

### Data flow

```
Template YAML source
  |
  v
compile() in compile.rs -- parse, compile_gate() per gate, build CompiledTemplate
  |
  v
template.validate() in types.rs:
  For each state:
    1. Validate gate type fields (existing: command/key/pattern required)
    2. Validate override_default (NEW D2): check each gate.override_default
       against gate_type_schema() -- all fields, no extras, correct types
    3. validate_evidence_routing (EXTENDED D3): add gates.* path depth check
       and gate_name/field_name lookup against state.gates + gate_type_schema()
    4. Validate gate reachability (NEW D4):
       - Collect pure-gate transitions
       - Skip if none
       - Build gate evidence map from override_defaults/builtins
       - Check at least one pure-gate transition's when conditions match
  |
  v
Compiled + validated CompiledTemplate (or anyhow::Error)
```

## Implementation Approach

All changes are in `src/template/types.rs`. The implementation can be done in one
cohesive commit or split into three focused commits.

### Phase 1: Schema registry

Add `GateSchemaFieldType` enum and `gate_type_schema()` function. Add
`gate_type_builtin_default()`. Add unit tests for all three gate types and the
unknown-type case.

Deliverables:
- `GateSchemaFieldType` enum with `Number`, `String`, `Boolean` variants
- `gate_type_schema()` returning static field slices for the three built-in types
- `gate_type_builtin_default()` returning built-in default values
- Unit tests: `gate_type_schema_command`, `gate_type_schema_context_exists`,
  `gate_type_schema_context_matches`, `gate_type_schema_unknown_returns_none`

### Phase 2: override_default validation

Add override_default validation to `validate()`. Unit tests covering: all valid
combinations, missing field error, extra field error, wrong type error (one per
gate type), non-object override_default.

Deliverables:
- Validation loop inside existing gate iteration in `validate()`
- Unit tests (via `compile()` with temp files): at least 6 new tests

### Phase 3: gates.* when clause validation

Extend `validate_evidence_routing()` to validate gates.* path depth (2-segment
error, 4+-segment error), gate name existence, and field name existence.

Deliverables:
- Path depth validation (2 segments → error, 4+ segments → error)
- Gate name existence check
- Field name existence check
- Unit tests: nonexistent gate name, nonexistent field name, 2-segment path,
  4-segment path, valid reference (all 3 gate types)

### Phase 4: Reachability check

Add `resolve_gates_path()` helper and `validate_gate_reachability()` called from
`validate()`. Add a compile-time warning print to stderr for unreferenced gate output
(PRD R9: "warn on gates whose output isn't referenced by any when clause").

Deliverables:
- `resolve_gates_path()` private helper
- `validate_gate_reachability()` method on `CompiledTemplate`
- Reachability error for states where no pure-gate transition fires under overrides
- Stderr warning for gates with no `when` clause references (does not fail compilation)
- Unit tests: dead-end state (override defaults satisfy no transition), reachable
  state (override defaults satisfy one transition), mixed-evidence state (exempt),
  no-gates state (exempt), unreferenced gate warning

## Security Considerations

The compiler runs on template files at authoring time, not at workflow execution time.
All three new validation passes are read-only operations on the parsed template data
structure — they add rejection paths but don't execute arbitrary code, make network
calls, or access the filesystem beyond the already-read template file.

Two implementation-level concerns are worth noting:

**Pass ordering constraint.** The segment-count check (D3: gates.* path depth
validation) must complete successfully before the reachability evidence walk (D4)
processes any path strings. A malformed `gates.*` path reaching the reachability
walker would silently return `None` instead of producing a clear validation error,
yielding a misleading reachability result. The three validation passes must run
in order — D2 (override_default), D3 (when clause paths), D4 (reachability) — and
D4 must not run if D2 or D3 produced errors.

**Duplication synchronization.** Two small duplications exist by design (circular
dependency constraint):
- `gate_type_builtin_default()` in `types.rs` mirrors `built_in_default()` in `gate.rs`
- `resolve_gates_path()` in `types.rs` mirrors `resolve_value()` in `advance.rs`

The mitigation is twofold: cross-reference comments in each function linking it to
its counterpart, and unit tests that assert `gate_type_builtin_default()` returns
values identical to `built_in_default()` for every `GATE_TYPE_*` constant. This
catches drift when a new gate type is added.

No new attack surface. No network access, no subprocess execution, no privilege
escalation, and no sensitive data is accessed or transmitted.

## Consequences

### Positive

- Template authors get compile-time errors for override_default schema mismatches,
  gate name typos in `when` clauses, field name typos in `when` clauses, and
  unreachable override paths — all previously silent runtime failures
- The schema registry (`gate_type_schema()`) is the single canonical source for
  gate output field definitions, making future gate type additions a one-file change
- The reachability check prevents a class of override-related dead ends without
  producing false positives on valid mixed-evidence states
- Exact match for override_default enforces the same contract as the built-in
  defaults, keeping the compile-time and runtime contract aligned

### Negative

- Templates with malformed `override_default` values that previously compiled will
  now fail — technically a breaking change, but these templates had silent runtime
  bugs (their overrides would produce evidence that didn't match any `when` condition)
- `gate_type_builtin_default()` in types.rs duplicates the values from
  `built_in_default()` in gate.rs — two places to update if a gate type's default
  changes (though this is a very stable interface)
- `resolve_gates_path()` in types.rs is a functional duplicate of `resolve_value()`
  in advance.rs — same algorithm, same risk of divergence

### Mitigations

- Templates broken by the override_default validation had silent runtime failures
  before; the compile error is strictly more useful than the previous silent behaviour
- Both duplications (`gate_type_builtin_default` and `resolve_gates_path`) are small,
  well-understood functions with no branching — divergence risk is minimal. A comment
  linking each to its counterpart is sufficient
- The warning for unreferenced gate output (stderr, non-fatal) gives authors feedback
  without blocking compilation, addressing legitimate cases where a gate is
  intentionally running but its output isn't used for routing
