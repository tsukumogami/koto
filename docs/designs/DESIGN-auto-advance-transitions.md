---
status: Proposed
problem: |
  Plan-backed orchestrator workflows contain states whose evidence submissions are
  fully deterministic given the current context -- yet agents must drive each state
  manually via koto next. A 9-child orchestrator run requires 36 mechanical
  round-trips before reaching states where actual decisions happen. The proposed
  fix is a skip_if predicate on template states that auto-advances when conditions
  are met, writes a synthetic Transitioned event so resuming agents know why a
  state was passed, and chains consecutive auto-advancing states within a single
  advance_until_stop() invocation.
---

# DESIGN: Auto-Advance Transitions via skip_if

## Status

Proposed

## Context and Problem Statement

When a koto template has states where the correct next step is fully deterministic
given the current workflow context, agents still drive each state manually. The
visible cost is token consumption and round-trip latency; the less-visible cost
is that templates become harder to author because authors must choose between
orientation (keeping states for resume-awareness) and efficiency (removing them
to eliminate mechanical driving).

Exploration confirmed the problem is real in plan-backed orchestrator workflows
in the shirabe plugin. A 9-child plan requires driving each child through 4
boilerplate states before reaching `analysis`, where actual decision-making
begins. Removing those states makes the template opaque on resume; keeping them
as-is costs 36 mechanical submissions per orchestrator run.

The solution is a `skip_if` predicate on individual states. When the predicate
is satisfied, koto fires the transition automatically, writes a `Transitioned`
event with `condition_type: "skip_if"` and the matched conditions as metadata,
and continues the advance loop to the next state. The state still appears in
history; the agent is never blocked waiting to submit known-constant evidence.

The three motivating cases establish the condition type requirements:

- `plan_context_injection`: auto-advance when `context.md` exists in the context
  store (expressible via a `context-exists` gate + skip_if referencing
  `gates.context_file.exists: true`)
- `plan_validation`: auto-advance when the workflow is in plan-backed mode and
  `verdict: proceed` would always follow (expressible via a template variable check)
- `setup_plan_backed`: auto-advance when `SHARED_BRANCH` is set
  (`vars.SHARED_BRANCH: {is_set: true}`)

## Decision Drivers

- **Orientation on resume must be preserved**: States must still appear in the
  event log. A resuming agent at `analysis` needs to know whether context came
  from a plan outline or a GitHub issue. Silent state collapse is explicitly
  rejected.
- **Chaining is required for the feature to deliver value**: Without consecutive
  auto-advance in a single loop turn, the feature saves evidence composition but
  not round-trips, delivering roughly 20% of the intended benefit.
- **Minimal engine and schema change**: The advance loop already supports
  chaining via implicit continue; cycle detection and chain limits already exist.
  The implementation should reuse these rather than duplicate them.
- **Template syntax must stay composable**: `skip_if` must coexist with `accepts`,
  `gates`, and `transitions` with clear, non-surprising semantics.
- **Public repo**: All documentation must be suitable for external contributors.

## Decisions Already Made

These decisions were settled during exploration and should be treated as constraints
by the design, not reopened without new evidence:

- **Synthetic event format**: Use the existing `Transitioned` event with
  `condition_type: "skip_if"`. Add an optional `skip_if_matched` field carrying
  the condition key-value pairs. Do not add a new event type or modify
  `EvidenceSubmitted`. Rationale: `Transitioned` is semantically correct; the
  `condition_type` discriminator was designed for this extensibility; no changes
  to state-derivation or epoch-scoping logic are needed.

- **Context-exists conditions deferred to v2**: Direct context-key existence
  predicates in `skip_if` (e.g., `context.md: exists: true`) require threading
  `ContextStore` into `advance_until_stop()`. For v1, authors use a `context-exists`
  gate and reference its output in `skip_if` (`gates.NAME.exists: true`).
  Side-effect: the implementation must extend `has_gates_routing` detection to
  include `skip_if` references to `gates.*` keys, not just transition `when` clauses.

- **Transition target selection via synthetic-evidence injection**: When `skip_if`
  fires, inject the condition key-value pairs as synthetic evidence into the merged
  evidence map and call `resolve_transition()` normally. For states with unconditional
  fallbacks, the fallback is selected. For states with conditional transitions,
  the correct path is selected by matching the injected values against `when` clauses.
  Compile-time validation must enforce that exactly one transition is reachable when
  `skip_if` fires.

- **Condition types for v1**: Template variable existence/value (`vars.NAME: {is_set: true}`)
  and gate output references (`gates.NAME.FIELD: value`). Evidence field checks are
  expressible but only meaningful when evidence has been pre-submitted in the current
  epoch -- not the primary use case. No AND/OR composition for v1 (flat conjunction
  only: all conditions must match).
