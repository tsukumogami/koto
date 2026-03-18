# Advocate: Action-Yielding State Machine

## Approach Description

The advancement engine is modeled as an iterator that yields typed action directives. The engine holds internal state (visited set, current state name, last classification result) and exposes a `step()` method. Each call to `step()` returns one of several directives:

```rust
enum EngineAction {
    EvaluateGates { state: String, gates: BTreeMap<String, Gate> },
    MatchEvidence { state: String, transitions: Vec<Transition>, evidence: Vec<Event> },
    InvokeIntegration { state: String, integration_name: String },
    AppendTransitioned { from: String, to: String, condition_type: String },
    Stop(NextResponse),
}
```

The handler (in `handle_next`) drives the engine in a loop:

```rust
let mut engine = AdvancementEngine::new(current_state, &compiled, &events);
loop {
    match engine.step() {
        EngineAction::EvaluateGates { gates, .. } => {
            let results = evaluate_gates(&gates, &working_dir);
            engine.feed_gate_results(results);
        }
        EngineAction::MatchEvidence { transitions, evidence, .. } => {
            let matched = match_evidence_to_transition(&transitions, &evidence);
            engine.feed_evidence_match(matched);
        }
        EngineAction::InvokeIntegration { integration_name, .. } => {
            let result = run_integration(&integration_name);
            engine.feed_integration_result(result);
        }
        EngineAction::AppendTransitioned { from, to, condition_type } => {
            append_event(&state_path, &EventPayload::Transitioned { .. }, &now_iso8601())?;
            engine.confirm_transition();
        }
        EngineAction::Stop(response) => {
            return response;
        }
    }
}
```

The engine never performs I/O. It requests I/O by yielding a directive, then receives the result through a typed `feed_*` method. This is a ping-pong protocol: engine yields, handler executes, handler feeds back, engine yields next.

## Investigation

### What exists today

The codebase already has a clean separation between pure classification and I/O:

- **`dispatch_next`** (`src/cli/next.rs`): Pure function that classifies a single state into a `NextResponse` or `NextError`. Takes `(state_name, template_state, advanced, gate_results)` -- no file access, no mutation. This function correctly handles the five classification branches: terminal, gate-blocked, integration-unavailable, evidence-required, and the fallback auto-advance candidate.

- **`handle_next`** (`src/cli/mod.rs`): The I/O-performing handler. Currently executes a single pass: load state, optionally handle `--to` or `--with-data`, evaluate gates, call `dispatch_next`, print result. No loop exists.

- **`evaluate_gates`** (`src/gate.rs`): Spawns shell commands in process-group isolation with configurable timeouts. Already designed to be called as a standalone operation and return structured results.

- **`validate_evidence`** (`src/engine/evidence.rs`): Validates evidence payloads against accepts schemas. Collects all errors without short-circuiting.

- **`append_event` / `derive_state_from_log` / `derive_evidence`** (`src/engine/persistence.rs`): Event log append with fsync, state derivation by replay, and epoch-boundary evidence scoping. All are standalone functions.

- **Event types** (`src/engine/types.rs`): `IntegrationInvoked` event type already exists but is never produced -- the integration runner hasn't been built.

### What the loop needs to do

Per the design doc's pseudocode, the advancement loop must:

1. Maintain a visited-state set for cycle detection
2. At each state: check terminal, check integration, evaluate gates, match evidence against `when` conditions
3. On a match or auto-advance: append a `transitioned` event with fsync, then continue the loop
4. Stop on: terminal, gate-blocked, integration, evidence-required-but-no-match, cycle

### How the action-yielding approach maps

The existing `dispatch_next` classifies a single state snapshot. The action-yielding engine wraps this in a loop and breaks the "classify then act" into explicit steps:

1. Engine enters a new state, checks the visited set. If cycle, yields `Stop(CycleDetected)`.
2. Engine checks if terminal. If so, yields `Stop(Terminal)`.
3. Engine checks if integration is configured. If so, yields `InvokeIntegration`.
4. Engine checks if gates exist. If so, yields `EvaluateGates`.
5. After receiving gate results: if any blocked, yields `Stop(GateBlocked)`.
6. Engine checks if accepts block exists. If so, yields `MatchEvidence`.
7. After receiving match result: if no match, yields `Stop(EvidenceRequired)`. If match, yields `AppendTransitioned`.
8. After append confirmation, updates internal state and goes to step 1.

The `dispatch_next` function could be refactored to become the engine's internal classification logic, or it could be kept as-is and called within the engine's `step()`. Since `dispatch_next` doesn't do I/O, it fits naturally inside the engine.

### Evidence matching -- the missing piece

There is currently no function that matches submitted evidence against transition `when` conditions. This needs to be built regardless of which approach is chosen. In the action-yielding model, the engine yields `MatchEvidence` with the transitions and evidence, and the handler (or a pure helper) performs the match and feeds back the result. The match itself is pure logic (compare JSON values), so it could live inside the engine too -- but the approach keeps the option open for the handler to do additional work (like re-reading evidence from the log after an append).

## Strengths

- **Every step is independently testable**: Each `EngineAction` variant can be tested by constructing an engine, calling `step()`, asserting the yielded action, feeding a canned result, and asserting the next yield. No filesystem, no process spawning, no timing dependencies. The engine's entire state machine can be verified through unit tests that run in microseconds.

