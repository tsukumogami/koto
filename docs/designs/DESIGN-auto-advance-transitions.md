---
status: Proposed
problem: |
  Plan-backed orchestrator workflows contain states whose evidence submissions are
  fully deterministic given the current context -- yet agents must drive each state
  manually via koto next. A 9-child orchestrator run requires 36 mechanical
  round-trips before reaching states where actual decisions happen. The proposed
  fix is a skip_if predicate on template states that auto-advances when conditions
  are met, writes a synthetic Transitioned event so resuming agents know why a
  state was passed, and chains consecutive auto-advancing states within a single
  advance_until_stop() invocation.
decision: |
  Add an optional skip_if field to template states. Its value is a flat
  BTreeMap<String, Value> using the same dot-path syntax as transition when
  clauses. After gate synthesis but before resolve_transition(), the engine checks
  skip_if conditions; if all match, it injects the condition values as synthetic
  evidence, calls resolve_transition() normally to pick the target, appends a
  Transitioned event with condition_type "skip_if" and the matched values, then
  continues the advance loop. Consecutive skip_if states chain naturally within a
  single advance_until_stop() call.
rationale: |
  Reusing the when-clause BTreeMap type and the existing resolve_transition()
  function means zero new matchers, no changes to state-derivation or epoch-scoping
  logic, and a single consistent syntax for template authors. The advance loop's
  existing implicit-continue mechanism and visited-set cycle detection handle
  chaining and safety without modification. Three exploration decisions and two
  design decisions were evaluated with alternatives before committing to this shape.
---

# DESIGN: Auto-Advance Transitions via skip_if

## Status

Proposed

## Context and Problem Statement

When a koto template has states where the correct next step is fully deterministic
given the current workflow context, agents still drive each state manually. The
visible cost is token consumption and round-trip latency; the less-visible cost
is that templates become harder to author because authors must choose between
orientation (keeping states for resume-awareness) and efficiency (removing them
to eliminate mechanical driving).

Exploration confirmed the problem is real in plan-backed orchestrator workflows
in the shirabe plugin. A 9-child plan requires driving each child through 4
boilerplate states before reaching `analysis`, where actual decision-making
begins. Removing those states makes the template opaque on resume; keeping them
as-is costs 36 mechanical submissions per orchestrator run.

The solution is a `skip_if` predicate on individual states. When the predicate
is satisfied, koto fires the transition automatically, writes a `Transitioned`
event with `condition_type: "skip_if"` and the matched conditions as metadata,
and continues the advance loop to the next state. The state still appears in
history; the agent is never blocked waiting to submit known-constant evidence.

The three motivating cases establish the condition type requirements:

- `plan_context_injection`: auto-advance when `context.md` exists in the context
  store, expressed via a `context-exists` gate and `skip_if: {gates.context_file.exists: true}`
- `plan_validation`: auto-advance when the workflow is in plan-backed mode,
  expressed via template variable check `skip_if: {vars.PLAN_BACKED: {is_set: true}}`
- `setup_plan_backed`: auto-advance when `SHARED_BRANCH` is set,
  expressed via `skip_if: {vars.SHARED_BRANCH: {is_set: true}}`

## Decision Drivers

- **Orientation on resume must be preserved**: States must still appear in the
  event log. A resuming agent at `analysis` needs to know whether context came
  from a plan outline or a GitHub issue. Silent state collapse is explicitly
  rejected.
- **Chaining is required for the feature to deliver value**: Without consecutive
  auto-advance in a single loop turn, the feature saves evidence composition but
  not round-trips, delivering roughly 20% of the intended benefit.
- **Minimal engine and schema change**: The advance loop already supports
  chaining via implicit continue; cycle detection and chain limits already exist.
  The implementation should reuse these rather than duplicate them.
- **Template syntax must stay composable**: `skip_if` must coexist with `accepts`,
  `gates`, and `transitions` with clear, non-surprising semantics.
- **Public repo**: All documentation must be suitable for external contributors.

## Considered Options

### Decision 1: skip_if Template Schema and Compile-Time Validation

**Context**

koto templates declare states with `accepts` (evidence schema), `gates`
(deterministic checks), and `transitions` (routing rules). The design needed to
specify the exact YAML field structure for the new `skip_if` field and what
compile-time rules enforce correct usage. The existing `when` clause on transitions
uses `BTreeMap<String, serde_json::Value>` with dot-path keys for evidence fields,
template variables (`vars.NAME: {is_set: bool}`), and gate output (`gates.NAME.FIELD: value`).

