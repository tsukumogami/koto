# PRD Gate-Transition Contract: Phase 2 Architecture Research

## Executive Summary

This research investigates two critical architectural questions for the gate-transition contract PRD:

1. **Lead 1 (Override Defaults)**: How should override defaults work across multiple gates with structured output?
2. **Lead 2 (Compiler Validations)**: What new validations ensure the contract is complete and unambiguous?

**Key Finding**: The proposal requires a significant architectural shift from boolean gate results to structured gate output. The current system treats gates and accepts blocks as separate evidence channels. The new model must unify them while maintaining clean separation of concerns and clear override semantics.

---

## Lead 1: Override Defaults Across Multiple Gates

### Current Architecture

**Gates Today** (from `src/gate.rs`):
- Gates evaluate to `GateResult`: `Passed`, `Failed { exit_code }`, `TimedOut`, or `Error { message }`
- All gates are evaluated without short-circuiting
- Results are returned as `BTreeMap<String, GateResult>`
- No data is produced by gates — only boolean pass/fail

**Evidence Handling** (from `src/engine/advance.rs` and `src/template/types.rs`):
- Agents submit evidence via `accepts` blocks on states
- Evidence is JSON: `BTreeMap<String, serde_json::Value>`
- Transition `when` clauses match against evidence fields exactly
- Gates and accepts blocks are currently orthogonal (no data flow from gates → transitions)

**Current Gate-Accepts Interaction** (from advance.rs lines 292-316):
```
6. Evaluate gates:
   - If any gate fails AND state has NO accepts block → return GateBlocked
   - If any gate fails AND state HAS accepts block → fall through to transition resolution
   - In transition resolution, gate_failed flag prevents unconditional transitions from firing
   - Agent must provide evidence to proceed (evidence_fallback mechanism)
```

### Proposal Analysis

The PRD proposes:
1. Gates declare output schemas (e.g., `{"status": "passed"}` with field types)
2. Each gate has an `override_default` value in the template
3. When agent overrides, the override defaults are applied
4. Compiler validates that override defaults + gate schemas fit with transition `when` clauses

### Multi-Gate Override Behavior: Design Recommendations

#### Question 1: Per-Gate Declaration vs State-Level Overrides?

**Recommendation: Per-Gate Declaration**

Each gate should declare its own `override_default` in the template. Example:

```yaml
states:
  validate:
    gates:
      ci_check:
        type: command
        command: "run-ci"
        timeout: 300
        output_schema:
          status: { type: "enum", values: ["passed", "failed"] }
          checks_run: { type: "number" }
        override_default:
          status: "passed"
          checks_run: 0
      
      lint:
        type: command
        command: "run-lint"
        output_schema:
          status: { type: "enum", values: ["passed", "failed"] }
        override_default:
          status: "passed"
    
    transitions:
      - target: review
        when:
          ci_check.status: "passed"
          lint.status: "passed"
      - target: fix
```

**Rationale**:
- Each gate is autonomous — it defines what it measures AND what override means for it
- Per-gate overrides are explicit in the template
- Aligns with gate evaluation logic (all gates evaluated independently)
- Supports selective override: agent can override only failing gates, not all

#### Question 2: Selective vs Total Overrides?

**Recommendation: Selective Override with Opt-In**

When an agent overrides:
- **Option A (Selective)**: Only gates that failed receive their override_default. Passing gates keep their actual output.
- **Option B (Total)**: All gates receive their override_default, ignoring actual results.

**Proposed behavior: Selective by default**

```
If agent calls: koto next --override ci_check

Result:
  ci_check: override_default applied (e.g., {status: "passed"})
  lint:    actual result from evaluation
  
This allows: "CI check looks suspicious, override it and proceed. Lint actually passed so use that result."
```

**Compiler implication**: Must verify that:
1. For each gate, there exists at least one transition path where `override_default` leads to a valid transition
2. No transition requires all gates to pass, because an agent might selectively override just one

#### Question 3: Override Defaults vs --with-data Precedence?

**Recommendation: Agent-Provided Data Takes Precedence**

```
Precedence (highest to lowest):
1. Agent supplies --with-data explicitly
2. Agent supplies --override (uses gate's override_default)
3. Actual gate evaluation results
4. Agent provides neither (use actual results, fail if gates fail)
```

**Rationale**:
- Allows agent to override a gate AND refine its output: `koto next --override ci_check --with-data '{"ci_check": {"status": "passed", "checks_run": 5}}'`
- Supports the override rationale audit trail (PR-108): agent's explicit data is more precise
- Maintains agent agency: explicit data always wins

#### Question 4: Merging Gate Output Into Evidence?

