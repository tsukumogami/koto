# Codebase Analyst Report: Gate-Transition Contract
## Phase 2 Research Findings

**Date:** 2026-03-30  
**Investigator:** Codebase Analysis Agent  
**Status:** Complete Research  

---

## Executive Summary

This report documents findings from deep analysis of the Koto codebase regarding two critical architectural questions for the gate-transition contract unification:

1. **How should gate output schemas be declared and what data do they produce?**
2. **How do gate outputs and agent evidence coexist in transition resolution?**

Key findings show that gates currently produce boolean-only results (pass/fail), and evidence is decoupled from gate evaluation. The proposed unification requires significant structural changes to both gate evaluation and transition resolution.

---

## Lead 1: Gate Output Schemas and Data Production

### Current Gate Architecture

#### Gate Types (src/gate.rs lines 13-15, 44-56)

Three gate types exist today, all producing **boolean-only results** via the `GateResult` enum:

```rust
pub enum GateResult {
    Passed,
    Failed { exit_code: i32 },
    TimedOut,
    Error { message: String },
}
```

**Gate Types:**
- **`command`** (GATE_TYPE_COMMAND): Spawns shell commands, captures exit code only
- **`context-exists`** (GATE_TYPE_CONTEXT_EXISTS): Binary check—does a context key exist?
- **`context-matches`** (GATE_TYPE_CONTEXT_MATCHES): Binary regex match against context content

#### What Data Each Gate Type Currently Produces

**Command Gates** (src/gate.rs lines 121-140):
- Exit code only (from `run_shell_command` output)
- Passes exit code 0 → `GateResult::Passed`
- Non-zero exit code → `GateResult::Failed { exit_code: <code> }`
- No stdout/stderr capture, no structured output

**Context-Exists Gates** (src/gate.rs lines 63-81):
- Binary result only: pass if key exists, fail if missing
- Returns `GateResult::Failed { exit_code: 1 }` when key is absent

**Context-Matches Gates** (src/gate.rs lines 83-119):
- Binary result: pass if regex matches, fail if no match
- Regex compilation errors → `GateResult::Error`
- Does NOT return match captures or matched content

#### Gate Declaration in Templates (src/template/types.rs lines 85-101)

The `Gate` struct currently has **no schema field**:

```rust
pub struct Gate {
    pub gate_type: String,      // "command", "context-exists", "context-matches"
    pub command: String,         // for command gates
    pub timeout: u32,            // timeout in seconds
    pub key: String,             // for context gates
    pub pattern: String,         // regex for context-matches
}
```

Gates are compiled from YAML sources (src/template/compile.rs lines 102-114) where declaration is similarly flat:

```yaml
gates:
  ci_check:
    type: command
    command: ./run-tests.sh
    timeout: 30
  context_check:
    type: context-exists
    key: deployment/status.json
```

There is **no existing schema declaration mechanism** for gates—they don't declare output field types or structures.

#### Relation to FieldSchema (src/template/types.rs lines 74-83)

The `FieldSchema` type used by `accepts` blocks defines field contracts:

```rust
pub struct FieldSchema {
    pub field_type: String,              // "enum", "string", "number", "boolean"
    pub required: bool,
    pub values: Vec<String>,             // for enum fields
    pub description: String,
}
```

**Key insight:** Gates have **zero parallel structure**. They produce binary results, not structured data. To unify them, gates need:
1. An output schema (similar to FieldSchema, or a new type)
2. Structured data production (not just pass/fail)
3. Schema declaration in templates (new YAML fields)

### Proposed Path for Gate Schemas

Based on codebase structure, a gate schema should:

**1. Extend the `Gate` struct** with:
```rust
pub schema: Option<BTreeMap<String, FieldSchema>>  // nullable for backward compat
```

**2. Declare in YAML** (example):
```yaml
gates:
  ci_check:
    type: command
    command: ./run-tests.sh
    timeout: 30
    schema:
      exit_code:
        type: number
        required: true
      status:
        type: string
        required: true
```

**3. Map each gate type to output structure:**

| Gate Type | Natural Output Fields | Field Types |
|-----------|----------------------|-------------|
| `command` | `exit_code`, `stdout`, `stderr` | number, string, string |
| `context-exists` | `exists` | boolean |
| `context-matches` | `matches`, `content` | boolean, string |

**4. Extend `GateResult`** to carry structured data:
```rust
pub enum GateResult {
    Passed { data: BTreeMap<String, serde_json::Value> },
    Failed { exit_code: i32, data: BTreeMap<String, serde_json::Value> },
    TimedOut,
    Error { message: String },
}
```

### Validation Integration

