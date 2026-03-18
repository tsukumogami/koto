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

## Current Status
**Phase:** 1 - Approach Discovery
**Last Updated:** 2026-03-17
