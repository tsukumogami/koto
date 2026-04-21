# Lead: Template schema and YAML syntax for skip_if

## Findings

### Template Schema Structure and Compilation Pipeline

**Compilation Flow** (`src/template/compile.rs`):
- YAML frontmatter is parsed via `serde_yaml_ng::from_str()` (line 162) into a `SourceFrontmatter` struct
- `SourceFrontmatter` contains a `states: HashMap<String, SourceState>` field (line 28)
- Each `SourceState` is compiled into a `TemplateState` (lines 260-272) through a stateful transformation
- Template validation happens after compilation: transition targets are validated (lines 277-286), initial_state is validated (lines 290-295)

**Current SourceState Fields** (lines 49-67 in `compile.rs`):
```rust
struct SourceState {
    transitions: Vec<SourceTransition>,      // Unconditional + conditional routing
    terminal: bool,                          // Final state marker
    gates: HashMap<String, SourceGate>,      // Deterministic checks (command, context-exists, etc.)
    accepts: HashMap<String, SourceFieldSchema>,  // Agent evidence schema
    integration: Option<String>,             // Integration hook path
    default_action: Option<SourceActionDecl>, // Auto-executed command
    materialize_children: Option<SourceMaterializeChildrenSpec>,
    failure: bool,                           // Terminal failure marker
    skipped_marker: bool,                    // Synthetic skip marker
}
```

**Compiled TemplateState** (lines 54-86 in `types.rs`):
Maps directly from SourceState with serde `#[serde(default, skip_serializing_if = ...)]` annotations:
- Empty strings, false bools, and None optionals are omitted from serialized JSON
- All fields are optional during deserialization (serde default)
- **No skip_if field exists yet**

### Existing Composition Rules: accepts + gates + transitions

**Evidence Blocking Semantics** (from `src/engine/advance.rs` lines 309-530):
1. Gates are evaluated first (lines 309-431)
2. Gate output is synthesized into an evidence map under `"gates"` namespace (lines 388-391, conditional on `has_gates_routing`)
3. If any gate fails AND no `accepts` block exists AND no `gates.*` when-clause references, return `GateBlocked` immediately (lines 420-430)
4. Otherwise, fall through to transition resolution with the synthesized evidence
5. Transition resolution matches when-clauses against combined evidence (agent-submitted + gate output)
6. If no transitions match and `accepts` exists, return `NeedsEvidence` (requiring agent submission)
7. If no transitions match and no `accepts`, return `GateBlocked` (hard block)

**Composition Pattern**: A state can have `accepts` + `gates` + `transitions`:
- If gates pass: transition resolution uses gate output + agent evidence if present; falls back to unconditional transition if no conditionals match
- If gates fail + accepts exists: requires agent evidence to resolve transitions (fallback available)
- If gates fail + accepts absent: returns GateBlocked (no fallback)

### Designing skip_if YAML Syntax

**Field Placement in SourceState**:
`skip_if` should be added at the same hierarchical level as `gates`, `accepts`, `transitions` — as an optional top-level state field. This mirrors the structural pattern:
- `gates`: dict of named gate declarations → deterministic checks
- `accepts`: dict of named evidence fields → agent-submission schema
- `transitions`: list of target+when pairs → routing rules
- **`skip_if`: boolean predicate → auto-advance condition** (NEW)

**Condition Type 1: Template Variable Existence/Value**

From the condition-types lead, template variables are stored in `WorkflowInitialized` event as `HashMap<String, String>` and accessed via the `vars.NAME: {is_set: bool}` matcher. The `skip_if` YAML syntax reuses this exact pattern:

```yaml
skip_if:
  vars:
    SHARED_BRANCH: true  # is_set: true → variable exists and is non-empty
```

Or in shorthand (matching when-clause style):
```yaml
skip_if:
  vars.SHARED_BRANCH:
    is_set: true
```

**Condition Type 2: Context Key Existence**

Context keys are checked via the `context-exists` gate evaluator (lines 118-146 in `src/gate.rs`). The skip_if syntax should mirror gate declaration style but embedded in the predicate:

```yaml
skip_if:
  context:
    context.md: true  # key exists → advance
```

Or equivalently:
```yaml
skip_if:
  context.md:
    exists: true
```

**Condition Type 3: Evidence Field Value**

Evidence matching already exists in transition `when` clauses via `resolve_transition()` (lines 596-627 in `advance.rs`). The skip_if predicate reuses this syntax exactly:

```yaml
skip_if:
  verdict: proceed  # Direct equality check on evidence field
```

Or nested JSON:
```yaml
skip_if:
  gates.config_check.exit_code: 0  # Gate output value
```

### Proposed YAML Syntax

