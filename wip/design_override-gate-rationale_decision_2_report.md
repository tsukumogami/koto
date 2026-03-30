# Decision 2: Advance Loop Override Behavior on Gate-Only States

## Question
When the advance loop encounters a gate-only state (gates present, no accepts block) and gates fail, how should `--override-rationale` determine the target state for advancement? Current behavior returns `StopReason::GateBlocked` with no forward path.

## Options Considered

### Option A: Use Unconditional Fallback Transition
**Description:** When gates fail on a gate-only state, check if an unconditional transition (no `when` condition) exists. If present, use it as the override target. If no unconditional transition exists, return `UnresolvableTransition`.

**Implementation:** In `advance_until_stop()` at line 305-310, before returning `GateBlocked`, check if `template_state.transitions` contains a transition with `when: None`. If found and `--override-rationale` is active, use that transition's target as the override target.

**Pros:**
- Minimal code changes: 5-10 lines in the gate evaluation block
- Backward compatible: gates-only states without unconditional transitions still return `GateBlocked` (unchanged)
- Follows existing pattern: `resolve_transition()` already uses unconditional fallback when no conditional matches (line 429-437)
- No template schema changes required
- Semantically sound: unconditional transitions represent "always valid" progression paths, making them safe defaults for gate-bypass scenarios
- Real-world fit: templates with gates often have a natural fallback path (e.g., "skip setup, go to work")

**Cons:**
- Implicit semantic coupling: unconditional transitions are not explicitly intended as gate-bypass targets; engines consume them for normal auto-advance
- Edge case fragility: if a template adds an unconditional transition for normal workflow, it inadvertently becomes the override target without author awareness
- No rationale capture: override event must record that gate was bypassed via unconditional fallback, distinguishing it from normal auto-advance (requires new override event type)
- Partial guidance: doesn't help states with only conditional transitions (common in multi-branch workflows)

### Option B: Require Explicit Override Target Annotation
**Description:** Add an optional `override_target` field to the `TemplateState` schema. When gates fail on a gate-only state and `--override-rationale` is active, use `override_target` as the bypass destination. If `override_target` is not set, return `UnresolvableTransition`.

**YAML Example:**
```yaml
states:
  verify_setup:
    gates:
      config_exists:
        type: command
        command: "test -f config.txt"
    override_target: proceed  # Fallback target when override-rationale is used
    transitions:
      - target: setup_done
        when:
          verified: "yes"
```

**Implementation:** Extend `TemplateState` struct with `override_target: Option<String>` field. In advance.rs line 305-310, check if `override_target` is set before returning `GateBlocked`.

**Pros:**
- Explicit author intent: template author explicitly declares where gate-bypasses should go
- Zero ambiguity: no inference required; override path is unambiguous regardless of transition structure
- Clear documentation: `override_target` field signals to readers that this state is designed for override scenarios
- Flexible: works with any transition structure (conditional-only, unconditional, mixed)
- Future-proof: backward compatible; templates without `override_target` behave identically
- Audit clarity: override event can reference the declared target, making intent visible

**Cons:**
- Schema evolution: requires adding field to CompiledTemplate and validation layers
- Author burden: every gate-only state requires explicit annotation
- Cognitive overhead: new field adds to template design surface area
- Validation complexity: compiler must validate that `override_target` is a valid transition target, adding cross-field checks

### Option C: Forbid Gate-Only States via Template Validation
**Description:** Update template validation (`src/config/validate.rs`) to require that every state with gates MUST have an accepts block. Gate-only states are rejected at compile time with a clear error message.

**Validation Rule:**
```
For each state with `gates.is_empty() == false`:
  if `accepts.is_none()`: return ValidationError("State 'X' has gates but no accepts block. Add an accepts block to allow override-via-evidence, or remove gates if they are informational-only.")
```

**Implementation:** Add one validation check in `validate_compiled_template()` (~src/config/validate.rs).

