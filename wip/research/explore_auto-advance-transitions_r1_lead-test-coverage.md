# Lead: Existing tests and test gaps for skip_if

## Findings

### Current Test Coverage of Auto-Advance

#### Existing Gherkin Scenarios (test/functional/features/)

The functional test suite has **11 .feature files** covering workflows. Key files for auto-advance are:

1. **gate-with-evidence-fallback.feature**: Three scenarios testing gate + evidence interaction
   - "Gate passes and auto-advances" (lines 3-12): Gate passes → auto-advance to done (unconditional fallback)
   - "Gate fails and evidence is required" (lines 14-22): Gate fails, `accepts` block exists → evidence_required
   - "Gate fails then evidence advances" (lines 24-36): Evidence triggers unconditional auto-advance to done

2. **structured-gate-output.feature**: Two scenarios testing gates.* routing
   - "Gate passes and auto-advances via gates.* routing" (lines 20-29): Structured routing with when: {gates.ci_check.exit_code: 0} auto-advances
   - "Gate fails and routes to fix state via gates.* routing" (lines 32-40): Different target via gates.ci_check.exit_code: 1

3. **mixed-gate-routing.feature**: Two scenarios testing combined gate + agent evidence
   - "Gate passes and agent evidence match combined when clause" (lines 4-15): Both gate output AND agent evidence required for auto-advance
   - "Agent evidence alone without matching gate output does not advance" (lines 17-28): Falls through to different unconditional transition

4. **workflow-lifecycle.feature**: One auto-advance scenario
   - "Next advances when gate passes" (lines 21-30): Gate passes with file → auto-advance to terminal state with `advanced: true`

5. **rewind.feature**: Tests resume/state recovery but NOT skip_if
   - Verifies state restoration after rewind; does not test synthetic events or auto-advance chains

#### Integration Test Coverage (tests/integration_test.rs)

Extensive testing of auto-advance chaining with unconditional transitions:

1. **auto_advance_reaches_verify_from_plan()** (line 1608): 
   - Template: `plan → implement → verify (stops)` (unconditional transitions)
   - Single `koto next` call auto-advances twice: `plan → implement → verify`
   - Stops at verify (evidence_required) because verify has `accepts` block

2. **evidence_triggers_auto_advance_chain()** (line 1651):
   - Tests auto-advance chain in response to evidence submission
   - `verify` state has conditional transitions: `reject → implement`, `approve → done`
   - Submit reject evidence → auto-advance chain: `verify → implement → verify` (stops at second verify)
   - Submit approve evidence → auto-advance: `verify → done` (terminal)
   - Verifies `advanced: true` and correct final state after chain

3. **default_action_creates_file_and_auto_advances()** (line 3621):
   - Tests default_action execution followed by auto-advance
   - Action creates condition that gates check, then gates pass → auto-advance

#### Fixture Templates (test/functional/fixtures/templates/)

- **simple-gates.md**: Unconditional fallback transition (line 28-29: `- target: done` with no `when` clause)
- **structured-routing.md**: Gates.* routing with both pass and fail paths (exit_code: 0 vs 1)
- **multi-state.md**: Multiple states with accepts blocks and conditional transitions
- **structured-gates.md**: Gate that always fails with unconditional fallback
- **mixed-routing.md**: Both gates.* AND agent evidence in same when clause

### Cycle Detection and Chain Limits

From `src/engine/advance.rs` (lines 16-18, 70-86, 208-225, 472-478):

1. **MAX_CHAIN_LENGTH = 100**: Safety limit on consecutive transitions per `advance_until_stop()` invocation
   - Checked at loop iteration (line 219-224)
   - Returns `StopReason::ChainLimitReached` if exceeded

2. **Cycle Detection via `visited` HashSet** (line 472-478):
   - Tracks states auto-advanced THROUGH during current invocation
   - **Starting state NOT added to visited** (lines 203-206): Allows legitimate loops like `review → implement → review`
   - Before transitioning, checks `if visited.contains(&target)` → returns `CycleDetected`
   - **Gap**: No test verifies cycle detection works (no integration test for this case)

3. **Current Auto-Advance Points**:
   - After gate evaluation: if gates pass and unconditional transition exists, auto-advance
   - After evidence submission: if new evidence resolves a conditional transition, can auto-advance to next state if that state is also unconditional
   - No test explicitly verifies multiple consecutive unconditional transitions (plan → implement → verify works, but verify requires evidence)

### Loop Structure and Evidence Management

From `src/engine/advance.rs` (lines 208-532):

The advance loop iteration does:
1. Check shutdown flag
2. Check chain limit
3. Look up current state
4. Check if terminal
5. Check integration
6. Execute default_action (if present)
7. Evaluate gates (emit GateEvaluated events)
8. Resolve transition (using merged evidence: agent + gate output)
   - If resolved → append Transitioned event, continue loop
   - If needs evidence → return EvidenceRequired
   - If gate blocked → return GateBlocked
