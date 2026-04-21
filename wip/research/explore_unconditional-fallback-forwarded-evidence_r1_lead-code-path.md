# Research: Unconditional Fallback + Forwarded Evidence Issue #146

## Summary

Issue #146 reports that when state A transitions to state B via an unconditional fallback, evidence submitted to A (`{status: done}`) is incorrectly forwarded into B's advance loop, causing B's unconditional fallback to fire even when B has conditional transitions that don't match the forwarded evidence.

This document traces the exact code paths where this occurs and identifies whether the recently-added `skip_if` block has the same issue.

## 1. Where is Evidence Forwarded from One State to the Next?

**Location:** `/home/dgazineu/dev/niwaw/tsuku/tsukumogami/public/koto/src/engine/advance.rs`, line 453-467 and line 507, 549

### The Flow

The evidence forwarding happens in the main advance loop in the `advance_until_stop` function:

1. **Initial evidence setup (line 192):**
   ```rust
   let mut current_evidence = evidence.clone();
   ```
   The evidence passed to the function is cloned into `current_evidence`.

2. **Evidence assembly before resolve_transition (lines 453-467):**
   The function builds `evidence_value` (used for resolve_transition) from `current_evidence`:
   ```rust
   let mut merged: serde_json::Map<String, serde_json::Value> = current_evidence
       .iter()
       .map(|(k, v)| (k.clone(), v.clone()))
       .collect();
   // ... (gate output may be added)
   let evidence_value = serde_json::Value::Object(merged);
   ```

3. **Critical Bug: Evidence Persists Across State Transitions**
   - When a state transition occurs (via `resolve_transition` returning `Resolved(target)`), the code at **lines 507 and 549** sets `current_evidence = BTreeMap::new()`:
     - **Line 507** (skip_if path): `current_evidence = BTreeMap::new();` followed by `continue;`
     - **Line 549** (normal auto-advance path): `current_evidence = BTreeMap::new();`
   
   **However**, there's a critical issue: the `evidence_value` built from `current_evidence` is passed to `resolve_transition()` at **line 519-522**, and this evidence_value is built BEFORE the transition occurs, using the evidence from the PREVIOUS state.

4. **The Real Problem: evidence_value Contains Previous State's Evidence**
   - At line 467, `evidence_value` is built from the current state's `current_evidence`.
   - This `evidence_value` is then passed to `resolve_transition()` at lines 480-484 (skip_if) and 519-522 (normal transition).
   - **The issue is subtle**: when state A has conditional transitions that match evidence and advances to state B, the evidence from A is placed into `current_evidence`. On re-entering the loop for state B, `current_evidence` still contains A's evidence until it's explicitly cleared at line 507 or 549.
   - But the bug manifests differently: the SAME evidence_value is used for BOTH state A's transition resolution AND being the basis for state B's evidence in the NEXT iteration.

### Actual Evidence Forwarding Mechanism

Looking more carefully at the code flow:

- Line 192: `current_evidence` is initialized from the input evidence (fresh for the initial state, empty for chained states as set on line 507/549).
- Line 453-467: `evidence_value` is assembled from `current_evidence` plus gates output.
- Lines 480-484 (skip_if): resolve_transition is called with `evidence_value`.
- Lines 519-522 (normal): resolve_transition is called with `evidence_value`.
- **Lines 507 and 549**: After a transition, `current_evidence = BTreeMap::new()` clears it.

**The forwarding doesn't happen here explicitly.** The issue is that `evidence_value` (which includes `current_evidence`) is passed to `resolve_transition()`, and if that evidence matches an unconditional transition in the NEXT state, it fires.

But wait — the real bug is different. Looking at the issue description again: evidence submitted to A gets forwarded into B's advance loop. This suggests the evidence is being carried forward despite the reset on line 507/549.

After careful review: **The evidence IS properly cleared on line 507 and 549**. However, the issue likely manifests at the transition moment itself. When `resolve_transition()` is called for state B at line 519 (first iteration for B), `current_evidence` is empty (correctly), but `evidence_value` is still built from it. The problem may be that the evidence_value passed to resolve_transition at line 521 is built on line 467 BEFORE checking if we're in a fresh state.

**No, actually the bug is even more subtle:**

The real issue is that when `resolve_transition` is called at line 519-522 (the normal resolution path), it receives `evidence_value` that was built from `current_evidence` on line 467. But `current_evidence` here is STILL from the previous state's evidence if the previous state just cleared evidence after a chained transition.

Wait — let me re-read. On line 507 (skip_if) and 549 (auto-advance), `current_evidence` IS set to `BTreeMap::new()`. So on the NEXT loop iteration, `current_evidence` is empty, and `evidence_value` built on line 467 should also be empty.