**Single-Predicate Form** (v1, simplest):

A `skip_if` field contains a dict of condition matchers (same syntax as `when` clause on transitions):

```yaml
states:
  plan_context_injection:
    gates:
      context_file:
        type: context-exists
        key: context.md
    skip_if:
      # Auto-advance if context file exists
      gates.context_file.exists: true
    transitions:
      - target: plan_validation

  plan_validation:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
    skip_if:
      # Auto-advance if verdict is proceed (almost always true in practice)
      verdict: proceed
    transitions:
      - target: setup_plan_backed

  setup_plan_backed:
    skip_if:
      # Auto-advance if SHARED_BRANCH variable is set
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: next_state
```

**Type Definition for Rust**:

Add to `src/template/types.rs`:
```rust
/// Skip-if predicate for deterministic auto-advance without agent evidence.
/// Evaluated as a conjunction of condition matchers (all must be true).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkipIfPredicate {
    /// Dict of condition matchers using the same syntax as transition `when` clauses.
    /// Keys can be:
    /// - Evidence field paths: "verdict", "status", "gates.CONFIG.exit_code", etc.
    /// - Variable existence: "vars.VARNAME: {is_set: bool}"
    /// - Context key existence: "context.KEY: {exists: bool}" (requires ContextStore refactoring)
    pub conditions: BTreeMap<String, serde_json::Value>,
}
```

Add to `SourceState` in `src/template/compile.rs`:
```rust
struct SourceState {
    // ... existing fields ...
    #[serde(default)]
    skip_if: Option<serde_json::Value>,  // Parsed dict of condition matchers
}
```

Add to `TemplateState` in `src/template/types.rs`:
```rust
pub struct TemplateState {
    // ... existing fields ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if: Option<BTreeMap<String, serde_json::Value>>,
}
```

### Composition with Accepts, Gates, and Transitions

**Semantic Rules**:

1. **skip_if fires → auto-advance immediately**
   - No evidence is submitted or required
   - Gates are evaluated (for their output if referenced in other conditions), but gate failure does NOT block skip_if
   - If `accepts` block exists, it is bypassed
   - If `transitions` are conditional, they are bypassed
   - A transition is implicitly selected (see "Transition Target Selection" below)

2. **skip_if does not fire → normal flow**
   - Gates are evaluated; if gates fail and `accepts` exists, return `NeedsEvidence`
   - If gates pass or no gates, transition resolution proceeds with `when` clauses
   - `accepts` behavior unchanged: required if transitions are conditional and no unconditional fallback

3. **A state can have both skip_if and accepts**
   - Semantic: "auto-advance on condition; if condition unmet, request evidence"
   - Implementation: evaluate skip_if first (before transition resolution); if it fires, skip rest; if not, proceed to normal gate → transition resolution flow

4. **skip_if does NOT interact with gates that have already evaluated**
   - Gates run as normal and produce output in the `gates.*` namespace
   - skip_if can reference gate output (e.g., `gates.CONFIG.exit_code: 0`), but gate failure does not prevent skip_if from firing
   - This is intentional: skip_if is a *deterministic* condition (variable set, context exists), not a fallback for gate failures

### Transition Target Selection

A state with `skip_if` must have a clear routing target when the condition fires. Three options:

**Option A (Simplest): Single unconditional transition**
```yaml
states:
  plan_context_injection:
    skip_if:
      gates.context_file.exists: true
    transitions:
      - target: plan_validation  # Only one, unconditional → auto-advance target
```

**Option B: Explicit target in skip_if**
```yaml
states:
  plan_context_injection:
    skip_if:
      target: plan_validation
      condition:
        gates.context_file.exists: true
```
This requires restructuring skip_if as an object, not a flat dict.

**Option C: Reuse conditional transitions (complex)**
```yaml
states:
  plan_context_injection:
    skip_if:
      gates.context_file.exists: true
    transitions:
      - target: plan_validation
        when:
          gates.context_file.exists: true  # Skip matching required
      - target: retry
        when:
          gates.context_file.exists: false
```
This couples skip_if and transition routing, causing duplication.

**Recommended: Option A + Option C Combined**
- If skip_if fires and there's an unconditional fallback transition, use it
- If skip_if fires and all transitions are conditional, pick the one where skip_if would match as a when-clause
- Validation error if ambiguous (multiple conditional transitions match the skip_if condition)

### Validation Rules

**Compile-Time Validations**:

1. **skip_if references must be valid**
   - Evidence field paths must exist in `accepts` schema or be synthesizable (gates.*)
   - `vars.VARNAME` references are always valid (variables are not compile-time-validated)
   - Context key references assume ContextStore is available at runtime

