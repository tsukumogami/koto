# Feasibility Review: PRD-gate-transition-contract

## Verdict: FAIL (Scope Too Large for Single Cycle)

This feature redesigns gate/transition coupling and requires 4+ implementation areas. With careful phasing, it's realistic, but attempting everything in one design+implementation cycle creates unmanageable technical debt and integration risk.

## Feasibility Concerns

### 1. Shell command to structured output (R2): Requires gate type refactoring
**Problem:** Current gate evaluation returns `GateResult::Passed | Failed | TimedOut | Error` — a boolean. The PRD requires gates to produce arbitrary JSON matching declared schemas. Command gates run shell commands; extracting structured output requires:
- Parsing command stdout as JSON (what if it's not valid JSON?)
- Defining how non-command gates (context-exists, context-matches) produce structured data
- Handling mismatches between actual output and declared schema

**Current architecture:** `evaluate_gates()` in `gate.rs` returns `BTreeMap<String, GateResult>` with no room for per-field output. The `GateResult` enum has no output variant.

**Impact:** HIGH. Every gate type needs redesign. Command gates need JSON parsing logic. Context gates need field extraction rules. This is a breaking change that touches evaluation, compilation, and transition resolution.

**Recommendation:** Phase this separately. Start with defining the gate output data model before touching gate evaluation.

---

### 2. Dot-notation namespacing (`gates.ci_check.status`) conflicts with exact JSON matching (R3)
**Problem:** Transition resolution uses exact JSON equality on the `when` clause:
```rust
let all_match = conditions
    .iter()
    .all(|(field, expected)| evidence.get(field) == Some(expected));
```

The PRD proposes namespaced field references like `gates.ci_check.status: passed`. But the transition resolver expects flat keys. Two options:
- *Unflatten gate output before matching:* Merges `gates.ci_check.{status, coverage}` into flat `gates.ci_check.status`. Requires renaming resolver or creating intermediate flattening layer.
- *Keep gates separate from evidence:* Pass gate output as a second parameter to resolver. Requires signature change throughout advance loop.

**Current architecture:** `resolve_transition(state: &TemplateState, evidence: &BTreeMap, gate_failed: bool)`. Gates aren't visible to the resolver.

**Impact:** MEDIUM-HIGH. The transition resolver is central to routing. Changing it from flat JSON to nested requires:
- Modifying `resolve_transition()` signature
- Updating all transition matching logic
- Rewriting validation in the compiler (which checks `when` clauses reference valid fields)
- Updating CLI/event output to show gate-namespaced data

**Recommendation:** Decide on data structure (flat vs. nested) early, then update resolver as first implementation step.

---

### 3. Compiler validation of reachability (R9): Static analysis vs. runtime simulation
**Problem:** R9 requires:
> "When override defaults are applied to all failing gates, at least one transition resolves (no dead ends on override)"

This is asking: "Can the compiler statically prove that for every possible gate failure scenario, applying override defaults makes at least one transition reachable?"

**Feasibility:** Only if gates are deterministic (pass/fail known at compile time). But the PRD explicitly enables gates that "poll CI, validate schemas, call external services" — inherently non-deterministic.

**Options:**
- *Static analysis assumption:* Assume gates can only fail/pass, not produce variant outcomes. Validate that for each `(gate, failing)` combination, applying that gate's override default lets at least one transition fire. Feasible but limited.
- *Runtime simulation:* Enumerate all possible gate failure combinations, synthesize override defaults, and check if any state is reachable. Expensive but accurate. Requires test case generation.
- *Best-effort warnings:* Warn if a gate has no transitions referencing its fields, but don't guarantee reachability. Pragmatic but incomplete.

**Current architecture:** Compiler only validates that transition targets exist and field types match. No simulation.

**Impact:** MEDIUM. This is a nice-to-have validation, not required for correctness. Missing it means templates can have dead ends (states where override defaults don't resolve any transition).

**Recommendation:** Defer R9 to Phase 2. Start with best-effort warnings. Implement full static analysis only if needed.

---

### 4. Backward compatibility (R10): Boolean gates with schema wrappers
**Problem:** Existing templates have gates without `output_schema`. The PRD requires all gates to declare schemas but allows omission with a warning.

Example conflict: Current code defines `GateResult::Passed | Failed | ...`. New code needs `GateResult::WithOutput(json)`. How does a gate without a schema map to JSON?

**Options:**
- *Implicit schema inference:* Gates without `output_schema` produce `{passed: boolean}`. Template authors must update transitions to use `gates.name.passed` instead of just checking gate pass/fail.
- *Dual mode:* Gates without schema use old boolean model, transitions use `when: {gate_name: "passed"}` (special syntax). Gates with schema use namespace syntax. Mixed syntax in one template.
- *Require schema on all gates:* Break backward compatibility. Update all existing templates.

**Current architecture:** Gates are checked in advance loop; if any fail, the entire state blocks. No schema layer.

**Impact:** MEDIUM. Backward compatibility is explicitly required (R10). Choosing implicit inference or dual mode adds complexity to the compiler (must distinguish old-style from new-style gates) and transition resolver (different matching logic).

**Recommendation:** Implement implicit `{passed: boolean}` schema for legacy gates. Requires minimal changes to evaluation but transition matching needs backward-compat mode.

---

### 5. Selective override (R5a) with selective gate-blocking: Event ordering and state machine consistency
**Problem:** R5a allows:
```bash
koto next --override-rationale "reason" --gate schema_check
# state remains blocked if size_check still fails
# second call: koto next --override-rationale "reason2" --gate size_check
```

This creates a partial override where the state doesn't advance. The engine must:
1. Apply override only to named gates
2. Check if remaining failing gates still block
3. If yes, emit override event but DON'T advance (just re-check gates)
4. On the next call, check gates again and apply selective overrides

**Challenge:** The advance loop expects gates to be evaluated once per invocation. Selective override means:
- Re-evaluate gates after override
- Check if remaining failures still exist
- If yes, emit override event but stop (don't advance or fire transitions)

**Current architecture:** Gates evaluated once at state entry. If any fail, entire state blocks. No per-gate override tracking.

**Impact:** MEDIUM-HIGH. The advance loop logic changes:
```rust
// Current: evaluate gates once, all-or-nothing block
let gate_results = evaluate_gates(&template_state.gates);
if any_failed { return GateBlocked }
else { continue to transitions }

// New: evaluate, apply selective override, re-evaluate, check if still blocked
let gate_results = evaluate_gates(&template_state.gates);
let mut overridden = gate_results.clone();
for gate in &override_gates {
    overridden[gate] = apply_override_default(gate);
}
let still_failing = overridden.values().filter(|r| !passes(r)).count();
if still_failing > 0 {
    emit GateOverrideRecorded event
    return StayInState(with_override_info)
}
else {
    continue to transitions with overridden data
}
```

This adds branching and state complexity.

**Recommendation:** Implement R5a as a separate phase. Start with all-or-nothing override (--override-rationale applies to all failing gates), then add `--gate <name>` filtering.

---

### 6. Gate override audit trail (R6): New event type and cross-epoch query
**Problem:** R6 requires emitting `GateOverrideRecorded` events with full gate output and override context. This is data-intensive:
```json
{
  "state": "validate",
  "gates_overridden": [
    {
      "gate": "schema_check",
      "actual_output": {"valid": false, "errors": 3},
      "override_applied": {"valid": true, "errors": 0}
    }
  ],
  "rationale": "Schema errors are in deprecated fields"
}
```

R8 requires a `derive_overrides` function to collect these across the full session. Current event layer doesn't have this cross-epoch query.

**Current architecture:** Events are appended sequentially. `EventPayload` enum has no `GateOverrideRecorded` variant. No query functions exist.

**Impact:** MEDIUM. This is straightforward but requires:
- New `EventPayload::GateOverrideRecorded` variant
- Serialization/deserialization for the event
- CLI command `koto overrides list` (new subcommand)
- Query logic to filter all events by type

**Recommendation:** Implement after gate output schema and override logic are working. Can be done independently.

---

### 7. Compiler validation of gate/transition contract (R9 + accepts/gate coexistence): Complex static analysis
**Problem:** The compiler must validate:
- Every gate with `output_schema` has a matching `override_default`
- Override defaults match the schema
- Transition `when` clauses reference only valid gate fields
- No unreachable states on override

The challenge is that gates and `accepts` blocks can coexist (R7). A transition `when` clause can reference both:
```yaml
transitions:
  - target: merge
    when:
      gates.lint.status: clean
      decision: approve
```

The compiler must validate that both `gates.lint.status` (from gate output) and `decision` (from accepts) are valid references.

**Current architecture:** Compiler has a `validate_evidence_routing()` method that checks `when` clauses against `accepts` fields. No gate field awareness.

**Impact:** HIGH. Validation logic needs to:
- Build a schema of all available fields (gates + accepts)
- Validate each `when` clause against this combined schema
- Handle namespace collisions (prevent both gate and accept from using same name)
- Detect dead-end transitions (for R9)

**Recommendation:** Implement after gate output types are stable. This is the last validation layer.

---

### 8. New CLI commands and interaction model: `--override-rationale`, `--gate`, `koto overrides list`
**Problem:** The CLI needs:
- `--override-rationale` flag on `koto next`
- `--gate` flag (repeatable) on `koto next`
- `koto overrides list` subcommand
- Validation that `--override-rationale` is non-empty

The interaction is stateful: `--override-rationale` only applies when gates are failing. On a non-blocked state, it's a no-op.

**Current architecture:** `koto next` accepts `--with-data` for evidence submission. No override support.

**Impact:** LOW. CLI changes are localized to the CLI layer. But interaction design must be clear:
- Does `--override-rationale` alone (no `--with-data`) work? Yes per R5.
- Can `--override-rationale` and `--with-data` be combined? Yes per R11 (strict event ordering).
- Is error returned for empty `--override-rationale`? Yes per R12 acceptance criterion.

**Recommendation:** Implement after core engine logic works. CLI is a thin layer.

---

## Phasing Recommendation

### Phase 1 (Foundation): Gate output data model and compiler support
- Add `output_schema` and `override_default` fields to `Gate` type
- Update compiler to parse these from YAML
- Add compiler validation for schema/default match and backward compatibility warnings
- Estimate: 1 week (design + implementation + tests)
- Risk: LOW (data model changes; no behavior changes yet)

### Phase 2 (Evaluation & Routing): Gate output production and transition matching
- Refactor `GateResult` to include output data
- Update gate evaluation to produce structured JSON
- Modify `resolve_transition()` to accept gate output alongside evidence
- Update transition matching logic to handle namespaced gates
- Estimate: 2 weeks (touches core evaluation loop)
- Risk: MEDIUM (central to engine; extensive testing needed)

### Phase 3 (Override & Audit): Override handling and event recording
- Implement all-or-nothing `--override-rationale` (no selective override yet)
- Add `GateOverrideRecorded` event type and emission
- Implement `derive_overrides` query function
- Add `koto overrides list` CLI command
- Estimate: 1 week (straightforward event handling)
- Risk: LOW (isolated to event layer)

### Phase 4 (Advanced Features): Selective override and reachability validation
- Implement `--gate <name>` filtering for selective override
- Add compiler R9 validation (or defer as best-effort warning)
- Handle edge cases (nonexistent gates, duplicate flags, etc.)
- Estimate: 1 week
- Risk: MEDIUM (adds branching to advance loop)

### Phase 5 (Polish): Backward compatibility, migration, docs
- Ensure legacy templates work without modification
- Document migration path for authors who want new schemas
- CLI UX refinement
- Estimate: 1 week
- Risk: LOW

**Total realistic timeline: 6-8 weeks** (not 1-2 week design+implementation cycle)

---

## Summary

The gate-transition contract PRD is architecturally sound but **underestimates scope and technical complexity**. The core idea—making gates produce structured data that feeds into transition routing—is solid. However, it requires:

1. Redesigning gate evaluation to produce JSON output (not just pass/fail)
2. Refactoring the transition resolver to handle namespaced gate data
3. Adding a new event type and audit layer for overrides
4. Extending the compiler with static analysis for reachability
5. Implementing new CLI interaction model (override flags, selective gating)

Each change touches the core advance loop, evaluation layer, or compiler — areas that must be correct. **The feature cannot be implemented in a single 1-2 week cycle without accumulating significant technical debt.**

**Recommendation:** Approve Phase 1 (data model) immediately. Phase 2 (evaluation) should start only after Phase 1 is code-reviewed and tests pass. Phases 3-4 can run in parallel. Plan for 6-8 weeks total, phased by architectural layer.