**Recommendation: Prefix-Namespaced Gate Output**

```yaml
# Template:
states:
  validate:
    gates:
      ci_check:
        type: command
        output_schema:
          status: { type: "enum", values: ["passed", "failed"] }
    
    accepts:
      override_reason: { type: "string" }
    
    transitions:
      - target: review
        when:
          gates.ci_check.status: "passed"
      - target: fix
        when:
          gates.ci_check.status: "failed"
          override_reason: "known_flake"
```

**Evidence Map Structure**:
```json
{
  "gates": {
    "ci_check": {
      "status": "passed"
    }
  },
  "override_reason": "known_flake"
}
```

**Rationale**:
- Keeps gate output distinct from agent-submitted evidence
- Prevents naming collisions
- Supports combined conditions: "CI failed but I have a valid reason"
- Aligns with current evidence routing validation logic
- Compiler can validate field references include the `gates.` prefix

### Summary: Multi-Gate Override Architecture

```
TEMPLATE DEFINITION:
├─ Each gate declares:
│  ├─ output_schema: {field: type, ...}
│  └─ override_default: {field: value, ...}
├─ Transitions reference: gates.<gate_name>.<field>
└─ Accepts block (agent-provided evidence) remains separate

RUNTIME BEHAVIOR:
├─ All gates evaluated → gate_output = {field: value, ...}
├─ Agent action options:
│  ├─ Accept actual results: auto-advance or needs evidence based on when clauses
│  ├─ Override gate N: replace gate_N output with its override_default
│  └─ Provide --with-data: override both actual output and override_defaults
└─ Evidence map = {gates: {gate_name: gate_output}, ...agent_fields}
```

---

## Lead 2: Compiler Validations for Gate-Transition Contract

### Current Validations (from `src/template/types.rs` lines 338-448)

The compiler already validates evidence routing (`validate_evidence_routing`):

1. **Empty when blocks rejected**: All `when` conditions must have at least one field
2. **when fields must be in accepts**: Field references must exist in the state's `accepts` block
3. **enum values must be valid**: For enum fields, `when` values must appear in the field's `values` list
4. **Pairwise mutual exclusivity**: No two conditional transitions can both match the same evidence
5. **when requires accepts**: Conditional transitions require the state to have an `accepts` block
6. **Scalar values only**: `when` values must be JSON scalars, not arrays/objects

### New Validations Needed for Gate-Transition Contract

#### Validation Set 1: Gate Schema Completeness

```
V1.1: Every gate must declare an output_schema
  Error: "state 'X' gate 'Y': output_schema is required"
  Applies to: command, context-exists, context-matches gates
  
V1.2: output_schema must be non-empty
  Error: "state 'X' gate 'Y': output_schema must have at least one field"
  
V1.3: output_schema field types must be valid
  Error: "state 'X' gate 'Y': field 'Z' has invalid type 'foo', must be enum/string/number/boolean"
  
V1.4: Enum schemas must have a values list
  Error: "state 'X' gate 'Y': enum field 'Z' must have a non-empty values list"
```

#### Validation Set 2: Override Default Contract

```
V2.1: Every gate must declare override_default
  Error: "state 'X' gate 'Y': override_default is required"
  
V2.2: override_default must match the output_schema
  Error: "state 'X' gate 'Y': override_default field 'Z' is not in output_schema"
         "state 'X' gate 'Y': override_default field 'Z' has type mismatch (expected string, got number)"
  
V2.3: override_default must include all required fields from schema
  Error: "state 'X' gate 'Y': override_default is missing required field 'Z'"
  
V2.4: override_default enum values must be valid
  Error: "state 'X' gate 'Y': override_default field 'Z' has invalid enum value 'invalid', must be in ['a', 'b']"
```

#### Validation Set 3: Transition-Gate Reference Validation

```
V3.1: when clauses can reference gate output via gates.<name>.<field>
  Extend current validation to recognize gates. namespace
  Error: "state 'X' transition to 'Y': when field 'gates.unknown_gate.status' references undefined gate"
         "state 'X' transition to 'Y': when field 'gates.Z.unknown_field' gate 'Z' has no such output field"
  
V3.2: when values for gate-output fields must match schema types
  Error: "state 'X' transition to 'Y': when value for 'gates.Z.status' (enum) must be in schema values ['a', 'b']"
         "state 'X' transition to 'Y': when value for 'gates.Z.count' (number) must be numeric, got string"
```

#### Validation Set 4: Override Default Reachability (Critical)