2. **Transition target selection**
   - If skip_if exists, state must have exactly one unconditional transition OR multiple conditional transitions where exactly one matches all skip_if conditions
   - Ambiguity error if multiple transitions match
   - Unreachable transition error if no transition matches but skip_if can fire

3. **No circular skip_if chains at compile time**
   - If A skip_if→B and B skip_if→A, the template compiles (valid YAML), but cycle detection at runtime catches it
   - No static analysis needed; runtime cycle detection suffices

4. **skip_if cannot reference accepts fields that are required for other routing**
   - If skip_if references `verdict: proceed` and `verdict` is the only routing field, all transitions must handle the skip_if case
   - Recommendation: warn if skip_if references evidence fields not also used in transitions

**Runtime Validations**:

1. **Synthetic event emission**
   - Append `Transitioned` event with `condition_type: "skip_if"` (new value, not "auto")
   - Include the skip_if condition values in the event payload for resumability
   - Example event: `{type: "Transitioned", from_state: "plan_context_injection", to_state: "plan_validation", condition_type: "skip_if", skip_if_matched: {gates.context_file.exists: true}}`

2. **Cycle detection**
   - Existing cycle detection (visited set, line 472 in `advance.rs`) applies to skip_if auto-advances
   - If A→skip→B→skip→C→skip→A, the return to A triggers CycleDetected
   - Note: Starting state exclusion (lines 203-206 in `advance.rs`) may need refinement for starting-state cycles

3. **Evidence epoch semantics**
   - After skip_if auto-advance, clear `current_evidence` (fresh epoch) as with other auto-advances (line 493)
   - Prevents skip_if in state B from inheriting evidence from state A

### Three Real-World Cases: YAML Examples

**Case 1: plan_context_injection — context key existence**

```yaml
states:
  plan_context_injection:
    gates:
      context_file:
        type: context-exists
        key: context.md
    skip_if:
      gates.context_file.exists: true
    transitions:
      - target: plan_validation
    default_action:
      command: "extract-context.sh"
```

**Semantics**: 
- Execute default action (`extract-context.sh`)
- Evaluate gate (check if `context.md` exists in context store)
- If gate passes (file exists), skip_if condition is true → auto-advance to plan_validation
- If gate fails (file missing), fall through to transition resolution; since no accepts and no gate routing, returns GateBlocked
- **On resume after interruption**: synthetic event shows condition_type="skip_if", preserving why state was passed

**Case 2: plan_validation — evidence field value**

```yaml
states:
  plan_validation:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
      rationale:
        type: string
    skip_if:
      verdict: proceed
    transitions:
      - target: setup_plan_backed
        when:
          verdict: proceed
      - target: validation_exit
        when:
          verdict: skip
```

**Semantics**:
- Accept evidence from agent (verdict + rationale)
- If evidence is {verdict: proceed}, skip_if fires → auto-advance to setup_plan_backed
- If evidence is {verdict: skip}, no skip_if match → transition resolution routes to validation_exit
- If no evidence submitted, transition resolution needs evidence → NeedsEvidence (agent must submit something)
- **On resume**: synthetic event shows condition_type="skip_if" and the matched verdict value

**Case 3: setup_plan_backed — template variable existence**

```yaml
variables:
  SHARED_BRANCH:
    description: "Shared branch name for plan orchestration (optional)"
    required: false

states:
  setup_plan_backed:
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: next_state
```

**Semantics**:
- If SHARED_BRANCH was set at `koto init --with SHARED_BRANCH=value`, skip_if fires → auto-advance
- If SHARED_BRANCH was not set or is empty, skip_if does not fire → transition resolution
- No `accepts` block: if skip_if doesn't fire, has no transitions routing, returns NoTransitions error (template design issue)
- **Corrected version should have**:

```yaml
states:
  setup_plan_backed:
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: next_state  # Unconditional fallback for when SHARED_BRANCH not set
```

Then if SHARED_BRANCH is set, skip_if fires and goes to next_state; if not set, unconditional transition takes it to next_state anyway (in this case, skip_if is redundant, but the pattern is clear).

**Alternative version with conditional routing**:

```yaml
states:
  setup_plan_backed:
    accepts:
      plan_mode:
        type: enum
        values: [shared_branch, standalone]
        required: false
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: branch_setup
        when:
          vars.SHARED_BRANCH:
            is_set: true
      - target: standalone_setup
        when:
          plan_mode: standalone
      - target: branch_setup  # Default if SHARED_BRANCH not set and no evidence
```

Here skip_if handles the SHARED_BRANCH=true case automatically; if false, agent can submit override evidence.

## Implications

