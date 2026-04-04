# Lead: How does the advance loop behave when a state's children are pending?

## Findings

### Current advance loop structure

The advance loop lives in `src/engine/advance.rs` (function `advance_until_stop`, line 166). It iterates through states checking stop conditions in this order:

1. **Signal received** (line 199) -- SIGTERM/SIGINT, returns `StopReason::SignalReceived`
2. **Chain limit** (line 208) -- 100-transition safety cap, returns `StopReason::ChainLimitReached`
3. **Terminal state** (line 226) -- `template_state.terminal == true`, returns `StopReason::Terminal`
4. **Integration** (line 235) -- state declares an integration, invokes the runner closure, returns `StopReason::Integration` or `StopReason::IntegrationUnavailable`
5. **Action execution** (line 269) -- state has `default_action`, runs it; may return `StopReason::ActionRequiresConfirmation`
6. **Gate evaluation** (line 298) -- evaluates all gates, emits `GateEvaluated` events, returns `StopReason::GateBlocked` if any fail (unless state has `accepts` or gates-routing transitions)
7. **Transition resolution** (line 422) -- matches evidence against `when` conditions, returns `StopReason::EvidenceRequired`, `StopReason::UnresolvableTransition`, or advances to the next state

The full `StopReason` enum (line 53) has 10 variants: `Terminal`, `GateBlocked`, `EvidenceRequired`, `Integration`, `IntegrationUnavailable`, `CycleDetected`, `ChainLimitReached`, `ActionRequiresConfirmation`, `SignalReceived`, `UnresolvableTransition`.

### How StopReason maps to JSON responses

The handler in `src/cli/mod.rs` (line 1800) maps each `StopReason` to a `NextResponse` variant defined in `src/cli/next_types.rs` (line 25). The key mapping:

| StopReason | action value | Key response fields |
|------------|-------------|---------------------|
| Terminal | `"done"` | state, advanced |
| GateBlocked | `"gate_blocked"` | state, directive, blocking_conditions |
| EvidenceRequired | `"evidence_required"` | state, directive, expects, blocking_conditions |
| Integration | `"integration"` | state, directive, integration (name + output) |
| IntegrationUnavailable | `"integration_unavailable"` | state, directive, integration (name, available:false) |
| ActionRequiresConfirmation | `"confirm"` | state, directive, action_output |
| CycleDetected | error (exit 3) | error.code = template_error |
| ChainLimitReached | error (exit 3) | error.code = template_error |
| SignalReceived | depends on state | reuses Terminal or EvidenceRequired |
| UnresolvableTransition | error (exit 3) | error.code = template_error |

### BlockingCondition structure

`BlockingCondition` (next_types.rs line 354) has: `name`, `type` (gate type string), `status` ("failed"/"timed_out"/"error"), `agent_actionable` (bool), `output` (JSON value). These are built from gate results via `blocking_conditions_from_gates` (line 405).

### Approach A: Children as a gate type (`children-complete`)

**Where it plugs in:** A new gate type `children-complete` would be added to the gate evaluator system (`src/gate/`). The template author would declare it in a state's `gates` block:

```yaml
gates:
  children-done:
    type: children-complete
    # no command -- the evaluator queries child workflow states
```

The advance loop would not change at all. At step 6 (line 298), `evaluate_gates` is called with the state's gate definitions. The gate evaluator closure (injected by the handler) would recognize `type: children-complete`, query child workflow state files, and return a `StructuredGateResult` with `Passed` or `Failed`. If children are still running, the gate fails, and the existing `GateBlocked` or `EvidenceRequired` (with `blocking_conditions`) flow handles it.

**Response JSON:**

```json
{
  "action": "gate_blocked",
  "state": "fan-out",
  "directive": "Wait for child workflows to complete",
  "advanced": true,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "children-done",
      "type": "children-complete",
      "status": "failed",
      "agent_actionable": false,
      "output": {
        "pending": ["child-a", "child-c"],
        "completed": ["child-b"],
        "failed": []
      }
    }
  ],
  "error": null
}
```