9. Fresh epoch: `current_evidence = BTreeMap::new()` after auto-advance (line 493)

**Key for skip_if**: Each auto-advance clears `current_evidence`. A new skip_if predicate must evaluate BEFORE this fresh epoch to use the same conditions.

### Test Gaps for skip_if Implementation

#### 1. Happy Path: Single skip_if fires
- **What to test**: State with `skip_if` condition evaluates true → auto-advances without evidence
- **Fixture needed**: New template with state having:
  - `skip_if: {condition_type: "context_file_exists", path: ".context/flag.md"}`
  - Unconditional transition to next state
- **Gherkin scenario**: Create context file, `koto next`, verify auto-advance
- **Complexity**: Trivial (reuse simple-gates template structure)

#### 2. Happy Path: Consecutive skip_if states chain in one loop turn
- **What to test**: `A → B → C → stops` where A and B have firing skip_if conditions, C requires evidence
- **Fixture needed**: Three-state chain, first two with skip_if, third with `accepts`
- **Gherkin scenario**: Single `koto next` auto-advances through A and B, stops at C
- **Complexity**: Medium (requires verifying loop chaining, not just single transition)
- **Key assertion**: `advanced: true`, final state is C, action is `evidence_required`

#### 3. Edge Case: skip_if condition unmet (falls through to evidence blocking)
- **What to test**: State with `skip_if` condition evaluates false → no auto-advance, waits for evidence
- **Fixture needed**: Same as #1 but without creating the context file
- **Gherkin scenario**: `koto next` without condition met, verify `evidence_required` or `gate_blocked`
- **Complexity**: Trivial (use existing evidence-fallback patterns)

#### 4. Edge Case: Resume after skip_if (synthetic event in log)
- **What to test**: After skip_if auto-advance, log contains `Transitioned` event with `condition_type: "skip_if"`; resume from that state works
- **Fixture needed**: Reuse skip_if template from #1
- **Gherkin/Integration test**: 
  - `koto init` → `koto next` (skip_if fires, auto-advances to B)
  - Read state file log → verify `Transitioned` event exists with condition_type
  - `koto next` again from B → continues normally
- **Complexity**: Complex (requires reading event log format, verifying event structure)
- **Prerequisite**: Must define Transitioned event schema change (synthetic: true? reason: "skip_if"?)

#### 5. Edge Case: skip_if + gates (gate already evaluated, skip_if uses gate output as condition)
- **What to test**: State has both gate and skip_if; gate evaluates, skip_if condition references gate output
- **Fixture needed**: 
  - State with gate that always passes/fails
  - `skip_if: {condition_type: "gates.gate_name.exit_code", value: 0}`
- **Gherkin scenario**: 
  - Gate evaluates → gate output injected into evidence
  - skip_if condition checks gate output → auto-advance if matches
- **Complexity**: Complex (requires skip_if evaluator to access gate_evidence_map)

#### 6. Edge Case: Cycle prevention (skip_if target already visited)
- **What to test**: Verify existing cycle detection catches skip_if-triggered loops
- **Fixture needed**: Template with skip_if creating a cycle (e.g., A → B → A via skip_if)
- **Integration test**: `koto next` should hit CycleDetected, not loop infinitely
- **Complexity**: Medium (reuse cycle detection logic already in code, just needs skip_if to trigger it)
- **Prerequisite**: Requires implementing skip_if evaluation in the loop