1. **No Architectural Blockers for v1**: Template variable (Condition 1) and evidence field (Condition 3) checks can use existing data structures and resolution logic. Context-exists checks (Condition 2) require refactoring `ContextStore` into `advance_until_stop()` signature, a moderate change but straightforward.

2. **Minimal Template Schema Changes**: Add one optional `skip_if: Option<BTreeMap<String, Value>>` field to both `SourceState` and `TemplateState`. Serde handles serialization with skip_serializing_if.

3. **Reuses Existing Evaluation Logic**: skip_if conditions use the same `resolve_transition()` matching logic as `when` clauses, reducing implementation complexity. No new condition parser needed.

4. **Single Event Type for Resume-Awareness**: Extend `Transitioned` event with `condition_type: "skip_if"` and optional `skip_if_matched` payload field. This preserves the audit trail for resuming agents and requires minimal event schema changes.

5. **Validation Burden Moderate**: Compile-time validation can check reference validity; cycle detection already exists at runtime. Template authors must ensure unconditional transitions exist for fallback, or all transitions match the skip_if condition.

6. **Composition with accepts = Clear Semantics**: A state with both `skip_if` and `accepts` has unambiguous meaning: "auto-advance if condition fires; request evidence if condition unmet." This is the primary use case (plan_validation).

## Surprises

1. **Template Variables Are String-Only**: Unlike JSON-valued evidence, variables are `HashMap<String, String>`. The matcher `vars.VARNAME: {is_set: bool}` checks existence, not equality. To support `vars.VERSION: "2.0"` would require either changing storage type or adding a new `{equals: "value"}` matcher syntax. The simple `is_set` check is sufficient for the three use cases (SHARED_BRANCH is just checked for existence).

2. **Transition without Unconditional Fallback is Risky**: If a state has `skip_if` and only conditional transitions (all matching the condition), and the condition doesn't fire, the state is stuck (NoTransitions error). Validation should enforce: either have unconditional fallback, or all conditional transitions collectively cover both condition-true and condition-false cases. This is a design footgun to warn about.

3. **Gate Failure Does Not Block skip_if**: From the advance-loop lead, gates are evaluated but gate failure doesn't prevent skip_if from firing. This is correct (skip_if is a *deterministic* condition, not a fallback), but authors might expect gates to have authority over skip_if. Clear documentation needed.

4. **Gate Output Requires has_gates_routing**: Gate output is only synthesized if at least one transition references `gates.*` (line 402-410 in advance.rs). If skip_if references `gates.X.exit_code` but transitions don't, the gate output might not be merged. This should either (a) be fixed (synthesize gates unconditionally), or (b) skip_if should be treated the same way (count `gates.*` references in skip_if as triggering synthesis). Recommend (a) for clarity.

## Open Questions

1. **Context-Exists Refactoring Scope**: Should skip_if support context-key checks, or defer to v2? The refactoring is moderate (pass `ContextStore` to `advance_until_stop()`), but it's an architectural change outside the template schema. Recommend: defer unless context conditions are required for the first use case.

2. **Synthetic Event Payload**: Should the event include matched condition values (e.g., `skip_if_matched: {verdict: "proceed"}`), or just the condition_type? Recommendation: include values for debugging and resume clarity, but make it optional to avoid event payload bloat.

3. **Redundant skip_if + Unconditional Transitions**: If a state has `skip_if: {vars.X: {is_set: true}}` AND an unconditional transition, skip_if is always true or always irrelevant (both paths go to the same target). Should validation warn about this? Recommendation: yes, as a compile-time warning.

4. **Multi-Condition Logic**: Should skip_if support AND/OR composition (e.g., `skip_if: {all: [{gates.X: true}, {vars.Y: {is_set: true}}]}`), or is flat conjunction sufficient? From the exploration scope, multi-condition logic is "out of scope for v1," so recommend: single flat dict (conjunction of all conditions).

5. **Gate Override Interaction**: If an agent has recorded a `GateOverrideRecorded` event for a gate on a skip_if state, does skip_if still fire? From the advance-loop lead (lines 320-362), gate overrides are injected directly without calling the gate evaluator. skip_if should probably respect overrides too (if agent has overridden the gate, the override value should be used). This needs clarification.

## Summary

The `skip_if` field should be added to `TemplateState` as an optional `BTreeMap<String, Value>` containing condition matchers using the same syntax as transition `when` clauses (evidence field paths, variable existence via `vars.NAME`, context existence). A state can have both `skip_if` and `accepts`, allowing auto-advance on deterministic conditions with fallback to agent evidence; the skip_if conditions are evaluated after gate synthesis but before transition resolution, with chaining supported naturally by the existing loop. The three real-world cases translate directly: context-injection gates on existence, plan_validation advances on evidence value, setup_plan_backed on variable existence.

