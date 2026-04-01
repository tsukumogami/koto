# Design Decision: Gate Pass Condition Evaluation

**Question:** How should the advance loop determine pass/fail from structured gate output?

**Context:** The current code at `/src/engine/advance.rs` (lines 297-299) checks `matches!(r, GateResult::Passed)` for each gate result to decide if any gate failed. With structured output, each gate type has a pass condition (command: `exit_code == 0`, context-exists: `exists == true`, context-matches: `matches == true`). We need to decide where pass conditions live and how the advance loop evaluates them.

---

## Constraints

- Pass conditions are per-gate-type, not per-gate-instance
- Each gate type has a fixed pass condition defined in the engine
- The advance loop determines "all gates pass" to decide auto-advance vs stop
- Future gate types will have their own pass conditions
- The `gate_failed` boolean feeds into `resolve_transition` (line 319, parameter)
- Backward compat: the existing `gate_failed` path must work for legacy templates

---

## Option Analysis

### Option A: Pass conditions as functions in a gate type registry

**Pattern:** Register a `fn(&serde_json::Value) -> bool` per gate type. The advance loop calls the registered function per gate.

**Implementation Sketch:**
```rust
type PassCondition = fn(&serde_json::Value) -> bool;

struct GateRegistry {
    conditions: HashMap<String, PassCondition>,
}

let registry = GateRegistry::new(); // pre-populated with command, context-exists, context-matches

let any_failed = gate_results
    .iter()
    .any(|(gate_name, result)| {
        let gate_type = template_state.gates[gate_name].gate_type;
        let condition = registry.get(&gate_type)?;
        !condition(&result.to_json())
    });
```

**Pros:**
- Fully extensible: new gate types only need to register a function
- Function signature is simple and flexible
- Decouples gate evaluation from pass logic
- Clear separation: evaluators produce structured output, registry evaluates it
- Works naturally with functional composition

**Cons:**
- Functions can't be serialized/debugged easily
- The registry must be initialized at engine startup (requires careful wiring)
- Function pointers in a hashmap are less discoverable than enum match statements
- Harder to unit test registry logic in isolation (functions are opaque)
- No declarative visibility into what pass means for a gate type

**Backward compat:** Works fine — legacy gates that already return `GateResult::Passed` just convert to JSON and the function evaluates it.

---

### Option B: Pass conditions as declarative rules (field/value pairs)

**Pattern:** Each gate type declares its pass condition as field/value pairs. The engine evaluates via JSON equality, same as when clause matching.

**Implementation Sketch:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatePassCondition {
    field: String,    // e.g., "exit_code"
    value: serde_json::Value,  // e.g., 0
}

static GATE_PASS_CONDITIONS: &[(&str, &[GatePassCondition])] = &[
    ("command", &[GatePassCondition { field: "exit_code".to_string(), value: json!(0) }]),
    ("context-exists", &[GatePassCondition { field: "exists".to_string(), value: json!(true) }]),
    ("context-matches", &[GatePassCondition { field: "matches".to_string(), value: json!(true) }]),
];

let any_failed = gate_results
    .iter()
    .any(|(gate_name, result)| {
        let gate_type = template_state.gates[gate_name].gate_type;
        let conditions = GATE_PASS_CONDITIONS
            .iter()
            .find(|(t, _)| *t == gate_type)
            .map(|(_, c)| c)?;
        
        !conditions.iter().all(|cond| {
            result.get(&cond.field) == Some(&cond.value)
        })
    });
```

**Pros:**
- Declarative and auditable: pass conditions are visible in code as data
- Reuses existing "when clause" matching logic already in the engine
- Easy to unit test and debug
- Backward compat: legacy gates return enum, convert to JSON, matching works
- Can be serialized for discovery tools

**Cons:**
- Single field/value pairs limit expressiveness (e.g., can't do "exit_code > 0 AND < 128")
- Requires a central registry of conditions (duplication vs gate definition)
- Adding new gate types still requires code change (same as Option A)
- Less flexible if a future gate needs complex logic

**Backward compat:** Works fine — legacy `GateResult::Passed` enum converts to `{"status": "passed"}` or similar and matches a declarative rule.

---

### Option C: Pass conditions embedded in GateResult/GateOutput type

**Pattern:** Each gate evaluation returns both structured data AND a boolean "passed" flag computed by the gate evaluator.

**Implementation Sketch:**
```rust
#[derive(Debug, Clone)]
pub struct GateOutput {
    pub gate_type: String,
    pub passed: bool,  // ← computed by evaluator
    pub data: serde_json::Value,  // structured output
}

