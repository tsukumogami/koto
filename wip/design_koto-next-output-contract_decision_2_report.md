# Decision: Threading Gate Results Through StopReason to EvidenceRequired

**Status**: DECIDED
**Chosen**: Option (a) -- Add gate data to StopReason::EvidenceRequired

## Context

When gates fail on a state that has an `accepts` block, the engine sets `gates_failed = true` and falls through to transition resolution (advance.rs line 297-311). The transition resolver returns `NeedsEvidence`, and the engine returns `StopReason::EvidenceRequired` -- but the gate results are lost. The CLI handler at mod.rs line 1708 receives a bare `EvidenceRequired` with no gate information, so it can't populate `blocking_conditions` on the response.

## Options Evaluated

### (a) Add gate data to StopReason::EvidenceRequired

Change `EvidenceRequired` from a unit variant to a struct variant carrying `Option<BTreeMap<String, GateResult>>`. The engine passes gate results when gates failed, `None` otherwise. The CLI handler converts to `Vec<BlockingCondition>` using the existing conversion logic. Empty/None maps to an empty array, preserving backward compatibility.

### (b) Re-evaluate gates in the CLI handler

After receiving `StopReason::EvidenceRequired`, call `evaluate_gates` again on the final state's gates. This recovers the data but runs shell commands a second time -- non-deterministic results, doubled latency, and the CLI layer shouldn't be making engine-level decisions about gate evaluation.

### (c) Thread gate results through a separate AdvanceResult field

Add a `gate_results: Option<BTreeMap<String, GateResult>>` field to `AdvanceResult` alongside `stop_reason`. This means every stop reason handler must decide whether to inspect the separate field, and the relationship between the gate data and the stop reason is implicit rather than explicit.

## Analysis

**Option (a) wins on all axes that matter:**

- **Locality**: The data travels with the stop reason it explains. No ambient state to forget.
- **Single evaluation**: Gates run once in the engine. No repeated side effects.
- **Minimal diff**: One enum variant gains a field; one construction site (advance.rs ~line 346) passes the local `gate_results`; one match arm (mod.rs ~line 1708) destructures it. The GateResult-to-BlockingCondition conversion already exists and can be extracted into a shared helper to eliminate the current duplication.
- **Backward compatibility**: `None` (or empty map) produces an empty `blocking_conditions` array. Existing tests that match on `StopReason::EvidenceRequired` need a minor pattern update but no logic change.
- **Type safety**: The compiler forces every match arm to handle the new field.

**Option (b) is rejected** because re-evaluating gates introduces non-determinism (gate commands may produce different results on second run), doubles execution time, and violates the separation between engine and CLI layers.

**Option (c) is rejected** because it scatters related data across two struct fields. Every handler must remember to check the separate field, and the compiler won't help if they forget. It also adds a field that's only meaningful for two of the nine stop reasons, polluting the common result type.

## Recommended Shape

```rust
pub enum StopReason {
    // ...
    EvidenceRequired {
        /// Gate results when gates failed on a state with an accepts block.
        /// None when no gates are defined or all gates passed.
        failed_gates: Option<BTreeMap<String, GateResult>>,
    },
    // ...
}
```

Construction in advance.rs:

```rust
TransitionResolution::NeedsEvidence => {
    if template_state.accepts.is_some() {
        let failed = if gates_failed {
            Some(gate_results.clone())
        } else {
            None
        };
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            stop_reason: StopReason::EvidenceRequired {
                failed_gates: failed,
            },
        });
    }
    // ...
}
```

Note: `gate_results` must be captured before the gate evaluation block ends. Currently it's scoped inside the `if !template_state.gates.is_empty()` block. It should be hoisted to a `let mut gate_results = BTreeMap::new()` before that block, populated inside it.

## Deduplication Opportunity

The GateResult-to-BlockingCondition conversion is duplicated in `src/cli/next.rs` (lines 42-55) and `src/cli/mod.rs` (lines 1684-1697). Extract a shared function:

```rust
pub fn blocking_conditions_from_gates(
    gate_results: &BTreeMap<String, GateResult>,
) -> Vec<BlockingCondition>
```

Both call sites and the new EvidenceRequired handler can use it.

## Assumptions

1. The `gate_results` local variable can be hoisted out of the gate evaluation block without changing semantics (it's only read, never mutated after population).
2. Existing tests that pattern-match `StopReason::EvidenceRequired` can be updated to use `StopReason::EvidenceRequired { .. }` without logic changes.
3. An empty `blocking_conditions` array in the JSON output is backward-compatible with consumers (agents treat empty array the same as absent field).