**Agent behavior:** The agent sees `action: "gate_blocked"` with a blocking condition of type `children-complete`. It knows to poll (call `koto next` again later) or take action on children. The structured `output` field tells it which children are pending. This uses existing agent patterns -- agents already know how to handle `gate_blocked`.

**Gates-routing variant:** If the state also has `accepts` and transitions with `when` clauses referencing `gates.children-done.*`, the gate output flows into the transition resolver (line 446). This means the parent could route differently based on child outcomes:

```yaml
transitions:
  - target: all-passed
    when:
      gates.children-done.failed: []
  - target: handle-failures
    when:
      gates.children-done.has_failures: true
```

**Pros:**
- Zero changes to the advance loop, StopReason enum, or NextResponse enum
- Reuses existing `blocking_conditions` response shape -- agents already understand it
- Gate output routing (`gates.*` when-clauses) works out of the box for child-outcome-dependent branching
- Gate overrides (`koto overrides record`) could let an agent skip waiting for a stuck child
- `agent_actionable` flag can signal whether the agent can override

**Cons:**
- Polling semantics are implicit: a `gate_blocked` response doesn't tell the agent "retry later" vs "you need to fix something." The agent must infer from the gate type that this is a temporal condition.
- Child status reporting is limited to what fits in the gate `output` JSON blob. There's no dedicated structure for child workflow summaries.

### Approach B: Children as a new StopReason (`ChildrenPending`)

**Where it plugs in:** A new check would be inserted between steps 3 and 4 (between terminal check and integration check), roughly at line 234:

```rust
// 3.5 Children pending
if has_pending_children(&state, child_registry) {
    let child_statuses = query_child_statuses(&state, child_registry);
    return Ok(AdvanceResult {
        final_state: state,
        advanced,
        stop_reason: StopReason::ChildrenPending {
            children: child_statuses,
        },
    });
}
```

This requires:
- A new `StopReason::ChildrenPending { children: Vec<ChildStatus> }` variant
- A new `NextResponse::ChildrenPending` variant with `action: "children_pending"`
- A new parameter to `advance_until_stop` (a child registry or query closure)
- Changes to every exhaustive match on `StopReason` and `NextResponse` across the codebase

**Response JSON:**

```json
{
  "action": "children_pending",
  "state": "fan-out",
  "directive": "Wait for child workflows to complete",
  "advanced": true,
  "children": [
    {"name": "child-a", "state": "implement", "status": "running"},
    {"name": "child-b", "state": "done", "status": "completed"},
    {"name": "child-c", "state": "review", "status": "running"}
  ],
  "expects": null,
  "error": null
}
```

**Agent behavior:** The agent sees a dedicated `action: "children_pending"` and knows immediately this is a fan-out wait. The `children` array gives first-class status reporting without parsing gate output.

**Pros:**
- Semantically precise: children-pending is clearly distinct from "a gate check failed"
- First-class response shape for child status reporting
- The agent doesn't need to learn that `children-complete` is a temporal gate vs a blocking gate

**Cons:**
- Requires changes in 4+ files: `advance.rs` (StopReason enum + loop), `next_types.rs` (NextResponse enum + serialization), `mod.rs` (handler mapping), plus any test files
- Every exhaustive match on StopReason and NextResponse must be updated
- The advance loop function signature already takes 9 parameters; adding a child query closure makes it 10
- Does not reuse gate infrastructure (overrides, routing, evidence injection)
- Gate-based routing on child outcomes would need separate implementation -- can't use `gates.*` when-clauses
- The agent needs to learn a new action value and response shape

### How gates interact with evidence today (relevant context)

When gates fail on a state that has an `accepts` block, the engine does NOT immediately return `GateBlocked`. Instead, it falls through to transition resolution with `gates_failed = true` (line 409). This prevents the unconditional fallback transition from firing, forcing the agent to submit evidence. The response becomes `EvidenceRequired` with `blocking_conditions` populated.