pub enum GateResult {
    Passed { output: GateOutput },
    Failed { output: GateOutput, reason: String },
    TimedOut { output: GateOutput },
    Error { message: String },
}

let any_failed = gate_results
    .values()
    .any(|r| {
        match r {
            GateResult::Passed { .. } => false,
            GateResult::Failed { .. } => true,
            GateResult::TimedOut { .. } => true,
            GateResult::Error { .. } => true,
        }
    });
```

**Pros:**
- Single source of truth: the gate evaluator computes "passed" when it evaluates
- No need for a separate registry or declarative rules
- Advance loop is trivial: just check the enum discriminant
- Backward compat: `GateResult::Passed` already carries the right meaning
- Minimal code change to existing evaluation functions

**Cons:**
- Pass logic is spread across the gate evaluation functions (command, context-exists, context-matches)
- Harder to unit test pass conditions in isolation
- If pass logic changes, must update all evaluator functions
- Less discoverable: no central registry tells you what "pass" means per gate type
- Duplicates the pass condition logic if multiple evaluators apply the same rule

**Backward compat:** Already works — no changes to `GateResult` enum itself needed, just carry the passed boolean separately.

---

## Evaluation

| Criterion | Option A | Option B | Option C |
|-----------|----------|----------|----------|
| **Extensibility** | Excellent — just register a function | Good — just add to registry | Fair — need to update evaluators |
| **Auditability** | Fair — functions are opaque | Excellent — data-driven rules | Fair — logic in functions |
| **Testability** | Fair — hard to mock functions | Excellent — test rules directly | Good — unit test evaluators |
| **Backward compat** | Good | Good | Excellent |
| **Code change scope** | Medium (add registry) | Small (add rules) | Minimal (current path) |
| **Reuses existing patterns** | No | Yes (when clauses) | Yes (enum match) |
| **Future gate type cost** | Low | Low | Medium |

---

## Recommendation

**Option C: Embedded pass flag in GateResult.**

**Rationale:**

1. **Minimal disruption:** The advance loop already matches on `GateResult` enums. We keep that pattern. The pass logic stays where it belongs — in the evaluators that run each gate.

2. **Backward compat is free:** The existing code path `matches!(r, GateResult::Passed)` continues to work without modification. Legacy templates benefit automatically.

3. **Pass conditions are per-evaluator:** Each gate type (command, context-exists, context-matches) already has one evaluation function that knows its pass condition. Adding a `passed: bool` field to the structured output avoids duplicating that logic elsewhere.

4. **Avoids over-engineering:** Option A (registry) and Option B (declarative rules) both solve a future problem (extensibility to unknown gate types). For the known gate types we have today, Option C is sufficient and simpler. If we later add a gate type that doesn't fit this pattern, we can revisit. YAGNI applies here.

5. **Testing is local:** Each gate evaluator is tested in isolation. The evaluator function tests verify that `passed` is computed correctly, right where the evaluator lives.

**Implementation:**

1. Modify `GateOutput` (if it exists) or create it in `src/gate.rs`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct GateOutput {
       pub passed: bool,
       #[serde(flatten)]
       pub data: serde_json::Value,  // Flatten structured output
   }
   ```

2. Update gate evaluators to compute `passed` based on their pass condition:
   ```rust
   // In evaluate_command_gate
   let passed = output.exit_code == 0;
   return GateOutput { passed, data: json!({ "exit_code": output.exit_code }) };

   // In evaluate_context_exists_gate
   let passed = store.ctx_exists(sess, &gate.key);
   return GateOutput { passed, data: json!({ "exists": passed }) };

   // In evaluate_context_matches_gate
   let passed = re.is_match(&content);
   return GateOutput { passed, data: json!({ "matches": passed }) };
   ```

3. Advance loop stays nearly identical:
   ```rust
   let any_failed = gate_results
       .values()
       .any(|output| !output.passed);
   ```

4. Backward compat: If a gate returns the old `GateResult::Passed` enum, convert it on deserialize or at call site.

---

## Decision Log

- **Decision:** Embedded pass flag (Option C)
- **Complexity:** Standard (fast path)
- **Risk:** Low — minimal API change, reuses existing patterns
- **Next step:** Implement `GateOutput` type and update evaluators
