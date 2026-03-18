# Advocate: Engine-Layer Advancement

## Approach Description

Create a new `src/engine/advance.rs` module that encapsulates the advancement loop as a reusable engine-layer construct. The handler (`handle_next` in `src/cli/mod.rs`) calls an `advance_until_stop()` function, passing closures or trait objects for all I/O operations: gate evaluation, event appending, integration invocation, and evidence retrieval. The advancement module owns cycle detection (visited-state set), stopping condition evaluation, and transition resolution (matching evidence against `when` conditions). The handler remains thin -- it performs flag validation, state loading, template verification, optional `--with-data`/`--to` processing, then delegates to the engine for the loop. Signal handling registers an `AtomicBool` flag; the engine checks it between iterations.

In concrete terms, the module would expose something like:

```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub transitions_made: Vec<String>,  // states chained through
    pub stop_reason: StopReason,
}

pub enum StopReason {
    Terminal,
    GateBlocked(BTreeMap<String, GateResult>),
    EvidenceRequired,
    IntegrationStop { name: String, output: Option<serde_json::Value> },
    IntegrationUnavailable { name: String },
    CycleDetected,
    SignalReceived,
}

pub fn advance_until_stop<F, G, I>(
    current_state: &str,
    template: &CompiledTemplate,
    events: &[Event],
    append_event: F,
    evaluate_gates: G,
    invoke_integration: I,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<()>,
    G: Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, GateResult>,
    I: Fn(&str) -> Result<IntegrationOutput, IntegrationError>,
```

The loop body mirrors the design's pseudocode: check visited set, check terminal, check integration, evaluate gates, match evidence against `when` conditions, append `transitioned` event if auto-advancing, continue. Each iteration either stops (returning `AdvanceResult`) or appends one event and continues.

## Investigation

### What I read

1. `src/cli/mod.rs` (lines 357-696): The `handle_next` function is a 340-line monolith. It handles flag validation, state loading, template hash verification, `--to` processing, `--with-data` evidence submission, gate evaluation, and dispatching -- all in one function. There is no advancement loop; it evaluates one state and returns.

2. `src/cli/next.rs`: The pure `dispatch_next` function classifies a single state into a `NextResponse` variant. It takes pre-computed inputs (state name, template state, gate results, advanced flag) and does no I/O. The fallback case (lines 98-112) returns `EvidenceRequired` with empty expects for auto-advance candidates, with a comment: "the caller loop in #49 handles this."

3. `src/cli/next_types.rs`: Defines `NextResponse` (5 variants), `NextError`, `ExpectsSchema`, and serialization. The `Integration` variant exists but is never constructed -- it's waiting for the runner.

4. `src/engine/persistence.rs`: `append_event` does fsync after every write. `derive_state_from_log` and `derive_evidence` replay the event log. These are the I/O operations the advancement loop needs to call.

5. `src/engine/evidence.rs`: Validates evidence payloads against `accepts` schemas. Already handles all field types.

6. `src/gate.rs`: Gate evaluator with process group isolation and configurable timeouts. Already evaluates all gates without short-circuiting. Returns `BTreeMap<String, GateResult>`.

7. `src/engine/types.rs`: `EventPayload::IntegrationInvoked` exists but is never constructed. `EventPayload::Transitioned` has a `condition_type` field that supports "auto", "gate", etc.

8. `src/template/types.rs`: `TemplateState` has `transitions: Vec<Transition>` where each `Transition` has `target: String` and `when: Option<BTreeMap<String, Value>>`. No transition resolution logic exists yet -- matching evidence against `when` conditions is unimplemented.

### How the approach fits

The codebase has a clear separation between pure logic (`dispatch_next`, `validate_evidence`) and I/O (`append_event`, `evaluate_gates`, `read_events`). The engine-layer approach extends this pattern: `advance_until_stop` would be a deterministic loop that calls out to I/O through injected functions. The existing `dispatch_next` can be reused inside the loop body to classify each state, but the loop adds the iteration, cycle detection, and transition resolution on top.

The `src/engine/` directory already contains `persistence.rs`, `evidence.rs`, `types.rs`, and `errors.rs`. Adding `advance.rs` here is a natural fit -- it's engine logic, not CLI logic.

Key missing piece: transition resolution. No code currently matches submitted evidence against `when` conditions to select a target state. This logic must be written regardless of which approach is chosen. Placing it in `src/engine/advance.rs` (or a sibling `src/engine/transition.rs`) keeps it in the engine layer where it's testable without CLI scaffolding.

## Strengths

- **Testability without I/O**: The core loop can be tested by passing mock closures for `append_event`, `evaluate_gates`, and `invoke_integration`. No need to set up state files, spawn shell processes, or hit the filesystem. The existing test pattern (pure `dispatch_next` tests in `src/cli/next.rs`) extends naturally to the loop: feed it a template and mock I/O, assert the chain of states visited and the stop reason.

- **Reusable beyond CLI**: If koto ever gains a library API (`pkg/` is already in the project structure), `advance_until_stop` is immediately usable as a programmatic entry point. The CLI handler becomes a thin adapter between clap args and the engine function. This is consistent with the existing `pkg/` directory structure suggesting a public library was always planned.

- **`dispatch_next` stays pure**: The existing pure function is preserved exactly as-is. The loop calls it per-iteration to classify each state, but the loop itself lives in the engine layer. No changes to the dispatcher's interface or behavior are needed.