This existing pattern is directly reusable for children: a `children-complete` gate that fails on a state with `accepts` would naturally trigger the evidence-fallback path, letting the agent submit override evidence (e.g., "skip child-c, it's stuck") while still seeing the blocking conditions.

A `ChildrenPending` StopReason would need to reinvent this interaction. Either it would bypass the gate+evidence interplay entirely, or it would need its own parallel mechanism for "children are pending but the agent can provide evidence to unblock."

## Implications

The gate approach is strongly favored by the current architecture. The advance loop was designed around composable stop conditions, and gates are the extensible mechanism for "check a condition before proceeding." Adding a new gate type is a leaf change (gate evaluator only). Adding a new StopReason is a cross-cutting change that touches the engine, CLI types, handler, and serialization.

The most important architectural insight: gate output routing (`gates.*` when-clauses) already solves the "route differently based on child outcomes" problem. A `children-complete` gate whose output includes structured child status naturally feeds into the transition resolver without any new code in the advance loop.

The `agent_actionable` field on `BlockingCondition` and the override mechanism (`koto overrides record`) provide a ready-made escape hatch for stuck children. An agent could override a `children-complete` gate with synthetic "all passed" output after deciding a child is irrecoverable.

## Surprises

1. **The advance loop already has a clean extensibility boundary at the gate evaluator closure.** The `evaluate_gates` parameter (line 179) is a closure injected by the caller, so adding child-aware gate evaluation requires zero changes to `advance_until_stop` itself. The gate evaluator closure in the handler (`src/cli/mod.rs`) is where child state queries would be wired in.

2. **Gate output already flows into transition routing for structured-mode states.** The `gates.*` when-clause mechanism (lines 391-399, 446-451) means child workflow status can influence parent transitions without any new routing infrastructure. This is a significant capability that a dedicated `ChildrenPending` StopReason would not get for free.

3. **The dispatch_next function in `src/cli/next.rs` is a legacy classifier** that predates the advance loop. The handler in `mod.rs` (line 1800) now maps `StopReason` directly to `NextResponse`, making `dispatch_next` partially redundant. This means a new gate type needs no changes to `dispatch_next` -- it flows through the existing `StopReason::GateBlocked` path in the handler.

## Open Questions

1. **Polling semantics:** When a `children-complete` gate fails, how does the agent know to retry later vs take corrective action? Should the gate output include a `retry_after` hint, or should `BlockingCondition` grow a `temporal: bool` field to distinguish "wait and retry" from "fix and retry"?

2. **Child discovery:** How does the gate evaluator know which children belong to a parent state? Options: (a) the gate declaration lists child workflow names explicitly, (b) children are discovered from a parent-child index maintained by `koto init --parent`, (c) a naming convention. This is more about the data model than the advance loop, but it determines what the gate evaluator closure receives.

3. **Partial completion routing:** When some children complete and others are pending, should the parent be able to act on partial results? Gate output routing supports this (route on `gates.children-done.completed` containing specific names), but the UX for template authors needs design.

4. **Cycle risk with child re-evaluation:** If the parent loops back to a fan-out state (e.g., after a review rejection), should child workflows be re-initialized, or does the existing child state carry over? This isn't about the advance loop per se, but the gate evaluator needs to know whether to look at live children or historical records.

## Summary

The advance loop's gate evaluation system (step 6, injected via closure) is the natural extension point for child-pending checks -- a `children-complete` gate type requires zero changes to the advance loop, reuses `blocking_conditions` in the response, and gets gate output routing (`gates.*` when-clauses) plus override support for free. A dedicated `ChildrenPending` StopReason would be a cross-cutting change across 4+ files that reimplements capabilities the gate system already provides. The biggest open question is polling semantics: how the agent distinguishes a temporal gate ("retry later") from a corrective gate ("fix something"), which may warrant a small addition to `BlockingCondition` rather than a whole new response type.