```
V4.1: For each gate with selective override, verify that override_default leads to a valid transition
  Error: "state 'X' gate 'Y': override_default does not match any transition when clause. 
           No path forward when this gate is overridden."
  
  Algorithm:
  1. For each gate G in the state
  2. Create a "simulated evidence" where G has its override_default, others have arbitrary values
  3. Try to resolve a transition
  4. If no transition matches, report V4.1 error

V4.2: Verify that override_default doesn't create ambiguous transitions
  Error: "state 'X' gate 'Y': override_default matches multiple transitions to [A, B]. 
           This is ambiguous when the agent overrides this gate."
  
  Same algorithm as V4.1 but check for multiple matches
```

**Example**: State with 2 gates, transitions depend on both:

```yaml
states:
  validate:
    gates:
      ci: { output_schema: {status: enum[passed/failed]}, override_default: {status: passed} }
      lint: { output_schema: {status: enum[passed/failed]}, override_default: {status: passed} }
    transitions:
      - target: review
        when:
          gates.ci.status: passed
          gates.lint.status: passed
      - target: fix
        when:
          gates.ci.status: failed
          gates.lint.status: passed

# V4.1 check:
# If agent overrides ci_check only:
#   Evidence: {gates: {ci: {status: passed}, lint: ???}}
#   Need to check: does SOME valid value for lint lead to a transition?
#   Actually: might need to warn that lint is unspecified, OR
#   Recommend: "When overriding ci_check, lint result is still required"
```

#### Validation Set 5: No Dead Ends on Override

```
V5.1: For each state with conditional transitions on gates, ensure that:
      - Either all gates have override_defaults that lead to a valid path
      - OR the state has an unconditional transition fallback
      - OR the state has an accepts block (agent can provide additional evidence)
  
  Error: "state 'X': conditional transitions require gates to have valid override paths or accepts block"
```

#### Validation Set 6: Consistency Between Multiple Gates

```
V6.1: If multiple gates are referenced in the same when clause, their output fields must not conflict
  Error: "state 'X' transition to 'Y': when clause references both gates.ci.status and gates.lint.status; 
           if both are overridden, ensure no field name collision"
  (This is more of a warning/design smell than an error)

V6.2: Document which gates are "required" vs "optional" for a transition
  Warning: "state 'X' transition to 'Y': references gate 'ci' but not gate 'lint'. 
            If lint fails and ci passes with override, what happens?"
```

### Modified Validation: Evidence Routing (Extends Current V3.1-V3.6)

The current `validate_evidence_routing` validates that:
- `when` fields are in `accepts`
- enum values are in the field's values list
- mutual exclusivity holds

**New behavior**: Must also validate gate references:

```rust
// Pseudo-code for updated validator
fn validate_evidence_routing(state_name, state):
  for transition in state.transitions:
    if transition.when:
      for (field_ref, value) in transition.when:
        if field_ref.starts_with("gates."):
          // Parse gates.GATE_NAME.FIELD_NAME
          (gate_name, field_name) = parse_gate_ref(field_ref)
          
          if gate_name not in state.gates:
            error("gate '{gate_name}' referenced but not declared")
          
          gate = state.gates[gate_name]
          if field_name not in gate.output_schema:
            error("gate '{gate_name}' has no field '{field_name}'")
          
          // Validate value matches field type
          if not type_matches(value, gate.output_schema[field_name]):
            error("type mismatch for '{field_ref}'")
        else:
          // Original validation for accepts fields
          if field_ref not in state.accepts:
            error("field '{field_ref}' not in accepts")
          ...
```

### Validation Error Messaging Strategy

All validation errors should include:

1. **Location**: State name, gate/transition name, field name
2. **Problem**: What's wrong in specific terms
3. **Fix suggestion**: How to resolve it

Example:
```
ERROR in state 'validate' gate 'ci_check':
  override_default is missing required field 'checks_run'
  
  Gate output_schema: {status: enum[...], checks_run: number}
  override_default: {status: "passed"}
  
  Fix: Add 'checks_run: 0' to override_default, or mark field as optional in output_schema
```

### Summary: Validation Checklist

| Category | Count | Examples |
|----------|-------|----------|
| Gate Schema | 4 | schema exists, non-empty, valid types, enum values |
| Override Default | 4 | required, matches schema, complete, enum valid |
| Gate-Transition Refs | 2 | gate exists, field exists |
| Reachability (Critical) | 2 | override leads to valid transition, no ambiguity |
| Dead Ends | 1 | state has escape route on override |
| Consistency | 2 | no conflicts, clear required/optional gates |
| **Total** | **15** | |

---

## Architectural Decisions Required

### Decision 1: Gate Output Representation

**Question**: Should gate output be part of the `Gate` struct definition or separate?

