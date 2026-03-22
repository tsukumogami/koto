<!-- decision:start id="plan-backed-mode" status="assumed" -->
### Decision: Plan-Backed Issue Support in Work-On Template

**Context**

The shirabe /plan workflow creates PLAN documents with issue tables containing plan-only
issues (no GitHub issue numbers). These issues have structured content: goal, acceptance
criteria, complexity, dependencies, and upstream design doc references. The /work-on skill
currently has no entry path for plan-only issues. The work-on koto template's two modes
are issue-backed (GitHub issue number required) and free-form (task description only).
A plan-backed invocation like `/work-on PLAN-<topic>#N` has no supported route.

Research found that /implement already handles plan-only issues for batch execution,
parsing Issue Outlines from PLAN docs, writing context artifacts, and managing
state for the full plan. The question is whether /work-on should handle individual
plan issues at the template level or at the skill-layer level.

Critically: the free-form path in the current template already has the right structure
for plan-backed issues. Free-form routes `entry → task_validation → research →
post_research_validation → setup_free_form → analysis`, skipping staleness entirely.
Plan-backed issues should also skip staleness (they just came from /plan and are
definitionally ready to work on). The free-form path provides this routing for free.

**Assumptions**

- The skill layer (shirabe /work-on skill instructions) is updated in Phase 3 to
  detect `PLAN-<topic>#N` input syntax, parse the PLAN doc issue, and populate
  `task_description` with the issue's goal, acceptance criteria, and design doc
  references before initializing the koto workflow as free-form mode.
- Plan-backed invocation quality depends on the skill layer's PLAN doc parsing
  fidelity. If the skill populates `task_description` poorly, task_validation or
  post_research_validation will catch gaps — but agent experience degrades.
- Users wanting to run ALL plan issues in sequence continue using /implement.
  /work-on handles single-issue execution only.

**Chosen: Free-form mode absorbs plan-backed via skill-layer parsing (Option b)**

No template changes are needed. The skill layer detects the `/work-on PLAN-<topic>#N`
invocation pattern, reads the PLAN doc issue at that sequence number, and constructs
a rich `task_description` containing: the issue's goal statement, acceptance criteria,
listed dependencies (as context, not blocking gates), and upstream design doc reference.
It then initializes the koto workflow using free-form mode (`mode: free_form`) with
this constructed description.

The two free-form validation states serve the plan-backed case correctly:
`task_validation` verifies the extracted AC is actionable and appropriately scoped
(cross-checks that the issue wasn't superseded since /plan ran); `post_research_validation`
verifies the codebase is ready for the implementation as described.

**Rationale**

Plan-backed vs. free-form is an input-format distinction, not a workflow-structure
distinction. Both paths reach `analysis` via `setup_free_form` without staleness checking,
which is the correct path for plan-only issues. The template enforces phases; the skill
layer translates between input formats and koto modes. Putting PLAN doc parsing into the
skill layer (not the template) keeps the template's mode count at 2 and its state count at
17. The koto template does not need to know whether a task description originated from
a user typing it or from a PLAN doc issue — it cares only that a task description exists.

Enforcement of "PLAN doc was read" does not benefit from a dedicated koto state the way
`context_injection` does for GitHub issues. GitHub issue extraction can fail (network,
access, not found); PLAN docs are local files that the skill layer always has access to.
The skill layer read is synchronous and reliable; a koto-enforced gate would add ceremony
without adding reliability.

Option (c) (delegate to /implement) misses the user's intent: /implement drives a full
plan batch, not single-issue work. An agent wanting to work on one plan issue interactively
should use /work-on, not /implement.

**Alternatives Considered**

- **Third template mode (a)**: Add `plan_context_extraction` and `setup_plan_backed` states
  (17 → 19 states). Entry's enum gains a third value `plan_backed`. This provides koto-level
  enforcement that the PLAN doc was read. Rejected because: (1) the free-form path already
  routes plan-backed issues correctly — adding a third mode adds ceremony without behavioral
  difference; (2) PLAN doc reads are local and reliable, making enforcement at the koto level
  less valuable than for GitHub issue context extraction; (3) 19 states increases template
  authoring cost and makes the document harder to maintain.

- **Delegate to /implement (c)**: Document plan-only issues as out of scope for /work-on.
  Rejected because the user's need is single-issue interactive work, which /implement's batch
  model doesn't serve. The user should not need a different skill to implement a plan issue
  vs. a GitHub issue.

**Consequences**

What changes: the /work-on skill instructions gain a PLAN doc parsing branch in Phase 3.
No template changes required. The decision report is added to the design doc's Considered
Options as a seventh decision.

What becomes easier: `/work-on PLAN-<topic>#N` works immediately once Phase 3 ships;
plan-only issues get the same structured workflow enforcement as free-form tasks.

What becomes harder: the quality of plan-backed execution depends on the skill layer's
PLAN doc parsing. A poorly constructed task_description reduces the value of the
pre-research validation step. The Phase 3 implementer must ensure AC is preserved faithfully.
<!-- decision:end -->