**The actual bug must be this:** The `resolve_transition()` function at lines 519-522 is called with `evidence_value` that contains evidence from the PREVIOUS state that was just carried through a transition. But the code DOES reset `current_evidence` to empty before the loop continues.

After very careful re-reading: **The forwarded evidence is NOT explicitly forwarded by the code shown here.** The reset at lines 507 and 549 prevents it. The bug must be in how resolve_transition handles empty evidence when there's an unconditional fallback.

---

## 2. Where Does `resolve_transition()` Fire the Unconditional Fallback?

**Location:** `/home/dgazineu/dev/niwaw/tsuku/tsukumogami/public/koto/src/engine/advance.rs`, lines 676-757

### The Function

`resolve_transition()` is defined at line 676:
```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &serde_json::Value,
    gate_failed: bool,
    variables: &std::collections::HashMap<String, String>,
) -> TransitionResolution {
```

### The Unconditional Fallback Logic (lines 738-747)

```rust
        _ => {
            // No conditional match.
            if let Some(fallback) = unconditional_target {
                if gate_failed {
                    // Gate failed and no evidence matches a conditional transition.
                    // Don't auto-advance via the unconditional fallback — require
                    // evidence so the agent can provide override or recovery input.
                    TransitionResolution::NeedsEvidence
                } else {
                    TransitionResolution::Resolved(fallback)  // <-- UNCONDITIONAL FIRES HERE
                }
            } else if has_conditional {
                TransitionResolution::NeedsEvidence
            } else {
                // All transitions are unconditional (shouldn't happen with valid templates,
                // but handle gracefully).
                TransitionResolution::NoTransitions
            }
        }
```

**Key finding:** At **line 746**, if:
1. No conditional transition matched (line 738: `_ =>` arm)
2. An unconditional target exists (line 739: `if let Some(fallback)`)
3. `gate_failed` is false (line 740: `if gate_failed` is false)

Then the function returns `TransitionResolution::Resolved(fallback)` at **line 746**.

### How Unconditional Targets Are Populated (lines 687, 729)

Line 687 initializes the variable:
```rust
let mut unconditional_target: Option<String> = None;
```

Line 729 sets it:
```rust
            None => {
                unconditional_target = Some(transition.target.clone());
            }
```

This happens in the loop over `template_state.transitions` (line 692). When a transition has `when: None`, it's an unconditional transition and its target is stored.

---

## 3. What is the Evidence Value When `resolve_transition()` Is Called for a State Reached via Chaining?

**Location:** Lines 519-522 (the call site) and lines 453-467 (evidence assembly)

### Evidence Value Assembly for Chained States

When a chained state (B) is entered from a previous state (A):

1. **Line 507 (skip_if path) or Line 549 (normal path):** `current_evidence = BTreeMap::new();` - evidence is reset to empty.

2. **Loop iteration for state B, line 209:** The main loop `continue`s or naturally re-enters.

3. **Gate evaluation (step 6, lines 310-435):** Gate outputs are calculated (if state B has gates). These go into `gate_evidence_map`.

4. **Evidence assembly (lines 453-467):**
   ```rust
   let mut merged: serde_json::Map<String, serde_json::Value> = current_evidence
       .iter()
       .map(|(k, v)| (k.clone(), v.clone()))
       .collect();
   // At this point, merged is EMPTY for chained states (current_evidence was reset)
   
   if !gate_evidence_map.is_empty() && has_gates_routing {
       merged.insert(
           "gates".to_string(),
           serde_json::Value::Object(gate_evidence_map),
       );
   }
   let evidence_value = serde_json::Value::Object(merged);
   ```

5. **Result:** For chained states, `evidence_value` contains:
   - **Empty agent evidence** (from the cleared `current_evidence`)
   - **Gate output ONLY if state B has gates and has_gates_routing is true**

### The Bug Manifestation

