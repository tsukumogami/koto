# Research: Gates/Accepts/When Interaction Model for Template Format v2

## Investigation Summary

This research explores the precise interaction model between koto-verifiable gates and agent-submitted evidence conditions (`accepts`/`when`) in template format v2. The strategic design (DESIGN-unified-koto-next.md) defines the high-level architecture but leaves several ambiguities unresolved. This document maps the exact interaction semantics, identifies design decisions needed, and surfaces one significant architectural tension.

## Current State

### v1 Template Format (Current)
From `src/template/types.rs` and `docs/designs/current/DESIGN-koto-template-format.md`:

```rust
pub struct TemplateState {
    pub directive: String,
    pub transitions: Vec<String>,      // flat list of target states
    pub terminal: bool,
    pub gates: BTreeMap<String, Gate>, // koto-verifiable conditions
}

pub struct Gate {
    pub gate_type: String,  // "field_not_empty", "field_equals", "command"
    pub field: String,
    pub value: String,
    pub command: String,
    pub timeout: u32,
}
```

Gate types:
- `field_not_empty`: checks if field exists and is non-empty in evidence map
- `field_equals`: checks if field equals specific value in evidence map
- `command`: executes shell command, checks exit code (default 30s timeout)

Gate semantics in v1:
- Gates are evaluated between validation and commit during state transition
- All gates on a state must pass (AND logic) before transition is allowed
- Gates block transition if any fail
- Exit codes: `0` success, `1` transient (gate_blocked), `3` config error

### v2 Strategic Design
From `DESIGN-unified-koto-next.md`:

The new format adds per-transition evidence schema and conditions:

```yaml
states:
  analyze_results:
    # Evidence field schema (generates `expects` in koto next output)
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
      rationale:
        type: string
        required: true

    # Per-transition routing conditions
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: escalate_review
        when:
          decision: escalate

    # Existing koto-verifiable gates (unchanged)
    gates:
      tests_passed:
        type: command
        command: ./check-ci.sh
```

Key quote from strategic design:

> **States with no `accepts` block and no `when` conditions are auto-advanced through when their `gates` are satisfied.**

## The Interaction Model: Five Cases

### Case 1: State with only gates (no accepts, no when)
**Execution:** Auto-advances when gates pass.

**Flow:**
1. Evaluate gates (command, field_not_empty, field_equals)
2. If all pass: append `transitioned` event → fsync → loop
3. If any fail: stop (agent must provide evidence to unlock gates)

**Decision needed:** How does evidence provision interact here? Can an agent submit evidence for a gate-only state?

**Rationale:** This preserves v1 semantics. A state like `wait_for_ci` (only command gates, no agent input) auto-advances when CI passes.

---

### Case 2: State with gates AND accepts/when
**Execution:** Hybrid model. Gates are prerequisites; `when` conditions route to transitions.

**Flow:**
1. Evaluate gates (koto-verifiable)
2. If any gate fails: stop
3. If all gates pass AND accepts block exists:
   - Evaluate `when` conditions against current evidence
   - If evidence matches exactly one `when`: append `transitioned` → loop
   - If evidence doesn't match any `when`: stop (expects evidence submission)
4. If all gates pass AND no accepts block: auto-advance (Case 1)

**Example:** A state with both:
```yaml
states:
  verify_impl:
    gates:
      tests_pass:
        type: command
        command: npm test
    accepts:
      approval:
        type: enum
        values: [approved, rejected]
        required: true
    transitions:
      - target: deploy
        when:
          approval: approved
      - target: rework
        when:
          approval: rejected
```

**Interpretation:** Tests must pass (gate) AND agent must provide approval evidence (accepts/when). If tests fail, stop. If tests pass but no evidence, stop and expect submission.

---

### Case 3: State with only accepts/when (no gates)
**Execution:** Pure evidence-driven routing. No koto-verifiable conditions.

**Flow:**
1. Check if accepts block exists
2. Evaluate `when` conditions against current evidence
3. If match found: auto-advance to matched target
4. If no match: stop (expects evidence submission)

**No gates involved.** Agent provides evidence, and the system automatically routes to the target transition.

---

### Case 4: State with field gates that shadow accepts fields
**Critical design question:** Can a state have both `accepts` field schema and `field_not_empty`/`field_equals` gates for the same field?

Example (problematic):
```yaml
accepts:
  decision:
    type: enum
    values: [proceed, escalate]
accepts:
  rationale:
    type: string
    required: true
gates:
  decision_not_empty:
    type: field_not_empty
    field: decision
```

**Three possible interpretations:**

A. **Gates evaluate first; when conditions are fallback** (currently implied by DESIGN-unified-koto-next)
   - Gate `decision_not_empty` checks if evidence["decision"] is non-empty
   - If gate fails: stop
   - If gate passes: evaluate `when` conditions
   - Problem: redundant — the `when` checks will fail anyway if gate fails

