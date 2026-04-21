# Lead: Template variable access and skip_if condition types

## Findings

### Template Variable Access at Advance-Time

**Location and Storage**: Template variables are extracted from the `WorkflowInitialized` event payload at the **start of the advance loop** (`src/engine/advance.rs`, lines 193-201):

```rust
let workflow_variables: std::collections::HashMap<String, String> = all_events
    .iter()
    .find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { variables, .. } => Some(variables.clone()),
        _ => None,
    })
    .unwrap_or_default();
```

The `variables` field is a `HashMap<String, String>` stored in `EventPayload::WorkflowInitialized` (defined in `src/engine/types.rs`, line 124). These are initialized from `koto init --with VARNAME=value`.

**Data Structure at Evaluation Point**: Inside `advance_until_stop()`, the `workflow_variables` HashMap is passed directly to `resolve_transition()` (line 468). At transition resolution, variables are accessed by name via the `vars.` namespace.

### Condition Type 1: Template Variable Existence/Value Check

**Current Implementation** (`src/engine/advance.rs`, lines 616-625):

Template variables are checked via the `vars.VARNAME: {is_set: bool}` matcher. The resolver implementation:

1. Checks if key starts with `vars.` prefix (line 616)
2. Extracts the variable name after the prefix (line 618)
3. Looks up the variable in the `workflow_variables` HashMap (line 619-622)
4. Returns `true` if the variable is present **and non-empty**, `false` otherwise

**Critical Detail**: An empty string counts as "not set" for `is_set` purposes (test at line 1193 in `advance.rs` confirms this). This prevents activation states that depend on meaningful variable values.

**Data Available**: The `resolve_transition()` function already receives `variables: &std::collections::HashMap<String, String>` as a parameter (line 584).

### Condition Type 2: Context Key Existence Check

**Current Implementation** (`src/gate.rs`, lines 118-146):

The `context-exists` gate evaluator checks whether a context key exists:

```rust
fn evaluate_context_exists_gate(
    gate: &Gate,
    context_store: Option<&dyn ContextStore>,
    session: Option<&str>,
) -> StructuredGateResult {
    let (store, sess) = match (context_store, session) {
        (Some(s), Some(n)) => (s, n),
        _ => { /* error */ },
    };
    if store.ctx_exists(sess, &gate.key) {
        StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exists": true, "error": ""}),
        }
    } else {
        StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({"exists": false, "error": ""}),
        }
    }
}
```

The gate requires a `ContextStore` trait object and a session name. **The ContextStore is not currently passed into `advance_until_stop()`** — it is only used by the gate evaluator closure passed as a parameter.

**Location in Advance Loop**: Gate evaluation happens at step 6 (lines 309-431 in `advance.rs`). The context-exists gate logic is already in place and working.

### Condition Type 3: Evidence Field Value (When Clause) Check

**Current Implementation** (`src/engine/advance.rs`, lines 596-627):

The `resolve_transition()` function evaluates `when` clauses by:

1. Matching evidence fields via dot-path traversal (`resolve_value()` function, lines 551-560)
2. Supporting nested JSON access (e.g., `gates.ci.exit_code`)
3. Applying special matchers for `evidence.FIELD: present` (line 606) and `vars.VAR: {is_set: bool}` (line 616)

The evidence comes from two sources:
- **Agent-submitted evidence**: merged from `EvidenceSubmitted` events in the current epoch (line 191, stored in `current_evidence`)
- **Gate output**: injected under the `"gates"` namespace during gate evaluation (lines 388-391, only for structured-mode states)

Gate output is only merged if the state has at least one `gates.*` when-clause reference (`has_gates_routing`, lines 402-410).

### Data Structures Available at Skip_If Evaluation Point

Inside `advance_until_stop()`, a `skip_if` predicate would have access to:

1. **`workflow_variables: HashMap<String, String>`** (lines 195-201) — template variable existence/value checks
2. **`template_state: &TemplateState`** (lines 228-234) — current state definition
3. **`current_evidence: BTreeMap<String, serde_json::Value>`** (line 191) — agent-submitted evidence for the current epoch
4. **`gate_evidence_map: serde_json::Map<String, serde_json::Value>`** (lines 313-314) — gate output (populated only if gates exist)
5. **`merged: serde_json::Value`** (lines 449-463) — the combined evidence map used by `resolve_transition()`, including both agent evidence and gate output
6. **`all_events: &[Event]`** — full event history (passed as parameter, line 171), enabling epoch-scoped or all-history queries

**NOT available (requires architectural change)**:
- `ContextStore` and session name — currently only injected into the gate evaluator closure, not passed to `advance_until_stop()` itself

### Can Skip_If Reuse Gate Evaluator Logic for Context-Exists?

**Direct Reuse**: No, not without refactoring.

**Current Architecture**:
- Gate evaluators are injected as closures (`evaluate_gates: &G` parameter on line 180)
- Context-exists gate logic is **nested inside the gate evaluator**, not extracted as a reusable function
- The `ContextStore` is passed to the gate evaluator closure, not to `advance_until_stop()` itself

