# Decision 1: CLI Flag Threading

## Question
How does --override-rationale thread from CLI to the advance loop? The rationale string must reach the gate-failure detection point in advance_until_stop so the engine can bypass gates and emit a GateOverrideRecorded event.

## Options Considered

### Option A: Separate Parameter to advance_until_stop
Pass override_rationale as an additional closure/parameter alongside evidence to `advance_until_stop()`. The advance loop receives it directly and injects it into the GateOverrideRecorded event when gates fail and evidence exists.

**Pros:**
- Minimal surface area: one new parameter, one new event type
- Clean separation: evidence flows through evidence validation; rationale flows separately
- Direct access at the detection point (line 296-315 of advance.rs where gates_failed is set)
- No event ordering complexity: rationale is bound to its override at emission time
- Works on gate-only states (no accepts block) if we allow override-rationale to bypass the evidence validation requirement
- Backward compatible: existing code paths unaffected; parameter defaults to None

**Cons:**
- Requires signature change to advance_until_stop (though already has many parameters)
- ThreadLocal or Arc<Mutex<>> temptation if caller forgets to pass it
- Tight coupling: rationale is only meaningful when gates_failed=true, not obvious from signature

**Implementation sketch:**
```rust
pub fn advance_until_stop<F, G, I, A>(
    current_state: &str,
    template: &CompiledTemplate,
    evidence: &BTreeMap<String, serde_json::Value>,
    override_rationale: Option<&str>,  // NEW
    append_event: &mut F,
    // ... rest unchanged
) {
    // At line ~312 when gates_failed and evidence exists:
    if let Some(rationale) = override_rationale {
        let payload = EventPayload::GateOverrideRecorded {
            state: state.clone(),
            gates_failed: failed_gate_results.clone(),
            override_evidence: current_evidence.clone(),
            rationale: rationale.to_string(),
        };
        append_event(&payload)?;
    }
}
```

In CLI handler (line 1676):
```rust
// Parse --override-rationale from args (if present)
let override_rationale = args.override_rationale.as_deref();

let result = advance_until_stop(
    current_state,
    &compiled,
    &evidence,
    override_rationale,  // NEW
    &mut append_closure,
    // ... rest unchanged
);
```

### Option B: Pre-Advance Event with Rationale
Before calling advance_until_stop, append a GateOverrideRequested event containing the rationale. The advance loop detects gates failing, checks if a GateOverrideRequested event exists for the current state/epoch, and emits GateOverrideRecorded if both gates and override are present.

**Pros:**
- No signature change to advance_until_stop: rationale is already in the event log
- Persistence is automatic: rationale is journaled before advancement
- Event ordering is explicit: GateOverrideRequested comes before transition events
- Works with gate-only states: override event is independent of evidence validation
- Future-proof: query layer can find override rationale without parsing closures

