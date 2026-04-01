# Design Decision: Structuring Gate Output for Transition Resolution
**Decision ID:** design_structured-gate-output_decision_2

## Problem Statement
Gate output must enter the transition resolver and support dot-path matching in `when` clauses. Currently, gate data cannot be referenced in conditions because:
- `resolve_transition` performs flat key matching: `evidence.get(field) == Some(expected)`
- Gate results are evaluated but never merged into the evidence map
- `when` clauses expect evidence to be flat, so "gates.ci_check.exit_code: 0" cannot be matched

The challenge is supporting both namespaced gate data (nested JSON) and flat agent evidence (strings, scalars) in a single evidence map while maintaining backward compatibility.

## Current Architecture
- **Evidence** (flat): `{"mode": "issue_backed", "issue_number": "42"}`
- **Gate Results** (structured): `{"ci_check": GateResult::Failed { exit_code: 1 }}`
- **Resolution**: Lines 319 + 411–413 in advance.rs use flat direct equality matching
- **Flow**: Gate results are computed at line 296 but remain isolated; never merged into evidence
- **Validation**: Template validation (template/types.rs) enforces that `when` fields match declared `accepts` fields—currently flat keys only

---

## Option Analysis

### Option A: Flatten Gate Data into Dot-Separated Keys
**Approach:** Convert gate results to flat keys before resolver:
```rust
// During gate evaluation (advance.rs ~296):
for (name, result) in gate_results {
    // Example: name="ci_check", exit_code=0 → "gates.ci_check.exit_code": 0
    let exit_code_key = format!("gates.{}.exit_code", name);
    current_evidence.insert(exit_code_key, json!(0));
}
```

**Pros:**
- No resolver changes needed; existing flat matching works as-is
- Backward compatible—agent evidence stays flat
- Simple, fast implementation
- Gate output immediately participates in resolver without modification

**Cons:**
- Gate result → scalar conversion loses structured context (error messages, timeouts)
- Awkward for complex gate outputs (future gate types might return richer data)
- Keys must be chosen at merge time; inflexible for future schema evolution
- Inconsistent with serde_json idioms (loses type safety)

**Performance:** O(1) — direct insertion during gate evaluation loop.

---

### Option B: Nested JSON + Dot-Path Traversal in Resolver
**Approach:** Keep gates data as nested JSON; add recursive traversal to condition matching:
```rust
// evidence: {"gates": {"ci_check": {"exit_code": 0}}, "mode": "issue_backed"}
// when condition: "gates.ci_check.exit_code" == 0

fn resolve_dot_path(evidence: &Value, path: &str) -> Option<&Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = evidence;
    for part in parts {
        current = &current[part]; // Navigate nested structure
    }
    Some(current)
}

// In resolve_transition (line 413):
let all_match = conditions
    .iter()
    .all(|(field, expected)| {
        resolve_dot_path(evidence, field) == Some(expected)
    });
```

**Pros:**
- Preserves full gate output structure (exit codes, error messages, timeout flags)
- Flexible for future gate types with richer outputs
- Idiomatic serde_json usage
- Cleaner separation: gates live under `gates.*` namespace

**Cons:**
- Requires resolver changes (lines 411–413 + new traversal function)
- Dot-path parsing happens on every transition check—**performance cost on hot path**
- Template validation must support dot-paths in `when` declarations
- Slightly more complex resolver logic

**Performance:** O(n) where n = depth of dot-path (typical: 2–3 levels = negligible). Still fast for single state advances, but cumulative if many conditional transitions exist.

---

### Option C: Preprocess When Clauses at Compile Time + Nested Evidence
**Approach:** Parse dot-paths in `when` clauses during template compilation; rewrite resolver to use native JSON path access:
```rust
// compile.rs: Process "gates.ci_check.exit_code": 0
// → Store as internal path structure: ["gates", "ci_check", "exit_code"]
//
// advance.rs resolver: Use serde_json::json! pointer access
// evidence["gates"]["ci_check"]["exit_code"] == 0
```

**Pros:**
- Moves complexity to compile time, not runtime
- Dot-path parsing happens once, not on every transition check
- Integrates with template validation cleanly
- Leverages JSON pointer semantics

**Cons:**
- Requires changes in two modules (compile.rs + advance.rs)
- More complex implementation; higher risk of bugs
- Validation logic must now handle nested paths
- Still needs error handling for malformed paths at runtime

**Performance:** O(1) for lookup (native JSON indexing), but upfront O(n) parsing during compilation.

---

## Recommendation: **Option B (Nested JSON + Dot-Path Traversal)**

### Rationale
1. **Correctness:** Preserves full gate output semantics (exit codes, error messages, timeout state). Future gates may return structured data beyond just pass/fail.
2. **Maintainability:** Simpler than Option C; fewer modules to change. Clear separation of concerns: gates under `gates.*`, evidence under root.
3. **Extensibility:** When clauses naturally express structured queries: `"gates.ci_check.exit_code": 0`, `"gates.deployment.logs": "success"`.
4. **Performance Trade-off:** Dot-path traversal is fast enough on the resolver's hot path (typical: 2–3 levels, called once per state transition). The resolver is not in a microsecond-critical loop.

### Implementation Plan
1. **Merge gate results into evidence** (advance.rs ~296):
   ```rust
   let mut gates_json = serde_json::Map::new();
   for (name, result) in gate_results {
       gates_json.insert(name.clone(), gate_result_to_json(result));
   }
   current_evidence.insert("gates".to_string(), serde_json::Value::Object(gates_json));
   ```

2. **Add dot-path resolver** (advance.rs):
   ```rust
   fn resolve_dot_path(value: &serde_json::Value, path: &str) -> Option<&serde_json::Value> {
       path.split('.')
           .try_fold(value, |acc, key| acc.get(key))
   }
   ```

3. **Update resolve_transition** (lines 411–413):
   ```rust
   let all_match = conditions
       .iter()
       .all(|(field, expected)| {
           resolve_dot_path(&serde_json::Value::Object(evidence), field)
               == Some(expected)
       });
   ```

4. **Update template validation** (template/types.rs):
   - Recognize dot-paths in `when` fields as valid if parent path exists
   - Warn if paths reference undefined gates, but don't block compilation

5. **Test coverage:**
   - Gate result merging (exit codes, timeouts, errors)
   - Dot-path matching (shallow & deep paths)
   - Backward compatibility (flat keys still work)
   - Mixed evidence (gates + agent evidence in single conditions)

---

## Risk Mitigation
- **Fallback:** If performance becomes an issue, cache parsed dot-paths in transition structs (compile time).
- **Validation:** Template compiler must reject invalid gate names in `when` clauses.
- **Documentation:** Clearly explain gate output schema and dot-path syntax in user docs.

---

## Decision Log
- **Date:** 2026-03-30
- **Status:** Recommended for implementation
- **Complexity:** Standard (fast path, clear changes)
- **Backward Compat:** Fully supported; flat keys continue to work