**Potential Refactoring**:
To enable `skip_if` to check context keys, the `ContextStore` would need to be:
1. Passed as an additional parameter to `advance_until_stop()`, or
2. Wrapped in a closure similar to the gate evaluator pattern

The `evaluate_context_exists_gate()` function in `src/gate.rs` (lines 118-146) could be extracted and reused by both the gate evaluator and a `skip_if` predicate, but this requires passing the `ContextStore` down to the advance loop.

### Minimal Data Structure for Skip_If Predicate

A `skip_if` condition could express all three types via:

```rust
pub enum SkipIfCondition {
    /// Template variable existence: vars.<NAME>: {is_set: bool}
    VariableSet { var_name: String, is_set: bool },
    
    /// Context key existence: context key path
    ContextExists { key: String },
    
    /// Evidence field value: dot-path key and expected value
    Evidence { path: String, value: serde_json::Value },
}

pub struct SkipIfPredicate {
    conditions: Vec<SkipIfCondition>,
    match_all: bool, // AND vs OR logic
}
```

This mirrors the existing structure used for transition `when` clauses:
- `vars.*` checks (existing, lines 616-625)
- Gate output routing (existing, but requires context refactoring for new condition type)
- Flat evidence matching (existing, line 626)

## Implications

1. **No Architectural Blocker for Conditions 1 and 3**: Template variable and evidence field checks can be implemented immediately using existing data structures already in scope inside `advance_until_stop()`.

2. **Refactoring Needed for Condition 2**: Context-exists checks require passing `ContextStore` and session name to `advance_until_stop()`, which is currently not done. This is a moderate change but follows the existing closure-injection pattern.

3. **Synthetic Event Emission**: A `skip_if` auto-advance would emit a `Transitioned` event with `condition_type: "skip_if"` (line 484 shows `condition_type` is already part of the event). The event appends at the same point where manual transitions are recorded (lines 480-486).

4. **Epoch Semantics**: Auto-advanced states via `skip_if` should reset evidence to empty (line 493), maintaining the existing epoch boundary semantics.

5. **Chain Compatibility**: Consecutive `skip_if` states can chain within a single `advance_until_stop()` invocation via the existing loop mechanism (lines 208-532). The visited set tracks auto-advanced states to prevent cycles (lines 186, 472-477).

## Surprises

1. **Variable Storage is String-Only**: The `WorkflowInitialized` event stores variables as `HashMap<String, String>` (not JSON values). This limits condition expressions to existence checks (via `{is_set: bool}`) rather than value comparisons like `vars.VERSION: "2.0"`. Equality checks on variables would require either (a) changing the storage type or (b) introducing a new matcher similar to `evidence.FIELD: present`.

2. **Gate Output Injection is Conditional**: Gate output is only merged into the evidence map when `has_gates_routing` is true (lines 402-410, 457). Legacy states (boolean pass/block, no `gates.*` references) do not expose gate output to the resolver. A `skip_if` predicate that references `gates.*` paths would need to respect the same condition.

3. **Context-Exists Logic is Deeply Nested**: The context-exists check is not a standalone utility — it's embedded in the gate evaluator. To enable `skip_if` to check context, the function would need to be extracted, which is a non-trivial refactoring.

## Open Questions

1. **Skip_If with Context Checks**: Should `skip_if` support context-exists conditions? The architectural refactoring is straightforward but requires adding `ContextStore` to `advance_until_stop()` signature. Is this worth the cost?

2. **Variable Value Comparisons**: Should `skip_if` support equality checks on template variables (e.g., `vars.SHARED_BRANCH: "feature/foo"`)? Currently, variables are strings but the matcher only supports existence checks (`{is_set: bool}`). Enabling value comparison would either require storing variables as JSON or introducing a new matcher syntax.

3. **Chaining Behavior**: Should auto-advanced states via `skip_if` be subject to the same 100-transition chain limit (`MAX_CHAIN_LENGTH`) as unconditional transitions? The existing limit applies (line 219), but the question is whether this is the intended behavior for deterministic skip transitions.

4. **Human Visibility**: Should auto-advances via `skip_if` appear in the event log with a distinct `condition_type` (e.g., `"skip_if"`), or should they use `"auto"` like unconditional transitions? The current code sets `condition_type: "auto"` (line 484), which would make them indistinguishable from unconditional advances in the log.

## Summary

Template variables are stored in `WorkflowInitialized` and extracted into a `HashMap<String, String>` at the start of the advance loop, where they are already accessible to transition resolution via the `vars.NAME: {is_set: bool}` matcher (Condition 1). Evidence field checks (Condition 3) use the same `resolve_transition()` function that would evaluate `skip_if` conditions (no new logic needed). Context-exists checks (Condition 2) require passing `ContextStore` to `advance_until_stop()` — currently only the gate evaluator has this access — but the gate logic itself is already implemented and could be reused after refactoring. The biggest open question is whether to support all three condition types or defer context-exists checks pending a design decision about architectural overhead.

