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

## Research Leads (Round 3: Observability Options)

1. **What would callers actually do with observability metadata from auto-advanced transitions?**
   Before choosing between `passed_through` and `transition_count`, understand the use cases. Would callers log it, display it to users, use it for progress tracking, or use it for debugging? Does the answer differ by caller type (skill, library consumer, human debugger)?

2. **How do `passed_through: Vec<String>` and `transition_count: usize` compare in terms of implementation cost, contract complexity, and future extensibility?**
   Trace through the engine code to understand what data is available at each transition point. Consider what each option costs to produce, serialize, and consume. Consider whether `passed_through` enables future enrichment (per-state metadata, gate results, action outputs).

3. **Is response-level observability even the right mechanism, or should koto lean on its existing event log instead?**
   The event log already records every transition with full detail. Maybe the response shouldn't try to summarize the journey -- maybe callers who want observability should query the event log via `koto query` or `koto status`. Compare the ergonomics of response metadata vs. event log queries for the actual use cases identified in lead 1.
