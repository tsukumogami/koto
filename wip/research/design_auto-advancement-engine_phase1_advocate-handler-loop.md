# Advocate: Handler-Layer Loop

## Approach Description

Add the advancement loop directly into `handle_next` in `src/cli/mod.rs` (or a helper function called from it). After the existing evidence-submission and directed-transition handling, the loop replaces the current single-shot `dispatch_next` call with an iterative chain:

1. Build a `HashSet<String>` of visited states.
2. Loop: call `dispatch_next` on the current state. Inspect the result:
   - **Terminal**: break, return the response.
   - **GateBlocked**: break, return the response.
   - **IntegrationUnavailable**: break (runner deferred; when implemented, invoke runner, append `integration_invoked`, break).
   - **EvidenceRequired with non-empty fields**: break (agent must submit evidence).
   - **EvidenceRequired with empty fields** (the fallback auto-advance case): resolve the unconditional transition target, append a `transitioned` event via `append_event` + fsync, update `current_state`, check visited set for cycle, continue loop.
3. Between iterations, check an `AtomicBool` signal flag. If set (SIGTERM/SIGINT received), stop the loop and return the last valid response. The signal handler is registered at the start of `handle_next` using `signal_hook::flag::register`.
4. `koto cancel` is a new subcommand that appends a `workflow_cancelled` event to the state file and exits.

The key design property: `dispatch_next` remains pure. All I/O (gate evaluation, event appending, template lookups) stays in the handler. The loop is a thin orchestrator that calls existing infrastructure.

## Investigation

### What I Read

- `src/cli/mod.rs`: the `handle_next` function (lines 375-696) -- the current single-shot handler
- `src/cli/next.rs`: `dispatch_next` -- the pure classifier (lines 27-112)
- `src/cli/next_types.rs`: `NextResponse` enum and its five variants
- `src/engine/persistence.rs`: `append_event` with fsync, `derive_state_from_log`, `derive_machine_state`
- `src/engine/types.rs`: `EventPayload` enum (already has `Transitioned`, `IntegrationInvoked`)
- `src/engine/evidence.rs`: `validate_evidence`
- `src/gate.rs`: `evaluate_gates` with process group isolation
- `src/template/types.rs`: `TemplateState`, `Transition`, `CompiledTemplate`
- `docs/designs/DESIGN-unified-koto-next.md`: the upstream design pseudocode

### How the Approach Fits

The codebase is already structured to support this approach. The separation between `handle_next` (I/O, side effects) and `dispatch_next` (pure classification) is explicitly documented in comments. The loop would sit in `handle_next`, wrapping the existing call to `dispatch_next` in a `loop {}` block.

**Current flow** (lines 678-695 of `mod.rs`):
```
evaluate gates -> dispatch_next -> print response -> exit
```

**Proposed flow**:
```
loop {
    evaluate gates -> dispatch_next -> match result {
        auto-advance candidate -> resolve target, append_event, continue
        stopping condition -> print response, exit
    }
    check signal flag
}
```

The auto-advance candidate is identifiable today: `dispatch_next` returns `EvidenceRequired` with empty `expects.fields` and empty `expects.options` (lines 100-112 of `next.rs`). The comment at line 101 explicitly says "the caller loop in #49 handles this."

**Transition target resolution** needs a new helper. For a state with no `accepts` block and passing gates, the loop must find which transition to take. The design pseudocode says: "if no accepts and gates pass: append transitioned event." The `TemplateState.transitions` vec contains the candidates. For auto-advance, the loop picks the first unconditional transition (one where `when` is `None`). Template validation already ensures transitions have valid targets.

**Evidence-matched transitions** also need a new helper. When `--with-data` is provided and evidence is submitted, the loop needs to check if any transition's `when` conditions match the submitted evidence. This requires comparing `when` field-value maps against the evidence. This logic doesn't exist yet but is straightforward: iterate transitions, check if all `when` entries match the evidence fields.

**Re-reading state between iterations**: the current code reads the event log once at the top of `handle_next` and derives `machine_state`. In the loop, instead of re-reading the file after each append, the handler can track the current state name in a local variable and look up the next `TemplateState` from the already-loaded `CompiledTemplate`. The event log doesn't need to be re-read because the loop itself is the authority on what was just appended.

## Strengths

- **Minimal architectural change**: the loop lives exactly where I/O already happens. No new modules, no new traits, no new abstractions. The `dispatch_next` function stays pure. The handler's existing pattern (load state, do I/O, call dispatcher, serialize output) extends naturally to iteration.

- **Existing infrastructure reuse**: `append_event` already does fsync. `evaluate_gates` already has process group isolation and timeout handling. `derive_expects` already computes the expects schema. Evidence validation already works. The loop calls these functions -- it doesn't replace them.