**Key assumptions:** v1 is flat conjunction only (all conditions must match); direct
context-key predicates are deferred (gate workaround covers this); `resolve_transition()`
runs unchanged when `skip_if` fires (synthetic evidence is the only input difference).

#### Chosen: Flat dict, reusing `when`-clause syntax

`skip_if` is `Option<BTreeMap<String, serde_json::Value>>`, identical in type to
`Transition.when`. No new matchers are needed. Template authors who know the `when`
clause syntax already know the full `skip_if` syntax.

Compile-time validation rules:

- **E-SKIP-TERMINAL**: `skip_if` on a `terminal: true` state is a compile error.
  The terminal check fires before skip_if in the advance loop, making skip_if unreachable.
- **E-SKIP-NO-TRANSITIONS**: `skip_if` with no declared transitions is a compile error.
  There is no target to advance to.
- **E-SKIP-AMBIGUOUS**: When all transitions are conditional, evaluate each `when`
  clause against the `skip_if` values as synthetic evidence at compile time. Zero
  matches or more than one match is a compile error. Exactly one must match.
- **W-SKIP-GATE-ABSENT**: A `skip_if` key of the form `gates.NAME.*` where no gate
  named `NAME` is declared on the state produces a compile warning. The condition
  will be silently unmatchable at runtime.
- **GATES-ROUTING-EXTENSION**: The `has_gates_routing` scan in `advance.rs` must
  include `skip_if` condition keys for `gates.*` references, not only transition
  `when` clauses. Without this, the context-exists gate workaround fails silently
  because gate evidence is not injected into the merged map.

#### Alternatives Considered

- **Structured predicate object** with `condition` and optional `target` subfields:
  Rejected because the `target` subfield would bypass `resolve_transition()`, requiring
  a separate resolution path and contradicting the synthetic-evidence injection design.
  The `condition` nesting wrapper adds indentation with no semantic benefit.
- **List of predicates with OR semantics**: Rejected because OR composition is explicitly
  deferred for v1. A list type locks in OR semantics before the use cases are understood,
  and reversing to a flat dict in a later version would be a breaking change.

---

### Decision 2: Test Coverage Plan

**Context**

`skip_if` touches five source areas (template schema, compile-time validation, advance
loop, event schema, state derivation) and introduces first-of-kind behaviors: synthetic
evidence injection, a new `condition_type` value in the event log, and chaining via
consecutive auto-advances. The user explicitly requested thorough testing. Two
coverage gaps existed in the pre-existing test suite (cycle detection and chain-limit
triggering), and `skip_if` is the first feature to exercise both.

**Key assumptions:** Gherkin functional tests serve as living documentation for
template authors; integration tests are needed for raw JSONL inspection; compile-time
validation is pure Rust logic suited to unit tests.

#### Chosen: Gherkin-first with selective integration tests

| # | Scenario | Test type | File |
|---|----------|-----------|------|
| 1 | Single skip_if fires | Gherkin | `test/functional/features/skip-if.feature` |
| 2 | Consecutive skip_if states chain in one loop turn | Gherkin (behavior) + Integration (JSONL) | `skip-if.feature` + `integration_test.rs` |
| 3 | skip_if condition unmet — falls through to evidence blocking | Gherkin | `skip-if.feature` |
| 4 | Resume after skip_if — log has `condition_type: "skip_if"` and `skip_if_matched` | Integration | `integration_test.rs` |
| 5 | skip_if + gates — skip_if references gate output via `gates.NAME.exists: true` | Gherkin | `skip-if.feature` |
| 6 | Cycle prevention — skip_if cycle triggers `CycleDetected` | Integration | `integration_test.rs` |
| 7 | skip_if + conditional transitions — correct branch selected | Gherkin | `skip-if.feature` |
| 8 | skip_if + accepts — fires when met; prompts evidence when not | Gherkin | `skip-if.feature` |
| 9 | skip_if with vars condition — fires when var set; doesn't fire when absent | Gherkin | `skip-if.feature` |
| 10 | Compile-time validation — E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS | Unit | `src/template/compile.rs` |
| 11 | Chain limit — skip_if chains don't bypass `MAX_CHAIN_LENGTH` | Integration | `integration_test.rs` |

Scenarios 6 and 11 fill pre-existing coverage gaps in addition to testing `skip_if`
specifically.

#### Alternatives Considered