**Option A**: Add to Gate struct (template-time definition)
```rust
pub struct Gate {
  pub gate_type: String,
  pub command: String,
  pub output_schema: BTreeMap<String, FieldSchema>,  // NEW
  pub override_default: serde_json::Value,           // NEW
  ...
}
```

**Option B**: Separate output definition block per state
```yaml
states:
  validate:
    gates:
      ci_check: { type: command, command: "..." }
    gate_outputs:  # NEW
      ci_check:
        schema: {status: enum[passed/failed]}
        override_default: {status: passed}
```

**Recommendation**: **Option A** — gate-centric (output lives with gate)
- Aligns with gate independence principle
- Output is integral to gate design
- Simpler compiler (fewer lookups)

### Decision 2: Gate Output in Evidence Map Namespace

**Question**: How should gate output appear in the evidence map at runtime?

**Option A**: Flat namespace
```json
{
  "ci_check.status": "passed",
  "lint.status": "passed"
}
```

**Option B**: Nested namespace (recommended)
```json
{
  "gates": {
    "ci_check": {"status": "passed"},
    "lint": {"status": "passed"}
  }
}
```

**Recommendation**: **Option B** — nested
- Prevents collisions with agent-provided fields
- Clear semantic separation
- Mirrors struct hierarchy

### Decision 3: Compiler Validation Strictness

**Question**: Should the compiler reject states where gates declare output but transitions don't use it?

**Option A (Permissive)**: Allow unused gate output
```yaml
gates:
  analyze: {output_schema: {result: string}, ...}
transitions:
  - target: done  # Doesn't reference analyze output — OK
```

**Option B (Strict)**: Require all declared gate outputs to be referenced
```yaml
# Same template → ERROR: gate 'analyze' declares output but no transition uses it
```

**Recommendation**: **Option A (Permissive)** initially, with warning
- Allows gradual migration
- Gates can declare output without transition routing yet
- Warn: "gate 'X' output not used in any transition"

### Decision 4: Multi-Gate Override Semantics

**Question**: Should agents be able to override individual failing gates or all gates at once?

**Option A**: Individual override (`--override gate_name`)
**Option B**: All-or-nothing override (`--override-all`)
**Option C**: Support both

**Recommendation**: **Option A** — individual, with grouping support
```
koto next --override ci_check        # Override only CI
koto next --override ci_check lint   # Override both
koto next --override ALL             # Override all failing gates
```

Aligns with agent autonomy and supports detailed audit trails.

---

## Data Flow Diagram

```
Template Compilation:
┌─────────────────────────────────────┐
│ YAML Template with Gates + Accepts  │
├─────────────────────────────────────┤
│ gates:                              │
│   ci: {output_schema: {...},        │
│        override_default: {...}}     │
│ accepts: {reason: string}           │
│ transitions:                        │
│   - when: {gates.ci.status: ...}    │
└─────────────────────────────────────┘
             │
             ↓ (compile)
         ┌───────────┐
         │ Validator │ ← Validations V1-V6
         └───────────┘
             │
             ↓ (if valid)
┌─────────────────────────────────┐
│ CompiledTemplate                │
│  .gates[name].output_schema     │
│  .gates[name].override_default  │
│  .transitions[].when            │
└─────────────────────────────────┘

Runtime Execution:
┌──────────────────────────────┐
│ State Entry: evaluate_gates()│
├──────────────────────────────┤
│ gate_output = {              │
│   ci: {status: "passed", ...}│
│ }                            │
└──────────────────────────────┘
           │
    ┌──────┴──────┐
    │             │
    ↓             ↓
Gate Success?   Agent Override?
    │             │
    ├─YES         ├─YES: use override_default
    │             │     (append to evidence)
    └─────────────┘
         │
         ↓
Resolve Transition:
  evidence = {
    gates: gate_output or override_defaults,
    ...agent_provided_fields
  }
  match(evidence, transition.when_clauses)
         │
         ↓
  Auto-advance or EvidenceRequired
```

---

## Risks and Mitigations

### Risk 1: Override Default Not Reaching Valid Transition

**Scenario**: Agent overrides a gate, but the override_default doesn't match any transition condition.

**Impact**: Agent stuck; workflow cannot proceed.

**Mitigation**: 
- V4.1 validation catches this at compile time
- CLI should warn: "Override default for gate 'X' may not lead to a valid path"
- Consider optional "secondary override defaults" per agent role

### Risk 2: Explosion of Transition Conditions

**Scenario**: With N gates each having multiple output fields, and M transitions, the number of conditional paths explodes.