- **Straightforward cycle detection**: a `HashSet<String>` initialized at the top of the loop, checked on each iteration. No graph analysis needed. If the current state is already in the set, the loop detected a cycle and stops. This matches the design pseudocode exactly.

- **Signal handling is simple**: `signal_hook::flag::register` sets an `AtomicBool` on SIGTERM/SIGINT. The loop checks `flag.load(Ordering::Relaxed)` between iterations. Since `append_event` does fsync before returning, the last event is durable before the flag is checked. No partial-write risk.

- **Testability of the loop logic**: the loop itself is thin orchestration. The individual components (`dispatch_next`, `evaluate_gates`, `append_event`, evidence matching) are already tested. Integration tests can verify the loop by running `koto next` against a state file with a chain of auto-advance states and verifying the final output + event log contents.

- **Single-process model**: no async runtime, no channels, no message passing. The loop is synchronous, which matches koto's existing execution model. Gate evaluation blocks (with timeout), event appending blocks (with fsync), and the loop continues. This is easy to reason about.

## Weaknesses

- **`handle_next` grows larger**: the function is already ~320 lines. Adding the loop, signal handler registration, transition resolution, and evidence matching will push it toward 400-500 lines. The function does too many things (flag validation, state loading, directed transitions, evidence submission, gate evaluation, dispatching, serialization). The loop makes this worse. Mitigable by extracting the loop into a helper function in the same module.

- **Transition resolution logic is new code**: finding the right transition target (unconditional for auto-advance, conditional for evidence-matched) doesn't exist yet. This is ~30-50 lines of new logic that needs its own tests. It's not complex, but it's the one piece that isn't just calling existing infrastructure.

- **Evidence matching after `--with-data` is trickier than auto-advance**: when evidence is submitted, the loop needs to check if any transition's `when` conditions match. The current code appends `evidence_submitted` and then calls `dispatch_next`, which returns `EvidenceRequired`. With the loop, after appending evidence, the loop should check `when` conditions against the submitted fields. If a match is found, it appends `transitioned` and continues the loop. This means the loop has two "advance" paths: auto-advance (no accepts, no when) and evidence-matched advance (accepts block, when conditions satisfied). Both need to be handled.

- **No re-evaluation of evidence from log**: the loop uses the just-submitted evidence from `--with-data` for matching, but doesn't re-derive evidence from the event log. This is correct for the first iteration but could be wrong in theory if multiple evidence submissions accumulate. In practice, `koto next --with-data` submits once per call, so this is fine -- but the assumption should be documented.

- **Integration runner is a stub**: the design says integration states should invoke a runner and append `integration_invoked`. Currently this is hard-coded to return `IntegrationUnavailable`. When the runner is implemented, it needs to fit into the loop body. The loop approach means the runner call goes inline in the match arm, which works but means the loop body knows about integration execution details.

## Deal-Breaker Risks

None identified. The approach aligns with the existing code structure, preserves the pure/impure separation the codebase already has, and the upstream design pseudocode describes essentially this pattern. The main risk -- `handle_next` growing unwieldy -- is a code quality concern, not a correctness risk, and is mitigable by extracting a helper function. The fsync-per-event guarantee from `append_event` means SIGTERM mid-chain can't corrupt the log; the worst case is stopping one state earlier than expected, which is the defined behavior.

## Implementation Complexity

- **Files to modify**: 2-3
  - `src/cli/mod.rs`: add loop in `handle_next` (or extracted helper), signal handler registration, transition resolution
  - `src/cli/mod.rs` or new subcommand file: add `koto cancel` command (new `Command::Cancel` variant, handler that appends `workflow_cancelled` event)
  - `src/engine/types.rs`: add `WorkflowCancelled` variant to `EventPayload` if `koto cancel` is included

- **New infrastructure**: minimal
  - `signal_hook` crate dependency for `AtomicBool` signal registration
  - Transition resolution function (~30-50 lines): given a `TemplateState` and optional evidence, find the matching transition target
  - `WorkflowCancelled` event payload variant (if `koto cancel` is in scope)

- **Estimated scope**: medium
  - The loop itself is ~40-60 lines
  - Transition resolution + evidence matching is ~50-80 lines
  - Signal handling is ~10 lines
  - `koto cancel` subcommand is ~30-50 lines
  - Tests for transition resolution, loop behavior, and cancel: ~150-200 lines
  - Total new/modified code: ~300-400 lines

## Summary

The handler-layer loop is the natural extension of the existing architecture. The codebase already separates pure state classification (`dispatch_next`) from I/O (`handle_next`), and the loop slots into the I/O layer without disturbing that boundary. All heavy lifting -- gate evaluation, event persistence with fsync, evidence validation -- is delegated to existing, tested infrastructure. The main cost is that `handle_next` grows larger and needs careful extraction into a helper to stay maintainable, but this is a code organization concern rather than an architectural risk.