- **Integration-test-first**: Clusters most coverage in the already-large
  `integration_test.rs`, reduces documentation value for template authors, and
  contradicts the established project pattern. Rejected.
- **All three layers for all scenarios**: Triples test-writing effort for scenarios
  fully covered by one layer. "Thorough" means all scenarios covered, not all scenarios
  covered three times. Rejected.

---

## Decision Outcome

`skip_if` is a flat `BTreeMap<String, serde_json::Value>` field on `TemplateState`
reusing the `when`-clause syntax template authors already know. The advance loop
evaluates it after gate synthesis and before transition resolution, using the
existing `resolve_transition()` function with skip_if condition values injected as
synthetic evidence. Chaining works via the loop's existing implicit-continue mechanism.
A `Transitioned` event with `condition_type: "skip_if"` and optional `skip_if_matched`
metadata preserves resume-awareness. Eleven test scenarios distributed across Gherkin,
integration, and unit layers provide thorough coverage including two pre-existing gaps.

These decisions work together without conflict. The flat-dict schema feeds directly
into the synthetic-evidence injection strategy (no type conversion needed). The
compile-time validation reuses the when-clause evaluator, which is the same evaluator
that `resolve_transition()` uses at runtime. The Gherkin test fixtures document the
same YAML syntax template authors will write.

## Solution Architecture

### Overview

`skip_if` is evaluated on each loop iteration of `advance_until_stop()` after gates
have been synthesized but before `resolve_transition()` is called. If all conditions
in the `skip_if` map match the available evidence (merged agent evidence + gate
output + template variables), the engine treats the state as if the agent had submitted
matching evidence, calls `resolve_transition()` to select the target, and continues
the loop. Consecutive skip_if states chain naturally because the loop implicitly
continues after each successful transition.

### Components

```
src/template/
  types.rs          -- Add skip_if field to TemplateState
  compile.rs        -- Add SourceState.skip_if; compile validation rules E-SKIP-*

src/engine/
  advance.rs        -- skip_if evaluation block; has_gates_routing extension
  types.rs          -- Add skip_if_matched to EventPayload::Transitioned

src/engine/
  persistence.rs    -- No changes needed (Transitioned is already state-changing)

test/functional/
  features/skip-if.feature             -- New Gherkin feature file
  fixtures/templates/skip-if-chain.md  -- 3-state chain fixture (A→B→C)
  fixtures/templates/skip-if-gate.md   -- Gate + skip_if fixture
  fixtures/templates/skip-if-vars.md   -- Vars condition fixture

tests/
  integration_test.rs  -- 4 new test functions

src/template/
  compile.rs (mod tests)  -- 3 new unit test functions
```

### Key Interfaces

**Template YAML:**

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

  plan_validation:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
    skip_if:
      verdict: proceed
    transitions:
      - target: setup_plan_backed
        when:
          verdict: proceed
      - target: validation_exit
        when:
          verdict: skip

  setup_plan_backed:
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: analysis
```

**Rust type changes:**

```rust
// src/template/types.rs -- TemplateState
#[serde(default, skip_serializing_if = "Option::is_none")]
pub skip_if: Option<BTreeMap<String, serde_json::Value>>,

// src/template/compile.rs -- SourceState
#[serde(default)]
skip_if: Option<HashMap<String, serde_json::Value>>,

