# Design Decision: Structured Gate Output

**Decision:** Option A - Replace GateResult with StructuredGateResult  
**Date:** 2026-03-30  
**Status:** Ready for Implementation

## Current State Analysis

### GateResult Enum (4 variants)
```rust
pub enum GateResult {
    Passed,
    Failed { exit_code: i32 },
    TimedOut,
    Error { message: String },
}
```

### Current Usage Patterns
1. **Blocking Condition Conversion** (`next_types.rs`): Pattern matches on enum variants to extract status strings
2. **Gate Blocking Logic** (`next.rs`, `advance.rs`): Checks `GateResult::Passed` to determine if gates block state advancement
3. **Evidence Flow**: GateResult stored in StopReason but never merged into evidence map
4. **Tests**: 17 test sites match on GateResult enum variants with specific patterns (e.g., `GateResult::Failed { exit_code: 1 }`)

### Schema Requirements
Each gate type has a fixed output schema:
- **command**: `{exit_code: number, error: string}`
- **context-exists**: `{exists: boolean, error: string}`
- **context-matches**: `{matches: boolean, error: string}`

### Key Constraint
All three scenarios (success, failure, timeout/error) must produce the same schema shape with fields populated appropriately.

## Option Analysis

### Option A: Replace GateResult with StructuredGateResult
```rust
pub struct StructuredGateResult {
    outcome: GateOutcome,  // enum: Passed, Failed, TimedOut, Error
    output: serde_json::Value,  // structured JSON matching gate schema
}
```

**Advantages:**
- Single return type carries both outcome AND structured data
- Clean separation: outcome enum for control flow, JSON for evidence merging
- No parallel types; single source of truth
- Easy to extend with additional metadata without enum variant explosion
- Tests can still pattern match on outcome while accessing JSON directly

**Disadvantages:**
- All callsites must extract `outcome` field for gate-blocking logic
- Slightly more verbose for control flow checks

**Backward Compatibility Path:**
- Phase 1: Keep GateResult alongside StructuredGateResult
- Phase 2: Replace blocking_conditions_from_gates() to accept StructuredGateResult
- Phase 3: Migrate tests incrementally
- Phase 4: Remove GateResult enum

**Migration Effort:** Medium (well-defined phases)

---

### Option B: Extend GateResult with JSON Field
```rust
pub enum GateResult {
    Passed(serde_json::Value),
    Failed { exit_code: i32, output: serde_json::Value },
    TimedOut(serde_json::Value),
    Error { message: String, output: serde_json::Value },
}
```

**Advantages:**
- Keeps single enum type
- Pattern matching still works (extract JSON from variant)

**Disadvantages:**
- Enum variants now carry heterogeneous data (some have JSON, some have message+JSON)
- Tests must change every pattern match to extract JSON values
- Harder to reason about: outcome and structured data tightly coupled
- Violates Rust enum design principle (variants should be semantically distinct)
- Creates asymmetric types (Error has message + JSON, Passed has only JSON)

**Backward Compatibility Path:**
- Very difficult; every test and match site must change
- 17 test sites all require rewriting

**Migration Effort:** High (requires changes everywhere GateResult is matched)

---

### Option C: Parallel Return Type (GateOutput alongside GateResult)
```rust
pub enum GateOutput {
    CommandOutput { exit_code: number, error: string },
    ContextExistsOutput { exists: boolean, error: string },
    ContextMatchesOutput { matches: boolean, error: string },
}

// evaluate_gates returns both
pub fn evaluate_gates(...) -> (BTreeMap<String, GateResult>, BTreeMap<String, GateOutput>) { ... }
```

**Advantages:**
- GateResult remains unchanged (easiest backward compatibility)
- Type-safe conversion at boundary

**Disadvantages:**
- Two parallel data structures that must stay synchronized
- Risk of silent failures if one is updated without the other
- Doubles iteration cost when populating evidence map
- Confusing: callers must remember both types
- Hard to reason about consistency between GateResult and GateOutput
- Violates DRY principle (outcome represented in both types)

**Backward Compatibility Path:**
- Best short-term compatibility (GateResult unchanged)
- But creates technical debt: two sources of truth

**Migration Effort:** Medium-High (maintain two parallel types, eventual consolidation)

---

## Evaluation Against Constraints

| Constraint | Option A | Option B | Option C |
|-----------|----------|----------|----------|
| Output is serde_json::Value | ✅ Clean | ⚠️ Embedded in variants | ✅ Clean |
| Fixed schema per gate type | ✅ Guaranteed | ⚠️ Scattered across variants | ✅ Guaranteed |
| Timeout/error same schema shape | ✅ Yes | ⚠️ Asymmetric types | ✅ Yes |
| Backward compatibility path | ✅ Phased replacement | ❌ High friction | ⚠️ Two systems conflict |
| Code clarity | ✅ Clear separation | ❌ Confusing coupling | ⚠️ Parallel redundancy |
| Test maintenance | ✅ Gradual | ❌ Pervasive changes | ⚠️ Dual representations |

---

## Decision Rationale

**Option A is selected** because:

1. **Single Source of Truth**: Outcome + structured data coexist without duplication or synchronization issues
2. **Backward Compatibility**: Phased replacement allows existing code to remain unchanged during migration
3. **Extensibility**: Future gate enhancements (context variables, metrics) add to JSON without enum sprawl
4. **Evidence Integration**: JSON output naturally merges into evidence map; outcome enum handles control flow
5. **Test Resilience**: Tests can still validate outcome via pattern matching while accessing JSON for payload verification
6. **Type Safety**: No asymmetric variants or coupled data; each piece serves its purpose

---

## Implementation Roadmap

### Phase 1: Introduce StructuredGateResult (non-breaking)
- Define `StructuredGateResult` struct in gate.rs
- Keep GateResult enum unchanged
- Add conversion function: `GateResult -> StructuredGateResult`
- Add helper: `structured_result.is_passed()` for control flow

### Phase 2: Update evaluation functions (internal refactor)
- Modify `evaluate_command_gate()` to return `StructuredGateResult`
- Update `evaluate_gates()` to return `BTreeMap<String, StructuredGateResult>`
- Update `advance.rs` to unpack outcome for blocking logic
- Update blocking_conditions_from_gates() to accept StructuredGateResult

### Phase 3: Migrate tests incrementally
- Update tests in gate.rs to verify both outcome and JSON payload
- Update integration tests in advance.rs and next.rs
- Verify backward compatibility helpers work correctly

### Phase 4: Remove GateResult enum
- Delete GateResult type after all callers migrated
- Clean up conversion helpers

---

## Output Schema Examples

### Command Gate (all scenarios)
```json
{
  "exit_code": 0,
  "error": ""
}
```
With timeout: `exit_code: -1, error: "command timed out"`  
With spawn error: `exit_code: -1, error: "spawn failed: ..."`

### Context-Exists Gate
```json
{
  "exists": true,
  "error": ""
}
```
With missing key: `exists: false, error: "key not found"`

### Context-Matches Gate
```json
{
  "matches": true,
  "error": ""
}
```
With regex error: `matches: false, error: "invalid regex pattern: ..."`

---

## Notes

- The outcome enum (Passed, Failed, TimedOut, Error) remains useful for control flow and gate-blocking decisions
- JSON field merges into evidence map for downstream processing
- Schema validation occurs at the boundary where evidence is submitted (in `engine/evidence.rs`)
- No changes needed to `validate_evidence()` or `FieldSchema` — gates produce structured data *before* evidence validation
