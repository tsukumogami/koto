# Design Summary: auto-advance-transitions

## Input Context (Phase 0)

**Source:** /explore handoff
**Problem:** Plan-backed orchestrator workflows require agents to drive 4 boilerplate
states per child manually before reaching states with actual decision value. A
`skip_if` predicate would auto-advance deterministic states within a single
`advance_until_stop()` invocation, writing a synthetic Transitioned event for
resume-awareness, with consecutive auto-advancing states chaining naturally.
**Constraints:**
- Orientation on resume must be preserved (states still appear in history)
- Chaining in a single loop turn is required (without it, ~20% of value delivered)
- Synthetic event format: `Transitioned` with `condition_type: "skip_if"` (decided)
- Context-exists conditions deferred to v2; use gate workaround for v1 (decided)
- Transition target: synthetic-evidence injection into resolve_transition() (decided)
- Compile-time validation: exactly one transition must match when skip_if fires (decided)
- has_gates_routing detection must include skip_if gate references (discovered gap)

## Current Status

**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-04-20
