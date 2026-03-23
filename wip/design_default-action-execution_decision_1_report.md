# Decision: Schema and execution point for default actions

## Chosen

Option B: New `default_action` field on TemplateState, new action closure on `advance_until_stop`.

## Confidence

High (90%).

## Rationale

The advance loop in `advance_until_stop` already handles the full lifecycle of state evaluation: terminal check, integration invocation, gate evaluation, and transition resolution. Actions are a new category of side effect that belongs in this sequence. Inserting action execution between integration and gates (or between gates and transition resolution) keeps the engine as the single authority on what happens when a state is entered.

Key observations from the code:

1. **The advance loop auto-advances through multiple states.** When the engine chains from plan -> implement -> verify, each intermediate state is entered and evaluated within the loop (lines 156-288 of advance.rs). Option C would miss actions on intermediate states because it only runs before the loop starts. This is a disqualifying flaw for the five use cases listed (context_injection and setup actions on auto-advanced-through states would silently not run).

2. **Integration semantics don't fit actions.** The integration closure currently returns `Result<serde_json::Value, IntegrationError>` and always stops the loop (lines 194-225 of advance.rs). Actions need different behavior: they execute for side effects, may or may not stop the loop (depending on `requires_confirmation`), and don't return output that becomes a `StopReason`. Overloading the integration closure (Option A) would conflate two distinct concepts and require the closure to distinguish between "this is a real integration" and "this is a default action" at the call site.

3. **A fourth closure is a clean extension point.** The current signature takes three closures (`append_event`, `evaluate_gates`, `invoke_integration`). Adding a fourth for action execution follows the same injection pattern. The signature change is contained: callers pass one more closure. The engine decides when to call it based on whether `default_action` is present on the current state.

4. **Test breakage is mechanical, not structural.** Every existing test constructs `TemplateState` directly and would need the new `default_action: None` field. This is a find-and-replace change. The new closure parameter in `advance_until_stop` calls requires a no-op closure in existing tests (same pattern as `unavailable_integration` and `noop_gates` already used). This is tedious but not risky.

5. **The ActionDecl struct parallels Gate.** Gates have `command`, `timeout`, and `gate_type`. ActionDecl would have `command`, `working_dir` (optional), and `requires_confirmation`. Both run shell commands via `sh -c` with variable substitution. The action evaluator can reuse the same process-group isolation from `gate.rs`, with the addition of stdout/stderr capture for action output reporting.

Proposed execution order within the loop (after this change):

```
1. Signal check
2. Chain limit check
3. Look up state
4. Terminal check
5. Integration check       (existing - stops loop)
6. Action execution        (NEW - stops loop if requires_confirmation)
7. Gate evaluation         (existing - stops loop on failure)
8. Transition resolution   (existing)
```

Actions run after integrations but before gates. This means a state can't have both an integration and an action (the integration would always preempt), which is a reasonable constraint. Gates still guard transitions, so an action's side effects (e.g., writing a file) can be verified by gates before the engine advances.

A new `StopReason::ActionCompleted` variant would signal when `requires_confirmation` is true, giving the caller a chance to present output and ask for confirmation before re-invoking `koto next`.

## Rejected Alternatives

**Option A (action via integration closure):** The integration closure has return-value semantics (`Result<Value, IntegrationError>`) and always halts the loop. Actions don't fit this contract. Making the closure dual-purpose would require it to inspect template state to decide behavior, pushing engine logic into the CLI layer. The integration concept is about delegating to external systems; actions are about running deterministic setup commands. Mixing them creates a confusing abstraction.

**Option C (action before advance loop in handle_next):** Fatally flawed for the auto-advancement use case. The advance loop chains through multiple states in a single `koto next` call. If states B and C both have default actions and the engine auto-advances A -> B -> C, Option C would only run A's action (or the initial state's action). The five listed use cases include `context_injection` and `setup_issue_backed`, which are likely on states reached via auto-advancement. This option would silently skip their actions.

## Assumptions

1. A state will not have both `integration` and `default_action`. If both are declared, the integration takes precedence (it runs first and stops the loop). This should be enforced at compile-time validation.

2. The `requires_confirmation` flag means the loop stops after action execution, returning a new `StopReason` variant. The CLI presents output and the user re-runs `koto next` to continue. When `requires_confirmation` is false, the action runs and the loop continues to gate evaluation and transition resolution.

3. Action commands use the same `sh -c` execution model and process-group isolation as gates. The existing `evaluate_single_gate` function in `gate.rs` can be factored into a shared `run_shell_command` utility.

4. Variable substitution (`Variables::substitute()`) for action commands happens in the CLI-layer closure, matching how gate commands are substituted in `gate_closure` (line 881 of cli/mod.rs). The engine remains unaware of variable resolution.

5. Action stdout/stderr should be captured and included in the stop reason or advance result, so callers can display it. Gates currently don't capture output; actions likely need to.
