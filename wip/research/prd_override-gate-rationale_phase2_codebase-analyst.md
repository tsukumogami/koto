# Phase 2 Research: Codebase Analyst

## Lead 1: Override Scenarios

### Findings

The codebase reveals **three distinct implicit override mechanisms** that bypass or skip gate enforcement:

#### 1. Evidence-Submitted Override on Gate-Failed States (WITH accepts block)
**Location:** `src/engine/advance.rs:300-315`, `src/cli/mod.rs:1411-1498`

When a state has BOTH gates and an `accepts` block:
- If ANY gate fails, the engine falls through to transition resolution instead of returning `GateBlocked` (line 305)
- The `gate_failed` flag (line 312) is set to `true` and passed to `resolve_transition()` (line 319)
- In `resolve_transition()` (lines 430-434), when `gate_failed=true`, the unconditional fallback transition is **blocked** from firing
- This forces the agent to submit evidence via `--with-data` to match a conditional transition
- **Key insight:** Submitting ANY evidence (even irrelevant fields) to a gate-failed state with accepts allows progression via conditional transition match
- **Evidence is implicit:** The evidence_submitted event (lines 1485-1488) is appended, but no explicit "override" marker exists — the gate bypass is inferred from evidence presence + prior gate failure

**Test:** `test/functional/features/gate-with-evidence-fallback.feature` lines 14-36 demonstrates: gate fails → evidence required → evidence submitted → transitions to next state (regardless of evidence content matching gate logic)

#### 2. Action Skipping via Evidence Presence (has_evidence check)
**Location:** `src/engine/advance.rs:264`, `src/cli/mod.rs:1592-1599`

When a state has a `default_action`:
- The action closure checks `has_evidence = !current_evidence.is_empty()` (advance.rs:264)
- If evidence exists, the action is **skipped without execution** (mod.rs:1598): `return ActionResult::Skipped`
- This applies to ALL states with default_action, regardless of gates or accepts
- **Override mechanism:** Evidence presence alone, regardless of its validity or relevance, causes action skipping
- **No explicit tracking:** No event signals the action was skipped due to override; only `DefaultActionExecuted` event exists for actually-run actions

**Code path:** When an agent submits evidence to a state, the action execution phase (advance.rs:262-290) immediately returns `Skipped`, allowing the engine to continue to gate evaluation and transition resolution without running the action.

#### 3. Directed Transition via --to Flag (BYPASSES ALL GATES)
**Location:** `src/cli/mod.rs:1305-1394`

The `--to` flag allows direct state transition without evaluation:
- Validates target is a valid transition from current state (lines 1324-1341)
- Appends `DirectedTransition` event (lines 1355-1367)
- **Critically:** Gate evaluation is explicitly SKIPPED (line 1371): `let gate_results = std::collections::BTreeMap::new();`
- Dispatches on the target state with empty gate_results, meaning no blocking conditions are reported
- **Complete bypass:** No gates are evaluated, no condition checking — pure command-driven transition
- **Mutually exclusive:** `--to` and `--with-data` are explicitly mutually exclusive (line 1131)

**Current behavior:** `--to` is single-shot: it appends the directed_transition event, dispatches, and exits. No advancement loop is run on the target state (line 1386: `std::process::exit(0)`).

#### 4. (IMPLICIT) Unconditional Fallback Transition (without gate failure or evidence)
**Location:** `src/engine/advance.rs:429-437`