B. **Semantic unification**: Field gates (`field_not_empty`, `field_equals`) are *compiled into* `when` conditions
   - `field_not_empty: decision` becomes internal `when: {decision: <non-empty>}`
   - `field_equals: decision=proceed` becomes internal `when: {decision: proceed}`
   - All are routed through the same `when` evaluation engine
   - Benefit: single evaluation framework, no redundancy
   - Risk: complex to explain to template authors

C. **Rejection**: Disallow field gates on states with `accepts` blocks
   - Compiler error: "state X has both gates and accepts; use only accepts for evidence-dependent branching"
   - Forces template authors to choose: gates-only (auto-advance) or accepts/when (evidence-driven)

---

### Case 5: Mutual Exclusivity Validation
The strategic design says:

> The template compiler validates that per-transition `when` conditions on the same state are mutually exclusive (same field, disjoint values) and rejects templates that are non-deterministic. ... the compiler can verify mutual exclusivity only for single-field conditions.

**Single-field validation (deterministic):**
```yaml
transitions:
  - target: deploy
    when:
      decision: proceed
  - target: escalate
    when:
      decision: escalate
```
Compiler can verify: `decision` field has disjoint values → deterministic routing.

**Multi-field validation (non-deterministic, author responsible):**
```yaml
transitions:
  - target: fast_track
    when:
      complexity: low
      tests_passing: true
  - target: review_gate
    when:
      complexity: high
      approval: manual
```
Compiler cannot verify that both conditions can't be true simultaneously (different fields, different semantics).

**Missing from strategic design:** What error does the compiler emit when it detects non-exclusive single-field conditions?

Example:
```yaml
transitions:
  - target: deploy
    when:
      decision: proceed
  - target: escalate
    when:
      decision: proceed  # ← DUPLICATE, non-deterministic
```

---

## Evidence Submission Timing

### When Evidence Becomes Active

**From DESIGN-unified-koto-next.md (data flow section):**

```
├─ If --with-data:
│   ├─ Validate payload against current state's `accepts` schema
│   ├─ Append evidence_submitted event → fsync
│   └─ Continue to advancement evaluation
```

**Interpretation:** Evidence is submitted, persisted, then immediately re-evaluated. If the new evidence matches a `when` condition, the state advances before returning.

**Question:** Can an agent submit evidence for a *past* state?

From the CLI output contract section:
> **Stale submission handling**: no `state` assertion field is required in `--with-data` payloads. If the workflow has advanced to a new state between the agent's last `koto next` call and a subsequent `--with-data` submission, koto validates the payload against the *current* state.

So: evidence is always validated against the *current* state. An old evidence key submitted to a new state is rejected if it's not in the new state's `accepts` schema.

---

## Gate Evaluation Order

**From data flow diagram in DESIGN-unified-koto-next.md:**

```
├─ Advancement loop:
│   ├─ evaluate gates: if any fail → stop (gate_blocked)
│   ├─ if accepts block: evaluate which transition's `when` conditions match current evidence
│   │   ├─ if none match: stop (expects evidence submission)
│   │   └─ if match: append transitioned event → fsync → continue loop
│   └─ if no accepts and gates pass: append transitioned event → fsync → continue
```

**Order is explicit:**
1. Evaluate gates first
2. If gates fail: stop (gate_blocked)
3. If gates pass and accepts block exists: evaluate `when` conditions
4. If gates pass and no accepts block: auto-advance

**Consequence:** A state with both gates and accepts behaves as a gate-then-branch model, not a pure OR.

---

## Do `field_not_empty` and `field_equals` Gates Make Sense in v2?

This is the core tension.

**Argument for keeping them:**
- Some states need *only* gate-based advancement (Case 1). No accepts block needed.
- Field gates are simpler than teaching template authors about `accepts` schema + `when` conditions.
- Backward compatibility with v1 concept (even though v2 is a breaking change).

**Argument for removing them in favor of pure `accepts`/`when`:**
- `field_not_empty` on a field is semantically equivalent to "accept this field as required" → express via `accepts` schema
- `field_equals` on a field is semantically equivalent to "accept this field and route based on its value" → express via `accepts` schema + `when`
- Having both creates the Case 4 ambiguity: do they coexist? Override? Are they mutually exclusive?
- Simpler mental model for template authors: two types of conditions, not three

**Hybrid position (recommended):**
- Keep all three gate types in v2 for backward compatibility with simple auto-advance patterns
- Document that states should use *either* gates (auto-advance) *or* accepts/when (evidence-driven branching), not both
- Compiler should emit a warning if both are present on the same state (not an error, for migration flexibility)

---

## Integration Field Interaction

The `integration` field (a string tag) is orthogonal to gates/accepts/when:

```yaml
states:
  deep_analysis:
    accepts:
      interpretation:
        type: string
        required: true
    transitions:
      - target: review
        when:
          interpretation: <non-empty>
    integration: delegate_analysis
```

**Flow:**
1. Agent calls `koto next`
2. Koto evaluates gates (none in this example)
3. No `when` condition is satisfied yet (no evidence submitted)
4. Integration is configured → invoke runner, append `integration_invoked` event
5. Return output with `expects: interpretation` and `integration: {name: delegate_analysis, output: ...}`

The integration is invoked *after* gate evaluation but *before* waiting for evidence submission. This is independent of the gates/accepts/when interaction.

---

## Ambiguities in Strategic Design

1. **Field gates on accepts states**: Can a state have both `field_not_empty: decision` gate and `accepts: decision` schema? Three interpretations possible (A, B, C above).

2. **Non-exclusive when conditions**: What error message when compiler detects duplicate `when` values for same field?

3. **gate_failed vs expects**: How does an agent distinguish in the CLI output between:
   - A state that stopped because gates failed (agent needs to fix environment, not submit evidence)
   - A state that stopped because no evidence matches any `when` (agent needs to submit evidence)

   From the CLI output contract section, there's a `blocking_conditions` field for gate-blocked states. But the schema differs from `expects` output. This is issue #48's responsibility, but the template model needs to support surfacing this distinction.

4. **Evidence for gate-only states**: The strategic design doesn't explicitly say whether agents can submit evidence to a gate-only state. The model suggests no (gates evaluate deterministically), but this should be explicit.

5. **Auto-advance with integration**: If a state has only gates and an integration (no accepts/when), does integration execution count as a stopping condition? The strategic design says integration invocation appends an `integration_invoked` event and stops. So yes — even auto-advance states stop if integration is configured.

---

## Proposed Resolution: The Clean Architectural Choice

To resolve ambiguities, the tactical design (issue #47) should adopt this model:

### States Have One of Three Control Models

1. **Gate-Gated (v1-style)**: Gates only, no `accepts`, no `when`
   - Auto-advances when gates pass
   - If integration configured: invoke, append event, stop
   - Evidence not accepted (optional: warn if provided)

2. **Evidence-Routed**: `accepts` + `when` conditions (no gates, or gates are compilation hints)
   - No auto-advance; requires evidence matching a `when` condition
   - Stops when waiting for evidence
   - Gates (if present) are prerequisites that must pass before evidence is evaluated

3. **Integration-Driven**: `integration` field is orthogonal
   - Present on either model
   - Invoked after gates pass but before evidence evaluation
   - Appends `integration_invoked` event and stops

### Design Decision for v2: Resolve the Field Gate Ambiguity

**Recommended:** Option C (rejection with clear error)

Compiler rule: **If a state declares `accepts` block, field gates (`field_not_empty`, `field_equals`) are forbidden. Use the `accepts` schema to declare required fields.**

Rationale:
- Eliminates Case 4 ambiguity entirely
- Forces clearer template intent: gates-only states (auto-advance) vs. evidence-driven states (accepts/when)
- Easier to explain to template authors: "Field gates and accepts are two ways to declare evidence requirements; choose one model for each state"
- Command gates remain allowed on accepts states (they're about external conditions, not evidence)

---

## Summary: Precise Interaction Rules

1. **Gate Evaluation Order**: Gates (all types) evaluated first. If any fails, stop.

2. **Evidence-Driven Branching**: After gates pass, if `accepts` block exists:
   - Evaluate `when` conditions against current evidence
   - If match: auto-advance to matched transition
   - If no match: stop (expects evidence submission)

3. **Auto-Advance**: If no `accepts` block and gates pass, auto-advance through single transition (or stop if multiple transitions exist, pending tactical design decision).

4. **Field Gates on Accepts States**: Compiler rejects with clear error.

5. **Mutual Exclusivity**: Compiler validates single-field `when` conditions for non-overlapping values. Multi-field conditions are author-responsibility.

6. **Integration Orthogonality**: `integration` field independent of gate/accepts/when. Invoked after gates pass, before evidence evaluation.

7. **Evidence Epoch**: Evidence is scoped to the most recent arrival at current state (per DESIGN-event-log-format.md). Evidence from prior visits is archived in log but not active.

---

## Open Questions for Tactical Design (#47)

1. What's the exact error message when compiler detects non-exclusive `when` conditions?
2. Should command gates be allowed on accepts states? (Recommend: yes, they're environmental checks)
3. How does `koto next` distinguish gate_blocked vs. expects_evidence in output? (Recommend: separate fields in JSON)
4. Should states with multiple transitions (no `when` conditions) auto-advance to any one, error, or require explicit selection? (Issue #49 territory)
5. What's the compiled JSON schema for v2? (Recommend: `transitions` becomes `[{target, when: {...}}]` with `when` optional)