Gates would integrate with the existing evidence validation system (src/engine/evidence.rs lines 45-98):
- Gate schema would be validated at compile time (like `accepts` schema)
- Gate output would be validated at runtime using `validate_evidence()` or similar
- Type mismatches (e.g., gate produces string, schema requires number) → error

---

## Lead 2: Gate Outputs and Agent Evidence Coexistence

### Current Flow: Evidence and Gates are Separate

#### How Evidence Reaches resolve_transition (src/engine/advance.rs lines 163-374)

The `advance_until_stop` loop:

1. **Takes evidence as input** (line 165): `evidence: &BTreeMap<String, serde_json::Value>`
2. **Evaluates gates independently** (line 296): `let gate_results = evaluate_gates(&template_state.gates);`
3. **Passes evidence to transition resolver** (line 319): `resolve_transition(template_state, &current_evidence, gates_failed)`

Gate results **never touch the evidence map**—they are binary blockers only.

#### resolve_transition Logic (src/engine/advance.rs lines 394-447)

```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
    gate_failed: bool,
) -> TransitionResolution
```

The resolver:
- Matches `when` conditions against evidence (line 411-414)
- Uses `gate_failed` flag to prevent unconditional fallback when gates block (lines 430-434)
- **Never inspects gate results directly**

**Critical behavior:** When a gate fails on a state with `accepts`, the loop doesn't return `GateBlocked`. Instead, it sets `gate_failed=true` (line 312) and passes it to the resolver, which then requires agent evidence.

#### Where Gate Data Would Enter (Two Options)

**Option A: Merge gate data into evidence before resolve_transition**

Pseudo-code at line 296 in advance.rs:
```rust
let gate_results = evaluate_gates(&template_state.gates);
// NEW: Convert gate results to evidence map
let mut combined_evidence = current_evidence.clone();
for (gate_name, result) in &gate_results {
    if let GateResult::Passed { data } = result {
        combined_evidence.extend(data);  // flat merge
    }
}
// Pass combined evidence to resolver
match resolve_transition(template_state, &combined_evidence, gates_failed) { ... }
```

**Option B: Pass gate data as separate parameter**

```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
    gate_data: &BTreeMap<String, BTreeMap<String, serde_json::Value>>,  // NEW
    gate_failed: bool,
) -> TransitionResolution
```

### Field Name Conflict Handling

#### Current State: No Conflicts Possible

Gates produce no data, so name conflicts cannot occur. But if unified:

**Scenario:** State has:
- `accepts: { status: string }`
- `gates: { check_status: { schema: { status: string } } }`
- Agent submits: `{ status: "ready" }`

**Three conflict resolution strategies:**

1. **Flat merge with namespacing:**
   - Gate data: `{ gates.check_status.status: "passed" }`
   - Agent data: `{ status: "ready" }`
   - No conflict, but routing becomes complex: `when: { gates.check_status.status: "passed" }`

2. **Flat merge with last-write-wins:**
   - Gate data: `{ status: "passed" }`
   - Agent data (later): `{ status: "ready" }`
   - Agent wins, but gate intent is lost

3. **Separate namespaces, require unique names:**
   - Compiler enforces: gate output names ≠ accept field names
   - Flat merge safe, but restricts gate naming

**Recommendation from codebase patterns:** Option 1 (namespacing) aligns with koto's principle that gates are infra-level checks. The namespace `gates.<gate_name>.<field>` clarifies origin.

### Gate Data Timing: Before or With resolve_transition?

#### Current Flow for Evidence (src/engine/advance.rs lines 184-186)

```rust
// Evidence is only used for the initial state; auto-advanced states start fresh.
let mut current_evidence = evidence.clone();
...
// Fresh epoch: auto-advanced states have no evidence
current_evidence = BTreeMap::new();
```

**Evidence is per-epoch**, not per-state. After each transition, evidence is discarded.

#### Gate Evaluation Timing (src/engine/advance.rs lines 292-316)

```rust
// 6. Evaluate gates
let mut gates_failed = false;
let mut failed_gate_results: Option<BTreeMap<String, GateResult>> = None;
if !template_state.gates.is_empty() {
    let gate_results = evaluate_gates(&template_state.gates);
    // ... check results, possibly set gates_failed ...
}
```

Gates are evaluated **fresh on every state**, not carried across transitions.

**Implication for the contract:**