In states with both conditional and unconditional transitions:
- If no conditional matches AND `gate_failed=false`, the unconditional fallback fires automatically (line 436)
- **Not strictly an override** (gates aren't bypassed), but represents auto-advancement without evidence
- **Becomes relevant in override scenarios:** When a state has gates + accepts + unconditional fallback, providing evidence is NOT required to bypass the gate IF that evidence matches a conditional transition

### Implications for Requirements

1. **Three distinct event types needed:**
   - `gate_override` (explicit): via evidence submission when gates fail on states with accepts
   - `action_skip_override` (explicit): via evidence submission that causes action skipping
   - `directed_override` (explicit): via `--to` flag usage

2. **Rationale tracking scope:**
   - Evidence-submitted and directed-transition events ALREADY exist; need to add rationale field to track "why override"
   - Need to distinguish between "override evidence" (explicit intent to bypass) vs. "normal evidence" (happens to resolve a transition)
   - **Key challenge:** Current evidence_submitted has no intent field; how do we know if evidence was submitted to bypass a gate vs. legitimately resolve a transition?

3. **Gate evaluation timing:**
   - Gates are evaluated ONCE per state (advance.rs:296), but if gates fail and state has accepts, execution continues
   - This means gate result is available to the override event but currently not tracked as "override reason"

4. **--to flag implications:**
   - Currently single-shot (no advancement loop on target state)
   - No gate evaluation on the directed transition itself
   - If PRD requires tracking gate bypass via --to, need to record which gates WOULD have failed on the target state

5. **Action skipping is invisible:**
   - Currently no event records action skipping
   - `DefaultActionExecuted` event only appears when action actually runs
   - For override tracking, need an `ActionSkipped` event or similar marker

### Open Questions

1. **Evidence Intent Disambiguation:**
   - How do we distinguish "override evidence" (submitted to bypass gate) from "resolving evidence" (submitted to match conditional)?
   - Should all evidence on gate-failed states be treated as overrides?
   - Should there be a separate API call or flag to explicitly mark evidence as "override rationale"?

2. **--to Flag Semantics:**
   - Should `--to` require rationale for audit trail?
   - Should `--to` evaluate gates on the target state and report bypass?
   - Should `--to` trigger advancement loop on the target state, or remain single-shot?

3. **Action Skipping Attribution:**
   - Is action skipping an "override" worthy of explicit auditing?
   - If yes, should it require explicit rationale, or is evidence presence sufficient justification?

4. **Multiple Gates Edge Case:**
   - If a state has 3 gates and 2 fail, is submitting evidence an override of both failures or only those that matter for transition resolution?
   - Should the override event list which specific gates failed?

---

## Lead 2: Edge Cases

### Findings

#### Edge Case 1: Multiple Gates on Single State (Partial Failure)
**Location:** `src/engine/advance.rs:295-315`, `src/gate.rs:30-61`

**Scenario:** State has 3 gates (A, B, C) and an accepts block; gates A and C fail, gate B passes.

Current behavior:
- `evaluate_gates()` evaluates ALL gates without short-circuit (gate.rs line 32 comment, evaluate_gates function lines 42-61)
- Returns map of all gate results (gate.rs:41: `BTreeMap<String, GateResult>`)
- If ANY gate failed (advance.rs:298: `any_failed`), the state is treated as "gates failed"
- Agent must provide evidence to proceed; evidence satisfies the accepts block, not the gates
- **No granular override:** Can't override specific gates; providing evidence overrides ALL failed gates simultaneously

**Implication:** Override tracking must handle partial gate failure. If we record "override", should it list:
- All failed gates, or
- Only gates whose failure was "overridden" by the transition?

#### Edge Case 2: State with Gates but NO Accepts Block
**Location:** `src/engine/advance.rs:305-310`

**Scenario:** State has gates but no accepts block.

Current behavior:
- Gates fail → immediately return `StopReason::GateBlocked(gate_results)` (advance.rs:309)
- Agent receives response: `NextResponse::GateBlocked` with blocking_conditions
- **No override path exists:** Agent cannot submit evidence to proceed
- Agent's only option: use `--to` to skip the state, or rewind

**Implication:** This is NOT an override scenario by current design. However, `--to` bypass still applies. If PRD tracks gate bypasses, need to ensure states with gates-only (no accepts) are NOT falsely flagged as supporting evidence-based overrides.

#### Edge Case 3: Accepts Block Without Gates
**Location:** `src/cli/next.rs:91-100`

**Scenario:** State has accepts block but NO gates.

Current behavior:
- No gates to evaluate (advance.rs:295)
- Transitions resolve normally; if no conditional matches, returns `EvidenceRequired` (advance.rs:345-353)
- Agent provides evidence to match a conditional transition
- **Not an override:** Evidence is required for normal transition resolution, not to bypass gates

**Implication:** Override tracking must distinguish:
- Evidence required due to gate failure (override scenario)
- Evidence required due to conditional transition (normal scenario)

The `StopReason::EvidenceRequired` includes `failed_gates: Option<BTreeMap<String, GateResult>>` (advance.rs:57) to distinguish these cases.

#### Edge Case 4: Evidence Submitted When Gates PASS
**Location:** `src/engine/advance.rs:295-315`, `src/cli/mod.rs:1541-1679`

**Scenario:** Agent submits evidence while at a state where gates actually pass.

Current behavior:
- Gates evaluate (advance.rs:296)
- `any_failed` is false (advance.rs:297-299)
- State does NOT set `gates_failed=true`; falls through to transition resolution with `gates_failed=false` (line 319)
- Evidence is merged and used for transition matching (advance.rs:184, 343)
- Unconditional fallback CAN fire even if evidence is present (line 436)
- **Not an override:** Gates passed legitimately; evidence is for transition resolution only

**Implication:** Override events must only be recorded when gates actually FAIL. Submitting evidence while gates pass should not generate override records.

#### Edge Case 5: Evidence Submitted on Non-Gate-Failed State With Accepts
**Location:** `src/engine/advance.rs:262-290`, `src/cli/mod.rs:1592-1599`

**Scenario:** State has accepts block and action, gates pass (or no gates), agent submits evidence.

Current behavior:
- No gates failed, so `gates_failed=false`
- Action execution checks `has_evidence = !current_evidence.is_empty()` (advance.rs:264)
- If evidence was submitted, action is skipped (mod.rs:1598)
- Evidence is used for transition resolution (normal, not override)

**Subtle implication:** Evidence submission on a passing-gate state still causes action skipping. This is **implicit action override** without gate bypass. Current code treats action skipping as unconditional on evidence presence, regardless of gate state.

#### Edge Case 6: Rewind to Previously Overridden State
**Location:** `src/cli/mod.rs:~1700+` (rewind implementation), `src/engine/persistence.rs:236-330` (derive_evidence)

**Scenario:** Agent overrides a gate at state A (via evidence), advances to state B, then rewinds to state A.

Current behavior (inferred from code):
- `rewind` appends `Rewound` event, marks epoch boundary (persistence.rs comments on epoch logic)
- Re-entry to state A has NO evidence (fresh epoch; advance.rs:343: `current_evidence = BTreeMap::new()`)
- Gates are re-evaluated from scratch
- **Implication:** Overrides are NOT persistent across rewind. Agent must re-submit evidence or use --to again.

**For override tracking:** If override event records the gate failure + evidence, rewinding + re-evaluating creates a NEW override event (or requires re-evidence).

#### Edge Case 7: States with Conditional Transitions but No Accepts Block
**Location:** `src/engine/advance.rs:354-359`, `src/cli/next.rs:56-68`

**Scenario:** State has conditional transitions but NO accepts block; no conditional matches.

Current behavior:
- Transition resolution returns `NeedsEvidence` (advance.rs:439)
- Since `template_state.accepts.is_none()` (advance.rs:346), engine returns `UnresolvableTransition` (line 358)
- Agent receives `NextResponse` with error or `UnresolvableTransition` stop reason
- **No override path:** Agent cannot provide evidence; must use `--to`

**Implication:** Override tracking must handle templates with logical gaps (conditional transitions without accepts). These are not override scenarios, but edge cases that might trigger `--to` usage (which IS an override).

### Implications for Requirements

1. **Partial Gate Failure Tracking:**
   - Override event should list which gates failed (not just "gates failed")
   - Should distinguish between gates failed (blocking) vs. gates bypassed (overridden)

2. **Evidence Intent Clarity:**
   - Evidence submitted when gates pass should NOT generate override events
   - Evidence submitted when gates fail AND accepts exists → override event required
   - Need gate evaluation result attached to override event

3. **Action Skipping Boundaries:**
   - Is action skipping an override only when gates also fail?
   - Or is action skipping an independent override (evidence presence = action skip)?
   - Current code treats them independently; requires PRD clarification

4. **--to Flag Semantics:**
   - Should --to on a gate-having state record which gates were bypassed?
   - Should --to enforce that target has valid transition from current state (already done)?

5. **Epoch Boundaries:**
   - Override events are per-epoch; rewind creates new epoch
   - Override audit trail should be queryable per-epoch or across epochs?

6. **States Without Override Paths:**
   - States with gates-only (no accepts): gate bypass only via --to
   - States with conditional-only (no accepts): unresolvable, --to only
   - Should PRD explicitly forbid these patterns, or handle them as edge cases?

### Open Questions

1. **Partial Gate Failure Override Semantics:**
   - If 3 gates fail but evidence satisfies transition (not gates), is this 1 override or 3 overrides?
   - Should override event be "gate bypass" or "transition override" or both?

2. **Evidence vs. --to Override Distinction:**
   - Should evidence-based and --to-based overrides be recorded differently?
   - Should --to require explicit rationale while evidence-based override infers rationale from evidence?

3. **Action Skipping Edge Case:**
   - Is action skipping "for gate override" or "independent override"?
   - Example: State has action, gates pass, evidence submitted → action skipped, no gate override. Should this be tracked?

4. **Rewind Override Semantics:**
   - If agent overrides at state A, advances to B, rewinds to A, and re-overrides — are these the same override or two separate events?
   - Should override event have a "rewind boundary" marker?

5. **Template Validation:**
   - Should PRD require templates to have accepts blocks on gate-having states, or tolerate gates-only states for compliance gates?

---

## Summary

The codebase implements **three implicit override mechanisms:** (1) evidence-submitted bypass of gates on states with accepts, (2) action skipping based on evidence presence, and (3) directed-transition bypass of ALL gates via --to flag. These are currently invisible to the audit trail—gates fail, agents submit evidence or use --to, and workflows proceed without explicit "override" events. Key edge cases include partial gate failures (some gates fail, some pass), states with gates but no accepts (no evidence-based bypass), and rewind semantics (overrides are epoch-scoped). The PRD must decide whether evidence intent is explicit (rationale field) or inferred (gate failure + evidence = override), handle partial failures granularly, and clarify whether action skipping is an independent or gate-dependent override worthy of audit.

