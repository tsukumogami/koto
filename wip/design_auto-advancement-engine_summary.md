# Design Summary: auto-advancement-engine

## Input Context (Phase 0)
**Source:** Issue #49 (feat(koto): implement auto-advancement engine)
**Problem:** koto next does single-step dispatch; needs a loop that chains through
auto-advanceable states, plus integration runner, signal handling, and koto cancel.
**Constraints:** Must build on existing dispatch_next, gate evaluator, and persistence
layer. Event types already defined. Public repo, tactical scope.

## Approaches Investigated (Phase 1)
- **Handler-Layer Loop**: Inline loop in handle_next; natural extension of existing I/O layer, reuses all infrastructure, risk is growing function size
- **Engine-Layer Advancement**: Extracted advance_until_stop() in src/engine/advance.rs with injected I/O callbacks; clean separation, testable loop, cost is closure ergonomics
- **Action-Yielding State Machine**: Iterator yielding action directives (EvaluateGates, AppendEvent, etc.); maximum testability, cost is manual coroutine encoding and protocol complexity

## Selected Approach (Phase 2)
Engine-Layer Advancement: advance_until_stop() in src/engine/advance.rs with injected
I/O closures. Chosen for testability, reuse potential, and clean separation of workflow
logic from CLI concerns.

## Investigation Findings (Phase 3)
- **Transition resolution**: Evidence matching uses exact JSON equality with conjunction semantics. Evidence from multiple events merged last-write-wins. Unconditional transitions serve as fallback. Compiler doesn't validate unconditional count -- engine must handle defensively.
- **Integration runner**: Types scaffolded but unimplemented. Config system deferred; engine takes a closure. Re-invocation prevention needed (check for existing integration_invoked event in current epoch).
- **Signal handling**: append_event crash safety sufficient (truncated-line recovery). signal-hook crate for AtomicBool. Advisory flock for concurrent access protection. Gate timeout can delay shutdown up to 30s.

## Current Status
**Phase:** 3 - Deep Investigation
**Last Updated:** 2026-03-17