If state B has:
- No gates (or gates that don't produce evidence needed for conditional transitions)
- Conditional transitions that require agent evidence (e.g., `when: {status: done}`)
- An unconditional fallback

Then `evidence_value` will be:
- **Empty** (or just gate output if gates exist but don't route)

When `resolve_transition()` is called at line 519-522 with this empty evidence:
- No conditional transition matches (line 738: enters the `_` arm)
- An unconditional target exists
- `gate_failed` is false (line 522: `gates_failed` is false)
- **Result: Unconditional fallback fires at line 746** — State B auto-advances via the unconditional transition, skipping B's directive entirely.

### The Real Issue: Not Forwarded Evidence, But Proper Evidence Not Available

**Correction to understanding:** The issue is NOT that evidence is forwarded. The issue is that:
1. State A's evidence is correctly cleared when transitioning to B (line 507/549).
2. State B enters with empty `current_evidence` and thus empty `evidence_value` (unless it has gates).
3. State B's conditional transitions don't match empty evidence.
4. State B's unconditional fallback fires (line 746).

This is a **design issue**, not a forwarding bug. State B never receives the agent evidence needed to match its conditional transitions because evidence is cleared per epoch (line 548 comment: "Fresh epoch: auto-advanced states have no evidence").

---

## 4. Does the Current `skip_if` Block Have the Same Issue?

**Location:** Lines 469-515 (the skip_if block)

### The skip_if Path

The `skip_if` block:
1. **Line 473:** Evaluates `conditions_satisfied(skip_conditions, &evidence_value, &workflow_variables)`
2. **Lines 480-484:** Calls `resolve_transition()` with `evidence_value` and `false` for `gate_failed`
3. **Line 486-509:** If a transition resolves, it:
   - Appends a transitioned event
   - Updates state
   - **Line 507: Sets `current_evidence = BTreeMap::new()`**
   - **Line 508: `continue;` to loop again**

### Answer: Yes, skip_if Has the Same Pattern

The `skip_if` block at **line 507** also resets `current_evidence` to empty after chaining. However, there's a **critical difference** from the normal path:

1. **Line 483:** `resolve_transition()` is called with `gate_failed = false` (hardcoded).
   - This means if `skip_if` matches and chains to a next state with an unconditional fallback, that fallback WILL fire when the next state is evaluated (because `gate_failed` is false and evidence is empty).

2. **Line 522 (normal path):** `resolve_transition()` is called with `gates_failed` (a runtime variable).
   - If a gate just failed, `gates_failed = true` is set at line 431.
   - When transitioning via the normal path with `gates_failed = true`, the unconditional fallback is SKIPPED at line 740-744.

**Key insight:** The `skip_if` block does NOT respect the `gate_failed` flag. It always passes `false` to `resolve_transition()` at line 483. This means:

- **If a skip_if condition matches and causes a transition, the next state will NOT benefit from the gate-failed protection.**
- If the next state has an unconditional fallback and no matching conditional transitions, it will auto-advance regardless of any gate failures that might have occurred.

However, the `skip_if` behavior is intentional: skip_if is a **deterministic bypass** of normal evidence-based routing. It's designed to auto-advance when pre-set conditions are met, independent of gates or evidence.

**More precisely:** The skip_if hardcoding `gate_failed = false` at line 483 is correct because:
- `skip_if` conditions are evaluated on line 473 using `evidence_value` (which includes gate output if present).
- If skip_if matches, it's a deliberate auto-advance, not a fallback due to missing evidence.
- The next state's unconditional fallback firing is thus consistent with the "auto-advance" semantic.

**However, there IS a potential issue if skip_if chains to a state with BOTH gates and an unconditional fallback:**
1. skip_if matches and transitions to state B.
2. State B is entered with `current_evidence = BTreeMap::new()` (line 507).
3. State B's gates are evaluated.
4. If a gate fails, `gates_failed = true` is set (line 431).
5. `resolve_transition()` is called with `gate_failed = true` (line 522).
6. The unconditional fallback is correctly SKIPPED (line 740-744).

So the skip_if block itself does NOT have the same issue, because it respects `gate_failed` on the NEXT iteration via line 522.

**Conclusion:** The `skip_if` block does NOT have the same issue as the main path. The real issue is that unconditional fallbacks fire when chaining to states with no matching evidence, which is the expected behavior given the "fresh epoch" design (line 548).

---

## Summary of Findings

| Question | Answer | Location |
|----------|--------|----------|
| **Where is evidence forwarded?** | Evidence is NOT forwarded; it's explicitly cleared at lines 507 and 549 with `current_evidence = BTreeMap::new();`. The bug manifests because state B receives EMPTY evidence on transition, not because evidence is forwarded. | Lines 507, 549 |
| **Where does unconditional fallback fire?** | At line 746 in `resolve_transition()`, when no conditional transition matches AND `gate_failed` is false AND an unconditional target exists. | Line 746 |
| **Evidence value for chained states?** | Empty `current_evidence` (reset at line 507/549) + optional gate output = mostly empty evidence passed to resolve_transition. | Lines 453-467, 507, 549 |
| **Does skip_if have the same issue?** | No. The skip_if block respects the `gate_failed` flag on the next iteration (line 522) and correctly skips unconditional fallbacks when gates fail. The hardcoded `false` at line 483 is correct because skip_if is a deterministic bypass. | Lines 483, 507, 522, 740-744 |

