---
status: Accepted
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Gate evaluation returns a boolean GateResult enum (Passed/Failed/TimedOut/
  Error) that carries no structured data. The advance loop uses a single
  gate_failed boolean to decide whether to block or advance. Transition
  routing can't use gate output because there isn't any -- the resolver only
  matches agent-submitted evidence. This is Feature 1 of the gate-transition
  contract roadmap: the foundation that all other features build on.
decision: |
  Replace GateResult with StructuredGateResult carrying a GateOutcome enum
  and serde_json::Value output. Gate data enters the evidence map as nested
  JSON under gates.*, and the transition resolver gains dot-path traversal
  for when clause matching. Pass/fail determined by the outcome field.
rationale: |
  StructuredGateResult provides both control flow (outcome) and data (output)
  in one type. Dot-path traversal is a ~5-line helper that unlocks gate data
  for routing while flat keys pass through transparently. The outcome field
  keeps pass/fail logic in each gate evaluator where it belongs.
---

# DESIGN: Structured gate output

## Status

Accepted

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

## Considered options

### Decision 1: How gate evaluation produces structured data

The `GateResult` enum in `src/gate.rs` has four variants that carry minimal
data (Passed, Failed{exit_code}, TimedOut, Error{message}). Gate evaluation
functions need to return structured JSON matching each gate type's schema
alongside the pass/fail outcome that the advance loop uses for control flow.

#### Chosen: StructuredGateResult with outcome enum and JSON output