#### 7. Integration: skip_if with conditional transitions (what happens after skip_if?)**
- **What to test**: State has `skip_if` AND conditional transitions; if skip_if fires, which path?
- **Fixture needed**: State with skip_if that auto-advances, next state has conditional transitions
- **Complexity**: Medium (overlaps with #2, but adds routing complexity)

### Fixture Adaptation Potential

- **simple-gates.md** → Can be adapted for single skip_if test by adding skip_if condition
- **multi-state.md** → Can be extended to add skip_if-eligible states
- **structured-routing.md** → Can test skip_if + gates interaction
- **New fixture required** for any skip_if-specific chaining test (no current template has skip_if)

### Event Log Structure (from src/engine/types.rs, lines 135-139)

Current `Transitioned` event:
```rust
Transitioned {
    from: Option<String>,
    to: String,
    condition_type: String,  // Currently "auto" or "evidence"
}
```

For skip_if, `condition_type` could be:
- `"skip_if"` (with optional metadata: reason, matched_condition)
- Or add new `SyntheticTransitioned` variant with skip_if-specific fields

### No Existing Tests for:

1. ✗ Cycle detection (no integration test verifies CycleDetected stop reason)
2. ✗ Chain limit (no test verifies MAX_CHAIN_LENGTH stopping)
3. ✗ Synthetic event format and log roundtrip
4. ✗ skip_if conditions (obviously)
5. ✗ Gates + skip_if interaction
6. ✗ Multiple consecutive auto-advances from same template state (verify in evidence_triggers_auto_advance_chain stops at second verify, doesn't test A→B→C→stops pattern)

## Implications

### For skip_if Design

1. **Event representation must be test-verifiable**: The synthetic Transitioned event needs clear `condition_type` or variant to distinguish skip_if from manual evidence in logs. Tests will need to read and assert on log structure.

2. **Cycle detection is already implemented**: No new cycle-prevention logic needed. Tests just need to verify skip_if respects the existing `visited` set.

3. **Chain limit is already in place**: skip_if chains are bounded by MAX_CHAIN_LENGTH=100. A test should verify this still holds with skip_if.

4. **Fresh evidence epoch exists**: After each auto-advance (line 493), `current_evidence` clears. skip_if evaluation must happen BEFORE this to not lose context, OR skip_if conditions must be re-evaluable (stateless predicates).

5. **Gate interaction is real**: Tests must verify that states with both gates and skip_if handle ordering correctly: gates evaluate first, skip_if sees gate output in the evidence map.

### For Test Strategy

1. **Fixture-first approach works**: Existing template structure (gates, accepts, transitions, terminal) is sufficient. Just add `skip_if` field.

2. **Gherkin + Integration test split**: 
   - Simple scenarios (single skip_if, falls-through) → Gherkin in test/functional/features/ (fast, human-readable)
   - Complex scenarios (chaining, log verification, cycle detection) → Integration test in tests/integration_test.rs (faster to write, full control)

3. **No backward-compatibility concerns**: New skip_if field is additive. Existing tests should all pass unchanged.

## Surprises

1. **Cycle detection doesn't include starting state**: The visited set excludes the initial state (lines 203-206), meaning `A → B → A` is legitimate. But there's no test verifying this works as intended. A skip_if test that creates a legitimate loop might accidentally expose a bug if cycle detection is too aggressive.

2. **No test verifies log event format end-to-end**: The integration tests check JSON output fields but don't read and assert on the actual state file log (.state.jsonl). This means the synthetic event structure must be designed carefully—tests will need to inspect raw event JSON.

3. **Max chain length is high (100)**: Allows very deep auto-advance chains. A template with 100 consecutive skip_if states could legitimately auto-advance. Most tests will hit evidence_required before hitting this limit.

4. **Fresh evidence epoch is aggressive**: After EACH auto-advance, all evidence is cleared. This means if a state has skip_if + gates, the gates are re-evaluated on the next loop turn. This is actually good (deterministic) but might surprise users who expect gate output to persist.

## Open Questions

1. **What is the exact syntax/schema for skip_if in template YAML?** 
   - Option A: `skip_if: {condition_type: "context_file_exists", path: "..."}`
   - Option B: `skip_if: ["context.exists:path/to/file", "vars.SOMEVAR=value"]`
   - This affects fixture design and test readability.

2. **Can a state have both skip_if AND accepts?**
   - If skip_if fires, accepts is irrelevant (auto-advance).
   - If skip_if doesn't fire, falls through to accepts + evidence_required.
   - Tests need to cover both paths for the same state (two separate test scenarios).

3. **Should skip_if conditions support gates.* references?**
   - If yes, gates evaluate first, then skip_if sees gate output.
   - This is covered in Edge Case #5 but requires clarification.

4. **What is the exact event structure for synthetic Transitioned events?**
   - Modify existing `condition_type` field to accept "skip_if"?
   - Or create new event variant `SkipIfTransitioned` with skip_condition metadata?
   - Tests will assert on this structure, so it must be locked down early.

5. **Should koto provide introspection tools for skip_if states?**
   - E.g., `koto introspect template.md --skip-if-eligible` to list states that would auto-advance?
   - Out of scope for unit tests but affects fixture design (should fixtures be designed to test introspection?).

## Summary

Existing tests thoroughly cover unconditional auto-advance (gates passing + fallback transitions) and evidence-triggered chaining via the `auto_advance_reaches_verify_from_plan()` and `evidence_triggers_auto_advance_chain()` integration tests. Cycle detection and MAX_CHAIN_LENGTH are implemented but untested. Introducing skip_if requires seven new test scenarios, with the most critical being: (1) single skip_if firing (trivial, reuse simple-gates fixture), (2) chained skip_if states in one loop turn (medium, new 3-state fixture), and (4) resume after skip_if with synthetic event verification (complex, requires log format finalization). The Transitioned event must be extended with a skip_if-specific `condition_type` or variant, and tests must verify that skip_if respects existing cycle detection and chain limits.