| Scenario | Current Behavior | With Gate Data |
|----------|------------------|-----------------|
| State A has gate + evidence, advances to B | Evidence cleared for B; gate irrelevant | Gate data cleared for B; agent must re-submit evidence for B |
| State A has gate blocking transition | Agent submits evidence to override + route | Agent submits evidence; gate data can route OR agent data can route |
| Agent and gate both provide conflicting values | No conflict (gates don't produce data) | Conflict resolution by namespace/precedence |

**Conclusion:** Gate data is **per-state, not per-epoch**. It should be evaluated and consumed **within the same state's advance loop iteration**, not carried across transitions. This mirrors the current `gates_failed` flag behavior.

### Multi-Source Evidence in Transition Resolution

#### Can a state have BOTH gate-produced and agent-submitted evidence?

**Yes, by design.** Example template:

```yaml
states:
  review:
    gates:
      ci_status:
        type: command
        command: ./get-ci-status.sh
        schema:
          passed: boolean
    accepts:
      decision:
        type: enum
        values: [approve, reject]
    transitions:
      - target: deploy
        when:
          gates.ci_status.passed: true
          decision: approve
      - target: rework
        when:
          decision: reject
```

Here:
- `gates.ci_status.passed` is produced by the gate
- `decision` is provided by the agent
- A `when` clause can require **both** to route to `deploy`

#### Validation Implications

From src/engine/evidence.rs lines 45-98, evidence validation occurs **after** agent submission but **before** transition resolution. If gate data is merged into evidence:

```rust
// After gate evaluation and merge
combined_evidence = merge(gate_data, agent_evidence);

// Validate only agent-submitted fields (not gate fields)
if let Some(accepts) = &state.accepts {
    validate_evidence(&agent_evidence, accepts)?;  // gates bypass this
}

// Resolve with combined evidence
resolve_transition(&state, &combined_evidence, gate_failed)?;
```

**Key design principle:** Gate output validation happens **at evaluation time** (during gate execution), not at transition resolution time. This keeps gates autonomous: they declare their schema, produce conformant data or fail.

### Unresolvable State Scenarios

Current stop reason (src/engine/advance.rs lines 79-81):

```rust
UnresolvableTransition,  // Conditional transitions exist but no evidence matches,
                         // and the state has no accepts block
```

This is returned when:
- Transitions are conditional (have `when` clauses)
- No evidence matches any `when`
- State has no `accepts` block

**With gate data unified:**

States can have:
1. Gates only (no accepts) → gate data alone may resolve transitions
2. Accepts only (no gates) → agent evidence alone may resolve
3. Both gates and accepts → either source can resolve, or both required

If neither gate data nor agent evidence matches any transition, the state remains unresolvable. The error message would need updating to reflect both sources.

---

## Detailed Findings Summary

### Finding 1: Gate Data Structure Design

**Current state:** Gates produce binary `GateResult` enum with no structured data.

**For unification:** Gates must:
- Declare output schema (new YAML field in gate definitions)
- Produce `BTreeMap<String, serde_json::Value>` instead of enum-only results
- Validate output against declared schema at runtime

**Implementation location:** 
- Extend `Gate` struct (src/template/types.rs:85-101)
- Extend `GateResult` enum (src/gate.rs:19-28)
- Add schema validation in compile phase (src/template/compile.rs)

### Finding 2: Evidence Flow Architecture

**Current state:** Evidence and gate results flow separately through `advance_until_stop`:
- Evidence: parameter to the loop, used by transition resolver
- Gate results: parameter to resolver, blocks unconditional fallback only

**For unification:** Two viable merge points:
1. **In advance.rs before resolve_transition:** Merge gate data + agent evidence into single map (cleaner, less refactoring)
2. **In resolve_transition:** Add parameter for gate data, merge inside resolver (more explicit, clearer separation)

**Recommendation:** Option 1 (merge before resolver) requires fewer signature changes and treats gate data as "first-class evidence."

### Finding 3: Namespace Strategy for Conflicts

**Current state:** No conflicts; gates don't produce data.

**For unification:** Use hierarchical namespace:
- Agent evidence: `{ field_name: value }`
- Gate evidence: `{ gates.<gate_name>.<field_name>: value }`

This:
- Prevents conflicts with agent fields
- Clarifies origin (gates vs. agents)
- Allows compiler to enforce no gate-name collisions
- Makes routing explicit: `when: { gates.ci.passed: true }` vs. `when: { decision: approve }`

### Finding 4: Validation Boundaries

**Current state:** Evidence validation happens in `validate_evidence()` (src/engine/evidence.rs:45-98) against `accepts` schema.

**For unification:**
- Gate output validation: happens during gate evaluation (GateResult creation)
- Agent evidence validation: happens after submission, against accepts schema
- **No cross-validation:** Gate outputs don't need to conform to accepts schema (they have their own schema)

This separation keeps gates and agents independent.

### Finding 5: State Advancement Semantics

**Current state:** Gates are evaluated fresh per-state, evidence is cleared per-epoch.

**For unification:** Gate data should follow **gate semantics, not evidence semantics**:
- Gate data is **local to the state** where the gate is declared
- Gate data is **cleared on transition** (like current evidence)
- Agent can't "carry" gate data to next state
- Each state must evaluate its own gates

This prevents gates from becoming cross-state assumptions.

### Finding 6: The "No Transitions without Accepts" Rule

Current validation rule (src/template/types.rs lines 363-369):

```rust
// Rule 5: when conditions require the state to have an accepts block.
if !has_accepts {
    return Err("when conditions require an accepts block on the state");
}
```

**Implication for gates:** If gates produce data and can route transitions, this rule must be relaxed:
- A state with **gates only** (no accepts) can have `when` transitions routed by gate data
- A state with **agents only** (no gates) can have `when` transitions routed by agent data
- A state with **both** can route on either

**Required compiler change:** Update `validate_evidence_routing()` to:
1. Check: does state have gates? → gates can produce data for routing
2. Check: does state have accepts? → agents can produce data for routing
3. Require: at least one of the above if transitions are conditional

---

## Architectural Integration Points

### 1. Template Compilation Phase

**File:** src/template/compile.rs

**Changes needed:**
- Parse `schema` field in gate YAML sources
- Validate gate schemas at compile time (like accepts schemas)
- Ensure gate schemas don't reference undefined types
- Update transition validation to account for gate-produced data

### 2. Gate Evaluation Phase

**File:** src/gate.rs

**Changes needed:**
- Modify `GateResult` enum to carry structured data
- Command gates: capture and parse stdout/stderr according to schema
- Context gates: return matched content in data field
- Validate output against schema before returning

### 3. Transition Resolution Phase

**File:** src/engine/advance.rs

**Changes needed:**
- Merge gate data into evidence before resolver
- Update `resolve_transition` to handle unified evidence
- Update `StopReason` variants to reflect both gate and evidence blockers

### 4. Response/Error Layer

**File:** src/cli/next_types.rs

**Changes needed:**
- Update `BlockingCondition` to distinguish gate vs. other blockers
- Update `NextResponse::EvidenceRequired` to describe gate and agent sources
- Update error messages to clarify what data is missing

---

## Risk Analysis

### Risk 1: Backward Compatibility

**Issue:** Adding gate schemas is backward-compatible (optional field), but changing `GateResult` breaks gate consumers.

**Mitigation:** 
- Keep `GateResult` as-is for gates without schemas
- Add new variant `GateResult::PassedWithData { data }`
- Or: wrap gates in new generic type `StructuredGateResult`

### Risk 2: Validation Timing

**Issue:** If gate output validation fails at runtime, the engine stops with an error (not a recoverable "gate failed" state).

**Mitigation:**
- Treat gate schema validation errors as `GateResult::Error`, not panics
- Agent can then override with evidence if accepts is present

### Risk 3: Namespace Complexity

**Issue:** Routing logic becomes complex with hierarchical namespace (`gates.ci.exit_code`).

**Mitigation:**
- CLI can offer convenience shortcuts: "show me what gates are checking"
- Template syntax can be sugared: `gate:ci.exit_code` instead of `gates.ci.exit_code`
- Validation ensures no ambiguity

### Risk 4: Evidence Merging Semantics

**Issue:** If agent and gate both produce same-namespace value (possible with flat merge), precedence unclear.

**Mitigation:** Use strict namespacing (gates.* vs. agent fields) to eliminate possibility.

---

## Conclusion

The proposed gate-transition contract unification is **architecturally sound** based on codebase analysis:

1. **Gate schemas can be declared** alongside gate definitions in YAML, extending the `Gate` struct with an optional `schema` field.

2. **Gate output is naturally structured** (e.g., command gates can produce `{exit_code: 0, stdout: "...", stderr: ""}`), matching the `BTreeMap<String, serde_json::Value>` shape of agent evidence.

3. **Gate and agent evidence coexist cleanly** via hierarchical namespacing (`gates.<name>.<field>` vs. flat agent fields), merged before transition resolution.

4. **Validation remains separated**: gates validate themselves at evaluation time; agents validate against accepts schema at submission time.

5. **The unification eliminates the current gap:** template authors can now write `when: { gates.ci.passed: true, decision: approve }` to require both gate and agent inputs for routing.

The main implementation effort is:
- Extending gate data types and evaluation logic
- Updating transition resolution to accept multiple evidence sources
- Compiler validation for gate schemas alongside accept schemas
- CLI/error messaging to clearly distinguish gate vs. agent sources

---

## References

- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/gate.rs` – Gate evaluation
- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/template/types.rs` – Gate and FieldSchema types
- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/template/compile.rs` – Gate compilation
- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/engine/advance.rs` – Transition resolution
- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/engine/evidence.rs` – Evidence validation
- `/home/dgazineu/dev/workspace/tsuku/tsuku-3/public/koto/src/cli/next_types.rs` – Response types