Replace `GateResult` with a `StructuredGateResult` struct carrying two
fields: `outcome: GateOutcome` (a simplified enum: Passed, Failed, TimedOut,
Error) for control flow, and `output: serde_json::Value` for the structured
data matching the gate type's schema.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateOutcome {
    Passed,
    Failed,
    TimedOut,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredGateResult {
    pub outcome: GateOutcome,
    pub output: serde_json::Value,
}
```

Derives `Serialize`/`Deserialize` proactively -- not needed for Feature 1
but Feature 2's event logging will require it. Zero cost to add now.

Each gate evaluator constructs both: the outcome for the advance loop's
pass/fail check, and the JSON output for injection into the evidence map.
Command gates produce `{"exit_code": N, "error": ""}`, context-exists gates
produce `{"exists": bool, "error": ""}`, etc. Timeout and error scenarios
produce the same shape with appropriate values (`exit_code: -1` for timeout,
`error` populated with the message).

The `outcome` field replaces the old `GateResult::Passed` matching. The
advance loop checks `outcome` for control flow, then uses `output` for
the evidence map. Single return type, single source of truth.

#### Alternatives considered

**Extend GateResult with a JSON field on each variant**: add
`output: serde_json::Value` to each existing variant (Passed{output},
Failed{exit_code, output}, etc.). Rejected because it couples control flow
data (exit_code on Failed) with structured output, leading to redundancy
(exit_code appears in both the variant field and the JSON). The struct
approach keeps them cleanly separated.

**Parallel return type alongside GateResult**: keep GateResult for control
flow and add a separate `GateOutput` type for structured data. Rejected
because it creates synchronization risk (two return values that must agree)
and doubles the return surface of every gate evaluator.

### Decision 2: How gate output enters the transition resolver

Gate data needs to reach `resolve_transition` as nested JSON under the
`gates.*` namespace. The resolver currently does flat key matching:
`evidence.get(field) == Some(expected)`. Template `when` clauses like
`gates.ci_check.exit_code: 0` require accessing nested values.

#### Chosen: nested JSON maps with dot-path traversal

Store gate output as nested JSON maps in the evidence:
`{"gates": {"ci_check": {"exit_code": 0, "error": ""}}}`. Add a dot-path
traversal helper to the resolver's condition matching that splits keys on
`.` and walks the nested map.

```rust
fn resolve_value<'a>(evidence: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = evidence;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}
```

The resolver's condition check at line 411-413 changes from
`evidence.get(field) == Some(expected)` to
`resolve_value(evidence_value, field) == Some(expected)`, where
`evidence_value` is the full evidence map wrapped as a `serde_json::Value`.
Flat keys (agent evidence like `"mode"`) work identically because a
single-segment path does one `.get()` call -- same as before.

This preserves the full structure of gate output (error messages, nested
fields) and supports future gate types with richer schemas. The evidence
map becomes a `serde_json::Value::Object` that contains both flat agent
evidence and nested gate data.

#### Alternatives considered

**Flatten gate data into dot-separated keys**: insert flat keys like
`"gates.ci_check.exit_code"` into the existing `BTreeMap`. No resolver
changes needed. Rejected because flattening loses structural information
(can't represent nested objects for future gate types), and dot-separated
flat keys are ambiguous (is `"a.b"` a nested path or a literal key name?).

**Compile-time when clause transformation**: split dot-paths during
template compilation into nested lookup instructions. Rejected as
unnecessary complexity -- the runtime dot-path traversal is simple (split
on `.`, walk the map) and adds negligible overhead.

### Decision 3: How the advance loop determines pass/fail

The advance loop currently checks `matches!(r, GateResult::Passed)` per
gate. With `StructuredGateResult`, each gate evaluation returns an
`outcome: GateOutcome` that serves as the pass/fail indicator.

#### Chosen: outcome field as the pass indicator

The `outcome` field on `StructuredGateResult` is the pass indicator. The
advance loop checks `matches!(result.outcome, GateOutcome::Passed)` -- the
same pattern as today, just on the new type. Each gate evaluator sets
`outcome` based on its gate-type-specific logic: command gates set Passed
when exit code is 0, context-exists gates set Passed when the key exists,
etc.

This keeps pass condition logic inside each gate evaluator function, which
already has all the context needed to determine pass/fail. No separate
registry, no declarative rules, no additional infrastructure. The advance
loop doesn't need to know gate type semantics -- it just checks the outcome.

#### Alternatives considered

**Pass conditions as functions in a registry**: register a
`fn(&serde_json::Value) -> bool` per gate type. The advance loop calls the
registered function. Rejected because it adds infrastructure (a registry
type, registration calls) for something each evaluator already computes.
Pass logic is gate-type-specific and belongs in the evaluator, not in a
separate lookup table.

**Declarative pass conditions (field/value pairs)**: define pass conditions
as data (`{exit_code: 0}`) and evaluate via JSON equality. Rejected because
it limits pass conditions to exact equality (can't express `status_code >=
200 && < 300` for future HTTP gates). Function-level logic in the evaluator
is more flexible.

## Decision outcome

### Summary

Gate evaluation functions return a `StructuredGateResult` carrying both a
`GateOutcome` enum (Passed/Failed/TimedOut/Error) and a `serde_json::Value`
with the structured output matching the gate type's schema. The advance loop
checks `outcome` for the pass/fail decision (same pattern as today's
`GateResult::Passed` matching), then injects each gate's `output` into the
evidence map under `{"gates": {gate_name: output}}`.

The transition resolver gains a dot-path traversal helper: `when` clause
keys like `gates.ci_check.exit_code` are split on `.` and walked through
the nested map. Flat agent evidence keys work identically (single-segment
path = one `.get()` call). The evidence map changes from
`BTreeMap<String, Value>` to a `serde_json::Value::Object` that holds both
flat agent evidence and nested gate output.

When gates don't all pass, the advance loop stops and reports the structured
gate output in the CLI response's `blocking_conditions` array (R4a). Each
blocking condition includes the gate name and its full structured output, so
agents can see exactly what the gate returned and decide whether to override.

Edge cases: timeout produces `{exit_code: -1, error: "timed_out"}` for
command gates (same schema, non-passing outcome). Spawn errors produce
`{exit_code: -1, error: "<message>"}`. States where `when` clauses don't
reference `gates.*` use legacy behavior (backward compat, Feature 4).

### Rationale

The three decisions fit together cleanly. `StructuredGateResult` (D1)
produces both the control flow signal (outcome) and the data (output) that
the other two decisions consume. The outcome field (D3) feeds the advance
loop's pass/fail check without requiring a separate registry or declarative
rules. The output field (D1) feeds the nested evidence map (D2) that the
resolver traverses via dot-paths.

The dot-path traversal (D2) is the single new capability that makes
everything work. It's a small function (~5 lines) that unlocks gate data
for `when` clause matching without changing the resolver's core logic
(it still does exact JSON equality per field). Flat keys pass through it
transparently, so backward compatibility is automatic.

## Solution architecture

### Overview

The change touches three layers: gate evaluation (`src/gate.rs`), the
advance loop (`src/engine/advance.rs`), and the CLI response builder
(`src/cli/next_types.rs`). Data flows from gate evaluators through the
advance loop into the transition resolver and CLI response.

### Components

**Gate evaluation** (`src/gate.rs`)
- Replace `GateResult` with `StructuredGateResult` (outcome + output)
- `evaluate_gates` returns `BTreeMap<String, StructuredGateResult>`
- Each evaluator function produces structured JSON:
  - `evaluate_command_gate`: `{exit_code: N, error: ""}` / `{exit_code: -1, error: "timed_out"}`
  - `evaluate_context_exists_gate`: `{exists: bool, error: ""}`
  - `evaluate_context_matches_gate`: `{matches: bool, error: ""}`

**Advance loop** (`src/engine/advance.rs`)
- At gate evaluation (~line 295): receive `BTreeMap<String, StructuredGateResult>`
- Check outcomes: `any_failed = results.values().any(|r| !matches!(r.outcome, GateOutcome::Passed))`
- Build gate evidence: construct `{"gates": {name: result.output, ...}}` as a `serde_json::Value`
- Merge into evidence: combine agent evidence first, then gate evidence on top. Gate output is merged after agent evidence so engine-produced data takes precedence (defense in depth until Feature 2's namespace reservation)
- Convert merged evidence internally before calling `resolve_transition`. The `advance_until_stop` signature keeps `&BTreeMap<String, Value>` for agent evidence; the conversion to `Value::Object` happens inside, scoping the type change to the resolver
- If any gate didn't pass: return with structured gate data for CLI response

**Transition resolver** (`src/engine/advance.rs`, `resolve_transition`)
- Change evidence parameter from `&BTreeMap<String, Value>` to `&serde_json::Value`
- Add `resolve_value(evidence: &Value, path: &str) -> Option<&Value>` helper
- Change condition matching from `evidence.get(field) == Some(expected)` to `resolve_value(evidence, field) == Some(expected)`

**CLI response** (`src/cli/next_types.rs`)
- `blocking_conditions_from_gates` updated to use `StructuredGateResult`
- Each blocking condition includes the gate's structured `output` field

### Key interfaces

**StructuredGateResult**:
```rust
pub struct StructuredGateResult {
    pub outcome: GateOutcome,
    pub output: serde_json::Value,
}
```

**resolve_value** (new helper):
```rust
fn resolve_value<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}
```

**Merged evidence map** (conceptual shape):
```json
{
  "gates": {
    "ci_check": {"exit_code": 0, "error": ""},
    "lint": {"exit_code": 1, "error": ""}
  },
  "mode": "issue_backed",
  "issue_number": "42"
}
```

### Data flow

```
Gate evaluators
  |
  v
