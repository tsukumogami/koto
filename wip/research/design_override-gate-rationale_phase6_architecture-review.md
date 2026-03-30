# Architecture Review: DESIGN-override-gate-rationale

Reviewer: architect-reviewer
Date: 2026-03-30

## Summary Verdict

The design fits the existing architecture well. It follows established patterns for event types, parameter threading, and derive functions. Three issues need resolution before implementation -- one blocking, two advisory.

## Findings

### 1. GateResult is not serializable -- event payload needs a conversion layer

**Severity: Blocking**

The design specifies `gates_failed: BTreeMap<String, GateResult>` as a field on the `GateOverrideRecorded` event payload (`src/engine/types.rs`). However, `GateResult` in `src/gate.rs:19` does not derive `Serialize` or `Deserialize`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum GateResult {
    Passed,
    Failed { exit_code: i32 },
    TimedOut,
    Error { message: String },
}
```

`EventPayload` derives `Serialize, Deserialize` via `#[serde(untagged)]`. Adding a variant that contains a non-serializable type will fail at compile time. The existing codebase handles this at the CLI boundary: `blocking_conditions_from_gates()` in `src/cli/next_types.rs` converts `GateResult` into a serializable `BlockingCondition` struct before JSON output.

Two options:

**Option A**: Add `#[derive(Serialize, Deserialize)]` to `GateResult`. This is the simplest fix but changes a type in `src/gate.rs` that currently has no serialization concern. Every `GateResult` consumer gains serde as a transitive dependency.

**Option B**: Define a serializable `GateFailure` struct within `src/engine/types.rs` (similar to how `BlockingCondition` works in the CLI layer) and convert at the point where the event is constructed. This keeps gate evaluation free of serialization concerns and gives the event payload its own stable schema, decoupled from the internal `GateResult` representation.

Option B is more consistent with how the codebase already handles this boundary (the CLI layer does the same conversion). The design should specify which approach and include the conversion in Phase 1 deliverables.

### 2. Design references BTreeMap but event example shows different shape

**Severity: Advisory**

The design's event payload example (line 306-311) shows:

```json
"gates_failed": {
  "ci_check": {"result": "failed", "exit_code": 1}
}
```

This JSON shape doesn't match `BTreeMap<String, GateResult>` serialized directly. `GateResult::Failed { exit_code: 1 }` would serialize differently depending on serde configuration (tagged vs untagged enum). This is a documentation inconsistency rather than a code issue, but it reinforces that the implementation needs a purpose-built serializable type (Finding 1, Option B) so the event schema is explicit rather than derived from an internal enum's serde behavior.

### 3. advance_until_stop signature: parameter count is accurate

**Severity: No issue**

The design claims the function "already has 7+ parameters." The actual signature in `src/engine/advance.rs:163-178` has exactly 8 parameters (current_state, template, evidence, append_event, evaluate_gates, invoke_integration, execute_action, shutdown). The function already carries `#[allow(clippy::too_many_arguments)]`. Adding `override_rationale: Option<&str>` brings it to 9. This matches the design's claim and the acknowledged negative consequence (line 437).

### 4. Gate evaluation block: line reference is close but not exact

**Severity: Advisory**

The design references "~line 295-315" for the gate evaluation block. The actual gate evaluation is at lines 292-316 in the current source. Close enough for implementation guidance, but worth noting that the `accepts.is_none()` check at line 305 is the exact insertion point where override logic branches. The design correctly describes the branching behavior at this point.

### 5. Event deserialization: manual dispatch arm needed

**Severity: Advisory**

The design mentions "deserialization via the existing untagged discriminant pattern" but doesn't call out that `Event`'s custom `Deserialize` impl (types.rs:128-248) uses a manual match on `event_type` strings. Adding `GateOverrideRecorded` requires:

1. A new `"gate_override_recorded"` arm in the match at line 156
2. A new helper struct `GateOverrideRecordedPayload` for typed deserialization (following the pattern of `TransitionedPayload`, `EvidenceSubmittedPayload`, etc.)

This is mechanical and follows an obvious pattern, but it's 15-20 lines of boilerplate that should be in Phase 1 scope explicitly. The design's Phase 1 says "implement serialization, deserialization, and round-trip tests" which covers it implicitly.

### 6. derive_overrides follows the correct precedent

**Severity: No issue**

The design correctly identifies `derive_visit_counts` (persistence.rs:330) as the precedent for cross-epoch queries. The existing epoch-scoped functions (`derive_evidence`, `derive_decisions`) filter from the last state-changing event forward. `derive_visit_counts` scans the full log. `derive_overrides` using the same full-scan pattern is architecturally consistent. No new abstraction needed.

### 7. CLI subcommand: koto overrides list

**Severity: Advisory**

The design proposes `koto overrides list <name>`. The existing CLI surface uses noun-verb patterns: `koto next`, `koto status`, `koto query`, `koto rewind`, `koto template compile`. The `overrides list` pattern introduces a two-level subcommand (noun + verb under noun). Check if this is the intended precedent for future query subcommands, or if `koto query --overrides` or `koto query overrides` would be more consistent with the existing surface. This is a CLI surface decision, not a structural violation -- either works, but the choice sets a pattern.

## Architecture Fit Assessment

### What fits well

- **Event type addition**: Adding a variant to `EventPayload` with a `type_name()` mapping follows the exact pattern used by all 9 existing variants. No new dispatch mechanism needed.
- **Parameter threading**: Passing `override_rationale` as an `Option<&str>` to `advance_until_stop` matches how evidence already reaches the advance loop. The design correctly rejects the pre-event and context-struct alternatives.
- **Unconditional fallback for gate-only states**: The `resolve_transition` function already handles unconditional fallbacks. Reusing this for override targets avoids new template schema concerns.
- **derive_overrides**: Purpose-built function following `derive_visit_counts` precedent. No new abstractions, no generic frameworks.

### What needs attention

- The `GateResult` serialization gap (Finding 1) is the only blocking issue. It's a mechanical fix but the design should specify the approach before implementation begins.
- The CLI subcommand naming (Finding 7) sets a precedent for how future query-style commands are structured. Worth a deliberate choice.

## Implementation Phase Sequencing

The three phases are correctly sequenced:

1. **Phase 1 (event + persistence)**: Foundation layer. No upstream dependencies. Testable in isolation.
2. **Phase 2 (advance loop)**: Depends on Phase 1 (emits the new event type). The signature change touches one call site in `src/cli/mod.rs:1676`.
3. **Phase 3 (CLI)**: Depends on both prior phases. Integration tests exercise the full stack.

One refinement: the GateResult-to-serializable conversion (Finding 1) should be explicitly scoped into Phase 1, since it affects the event payload struct definition.

## Alternatives Not Considered

The design's alternative analysis is thorough for the three decisions it covers. One alternative worth noting:

**StopReason variant instead of parameter**: Instead of adding `override_rationale` as a parameter, the advance loop could return a new `StopReason::GateOverridable` when gates fail and the caller re-invokes with confirmation. This would keep the advance function signature stable but introduce a two-call pattern (check, then override) that adds complexity to the CLI handler. The design's single-call approach is simpler. Not a recommendation to change -- just documenting that this was implicitly rejected.
