# Phase 2 Research: Codebase Analyst

## Lead 1: `advanced` field semantics

### Findings

The `advanced` field is defined in three distinct code paths, each with a different semantic:

**Path A: `advance_until_stop` loop (src/engine/advance.rs:179, 336)**
- Initialized as `false` at line 179
- Set to `true` only at line 336, inside `TransitionResolution::Resolved` -- meaning at least one state transition was actually made during the loop
- Semantic: "did the engine move the workflow to a different state during this invocation?"
- This is the cleanest definition. It's a boolean answering: "did the current state change?"

**Path B: `--to` handler (src/cli/mod.rs:1344)**
- Hard-codes `advanced: true` when calling `dispatch_next(target, target_template_state, true, &gate_results)`
- The `--to` handler has already appended a `DirectedTransition` event and is now dispatching the response for the target state
- Semantic here is overloaded: it means "a transition happened" but it could also mean "you are now looking at a state you weren't at before this invocation"
- Comment at line 1279 says "single-shot, no advancement loop" -- the `true` value is set unconditionally regardless of whether the target state is actually different from the current state (no guard against `--to` pointing at the same state, though transition validation would catch most cases)

**Path C: `dispatch_next` pure classifier (src/cli/next.rs:25-27)**
- The docstring says: "The `advanced` flag is set by the caller (true when an event was appended before dispatching)"
- This is a third definition: "was an event appended before we classified this state?"
- `dispatch_next` is a pure function that just passes through whatever `advanced` value it receives

**The three meanings:**
1. Engine loop: "at least one state transition occurred" (computed)
2. `--to` handler: always `true` (hard-coded)
3. `dispatch_next` docs: "an event was appended" (caller-dependent)

There is no single coherent definition that covers all three. The engine loop definition (Path A) is the most useful for callers because it directly answers "did my workflow state change?" The `--to` hard-coded `true` is defensible since `--to` always changes state. The `dispatch_next` docstring definition is the weakest -- it conflates event recording with state progression.

**Backward compatibility analysis:**
- `advanced` appears in every `NextResponse` variant (6 variants), always as a `bool`
- Renaming the field would break any caller parsing JSON output
- The field is never used internally for control flow after being set -- it's purely for caller consumption
- Replacing it with a more descriptive name (e.g., `state_changed`) would be a breaking change to the JSON wire format
- Adding a new field alongside `advanced` (e.g., `transitions_made: u32`) would be additive and non-breaking

### Implications for Requirements

1. The PRD should define ONE canonical meaning for `advanced`. The engine loop's "at least one transition was made" is the strongest candidate.
2. The `--to` hard-coded `true` is correct under that definition (a directed transition is still a transition).
3. The `dispatch_next` docstring needs updating regardless of which definition wins.
4. If the field is renamed, it's a breaking change requiring a major version bump or deprecation period. Adding a companion field is the safer path.
5. Consider whether callers actually need this field. The `action` field (`execute`/`done`/`confirm`) plus the `state` field already tell callers what to do. `advanced` is supplementary context.

### Open Questions

1. Do any external callers currently branch on `advanced`? If so, which behaviors depend on `true` vs `false`?
2. Should `advanced` be a count (number of transitions) rather than a boolean? The engine tracks `transition_count` internally but doesn't expose it.
3. For `--to` to a terminal state: `advanced=true` is returned, but should it be? The workflow ended immediately after the directed transition.

## Lead 3: Complete response shape catalog

### Findings

There are two distinct output categories: **success responses** (exit 0) and **error responses** (exit 1, 2, or 3). They have completely different JSON shapes.

#### Success Responses (exit 0)

All success responses come from `NextResponse` variants. There are **6 variants** defined in `next_types.rs`:

**1. EvidenceRequired** (action: "execute")
```json
{
  "action": "execute",
  "state": "<string>",
  "directive": "<string>",
  "advanced": <bool>,
  "expects": { "event_type": "evidence_submitted", "fields": {...}, "options?": [...] },
  "error": null
}
```
Produced by: StopReason::EvidenceRequired, StopReason::SignalReceived (with accepts), `dispatch_next` fallback (empty expects)

