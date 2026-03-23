# Decision 2: Polling/retry execution model

## Question

How should polling/retry execution work for states like ci_monitor?

## Context

Issue #71 adds default action execution to koto. Most deterministic states run a command once (one-shot). But ci_monitor needs to poll repeatedly -- run `gh pr checks` every ~30 seconds until CI passes or a timeout (~30 minutes) is reached.

The advance loop (`src/engine/advance.rs`) currently runs synchronously, checking gates, resolving transitions, and persisting events. Gates (`src/gate.rs`) already support command execution with configurable per-gate timeouts and process-group isolation. The advance loop stops on gate failure (`StopReason::GateBlocked`) and returns immediately -- it has no retry or wait behavior.

## Options evaluated

### Option A: Unified action model with optional polling parameters

ActionDecl gets a `polling: Option<PollingConfig>` field. When `None`, the action runs once. When `Some`, the engine loops: execute command, check gate, sleep interval, repeat until gate passes or timeout.

**Strengths:**
- Single code path for all action execution. Template authors add two fields to get polling behavior -- no new concepts.
- The engine already has a synchronous advance loop with a shutdown flag (`AtomicBool`). A polling sleep loop fits naturally: check `shutdown` between iterations, just like the advance loop checks it between state transitions.
- Gate evaluation already handles command timeouts and process-group cleanup. The polling loop reuses this infrastructure -- each poll iteration is effectively a gate check.
- Template schema stays small. A ci_monitor state's YAML adds `polling: { interval_secs: 30, timeout_secs: 1800 }` alongside its existing action and gate declarations.

**Weaknesses:**
- One-shot and polling share the `execute_action` entry point. If polling grows features (backoff, jitter, partial-success handling), the shared path accumulates conditionals. This is a real risk but manageable -- the branch point is a single `match` on `polling`.

### Option B: Separate execution models

ActionDecl gets an `execution_model` enum discriminator. One-shot and polling have separate execution logic, separate schema fields.

**Strengths:**
- Clean separation. Each model can evolve independently without touching the other.
- Explicit in the template: the author declares intent, not just parameters.

**Weaknesses:**
- More schema surface. Template authors must understand and choose between models. For the current use case (4 one-shot states, 1 polling state), the added complexity isn't justified.
- More code duplication. Both models still spawn commands, capture output, check exit codes. The shared logic is the bulk of the work; only the retry wrapper differs.
- The `execution_model` field is redundant information -- presence of polling parameters already implies the model. Explicit discriminators are valuable when the difference is semantic, but here the difference is purely mechanical (run once vs. run in a loop).

### Option C: Action as one-shot only, polling via gate retry

Actions only support one-shot. For ci_monitor, the gate itself retries on failure.

**Strengths:**
- Simpler action model -- actions always run once.

**Weaknesses:**
- Gates currently have no retry semantics. Adding retry to gates conflates two concerns: "is the condition met?" (gate's job) and "keep checking until it is" (orchestration's job). This is a design smell.
- The advance loop stops on `StopReason::GateBlocked` and returns to the caller. Making gates retry internally means the gate evaluator blocks for up to 30 minutes. This breaks the current contract where gate evaluation is bounded by the per-gate timeout (default 30s).
- Gate retry parameters would be specific to the polling use case but attached to a general-purpose mechanism. Future gate types (e.g., file-exists checks) shouldn't inherit retry semantics.

## Recommendation: Option A

Option A is the right choice for three reasons:

**1. It matches the existing architecture.** The advance loop already has the control-flow patterns polling needs: a synchronous loop, a shutdown flag for graceful interruption, and gate evaluation as a stopping condition. Adding a polling wrapper around the execute-then-check-gate sequence slots into this structure without architectural changes.

**2. The discriminator is the data, not a field.** `polling: None` vs `polling: Some(...)` is the execution model. Adding a separate `execution_model` field (Option B) creates two sources of truth. Option A uses the type system to enforce the invariant: if polling config exists, poll; otherwise, run once.

**3. Gates stay pure predicates.** Option C turns gates from "check a condition" into "check a condition repeatedly until it passes," which muddles their contract. With Option A, gates remain single-shot boolean checks. The polling loop is action-level orchestration that happens to re-evaluate gates between iterations -- a clean layering.

**Implementation sketch for the polling loop:**

```
fn execute_action_with_polling(action, gates, working_dir, shutdown):
    deadline = now() + action.polling.timeout_secs
    loop:
        if shutdown.load(): return SignalReceived
        if now() > deadline: return TimedOut
        run action.command
        gate_results = evaluate_gates(gates, working_dir)
        if all_passed(gate_results): return Passed
        sleep(action.polling.interval_secs)  // interruptible via shutdown
```

This loop lives in the action executor, not in the advance loop. The advance loop calls the action executor, which internally handles polling. The advance loop sees the same interface regardless of execution model.
