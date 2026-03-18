# Design Summary: auto-advancement-engine

## Input Context (Phase 0)
**Source:** Issue #49 (feat(koto): implement auto-advancement engine)
**Problem:** koto next does single-step dispatch; needs a loop that chains through
auto-advanceable states, plus integration runner, signal handling, and koto cancel.
**Constraints:** Must build on existing dispatch_next, gate evaluator, and persistence
layer. Event types already defined. Public repo, tactical scope.

## Current Status
**Phase:** 0 - Setup (Freeform)
**Last Updated:** 2026-03-17
