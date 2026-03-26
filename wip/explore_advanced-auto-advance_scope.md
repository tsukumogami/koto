# Explore Scope: advanced-auto-advance

## Visibility

Public

## Core Question

Issue #89 asks koto to auto-advance past `advanced: true` phases instead of requiring callers to double-call `koto next`. The work-on skill already works around this with an explicit step in its execution loop. We need to understand whether this change fits koto's state machine philosophy and architecture, or whether the double-call pattern serves a purpose that would be lost.

## Context

An agent filed #89 after its first experience using koto through the work-on skill. The skill's execution loop has an explicit workaround: "if `advanced: true`, run `koto next` again." The issue argues this is mechanical overhead with no decision value. The exploration needs to evaluate this against koto's design principles -- state machine integrity, observability, and the separation between engine and CLI concerns.

## In Scope

- The role and semantics of `advanced: true` in koto's engine and templates
- State machine invariants that auto-advance might affect
- How templates currently use `advanced` phases
- Where in the architecture (engine vs CLI) auto-advance belongs
- Observability and audit trail implications

## Out of Scope

- Alternative workflow orchestration tools
- Redesigning the state machine model
- Changes to the work-on skill itself (that's downstream)

## Research Leads

1. **What does `advanced: true` mean in koto's engine, and what happens during an advanced phase transition?**
   Understanding the current implementation is essential before proposing changes. Need to trace the code path for advanced phases vs normal phases.

2. **Is the double-call pattern an intentional design choice or an emergent workaround?**
   Check git history, design docs, and template definitions for evidence of whether advanced phases were designed for agent auto-consumption or for human-visible checkpoints.

3. **What state machine invariants or side-effects would auto-advance need to preserve?**
   The issue mentions "gate logic for advanced phases still executes." Need to verify what side-effects exist and whether collapsing the calls can preserve them.

4. **How do existing koto templates use `advanced` phases, and are there cases where stopping matters?**
   Survey all templates to understand usage patterns. If every consumer immediately re-calls, the pattern is purely mechanical. If any consumer inspects the advanced phase, auto-advance would change behavior.

5. **Should auto-advance live in the engine, the CLI, or the caller convention?**
   Koto separates pkg/engine from cmd/koto. The right layer for this optimization affects API stability and whether library consumers get the same behavior.
