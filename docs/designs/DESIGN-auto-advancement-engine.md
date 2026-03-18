# DESIGN: Auto-Advancement Engine

## Status

Proposed

## Upstream Design Reference

Parent: `docs/designs/DESIGN-unified-koto-next.md` (Phase 4: Auto-advancement engine)

Relevant sections: Solution Architecture > Data Flow (advancement loop pseudocode),
Sub-Design Boundaries (scope definition), Security Considerations (signal handling,
integration invocation).

## Context and Problem Statement

`koto next` currently evaluates one state and returns. If the current state has no
`accepts` block, passing gates, and an unconditional transition, the agent gets back
a response that says "you can advance" but doesn't actually advance. The agent must
call `koto next --to <target>` manually for every intermediate state, turning what
should be automatic chaining into a tedious back-and-forth.

The strategic design specifies an advancement loop that chains through states until
hitting a stopping condition (terminal, gate blocked, evidence required, integration,
or cycle). This design covers that loop, plus the integration runner that the
strategic design deferred, signal handling for clean shutdown, and `koto cancel` for
workflow abandonment.

The existing codebase has solid foundations: event types are defined, the gate
evaluator has process group isolation, evidence validation works, and the pure
`dispatch_next` function classifies states correctly. What's missing is the loop
that ties them together.

## Decision Drivers

- **Correctness over speed**: the loop must never corrupt the event log, even on
  SIGTERM mid-chain
- **Pure function preservation**: `dispatch_next` stays pure (no I/O); the loop
  lives in the handler layer
- **Reuse existing infrastructure**: gate evaluator, evidence validation, and
  persistence layer are battle-tested; don't rewrite them
- **Integration runner must degrade gracefully**: a missing or misconfigured
  integration returns `IntegrationUnavailable`, not a crash
- **Cycle detection must be simple**: visited-state set per invocation; no need
  for graph analysis

## Considered Options

### Decision: Where does the advancement loop live?

**Context:** The advancement loop chains through auto-advanceable states, calling
existing I/O functions (gate evaluation, event appending, integration invocation)
between iterations. The question is where to place the loop relative to the existing
`dispatch_next` (pure classifier) and `handle_next` (I/O handler) split.

**Chosen: Engine-Layer Advancement.**

A new `src/engine/advance.rs` module exposes `advance_until_stop()`, which takes
the current state, compiled template, and closures for I/O operations. The engine
owns cycle detection (visited-state `HashSet`), transition resolution (matching
evidence against `when` conditions), and stopping condition evaluation. The handler
sets up the I/O closures and calls the engine, then translates the `StopReason`
result into a `NextResponse` for serialization.

This approach fits the codebase's existing architecture: `src/engine/` already owns
persistence, evidence validation, and type definitions. The advancement loop is
workflow logic, not CLI logic, and belongs alongside those modules. It preserves
`dispatch_next` as a pure function (called within the engine's loop body), makes
the loop testable through injected I/O callbacks without touching the filesystem,
and maps 1:1 to the upstream design's pseudocode.

*Alternative rejected: Handler-Layer Loop.* Adding the loop directly into
`handle_next` is the simplest change (~300-400 lines vs ~600-800) and requires no
new abstractions. It was rejected because `handle_next` is already 340 lines of
inline logic and adding the loop would push it further without structural
improvement. The loop logic isn't testable in isolation -- it requires integration
tests that set up state files and spawn processes. For a correctness-critical loop
that must handle cycle detection, signal interruption, and five stopping conditions,
unit-testable engine logic is worth the extra abstraction.

*Alternative rejected: Action-Yielding State Machine.* An iterator yielding typed
directives (`EvaluateGates`, `AppendTransitioned`, etc.) that the handler executes
in a ping-pong protocol. This maximizes testability -- every step is observable and
assertable. It was rejected because Rust lacks native generators, requiring manual
coroutine state encoding that adds boilerplate and a new class of protocol bugs
(calling the wrong `feed_*` method). The loop has five stopping conditions and
three I/O operations; this level of machinery is more than the problem warrants.
The engine-layer approach captures most of the testability benefit through injected
closures without the protocol complexity.

## Decision Outcome

The auto-advancement engine lives in `src/engine/advance.rs` as a function that
takes I/O callbacks, iterates through states using the existing `dispatch_next`
classifier, and returns a structured `StopReason` when it hits a stopping condition.

Key properties:
- `dispatch_next` stays pure; the engine calls it per-iteration for classification
- I/O operations (gate evaluation, event appending, integration invocation) are
  injected as closures, making the loop unit-testable with mocks
- Cycle detection uses a `HashSet<String>` scoped to the invocation
- Signal handling checks an `AtomicBool` between iterations; the last fsync'd event
  is always durable before the check
- `StopReason` maps to `NextResponse` in the handler, keeping CLI serialization
  concerns out of the engine
- `koto cancel` is a new subcommand that appends a `workflow_cancelled` event