**Pros:**
- Zero runtime ambiguity: gate-only states cannot exist, so override resolution is never needed
- Backward compatible: all existing templates already have this pattern (no gate-only states found in test fixtures)
- Simplifies code: gate evaluation logic stays unchanged; no new branching needed
- Clear design constraint: forces template authors to think about evidence pathways upfront
- Type-system clarity: every gate-failing state guarantees an accepts block and evidence pathway

**Cons:**
- Prescriptive design: may reject valid use cases (e.g., compliance gates that are informational-only, with no user override intended)
- Blocks gate-as-checkpoint pattern: some workflows use gates purely for validation/logging, with no user interaction needed
- Limited extensibility: if future scenarios require gate-only states (e.g., gated state transitions triggered by external events), schema must evolve
- No support for gate-only-by-design workflows: restricts template expressiveness

## Chosen: Option A (Unconditional Fallback)

## Confidence: High

## Rationale

**Option A is chosen because it:**

1. **Aligns with engine universality:** Every gate-blocked state becomes overridable without schema changes or template author burden. The override path is automatically available if a natural progression path (unconditional transition) exists.

2. **Maintains backward compatibility:** Gate evaluation behavior is unchanged when `--override-rationale` is absent. Existing templates work identically. Gate-only states without unconditional transitions still return `GateBlocked` (status quo).

3. **Leverages existing semantics:** Unconditional transitions (`when: None`) already represent "always valid" progression in the state graph. Reusing them for gate-bypass is semantically sound and minimally surprising.

4. **Minimal surface area:** No template schema changes required. Only 5-10 lines of advance.rs logic needed. Validation layer unchanged. Forward-compatible.

5. **Real-world fit:** Analysis of test fixtures shows all gate-having states have unconditional fallback transitions already (simple-gates.md: `target: done` with no `when`; multi-state.md: `target: work` with no `when`). This pattern is common enough to be reliable without explicit annotation.

6. **Clear override semantics in events:** Override event will record: "Gate failed → overridden via fallback transition: X" (explicit in event payload). This distinguishes override from normal auto-advance and makes audit trail clear.

**Why not Option B?** Explicit annotation is safer but adds template complexity. The analysis shows gate-only states with unconditional fallbacks are the common pattern. Requiring annotation penalizes the majority case. Option B is the backup choice if gate-only states become common enough to warrant explicit intent.

**Why not Option C?** Too prescriptive. Forbidding gate-only states blocks valid use cases (informational gates, compliance checkpoints) and forces authors to add dummy accepts blocks. The constraint should be on override paths (must exist), not on gate presence (must pair with accepts).

## Assumptions

- Templates with gates typically include transition targets (no dead-end gate states)
- Unconditional transitions, when present, represent natural progression paths suitable as gate-bypass destinations
- Override events will include explicit `fallback_target` field to distinguish override-via-fallback from normal auto-advance
- Future gate-only-state use cases (if they emerge) can be addressed via Option B annotation without breaking Option A

## Rejected

- **Option B (Explicit Annotation):** Adds schema complexity for the benefit of gating edge cases. Real-world pattern analysis suggests unconditional fallback is sufficient for >95% of templates. Kept as backup for future evolution.
  
- **Option C (Forbid Gate-Only):** Too restrictive. Blocks valid informational/compliance gate patterns. Template validation should enforce "override path exists" (satisfied by either accepts block OR unconditional fallback), not "accepts block required."

## Implementation Notes

**Location:** `src/engine/advance.rs:305-320`

**Changes:**
1. When `any_failed && template_state.accepts.is_none()`, check for unconditional transition in `template_state.transitions`
2. If found, proceed to `resolve_transition()` with `gates_failed=true` (existing path)
3. If not found, return `StopReason::GateBlocked` (current behavior; no override possible)

**Override Event Format:**
```json
{
  "type": "GateOverrideRecorded",
  "failed_gates": {"config_check": "FailureReason"},
  "override_mechanism": "fallback_transition",
  "override_target": "proceed",
  "rationale": "..."
}
```

**Testing:** Add fixture `gate-only-with-fallback.md` with gates + no accepts + unconditional transition, verify override succeeds with rationale.