**2. GateBlocked** (action: "execute")
```json
{
  "action": "execute",
  "state": "<string>",
  "directive": "<string>",
  "advanced": <bool>,
  "expects": null,
  "blocking_conditions": [{ "name": "<string>", "type": "<string>", "status": "failed"|"timed_out"|"error", "agent_actionable": <bool> }],
  "error": null
}
```
Produced by: StopReason::GateBlocked

**3. Integration** (action: "execute")
```json
{
  "action": "execute",
  "state": "<string>",
  "directive": "<string>",
  "advanced": <bool>,
  "expects": <object|null>,
  "integration": { "name": "<string>", "output": <any> },
  "error": null
}
```
Produced by: StopReason::Integration

**4. IntegrationUnavailable** (action: "execute")
```json
{
  "action": "execute",
  "state": "<string>",
  "directive": "<string>",
  "advanced": <bool>,
  "expects": <object|null>,
  "integration": { "name": "<string>", "available": false },
  "error": null
}
```
Produced by: StopReason::IntegrationUnavailable, `dispatch_next` integration branch

**5. Terminal** (action: "done")
```json
{
  "action": "done",
  "state": "<string>",
  "advanced": <bool>,
  "expects": null,
  "error": null
}
```
Note: no `directive` field. Produced by: StopReason::Terminal, StopReason::SignalReceived (when terminal)

**6. ActionRequiresConfirmation** (action: "confirm")
```json
{
  "action": "confirm",
  "state": "<string>",
  "directive": "<string>",
  "advanced": <bool>,
  "action_output": { "command": "<string>", "exit_code": <int>, "stdout": "<string>", "stderr": "<string>" },
  "expects": <object|null>,
  "error": null
}
```
Produced by: StopReason::ActionRequiresConfirmation

#### Error Responses (exit 1 or 2)

Domain errors use `NextError` with a structured format:
```json
{
  "error": {
    "code": "<snake_case_error_code>",
    "message": "<string>",
    "details": [{ "field": "<string>", "reason": "<string>" }]
  }
}
```

**Error codes and exit codes:**
| Code | Exit | Meaning |
|------|------|---------|
| `gate_blocked` | 1 | Transient: gates failed (only from `dispatch_next`, not from the loop path which uses GateBlocked response) |
| `integration_unavailable` | 1 | Transient: integration not configured |
| `invalid_submission` | 2 | Caller error: bad evidence payload, invalid JSON, validation failure, payload too large |
| `precondition_failed` | 2 | Caller error: mutually exclusive flags, no accepts block, invalid --to target, cycle detected, chain limit, unresolvable transition, concurrent lock, reserved variable collision |
| `terminal_state` | 2 | Caller error: workflow already terminal or cancelled |
| `workflow_not_initialized` | 2 | Caller error: no workflow found |

#### Infrastructure Errors (exit 3)

These are unstructured and use an ad-hoc JSON shape:
```json
{
  "error": "<string>",
  "command": "next"
}
```

Produced by: state not found in template, template read failures, template hash mismatch, template parse failure, non-unix platform. Exit code 3 is used via `EXIT_INFRASTRUCTURE` constant.

#### Counting outcomes

Mapping from the advancement loop's `StopReason` variants to response types:

| # | StopReason | Response | Exit |
|---|-----------|----------|------|
| 1 | Terminal | Terminal | 0 |
| 2 | GateBlocked | GateBlocked | 0 |
| 3 | EvidenceRequired | EvidenceRequired | 0 |
| 4 | Integration | Integration | 0 |
| 5 | IntegrationUnavailable | IntegrationUnavailable | 0 |
| 6 | ActionRequiresConfirmation | ActionRequiresConfirmation | 0 |
| 7 | CycleDetected | error (precondition_failed) | 2 |
| 8 | ChainLimitReached | error (precondition_failed) | 2 |
| 9 | SignalReceived + terminal | Terminal | 0 |
| 10 | SignalReceived + accepts | EvidenceRequired | 0 |
| 11 | SignalReceived + no accepts | EvidenceRequired (empty expects) | 0 |
| 12 | UnresolvableTransition | error (precondition_failed) | 2 |