- **Cycle detection is trivially correct**: A `HashSet<String>` initialized at the start of `advance_until_stop` and checked on each iteration. The visited set lives on the stack, scoped to one invocation. No global state, no graph analysis, no edge cases.

- **Signal handling is clean**: Register an `AtomicBool` via `signal_hook::flag::register` before calling `advance_until_stop`. The engine checks it at the top of each iteration. If set, it returns `StopReason::SignalReceived` after the in-progress fsync completes. The atomic check is a single load per iteration -- negligible cost, and the signal handler never touches the event log.

- **Matches the design's data flow exactly**: The design document's pseudocode (lines 451-463 of the design) maps 1:1 to the loop body. Each bullet in the pseudocode becomes a branch in the `advance_until_stop` function. There's no impedance mismatch between design intent and code structure.

- **Integration runner has a natural home**: The `invoke_integration` closure parameter defines the contract: given an integration name, return output or error. The engine handles both cases (append `integration_invoked` event on success, return `IntegrationUnavailable` on error) without the CLI layer needing to know about integration mechanics. Graceful degradation (return unavailable, don't crash) is enforced by the closure's return type.

## Weaknesses

- **Closure ergonomics in Rust**: Passing three closures (or trait objects) to `advance_until_stop` is workable but not elegant. The `append_event` closure needs `&mut` access to the state path, `evaluate_gates` needs the working directory, and `invoke_integration` needs configuration. These captures create borrow-checker pressure if the closures close over the same data. A trait-based approach (single `AdvanceIO` trait with three methods) is cleaner but adds a trait definition and an impl block. Either way, the call site in `handle_next` will be 15-20 lines of closure setup.

- **Re-reading events after each append**: The current `append_event` function appends to the file then syncs. But the loop needs to know the updated evidence set after each transition. Either the loop re-reads the event log after each append (expensive for long logs), or it maintains an in-memory event list alongside the file. Maintaining the in-memory list is correct but means the engine must track state that's also on disk -- a minor duplication of truth. The existing `derive_evidence` function operates on `&[Event]`, so passing the in-memory list is straightforward, but the caller must remember to push the new event into it after each append.

- **`handle_next` refactor is nontrivial**: The current `handle_next` is 340 lines of inline logic. Extracting the loop into the engine means also extracting or refactoring the state loading, template verification, and `--to`/`--with-data` processing that currently precede it. The handler will still need all that setup code, but the flow changes from "dispatch once, return" to "set up, call engine loop, translate result to response." This is a medium-size refactor of a function that's the most complex in the codebase.

- **Two classification systems**: `dispatch_next` classifies a single state into a `NextResponse`. The engine loop's `StopReason` also classifies why advancement stopped. These overlap but aren't identical (e.g., `StopReason::CycleDetected` has no `NextResponse` counterpart). The handler must translate `AdvanceResult` into `NextResponse`, which introduces a mapping layer. The mapping is straightforward but is another place where semantics could diverge.

- **New dependency for signal handling**: Using `signal_hook` (or similar) for `AtomicBool`-based signal registration adds a crate dependency. The project currently has minimal dependencies. Alternatively, raw `libc::signal` works but is `unsafe` and less portable. The gate evaluator already uses `libc` for `setpgid`/`killpg`, so this isn't entirely new territory.

## Deal-Breaker Risks

- **None identified**: The approach aligns with the codebase's existing separation between pure logic and I/O. The engine layer already exists (`src/engine/`), the I/O functions are already factored out (`append_event`, `evaluate_gates`), and the pure dispatcher is already isolated. The main implementation risk is the `handle_next` refactor, but that refactor is required by any approach -- the 340-line function must be restructured to add a loop regardless of where the loop lives. The closure/trait ergonomics concern is a code style issue, not a correctness risk. The in-memory event list concern is a performance optimization decision, not an architectural problem.

## Implementation Complexity

- **Files to modify**: 3-4
  - `src/engine/mod.rs`: add `pub mod advance;`
  - `src/engine/advance.rs`: new file, ~200-300 lines (loop, cycle detection, transition resolution, stop reason types)
  - `src/cli/mod.rs`: refactor `handle_next` to call `advance_until_stop` instead of single-shot dispatch (~100 lines changed)
  - `src/cli/next.rs`: possibly add a mapping from `AdvanceResult`/`StopReason` to `NextResponse`, or this can live in `handle_next`

- **New files**: 1 (`src/engine/advance.rs`)

- **New infrastructure**: Transition resolution logic (matching evidence against `when` conditions) must be written. This is ~50-80 lines of code and is needed regardless of approach. Signal handling adds either a `signal_hook` dependency or ~15 lines of unsafe `libc` code.

- **Estimated scope**: Medium. The core loop is ~100 lines. Transition resolution is ~60 lines. The `handle_next` refactor is ~100 lines of changes. Tests are the bulk: ~300-400 lines for loop behavior, cycle detection, signal handling, and transition resolution. Total new/changed code: ~600-800 lines.

## Summary

Engine-layer advancement is a natural extension of the codebase's existing architecture. The `src/engine/` package already owns persistence, evidence validation, and type definitions; the advancement loop belongs there because it's workflow logic, not CLI logic. The approach preserves `dispatch_next` as a pure function, makes the loop testable through injected I/O callbacks, and maps cleanly to the design's pseudocode. Its main costs are closure ergonomics at the call site and a two-step translation from `StopReason` to `NextResponse` -- real but manageable trade-offs that don't threaten correctness.
