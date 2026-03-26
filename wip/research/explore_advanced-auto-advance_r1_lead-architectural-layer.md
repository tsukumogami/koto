# Lead: Where should auto-advance live in koto's architecture?

## Findings

### Current Architectural Layers

**CLI Layer (cmd/koto -> src/cli/):**
- `handle_next()` in src/cli/mod.rs orchestrates the entire flow
- Handles all I/O, flag parsing, file locking, signal handling
- Contains the "main loop" that coordinates with the engine

**Dispatcher Layer (src/cli/next.rs):**
- Pure classification function `dispatch_next()`
- Takes pre-computed state and returns a NextResponse variant
- NO I/O, NO state mutations
- Maps state properties (terminal, gates, accepts, integration) to response types

**Engine Layer (src/engine/advance.rs):**
- `advance_until_stop()` is the core advancement loop
- Iterates through auto-advanceable states until hitting a stopping condition
- Returns `AdvanceResult` with `final_state`, `advanced` (bool), and `stop_reason`
- Already implements auto-advancement -- chains through states when unconditional transitions exist

### The Semantic Gap

The `advanced: true` response reflects a state the engine auto-advanced to, not the state the caller provided evidence for. When `advanced: true`, the next call to `koto next` dispatches on a different state. Callers must call again to get the directive for the new state.

### Three Options

**Option A: In the Engine (advance_until_stop)**
- Extend loop condition: continue past states with no `accepts` block
- Pros: Library consumers get auto-advance automatically; ALL callers behave the same
- Cons: Engine becomes more opinionated about what constitutes a "stopping state"
- Impact: Small change (5-10 lines in loop condition)

**Option B: In the CLI (handle_next)**
- Add outer loop: call advance_until_stop(); if response.advanced=true && state.accepts=None && !terminal, call again
- Pros: CLI-only optimization; library consumers aren't affected; preserves engine simplicity
- Cons: CLI must re-implement loop logic; diverges behavior between CLI and library users
- Impact: Moderate change (10-20 lines)

**Option C: Keep as Caller Convention**
- Document that callers must loop on advanced=true
- Pros: Preserves observability; simple engine
- Cons: Mechanical burden on all callers; issue #89 remains unresolved

### Architectural Precedent

The engine's `advance_until_stop()` is already a closed-loop orchestrator, not a single-step iterator. It handles all stopping conditions internally. The loop-or-stop decision is entirely in the engine. This suggests the engine layer is already designed as an orchestrator.

### Public API Surface

The engine module is public (`pub mod engine`), but `advance_until_stop` takes I/O closures that make it CLI-coupled in practice. No external library consumers were found in the codebase.

## Implications

The engine is already an orchestrator managing multi-state progression. Extending its loop condition (Option A) is the most coherent choice because:
1. It preserves the existing pattern of "engine decides when to stop"
2. Library consumers (theoretical or future) get the same behavior as CLI
3. The change is small and additive to existing logic

Option B creates a second orchestration layer in the CLI, duplicating logic the engine already owns. Option C is the status quo that #89 wants to change.

## Surprises

1. The engine is already closer to an orchestrator than a pure state machine primitive -- advance_until_stop manages the entire progression loop
2. No external library consumers exist, making the "library consumer story" theoretical
3. The dispatcher (dispatch_next) is a pure classifier that never performs transitions -- it's downstream of the engine, not an alternative to it

## Open Questions

1. If auto-advance goes in the engine, should there be an opt-out for callers who want intermediate visibility?
2. Are there actual external library consumers, or is this theoretical?
3. Should the response include a `transitions` array showing the chain for observability?

## Summary

Auto-advance already lives in `advance_until_stop()`, which loops through workflow states until hitting a stopping condition. The issue is that the loop's stopping conditions don't include "reached a state where the caller has nothing to do." The most coherent fix is extending the engine's loop condition (Option A), since the engine is already an orchestrator and library consumers should get identical behavior to the CLI.