// src/engine/types.rs -- EventPayload::Transitioned
Transitioned {
    from: Option<String>,
    to: String,
    condition_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_if_matched: Option<BTreeMap<String, serde_json::Value>>,
},
```

**Advance loop integration (src/engine/advance.rs):**

Insertion point: after line 431 (gate synthesis complete), before line 464
(`resolve_transition()` call):

```rust
// skip_if evaluation block
if let Some(skip_conditions) = &template_state.skip_if {
    // Build synthetic evidence from skip_if conditions
    let synthetic_evidence: BTreeMap<String, serde_json::Value> =
        skip_conditions.clone();

    // Merge with current evidence (skip_if values take precedence for routing)
    let mut merged_for_skip = current_evidence.clone();
    for (k, v) in &synthetic_evidence {
        merged_for_skip.insert(k.clone(), v.clone());
    }

    // Check if skip_if conditions match the available context
    // (variables, gate output already in merged_for_skip via gate synthesis)
    if conditions_satisfied(skip_conditions, &merged_for_skip, &workflow_variables) {
        // Resolve transition using synthetic evidence
        let skip_evidence = serde_json::Value::Object(
            merged_for_skip.into_iter().collect()
        );
        match resolve_transition(&template_state, &skip_evidence, false, &workflow_variables) {
            TransitionResolution::Resolved(target) => {
                if visited.contains(&target) {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::CycleDetected { state: target },
                    });
                }
                persistence.append_event(EventPayload::Transitioned {
                    from: Some(state.clone()),
                    to: target.clone(),
                    condition_type: "skip_if".to_string(),
                    skip_if_matched: Some(skip_conditions.clone()),
                })?;
                visited.insert(target.clone());
                state = target;
                advanced = true;
                transition_count += 1;
                current_evidence = BTreeMap::new();
                continue; // chains to next iteration
            }
            _ => { /* skip_if didn't resolve; fall through to normal flow */ }
        }
    }
}
// ... existing resolve_transition() call at line 464
```

**`has_gates_routing` extension:**

The existing detection at lines 402-410 must also scan `skip_if` keys:

```rust
let has_gates_routing = template_state.transitions.iter().any(|t| {
    t.when.as_ref().map_or(false, |w| {
        w.keys().any(|k| k.starts_with("gates."))
    })
}) || template_state.skip_if.as_ref().map_or(false, |s| {
    s.keys().any(|k| k.starts_with("gates."))
});
```

**`conditions_satisfied` helper:**

A new private function within `advance.rs` that evaluates each `skip_if` key-value
pair against the merged evidence map and template variables, reusing the same
dot-path resolution logic already in `resolve_transition()`:

```rust
fn conditions_satisfied(
    conditions: &BTreeMap<String, serde_json::Value>,
    merged_evidence: &BTreeMap<String, serde_json::Value>,
    variables: &HashMap<String, String>,
) -> bool
```

Returns `true` only if every key-value pair in `conditions` matches the corresponding
value in `merged_evidence` (which by this point includes gate output for `gates.*`
keys, if `has_gates_routing` is true). Template variable checks (`vars.NAME:
{is_set: bool}`) are resolved using the `variables` map with the same logic as
the existing `resolve_transition()` `vars.*` path.

### Data Flow

```
koto next called
  → load events, derive current state, merge epoch evidence
  → advance_until_stop()
      loop:
        1. shutdown / chain-limit / terminal / integration checks
        2. default_action (if any)
        3. gate evaluation → gate_evidence_map
        4. has_gates_routing check (transitions + skip_if)
        5. gate output injected into merged evidence
        [NEW] 6. skip_if evaluation:
                  conditions_satisfied()?
                    yes → resolve_transition(synthetic evidence)
                           → Transitioned(condition_type="skip_if", skip_if_matched)
                           → continue loop (chains)
                    no  → fall through
        7. resolve_transition(agent evidence + gate output)
           → Resolved → Transitioned(condition_type="auto") → continue
           → NeedsEvidence → return EvidenceRequired
           → GateBlocked → return GateBlocked