From `AdvanceError`:
| 13 | AmbiguousTransition | error (precondition_failed) | 2 |
| 14 | DeadEndState | error (precondition_failed) | 2 |
| 15 | UnknownState | error (precondition_failed) | 2 |
| 16 | PersistenceError | error (precondition_failed) | 2 |

Pre-loop errors:
| 17 | --with-data + --to mutual exclusion | error (precondition_failed) | 2 |
| 18 | Payload too large | error (invalid_submission) | 2 |
| 19 | Workflow not found | error (workflow_not_initialized) | 2 |
| 20 | No events in state file | error (unstructured) | 1 |
| 21 | Workflow cancelled | error (terminal_state) | 2 |
| 22 | State not in template | error (unstructured) | 3 |
| 23 | Template read failure | error (unstructured) | 3 |
| 24 | Template hash mismatch | error (unstructured) | 3 |
| 25 | Template parse failure | error (unstructured) | 3 |
| 26 | Corrupt state file | error (unstructured) | 1 |
| 27 | Variable re-validation failure | error (unstructured) | 3 |
| 28 | Reserved variable collision | error (precondition_failed) | 2 |
| 29 | Evidence: terminal state | error (terminal_state) | 2 |
| 30 | Evidence: no accepts block | error (precondition_failed) | 2 |
| 31 | Evidence: invalid JSON | error (invalid_submission) | 2 |
| 32 | Evidence: validation failure | error (invalid_submission) | 2 |
| 33 | Evidence: append failure | error (unstructured) | 1 |
| 34 | Lock contention | error (precondition_failed) | 2 |
| 35 | State file read failure (2nd read) | error (varies) | varies |
| 36 | Non-unix platform | error (unstructured) | 3 |

**--to specific errors:**
| 37 | --to: state not in template | error (unstructured) | 3 |
| 38 | --to: invalid transition target | error (precondition_failed) | 2 |
| 39 | --to: target state not in template | error (unstructured) | 3 |
| 40 | --to: event append failure | error (unstructured) | 1 |

The exploration's count of 14 outcomes likely refers to the 10 `StopReason` variants (9 from the enum + the 3 SignalReceived sub-paths) plus the 4 `AdvanceError` variants, which totals 16 if you split SignalReceived. The actual total number of distinguishable exit paths from `handle_next` is much larger (approximately 40 when including all pre-loop validation errors and infrastructure errors).

For the caller-facing contract, the relevant "response shapes" are:
- **6 success response shapes** (the NextResponse variants, exit 0)
- **1 structured error shape** (NextError with code/message/details, exit 1 or 2)
- **1 unstructured error shape** (ad-hoc JSON with `error` string, exit 1 or 3)

### Implications for Requirements

1. The unstructured error shape (`{"error": "<string>", "command": "next"}`) should be standardized. It's inconsistent with the `NextError` format and harder for callers to parse.
2. Exit code 3 (infrastructure) errors have no structured code field, making programmatic handling difficult.
3. Some `StopReason` outcomes (CycleDetected, ChainLimitReached, UnresolvableTransition) produce errors rather than responses, but they include `advanced` and `final_state` information that gets discarded. If the engine made 50 transitions before hitting a cycle, the caller loses that context.
4. The SignalReceived path fabricates an EvidenceRequired response with empty expects for states without an accepts block -- this is misleading since the state doesn't actually accept evidence.
5. `GateBlocked` appears both as a success response (from the advancement loop, exit 0) and as an error code (from `dispatch_next` via `--to`, but only if `--to` were to use gates -- currently it skips gate evaluation). This dual nature is confusing but currently doesn't manifest because `--to` bypasses gates.

### Open Questions

1. Should the unstructured error shape be eliminated in favor of always using `NextError`?
2. Should exit code 3 errors also use the `NextError` format with a new code like `infrastructure_error`?
3. Should CycleDetected/ChainLimitReached return a success response with the partial-progress state, rather than an error? The engine did make progress before stopping.
4. How should callers distinguish "EvidenceRequired because the state needs input" from "EvidenceRequired because we were interrupted by a signal"?

