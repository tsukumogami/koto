<!-- decision:start id="free-form-validation-placement" status="assumed" -->
### Decision: Free-form mode validation placement

**Context**

The /work-on template's free-form path (task description, no GitHub issue) needs a
mechanism to reject tasks that aren't ready for direct implementation. The current design
places a single `task_validation` state before research, with a `verdict: enum[ready,
needs_design, needs_breakdown, ambiguous]` schema. This catches tasks whose descriptions
are obviously wrong — ambiguous wording, oversized scope visible from the description
alone — but it assesses only the description text, not the current codebase state.

Two independent panel reviewers identified the same gap: after research runs, the agent
may discover that the task is misconceived relative to the current codebase (a module was
rewritten, the feature already exists, the dependency was removed). The current design has
no mechanism to exit cleanly at that point. An agent that reaches this realization can only
route to `done_blocked`, a terminal dead-end with no reconsideration path. The workflow
practitioner noted: "There's no 'given what research revealed, does this task still make
sense?' gate." The skill implementer noted: "Research happens after validation is already
decided. The sequence is wrong."

The real just-do-it skill addresses this with two jury phases: Phase 1 (Initial Jury)
before research, and Phase 3 (Validation Jury) after research. Research informs the second
jury. The current design borrows the free-form concept from just-do-it but only captures
the first jury.

**Assumptions**

- Decision 1 (mode routing topology) does not eliminate the free-form validation states
  regardless of whether the `entry` state is kept or replaced by init-time routing. Both
  validation gates belong to the free-form path itself, not to the routing mechanism.
  If Decision 1 changes the entry point, the validation states shift position but remain
  present.
- The pre-research check (`task_validation`) and post-research check (`post_research_validation`)
  have meaningfully different evidence schemas and serve genuinely different purposes, so they
  do not constitute the "validation logic duplication" the constraint warns against.
- A 16-state template is acceptable given that both states are load-bearing. The constraint
  treats state count as a cost, not an absolute limit.
- "Lightweight" for the pre-research check means a binary gate (proceed or exit) with no
  sub-agent jury mechanics, keeping it fast and low-ceremony.

**Chosen: Option (c) — lightweight pre-research check + post-research validation**

Two validation states in the free-form path:

1. `task_validation` (before research): assesses the task description for obvious non-fit.
   Binary verdict: `proceed` continues to research, `exit` routes to `validation_exit`.
   Evidence schema: `verdict: enum[proceed, exit]`, `rationale: string`. This is a
   lightweight single-agent gate, not a jury. Purpose: avoid spending research effort on
   tasks that are clearly out of scope, ambiguous, or require a design first.

2. `post_research_validation` (after research, before setup): assesses the task against
   what research revealed about the current codebase state. Evidence schema:
   `verdict: enum[ready, needs_design, validation_exit]`, `rationale: string`, optionally
   `revised_scope: string` when the task can proceed with a narrowed scope. Purpose: catch
   tasks that are misconceived relative to current codebase reality, and provide a clean
   audit record of the post-research scoping decision.

Both states can route to `validation_exit` (the terminal state). The `validation_exit`
directive instructs the agent to communicate the verdict with the rationale and suggest
the appropriate next step (create an issue, write a design doc, narrow the scope).

The free-form path becomes: `task_validation` → `research` → `post_research_validation`
→ `setup` → (convergence to `analysis` with issue-backed path). This matches just-do-it's
Jury → Research → Validation Jury → Setup sequence in structure, without requiring the
multi-agent jury mechanics.

**Rationale**

Option (a) — pre-research only — is insufficient on its own. Both reviewers independently
found the post-research gap. An agent that discovers a misconception in research but can
only route to `done_blocked` has reached a dead end; there's no way to communicate "this
task doesn't make sense given what I found" without terminating the workflow permanently.
`validation_exit` exists precisely for this outcome and should be reachable from the
post-research position.

Option (b) — post-research only — solves the misconception gap but wastes research effort
on tasks that are obviously wrong before research runs. The pre-research check is fast and
cheap; removing it means the template invests in research even for tasks that a quick
description review would reject.

Option (d) — no validation state — fails the constraint that `validation_exit` must be
reachable. Without an explicit state, the routing to `validation_exit` becomes an implicit
agent decision from within `research` or `analysis`, with no structured evidence record.
This weakens both enforcement and auditability.

Option (c) adds one state to the template (16 total). The two validation states have
distinct schemas: `task_validation` is a binary gate on description quality;
`post_research_validation` is a ternary routing decision informed by codebase findings.
These are not duplicates of the same logic. The schema difference maps to a real functional
difference in what the agent is being asked to assess.

**Alternatives Considered**

- **Option (a) — pre-research validation only**: Single `task_validation` state before
  research. Rejected because it provides no exit path when research reveals the task is
  misconceived. Two independent reviewers confirmed this gap. An agent in this situation
  can only reach `done_blocked`, permanently terminating the workflow rather than allowing
  a graceful "not ready" exit.

- **Option (b) — post-research validation only**: Single validation state after research.
  Rejected because it removes the pre-research filter. Research takes real agent effort;
  running it on obviously-wrong tasks (ambiguous description, clearly oversized scope)
  wastes that effort. The pre-research gate is lightweight and provides an early exit that
  costs less than a full research round.

- **Option (d) — no validation state**: Agent self-assesses scope during analysis.
  Rejected because it violates the constraint that `validation_exit` must be reachable
  from a defined state, and removes the structured audit record of the validation decision.
  Without a dedicated state, the routing to `validation_exit` is an implicit agent behavior
  that koto cannot enforce.

**Consequences**

The free-form path grows from 3 pre-convergence states to 4: `task_validation`, `research`,
`post_research_validation`, `setup`. The total template state count becomes 16.

The `validation_exit` terminal is now reachable from two distinct points in the free-form
path, giving the agent a clean exit at both the description-assessment stage and the
codebase-context stage.

Agents get a structured audit record for both validation decisions. The event log shows
not just "validation happened" but when it happened and what the agent assessed — before
or after research, and against what evidence.

The added state increases template authoring complexity modestly. The `post_research_validation`
directive needs to clearly instruct the agent on what "given what research revealed" means
in practice — it should reference the research artifacts (context summary, findings) and
ask the agent to re-assess scope against current codebase state, not just re-read the
original task description.

The pre-research `task_validation` schema simplifies: the multi-value verdict enum from
the current design (`ready, needs_design, needs_breakdown, ambiguous`) collapses to a
binary (`proceed, exit`) since nuanced reasons now live in `rationale`. The richer
routing context moves to `post_research_validation`, which has better information to
make those distinctions.
<!-- decision:end -->