```

## Implementation Approach

### Phase 1: Template Schema

Add `skip_if` field to `SourceState` and `TemplateState`. Add compile-time
validation rules.

Deliverables:
- `src/template/types.rs`: `skip_if: Option<BTreeMap<String, serde_json::Value>>` on `TemplateState`
- `src/template/compile.rs`: `skip_if: Option<HashMap<String, serde_json::Value>>` on `SourceState`; compile pass for E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS, W-SKIP-GATE-ABSENT
- `src/template/compile.rs` (mod tests): 3 unit tests for compile error cases
- `cargo test --lib` passes

### Phase 2: Event Schema

Extend `EventPayload::Transitioned` with the optional `skip_if_matched` field.
Existing log readers see an unknown field and ignore it (forward-compat via serde
defaults). No changes to state-derivation or evidence-merging logic.

Deliverables:
- `src/engine/types.rs`: `skip_if_matched: Option<BTreeMap<String, serde_json::Value>>` on `Transitioned` variant
- All existing tests pass (the field is `skip_serializing_if = "Option::is_none"`)

### Phase 3: Advance Loop

Add the `conditions_satisfied` helper and the `skip_if` evaluation block. Extend
`has_gates_routing` detection. Wire up the `Transitioned` event emission.

Deliverables:
- `src/engine/advance.rs`: `conditions_satisfied()` helper
- `src/engine/advance.rs`: `skip_if` evaluation block after gate synthesis
- `src/engine/advance.rs`: `has_gates_routing` extended to include `skip_if` keys
- `cargo test` passes

### Phase 4: Fixture Templates and Gherkin Tests

Add fixture templates and the Gherkin feature file covering scenarios 1-3, 5, 7-9.

Deliverables:
- `test/functional/fixtures/templates/skip-if-chain.md`
- `test/functional/fixtures/templates/skip-if-gate.md`
- `test/functional/fixtures/templates/skip-if-vars.md`
- `test/functional/features/skip-if.feature`
- Functional tests pass

### Phase 5: Integration Tests

Add integration tests covering scenarios 2 (chain JSONL assertion), 4 (log format),
6 (cycle detection -- fills existing gap), 11 (chain limit -- fills existing gap).

Deliverables:
- `tests/integration_test.rs`: `skip_if_log_records_condition_type_and_matched_values`
- `tests/integration_test.rs`: `skip_if_consecutive_chain_emits_correct_events`
- `tests/integration_test.rs`: `skip_if_cycle_detection`
- `tests/integration_test.rs`: `skip_if_chain_triggers_limit`
- All integration tests pass

## Security Considerations

`skip_if` evaluation is pure in-memory equality comparison using `serde_json::Value`.
It does not execute template values as code, spawn subprocesses, or make network calls.
Conditions are evaluated against data already in the advance loop (template variables,
gate output, evidence fields). No new permission scope is introduced.

**Matched values are recorded in the event log.** The `skip_if_matched` field on the
`Transitioned` event writes the condition map verbatim to the JSONL state file. The
values in this map come from template constants, template variables (already in
`WorkflowInitialized.variables`), and gate output (already in `GateEvaluated.output`).
No new data categories are introduced; the field is a projection of data that already
exists in the log. Template authors who use specific values (rather than boolean
predicates) in `skip_if` conditions should know those values are preserved in the
event log under `skip_if_matched`.

**Chaining is bounded by existing safety guards.** `MAX_CHAIN_LENGTH = 100` caps
consecutive skip_if transitions in a single `advance_until_stop()` call. The visited-set
cycle detection prevents revisiting a state that was already auto-transitioned through
in the current invocation. Both guards apply to skip_if on the same code paths as all
other transitions; no new execution bounds are needed.

**Compile-time and runtime evaluators must stay aligned.** E-SKIP-AMBIGUOUS validation
at compile time uses the same condition evaluator as the runtime advance loop. Future
changes to `resolve_transition()` must preserve this alignment. A comment in the
compile validation code is sufficient to maintain the invariant.

## Consequences

### Positive

- **Eliminates mechanical driving for deterministic states**: A 9-child plan
  orchestrator drops from 36 manual submissions to 0 for the boilerplate states,
  saving token cost and reducing latency.
- **Resume-awareness is preserved**: The `Transitioned` event with `skip_if_matched`
  gives resuming agents the same reconstruction ability they have today; no states
  disappear from the history.
- **Zero new syntax concepts**: Template authors learn one new field name (`skip_if`);
  the value uses the `when`-clause syntax they already know.
- **No changes to resolve_transition()**: The core routing logic is unchanged. The
  skip_if path is additive, not a rewrite.
- **Fills two pre-existing test gaps**: The cycle-detection and chain-limit integration
  tests address gaps that existed before this feature.

### Negative

- **E-SKIP-AMBIGUOUS adds compile-time complexity**: Validating that exactly one
  conditional transition matches the `skip_if` values requires running the when-clause
  evaluator against synthetic evidence at compile time. Templates with many conditional
  transitions are harder to reason about.
- **Gate output synthesis condition expands**: Extending `has_gates_routing` to
  include `skip_if` keys means gate output is synthesized in more states, which adds
  a small amount of work per loop iteration. The effect is negligible in practice but
  is a latent coupling between skip_if and the gate synthesis path.
- **Context-exists conditions require a gate workaround**: Authors who want to
  skip_if on context key existence must declare a `context-exists` gate and reference
  its output. This is two YAML stanzas instead of one, and the workaround is not
  obvious from the `skip_if` field alone.

### Mitigations

- **E-SKIP-AMBIGUOUS**: The compile error message must identify which transitions
  matched and which values caused the ambiguity. Clear error text makes the problem
  diagnosable without reading the source.
- **Gate output synthesis**: No mitigation needed; the cost is negligible.
- **Context-exists workaround**: Document the workaround pattern with a concrete
  example in the template authoring guide. The v2 design for direct context-key
  predicates can reference this design doc.
