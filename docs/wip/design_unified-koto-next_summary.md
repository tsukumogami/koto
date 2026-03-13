# Design Summary: unified-koto-next

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-unified-koto-next.md
**Problem (implementation framing):** The engine's gate model, evidence model, and controller
loop all need structural changes to support a single `koto next` command that auto-advances,
accepts evidence submissions, and handles human-directed transitions.

## Selected Approach (Phase 2)

Approach A: Controller-owned advancement loop. The engine stays a single-transition
executor with an extended type model (`TransitionDecl`). The controller gains `Advance()`
with visited-state tracking and stopping-condition evaluation. `IntegrationRunner` is
injected into the controller. No new packages. Chosen over engine-owned (Approach B)
because mixing transaction and orchestration logic in the engine creates long-term
maintainability risk; chosen over new workflow layer (Approach C) because the indirection
isn't justified when the controller already serves as the orchestration layer.

## Investigation Findings (Phase 3)

- **engine-types**: `TransitionDecl` is a new struct replacing `[]string`; `GateDecl` reused as-is;
  evidence clearing is 2-line addition before `persist()`; external caller blast radius minimal
  (one CLI callsite)
- **controller-loop**: No `DelegateChecker` or `IntegrationRunner` pattern exists yet — must be
  designed; `MachineState` has no integration metadata fields; `Directive` is a flat struct with
  no HATEOAS schema; `IntegrationRunner` must be injected at construction time
- **template-format**: Format version field exists (v1 → v2 bump); pipeline clean and well-separated;
  template format design doc already anticipated transition-level gates (deferred to Phase 2);
  cache uses SHA-256 addressing so no special invalidation needed

## Current Status
**Phase:** 4 - Architecture
**Last Updated:** 2026-03-13