## Lead 4: --to and auto-advancement

### Findings

The `--to` handler is at src/cli/mod.rs:1279-1365. Key observations:

**Why it doesn't chain into the advancement loop:**
- Line 1279 has an explicit comment: "Handle --to (directed transition) -- single-shot, no advancement loop"
- After appending the `DirectedTransition` event, it calls `dispatch_next` (the pure classifier) rather than `advance_until_stop` (the loop engine)
- Gate evaluation is explicitly skipped: `let gate_results = std::collections::BTreeMap::new()` (line 1342) creates empty gates
- It exits immediately with `std::process::exit(0)` after printing the response

**No design note explaining why it doesn't chain:**
- The only rationale is the comment "single-shot, no advancement loop"
- No TODO, no issue reference, no rationale for this decision
- The `dispatch_next` function's docstring references "#49" for the integration runner but says nothing about `--to` chaining

**Test coverage:**
- `dispatch_next` has test coverage in src/cli/next.rs (tests for terminal, gate_blocked, evidence_required, etc.)
- No integration tests specifically verify that `--to` does NOT chain into auto-advancement
- The `advanced: true` hard-coding is tested indirectly by `terminal_state_with_advanced_true` and `evidence_required_advanced_flag_propagates` in next.rs

**What would change if --to fed into the advancement loop:**
1. `--to` currently bypasses gate evaluation entirely. If it fed into the loop, gates on the target state would run. This is a significant behavioral change -- a caller using `--to` to skip past a gate-blocked state would no longer be able to do so.
2. If the target state has unconditional transitions to further states, those would auto-fire. Currently, `--to` lands you exactly on the specified state and stops. Chaining would potentially advance past it.
3. If the target state has a `default_action`, it would execute. Currently it doesn't.
4. The `dispatch_next` classifier doesn't handle `Integration` (only `IntegrationUnavailable`). The advancement loop handles `Integration` properly. So `--to` to an integration state currently always returns `IntegrationUnavailable`, even if a runner were configured.

**Semantic question:** `--to` is explicitly a "directed transition" -- the caller chose the destination. Auto-advancing past that destination would undermine the caller's intent. The current behavior is defensible: the caller said "go to state X" and should get X's status back, not auto-advance through X to Y.

### Implications for Requirements

1. The PRD should explicitly state whether `--to` runs gates on the target state. Currently it doesn't, which is arguably a bug for workflows that depend on gates as safety checks.
2. If `--to` should NOT auto-advance, this should be documented as intentional with a clear rationale (caller chose the destination, so honor it).
3. If `--to` should auto-advance, it needs to switch from `dispatch_next` to `advance_until_stop`, which would require handling all the `StopReason` variants -- a significant change.
4. The `--to` + `default_action` interaction needs specification: if a caller uses `--to` to reach a state with a default action, should that action run? Currently it doesn't.
5. Gate bypass is the most consequential behavior to decide on. If `--to` is meant as an escape hatch (e.g., "skip past this blocked state"), then skipping gates is by design. If `--to` is meant as "navigate to a valid transition target", then gates should run.

### Open Questions

1. Is `--to` intended as an override/escape mechanism or as a navigation shortcut? The answer determines whether gates should be evaluated.
2. Should `--to` to an auto-advanceable state (no accepts, unconditional transition) behave differently than `--to` to a state that needs evidence?
3. If we add chaining to `--to`, should there be a `--to --no-advance` escape hatch for the single-shot behavior?

## Summary

The `advanced` field carries three different meanings depending on the code path: "at least one transition occurred" (engine loop), "always true" (`--to` handler), and "an event was appended" (`dispatch_next` docstring). No single definition covers all paths cleanly, and adding a companion field (like `transitions_made`) is safer than renaming. The actual response shape catalog is larger than the exploration's 14-outcome count -- there are 6 success response shapes, 1 structured error shape, and 1 unstructured error shape, with approximately 40 distinct exit paths when counting all validation and infrastructure errors. The `--to` handler intentionally bypasses both gate evaluation and auto-advancement, which is defensible as "honor the caller's chosen destination" but creates a gap where safety gates and default actions are silently skipped.
