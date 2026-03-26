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

## Research Leads (Round 2: Semantic Question)

1. **What information does `advanced` actually carry for callers, and what happens to it after auto-advance?**
   If auto-advance eliminates most cases where `advanced: true` is returned, does the field become vestigial? Trace every place the field is set and consumed to understand its full role.

2. **Does the distinction between "I caused this transition" vs "the engine caused it" matter for any real caller scenario?**
   The design-intent agent proposed disambiguating with `advanced_by: "agent" | "engine"`. Investigate whether any caller logic (current or foreseeable) would branch on this distinction.

3. **What should the `koto next` response contract look like after auto-advance is implemented?**
   The current contract was designed pre-auto-advancement. If the behavioral fix goes in, should the response be redesigned? Look at what callers actually need: current state, whether input is required, what happened along the way.