**Cons:**
- Two-phase pattern: emit event, then call advance loop; if advance fails, we've persisted intent without outcome
- Correlation logic: advance loop must look backward in event log to find the override event for this state
- Event ordering with --with-data: both EvidenceSubmitted and GateOverrideRequested appended before advance; order must be deterministic
- Adds complexity to derive_evidence: do we include override rationale in the epoch evidence map?
- If advance_until_stop fails to detect the override (e.g., gates don't fail), the GateOverrideRequested event is orphaned in the log

**Implementation sketch:**
```rust
// CLI handler (after evidence validation, before advance)
if let Some(rationale) = args.override_rationale {
    let payload = EventPayload::GateOverrideRequested {
        state: current_state.clone(),
        rationale,
    };
    backend.append_event(&name, &payload, &now_iso8601())?;
}

// Inside advance_until_stop (at line ~312):
if gates_failed && !current_evidence.is_empty() {
    // Look back in recent events for GateOverrideRequested
    // OR pass the rationale from outside
    let payload = EventPayload::GateOverrideRecorded {
        state: state.clone(),
        gates_failed: failed_gate_results.clone(),
        override_evidence: current_evidence.clone(),
        rationale: "...".to_string(),  // Must retrieve from log
    };
    append_event(&payload)?;
}
```

### Option C: Wrap in Context Struct
Create a struct `AdvanceContext` containing optional override_rationale, along with other future state (e.g., audit metadata, execution tracing). Pass this single struct to advance_until_stop instead of individual parameters.

**Pros:**
- Extensible: future metadata (audit ID, execution trace, user identity) fits naturally
- Reduces parameter count: one struct instead of multiple args
- Decouples caller from function signature: can add fields to struct without breaking callers
- Self-documenting: struct fields explain all inputs to the advance loop

**Cons:**
- Over-engineering for current need: only two fields (evidence map + override rationale)
- Adds a new type that must be versioned separately from function
- Similar to Option A in thread-safety concerns: context must be passed, not stored globally
- Signature change is still required, just grouped differently
- Backward compatibility requires a factory function or default implementation

**Implementation sketch:**
```rust
pub struct AdvanceContext {
    pub evidence: BTreeMap<String, serde_json::Value>,
    pub override_rationale: Option<String>,
    // future: pub audit_id: Option<String>,
    // future: pub trace_spans: Vec<String>,
}

pub fn advance_until_stop<F, G, I, A>(
    current_state: &str,
    template: &CompiledTemplate,
    context: &AdvanceContext,  // CHANGED
    append_event: &mut F,
    // ... rest unchanged
) {
    // At gate detection:
    if gates_failed && !context.evidence.is_empty() {
        if let Some(rationale) = &context.override_rationale {
            let payload = EventPayload::GateOverrideRecorded {
                state: state.clone(),
                gates_failed: failed_gate_results.clone(),
                override_evidence: context.evidence.clone(),
                rationale: rationale.clone(),
            };
            append_event(&payload)?;
        }
    }
}
```

In CLI:
```rust
let context = AdvanceContext {
    evidence,
    override_rationale: args.override_rationale.clone(),
};

let result = advance_until_stop(
    current_state,
    &compiled,
    &context,  // CHANGED
    &mut append_closure,
    // ... rest unchanged
);
```

## Chosen: Option A

## Confidence: High

## Rationale

**Option A (direct parameter) is the best fit for this constraints and design phase.**

1. **Minimal surface area:** One new parameter and one new event type is the absolute minimum required to pass rationale through. Option C over-engineers for extensibility we don't need yet; Option B adds a two-phase pattern that introduces failure modes (orphaned events).

2. **Event ordering is deterministic:** Evidence is appended before advance_until_stop is called (line 1489); override_rationale is passed as a parameter, not appended before. This ensures GateOverrideRecorded is emitted during advancement, not before. No ordering ambiguity with --with-data.

3. **No log correlation complexity:** Option B requires the advance loop to look backward in the event log to find GateOverrideRequested for the current state. This is fragile (what if the event is missing? what if there are multiple?) and couples the advance loop to the persistence layer's structure. A direct parameter avoids this entirely.

4. **Backward compatible:** Existing code paths are unaffected. Any code not passing override_rationale (the common case initially) defaults to None, and the gate override path doesn't execute.

5. **Works on gate-only states:** The PRD requirement is "must work on gate-only states (no accepts block) too." Option A naturally supports this: if gates fail and override_rationale is present, emit the override event regardless of whether accepts exists. Option B would also work here, but Option A is simpler.

6. **Tight coupling is acceptable here:** The override_rationale parameter is only meaningful when gates fail and evidence exists. This is intentional coupling: it captures the semantic constraint that overrides are only relevant at the gate detection point. Making this coupling invisible (Option B via log lookback) is actually worse.

7. **Matches existing pattern:** The evidence parameter already flows directly to advance_until_stop (line 166). Rationale flows the same way; this is consistent.

## Assumptions

- Gate override events are only emitted when gates fail AND evidence is present to bypass them. Override rationale without evidence or without gate failure is an error (caught at CLI validation, not emitted).
- The GateOverrideRecorded event will include the failed gate results and the evidence that bypassed them, for full audit trail. Rationale is the agent's explanation.
- "Override" is narrowly scoped to the gate-failure path, not directed transitions (--to) or action skips. Those are separate override patterns.
- The signature change to advance_until_stop is acceptable (it already has 7 parameters; one more is standard Rust).

## Rejected

- **Option B (Pre-Advance Event):** Introduces a two-phase pattern where GateOverrideRequested is persisted before the advance loop runs. If the loop detects the override and gates fail, great. But if gates don't fail, the event is orphaned in the log. Correlation via log lookback adds complexity and fragility. Event ordering with --with-data becomes non-obvious.

- **Option C (Context Struct):** Over-engineering. A context struct is useful when multiple related pieces of state need to be threaded together (e.g., audit ID, execution trace, user identity). Today, we only need evidence (already in scope) and override rationale. The minimal addition is a parameter, not a new abstraction. We can refactor to Option C later if extensibility demand materializes.

## Implementation Path

1. Add `override_rationale: Option<&str>` parameter to `advance_until_stop` signature (src/engine/advance.rs line 163).
2. Add `GateOverrideRecorded` variant to `EventPayload` enum (src/engine/types.rs, line 30), with fields: state, gates_failed (BTreeMap<String, GateResult>), override_evidence (HashMap for the evidence that bypassed), rationale (String).
3. In advance_until_stop, at the gate detection point (line 312 after `gates_failed = true`), check if override_rationale is Some and emit GateOverrideRecorded before continuing.
4. In CLI handler (src/cli/mod.rs line 1676), parse --override-rationale and pass to advance_until_stop.
5. Update EventPayload deserialization (types.rs line 156+) to handle the new variant.
6. Add tests for override event emission and serialization.

---

**3-Line Summary:**
- **Chosen:** Option A (direct parameter to advance_until_stop)
- **Confidence:** High
- **Key Reason:** Minimal surface area, deterministic event ordering, no log correlation complexity, directly matches the existing evidence parameter pattern.
