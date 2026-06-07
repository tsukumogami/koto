# Decision 4: Converge point surfacing via GateBlocked; converge-set definition

Executed INLINE.

## Question
How does the parent's converge point surface and inline children's results
through the existing GateBlocked directive, and how is the converge set
defined and reported while blocked?

## Options
- **4A — Reuse the children-complete gate + GateBlocked directive.** The
  converge point IS the existing `children-complete` gate on a
  `materialize_children` parent state. The gate's structured output gains a
  per-child `result` field; while any child in the set lacks a result the gate
  is non-passing and `koto next` returns `GateBlocked` with the outstanding
  children named; when all results are present the gate passes and the cleared
  directive carries every child's inlined result.
- **4B — New dedicated converge gate type + new NextResponse variant.** Add a
  `converge` gate type and a `Converged`/`ConvergeBlocked` response shape.

## Chosen: 4A — reuse children-complete gate + GateBlocked

The converge set is already defined by koto: it is the set of the parent's
linked children that the `children-complete` gate enumerates via
`backend.list()` filtered on `parent_workflow == parent` (plus the
`ChildCompleted` fallback for cleaned-up children). This is exactly "the same
children the parent discovers as needing an agent and dispatches" (PRD R5) — no
new set abstraction.

`build_children_complete_output` already produces a structured JSON `output`
with a `children` array and `all_complete` / `pending` counts. The design
extends each child entry in that array with a `result` field populated from the
dereferenced `WorkflowResult` (Decision 3): the child's `request_store.result`
event when the session is live, or the `result` carried on the parent's
`ChildCompleted` event when the child was cleaned up. The gate's pass predicate
is tightened so the gate is non-passing while any non-skipped child in the set
has `has_result == false`. While non-passing, `koto next` already returns the
`NextResponse::GateBlocked` variant with `blocking_conditions` — the converge
condition names the outstanding children by their fan-out identity (short task
name / `child_session_id`), and the parent is not advanced past the state. When
the last result lands, the gate passes, the state advances, and the directive
the agent reads carries every child's result inline in the gate output. No
child log is opened to produce them (PRD R4 / R8 / AC4 / AC8).

This reuses the existing `GateBlocked` surface verbatim — no new `NextResponse`
variant and no new command noun (PRD D4 / R7 / AC6). The `children-complete`
gate's `temporal` blocking category (retry-later) already fits a converge point
that clears as children finish.

Uniformity/recursion (PRD R6 / AC7): because the converge point is just a
`children-complete` gate and the result is just the auto-promoted envelope, a
mid-level coordinator converges its own children through the same gate and then
auto-promotes its OWN result on its terminal tick for its parent — the same two
mechanisms at every depth, no depth-specific code path.

## Rejected: 4B — new gate type + new response variant
Introduces the parallel surface PRD R7 / D4 / AC6 explicitly forbid: a second
gate model and a new top-level response shape an agent must learn, against
koto's minimal-surface design. The `children-complete` gate + `GateBlocked`
already provide blocked-until-results-in semantics with a named outstanding
set; a new variant would duplicate it.

## Confidence: high. PRD R5 / D4 already point here; the gate and the
GateBlocked variant exist and compose directly.