BTreeMap<String, StructuredGateResult>
  |
  +-- Check outcomes -> any_failed bool (advance loop control)
  |
  +-- Extract outputs -> {"gates": {name: output, ...}}
  |
  v
Merge with agent evidence -> serde_json::Value::Object
  |
  v
resolve_transition(merged_evidence, ...)
  |
  +-- For each when condition:
  |     resolve_value(evidence, "gates.ci_check.exit_code") == Some(0)
  |     resolve_value(evidence, "mode") == Some("issue_backed")
  |
  v
TransitionResolution::Resolved(target) | NeedsEvidence | ...
```

## Implementation approach

### Phase 1: StructuredGateResult and evaluator changes

Replace `GateResult` with `StructuredGateResult` in `src/gate.rs`. Update
all three evaluator functions to return structured JSON output. Update
`evaluate_gates` return type. Fix all compilation errors from the type
change across the codebase.

Deliverables:
- `StructuredGateResult` struct with `GateOutcome` enum
- Updated command, context-exists, context-matches evaluators
- All callers updated (advance.rs, next_types.rs, tests)
- Unit tests for each gate type's structured output

### Phase 2: Evidence merging and dot-path resolver

Build gate evidence from `StructuredGateResult` outputs. Change
`resolve_transition` to accept `&serde_json::Value` and use dot-path
traversal. Update the advance loop to merge gate output into the evidence
map before calling `resolve_transition`.

Deliverables:
- `resolve_value` helper function with unit tests
- `resolve_transition` parameter change and condition matching update
- Gate evidence merging in the advance loop
- Integration tests: gate output + agent evidence routing together

### Phase 3: CLI response and blocking conditions

Update `blocking_conditions_from_gates` to use `StructuredGateResult` and
include structured gate output in the response. Update functional tests.

Deliverables:
- Updated blocking conditions with structured output
- Functional tests for gate-blocked responses with structured data
- `koto next` response shape matches R4a

## Security considerations

This change modifies how gate data flows through the engine but doesn't
introduce new attack surface. Gate output is produced by the engine itself
(from commands it runs or context it reads), not from external input. The
dot-path traversal is a read-only operation on engine-produced data.

The `gates` namespace reservation (R7, Feature 2) prevents agents from
injecting fake gate data via `--with-data`. That's implemented in Feature 2,
not here. For Feature 1, gate output is engine-produced and trusted.

## Consequences

### Positive

- Gate output is available for transition routing, closing the foundational
  gap that the entire gate-transition contract is built on
- The dot-path resolver works transparently for flat keys, so backward
  compatibility is automatic at the resolver level
- Each gate evaluator produces both control flow (outcome) and data (output)
  in one return value, no synchronization risk
- Future gate types just need to implement an evaluator that returns
  StructuredGateResult -- the rest of the pipeline handles it

### Negative

- `resolve_transition` signature change from `BTreeMap` to `Value` touches
  every caller and test that constructs evidence maps
- The evidence map changes from a typed `BTreeMap<String, Value>` to a
  loosely-typed `Value::Object`, losing some compile-time safety
- Dot-path traversal is a new behavior that template authors need to learn
  (though it's natural for anyone who's used JSON paths)

### Mitigations

- The `BTreeMap` to `Value` migration can be done mechanically (wrap in
  `serde_json::to_value()` at each call site)
- Template authors only encounter dot-paths when writing `gates.*` in `when`
  clauses; flat evidence keys work exactly as before
- The resolver's core logic (exact equality per field) doesn't change --
  only how it looks up the field value changes