**Impact**: Hard to reason about; validation becomes expensive.

**Mitigation**:
- Recommend grouping: per-gate output should relate to one logical decision
- Warn if a state has >3 gates
- Example: One "integration_status" gate, not separate ci/lint/sec gates

### Risk 3: Gate Output Doesn't Match Override Default at Runtime

**Scenario**: Gate produces `{status: "passed"}` but override_default is `{status: "failed"}` for some reason (schema drift).

**Impact**: Silent inconsistency; hard to debug.

**Mitigation**:
- V2.2-V2.4 catch schema drift at compile time
- Log actual vs override at runtime for audit trail
- Consider requiring override_default to match at least one schema value for enums

### Risk 4: Backward Compatibility

**Scenario**: Existing templates without gate output schemas/overrides still need to work.

**Impact**: All gates would fail validation.

**Mitigation**:
- Introduce a feature flag: `enable_gate_output: true` in template header
- Default: gates are boolean only (current behavior)
- Gradual migration path for existing workflows

---

## Recommendations

### Immediate (Phase 2)

1. **Extend Gate struct** to include `output_schema` and `override_default`
2. **Implement V1.1-V2.4 validations** (gate completeness and override contract)
3. **Implement V3.1-V3.2 validations** (transition-gate references with gates.* prefix)
4. **Update evidence routing validator** to handle gate references

### Short Term (Phase 3)

5. **Implement V4.1-V4.2 validations** (reachability and ambiguity checks)
6. **Add CLI support**: `--override GATE_NAME` and `--override-all`
7. **Implement override rationale capture** (PR-108 integration)

### Medium Term (Phase 4)

8. **Add gate output to audit trail**: record actual vs override values
9. **Support conditional gate execution**: some gates only run if prior gates pass
10. **Visualization**: show gate→transition dataflow in CLI output

---

## Test Strategy

### Compile-Time Validation Tests

```rust
#[test]
fn gate_without_output_schema_fails() { ... }

#[test]
fn override_default_missing_required_field_fails() { ... }

#[test]
fn override_default_not_matching_any_transition_warns() { ... }

#[test]
fn multiple_gates_with_valid_overrides_passes() { ... }

#[test]
fn enum_override_value_not_in_schema_fails() { ... }
```

### Runtime Tests

```rust
#[test]
fn gate_output_flows_to_transition_resolution() { ... }

#[test]
fn selective_override_applies_only_to_specified_gate() { ... }

#[test]
fn agent_provided_data_overrides_gate_override_default() { ... }

#[test]
fn ambiguous_transition_on_override_rejected() { ... }
```

### Integration Tests

```
Feature: Gate Output with Structured Data
  Scenario: Multiple gates with independent schemas
    Given: state with ci_check and lint gates
    When:  ci_check fails, lint passes
    And:   agent overrides ci_check only
    Then:  evidence = {gates: {ci_check: override_default, lint: actual}}
    And:   transition resolved to valid target

  Scenario: Override default doesn't lead to transition
    Given: state with gate having override_default = {status: failed}
    When:  compiler validates template
    Then:  ERROR: "override_default does not lead to any valid transition"
```

---

## Open Questions

1. **Should gates be able to produce different output schemas based on result?**
   - E.g., passing returns `{status: "passed", checks: 42}`, failing returns `{status: "failed", error: "..."}`
   - Adds complexity but more accurate

2. **Should override default be optional or required?**
   - If optional: gate can declare output but no override path
   - If required: every gate must be "overridable"

3. **How should gate output interact with context-aware gates?**
   - `context-matches` gate matches a pattern — what's the output_schema?
   - E.g., `{matched: boolean, content: string}`?

4. **Should there be a "gate combination" concept?**
   - E.g., "all ci-related gates" bundled with one override_default?
   - Or always per-gate?

5. **Precedence when both gates AND accepts have a field named "status"?**
   - Use `gates.status` vs `status` to disambiguate?
   - Or forbid entirely?

---

## Conclusion

The gate-transition contract requires:

1. **Structured output from gates**: Per-gate output schemas and override defaults
2. **Namespace isolation**: gate output in `{gates: {...}}` to avoid collisions
3. **Extensive compile-time validation**: 15+ validation rules covering schema, override, reachability, and ambiguity
4. **Clear override semantics**: Selective per-gate override with precedence rules
5. **Audit trail integration**: Record which gates were evaluated, which overridden, which had data provided by agent

The architecture is sound and implementable. The key implementation challenge is the reachability validation (V4.1-V4.2), which requires simulating transition resolution with partial evidence. The payoff is significantly improved template authoring experience: gates become first-class citizens in the transition routing logic.