- **I/O ordering is explicit and auditable**: The handler sees exactly what I/O the engine wants in what order. There's no hidden state mutation inside the engine -- every side effect is visible as a yielded action. This makes it straightforward to verify that fsync happens before the loop continues, that gates are evaluated before evidence matching, and that integration invocation happens at the right point.

- **Signal handling integrates naturally**: The handler's loop is the natural place to check for signals. Between any two `engine.step()` calls, the handler can check a signal flag and break. Since each `AppendTransitioned` action completes atomically (append + fsync) before the next `step()`, a SIGTERM between iterations leaves the log at the last valid event -- exactly what the design requires.

- **`dispatch_next` stays untouched or minimally changed**: The existing pure function can be reused as the engine's internal classifier. No regression risk on the classification logic that already has thorough tests.

- **Integration runner slots in cleanly**: `InvokeIntegration` is just another action variant. The handler can implement it as a subprocess call, a plugin invocation, or a stub that returns `IntegrationUnavailable`. The engine doesn't need to know which -- it just receives the result.

- **Composable with future features**: If koto later needs to support parallel gate evaluation, async integration invocation, or progress reporting, the handler can implement those without changing the engine. The engine's contract is "tell me what to do next"; how the handler executes it is unconstrained.

## Weaknesses

- **Protocol complexity between engine and handler**: The ping-pong protocol requires typed `feed_*` methods for each action variant. If the handler calls the wrong `feed_*` method (e.g., `feed_gate_results` when the engine expects `feed_evidence_match`), the engine must detect and handle the protocol violation. This is a new class of bug that a simpler loop-with-callbacks approach wouldn't have.

- **More types and boilerplate**: The `EngineAction` enum, the `feed_*` methods, and the engine's internal state enum add code surface. For a loop with five stopping conditions, this may be over-engineered. A direct loop in the handler that calls existing functions inline would be shorter and arguably clearer.

- **Rust doesn't have native coroutines**: Unlike languages with yield/generators (Python, C#, Kotlin), Rust requires manually encoding the coroutine state machine. The engine must track "where am I in the step sequence" with an explicit state enum. This is mechanical but tedious, and the state enum can drift from the actual logic if not carefully maintained. Rust's `async`/`await` or `gen` blocks (unstable) could help but add their own complexity.

- **Evidence matching could be pure anyway**: The `MatchEvidence` action asks the handler to match evidence, but the matching logic is purely computational (compare JSON scalars). There's no reason it can't live inside the engine, which would eliminate one action variant and one `feed_*` method. The action-yielding model forces this to be external even when it doesn't need to be.

- **Testing the handler is harder**: While the engine is easy to test in isolation, the handler's loop that drives the engine still needs integration tests. The handler must correctly sequence `step()` and `feed_*` calls, handle errors from each I/O operation, and manage signal checking. These integration tests are roughly as complex as testing a direct loop.

## Deal-Breaker Risks

None identified. The approach's weaknesses are real costs -- more boilerplate, more types, manual coroutine encoding -- but none prevent correct implementation. The existing infrastructure (gate evaluator, evidence validator, persistence layer, event types) all function as standalone operations that map directly to action variants. The ping-pong protocol adds complexity but is a well-understood pattern. Rust's lack of native generators makes the implementation more verbose than it would be in other languages, but explicit state machines are idiomatic Rust and widely used (every `Future` is one).

The one risk worth monitoring is that the protocol between engine and handler must be kept in sync -- calling the wrong `feed_*` at the wrong time is a logic error. This can be mitigated by making the engine's internal state private and having `feed_*` methods return errors when called out of sequence, or by using a typestate pattern where the engine's type changes after each yield (though this adds even more complexity).

## Implementation Complexity

- **Files to modify**: 3-4 existing files (`src/cli/mod.rs` for handler loop, `src/cli/next.rs` to either reuse or refactor `dispatch_next`, `src/cli/next_types.rs` for new response variants if needed, `src/engine/persistence.rs` for evidence-to-transition matching)
- **New files**: 1-2 (`src/engine/advancement.rs` for the engine struct and `EngineAction` enum, possibly `src/engine/evidence_match.rs` for transition matching logic)
- **New infrastructure**: Yes -- the `AdvancementEngine` struct, `EngineAction` enum, and the `feed_*` protocol. This is roughly 200-400 lines of new code for the engine, plus the evidence-to-transition matching that any approach needs.
- **Estimated scope**: Medium. The engine itself is moderately complex but well-bounded. The evidence matching logic and integration runner are required regardless of approach and represent the bulk of new functionality.

## Summary

The action-yielding state machine preserves `dispatch_next`'s purity guarantee while adding the advancement loop as a controlled iteration protocol. Its main advantage is testability: every step the engine takes is observable and assertable without I/O. Its main cost is the manual coroutine encoding in Rust and the protocol complexity between engine and handler. For a project that values correctness over speed and already has strong pure/impure separation, the pattern is a natural extension of existing architecture -- but it may be more machinery than the problem requires, given that the loop has only five stopping conditions and the I/O operations are already well-isolated functions.
