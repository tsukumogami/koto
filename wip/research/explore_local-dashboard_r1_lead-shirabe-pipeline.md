# Lead: shirabe Pipeline Structure for Composable Workflow Modeling

## Findings

### Phase-by-phase structure of each skill

#### /explore (6 phases)

Purpose: determine which artifact type is needed before committing to one.

| Phase | Name | Artifact produced |
|-------|------|-------------------|
| 0 | Setup | Feature branch |
| 1 | Scope | `wip/explore_<topic>_scope.md` |
| 2 | Discover | `wip/research/explore_<topic>_r<N>_lead-<name>.md` (parallel agents) |
| 3 | Converge | `wip/explore_<topic>_findings.md` |
| 4 | Crystallize | `wip/explore_<topic>_crystallize.md` (artifact-type decision) |
| 5 | Produce | Handoff artifact for target skill |

Phases 2-3 form a loop. The loop repeats until the user says "ready to decide," at which point a `## Decision: Crystallize` marker is added to the findings file and phase 4 runs. Phase 4 scores all possible artifact types and presents a recommendation. Phase 5 writes a handoff artifact tailored to the chosen downstream skill and, for /prd and /design, auto-invokes that skill in the same session.

Inputs: a topic string or GitHub issue number.
Outputs: a handoff artifact (e.g., `wip/prd_<topic>_scope.md` for a PRD handoff, a design-doc skeleton for a design handoff).

#### /prd (5 phases)

Purpose: capture WHAT to build and WHY.

| Phase | Name | Artifact produced |
|-------|------|-------------------|
| 0 | Setup | Feature branch |
| 1 | Scope | Problem statement + research leads (conversational) |
| 2 | Discover | `wip/research/prd_<topic>_phase2_*.md` (parallel agents) |
| 3 | Draft | `docs/prds/PRD-<topic>.md` (status: Draft) |
| 4 | Validate | PRD transitions to Accepted after 3-agent jury review |

When /explore hands off to /prd, it writes `wip/prd_<topic>_scope.md` matching Phase 1's output format. The PRD skill detects this file and resumes at Phase 2, skipping Phase 1 entirely.

Inputs: topic or accepted /explore handoff artifact.
Outputs: `docs/prds/PRD-<topic>.md` with status "Accepted."
Next step: /design (if complex) or /plan (if medium/simple).

#### /design (7 phases)

Purpose: decide HOW to build something.

| Phase | Name | Artifact produced |
|-------|------|-------------------|
| 0 | Setup | Branch, PRD extraction or freeform scoping; `wip/design_<topic>_summary.md` |
| 1 | Decompose | Independent decision questions; `wip/design_<topic>_coordination.json` |
| 2 | Execute | Per-decision reports via parallel agents; `wip/design_<topic>_decision_<N>_report.md` |
| 3 | Cross-validate | Assumption conflicts resolved; Considered Options section |
| 4 | Investigate | Architecture synthesis; Solution Architecture section |
| 5 | Security | Mandatory security review; Security Considerations section |
| 6 | Finalize | Complete design doc, commit, PR |

When /explore hands off to /design, it writes both `docs/designs/DESIGN-<topic>.md` (skeleton) and `wip/design_<topic>_summary.md`. The design skill detects these and resumes at Phase 1.

Inputs: `docs/prds/PRD-<topic>.md` (Accepted) or freeform topic or /explore handoff.
Outputs: `docs/designs/DESIGN-<topic>.md` with status "Proposed."
Next step: /plan.

#### /plan (7 phases)

Purpose: decompose an Accepted design into implementable issues.

| Phase | Name | Artifact produced |
|-------|------|-------------------|
| 1 | Analysis | `wip/plan_<topic>_analysis.md` |
| 2 | Milestone | `wip/plan_<topic>_milestones.md` |
| 3 | Decomposition | `wip/plan_<topic>_decomposition.md` + execution mode selection |
| 3.5 | Execution mode | Recorded in decomposition artifact (single-pr vs multi-pr) |
| 4 | Generation | `wip/plan_<topic>_issue_*.md` + `wip/plan_<topic>_manifest.json` (parallel agents) |
| 5 | Dependencies | `wip/plan_<topic>_dependencies.md` |
| 6 | Review | `wip/plan_<topic>_review.md` |
| 7 | Creation | `docs/plans/PLAN-<topic>.md`; multi-pr also creates GitHub milestone + issues |

Inputs: `docs/designs/DESIGN-<topic>.md` (Accepted) or PRD or roadmap.
Design status transitions from Accepted -> Planned.
Outputs: `docs/plans/PLAN-<topic>.md`; in multi-pr mode, GitHub milestone and atomic issues.
Next step: /work-on per issue.

#### /work-on (koto-backed, ~10 states)

Purpose: implement a single GitHub issue end-to-end.

State machine (from `koto-templates/work-on.md`):

```
entry
  -> context_injection (issue_backed) -> setup_issue_backed -> staleness_check
                                                                  -> introspection (if stale)
                                                                  -> analysis
  -> task_validation (free_form) -> research -> post_research_validation -> setup_free_form
                                                                              -> analysis

analysis -> implementation -> finalization -> pr_creation -> ci_monitor
                                                              -> done (terminal)
                                                              -> done_blocked (terminal)
```

Key states and their evidence:
- `entry`: submits mode (issue_backed / free_form) and issue number or task description
- `analysis`: produces `plan.md` context key; submits plan_outcome
- `implementation`: produces code commits; gate checks `go test ./...`; submits implementation_status
- `finalization`: produces `summary.md` context key; submits finalization_status
- `pr_creation`: submits pr_url
- `ci_monitor`: gate checks CI via `gh pr checks`; submits ci_outcome

Terminal states: `done` (success), `done_blocked` (human intervention needed), `validation_exit` (task not ready).

---

### Full pipeline as a koto workflow tree

```
feature-pipeline (parent, hypothetical)
  |
  +-- explore.<topic> (child session)
  |     phases: setup -> scope -> [discover <-> converge]+ -> crystallize -> produce
  |     terminal: produce completes, handoff artifact written
  |
  +-- prd.<topic> (child session, spawned when explore decides PRD)
  |     phases: (scope pre-filled) -> discover -> draft -> validate
  |     terminal: PRD status = Accepted
  |
  +-- design.<topic> (child session, spawned when explore decides Design Doc, or after PRD)
  |     phases: setup -> decompose -> execute -> cross-validate -> investigate -> security -> finalize
  |     terminal: design status = Proposed (then user-approved -> Accepted)
  |
  +-- plan.<topic> (child session, spawned after design Accepted)
  |     phases: analysis -> milestone -> decomposition -> generation -> dependencies -> review -> creation
  |     terminal: PLAN doc created, GitHub issues created (multi-pr)
  |
  +-- work-on.issue-<N> (child session per issue, spawned by plan)
        states: entry -> setup -> analysis -> implementation -> finalization -> pr_creation -> ci_monitor
        terminal: done (PR merged, CI passing) or done_blocked
```

#### Evidence submitted at each node

| Session | Key evidence fields | "Done" looks like |
|---------|--------------------|--------------------|
| explore | crystallize.md artifact; chosen artifact type | Phase 5 produce completes; handoff file committed |
| prd | PRD file at docs/prds/PRD-<topic>.md; status "Accepted" | Jury validation passes, user approves |
| design | design doc at docs/designs/DESIGN-<topic>.md; status "Proposed" | Phase 6 finalize; user approves |
| plan | PLAN doc + GitHub issues created | Phase 7 creation; design status -> Planned |
| work-on | pr_url, ci_outcome, plan.md, summary.md koto context keys | ci_monitor state with ci_outcome: passing -> done |

#### "Stuck" vs. "progressing" at each level

| Session | Stuck signals | Progressing signals |
|---------|---------------|---------------------|
| explore | Rounds keep repeating, crystallize never reached; user hasn't responded to AskUserQuestion | New research files appearing each round; findings file growing |
| prd | Phase 3 draft is open for review, user hasn't approved | Research files accumulating; PRD file updated |
| design | Phase 2 parallel decision agents not completing; cross-validate surfacing conflicts user hasn't resolved | Coordination JSON advancing; design doc sections filling in |
| plan | Decomposition stalled; execution mode not selected | Issue outlines appearing; manifest.json growing |
| work-on | `done_blocked` terminal; gate failing (is_blocked=true on implementation or ci_monitor) | States advancing; commits appearing on branch |

---

### Dashboard information needs at each level

#### Parent-level (feature-pipeline view)

A user monitoring the overall pipeline wants:
- Which phase is the pipeline currently in? (explore / prd / design / plan / work-on)
- Is the current child session progressing or stuck?
- How many work-on children exist and what fraction are done/blocked?
- Which child is the bottleneck?
- What's the overall completion trajectory?

Minimal data needed from koto per session: session name, current_state, is_terminal, is_blocked, parent_workflow, plus the template name (to infer which skill it represents).

#### Child-level (individual session view)

A user drilling into a specific session wants:
- Current state name within the template
- Most recent gate result (PASS/FAIL, gate name, command)
- Last evidence submitted (fields + values)
- How long the session has been in this state
- For work-on: which issue this maps to (from the ARTIFACT_PREFIX variable or ISSUE_NUMBER variable in the header)
- For explore: which round of discover-converge the session is on (derivable from how many research files exist, though this isn't directly in koto state)

#### What the dashboard currently exposes

The koto dashboard already surfaces: session name, current_state, is_terminal, is_blocked, parent_workflow (for tree construction), gate name and result, last evidence fields, elapsed time in current state.

---

### What's missing from the current koto model

1. **No parent-initiated child spawning.** The current model records `parent_workflow` in a child's header, but there is no mechanism for a parent session to spawn a child. In the shirabe pipeline, a parent workflow would need to call `koto init child.<topic>` during a transition. This would require a new `spawn_child` action type or an agent issuing CLI commands mid-workflow.

2. **No parent observation of child completion.** A parent session has no gate that watches for a child session's terminal state. To implement "wait for prd.topic to reach done, then advance plan.topic," the parent would need either:
   - A `child-terminal` gate type that checks `koto query <child>` for terminal status
   - Or a polling mechanism external to the state machine

3. **Template metadata not visible from session header.** The dashboard can see session name and parent_workflow but cannot currently determine which skill/template a session is running from the header alone. The `StateFileHeader` has `template_hash` but not a human-readable template name. To show "this is a prd session" at the parent level, the template's compiled JSON must be read from disk and its `name` field extracted.

4. **Variable values not in the snapshot.** `ISSUE_NUMBER` and `ARTIFACT_PREFIX` are set at init time but are not surfaced in the `CachedSession` struct used by the dashboard. Showing "issue #42" in a work-on child row requires either reading the full event log or adding variables to the cached snapshot.

5. **No inter-session handoff artifacts in koto context.** The current handoff between skills is file-based (wip/ markdown files). If skills become koto sessions, a mechanism to pass artifacts between parent and child koto contexts would be needed (e.g., the parent writing to the child's context store, or a shared context namespace).

6. **Explore's loop count is not a koto state.** The discover-converge loop iteration (round N) is tracked only through file naming conventions (`_r<N>_`), not as koto state. A koto-backed explore would need a variable or state to count rounds.

---

## Implications

### What the dashboard must support

- **Tree layout with per-level context.** The parent pipeline row must show the phase name (which skill is active), not just the koto session name. This requires mapping template names to human-readable phase labels.
- **Child count and completion fraction.** For plan->work-on fanout, the parent view needs "N of M issues done" without drilling into each child.
- **Blocked child propagation.** If any work-on child is in `done_blocked`, the parent pipeline should show a warning. The current `is_blocked` flag covers gate failures within a session; blocked terminal states need separate signaling.
- **Variable display in child rows.** For work-on sessions, the issue number from `ISSUE_NUMBER` should appear in the row label. This requires reading variables from the event log (WorkflowInitialized event carries initial variables).
- **Template name resolution.** The dashboard needs to resolve the template hash to a template name to label sessions by skill rather than by opaque session ID.

### What koto might need

- A `child-terminal` gate type (or a `spawn_child` transition action) to let parent sessions observe or initiate child completion.
- Variables surfaced in `CachedSession` (add to the snapshot struct or make them derivable from the WorkflowInitialized event already in the log).
- A `template_name` field in the state file header (alongside `template_hash`) so the dashboard can label sessions without reading the compiled template from disk.
- A mechanism for expressing the full pipeline as a single template with spawned children, rather than requiring an external orchestrator to wire them together.

---

## Surprises

- /work-on is the only skill already backed by a koto template. All other skills (explore, prd, design, plan) remain purely prompt-driven with wip/ file-based state. The gap between "what koto currently backs" and "the full pipeline as koto sessions" is larger than it might appear.

- /explore's Phase 5 auto-invokes the downstream skill (/prd or /design) in the same session rather than stopping and waiting for the user. This means explore and prd are currently merged into a single agent session with no boundary. Making them separate koto child sessions would require an explicit handoff point that today doesn't exist.

- The crystallize step in /explore produces a "no artifact" outcome for some explorations, meaning the pipeline can terminate at explore without spawning any downstream sessions. A parent pipeline template would need to model this branching.

- /plan has a single-pr vs multi-pr mode split. In single-pr mode, no GitHub issues are created, so the plan->work-on fanout doesn't happen. The parent pipeline template would need to model two different fanout patterns depending on this choice.

- The `is_blocked` flag in `CachedSession` only captures gate failures, not the `done_blocked` terminal state. A pipeline monitoring multiple work-on sessions needs to check both `is_blocked` AND `is_terminal` with a `done_blocked` state name to get the full picture of sessions requiring human attention.

---

## Open Questions

1. **Who spawns children?** If explore, prd, design, and plan become separate koto sessions, something must create each child when the previous phase completes. Is this an agent action (the agent runs `koto init child` as part of submitting evidence), a new `spawn_child` transition action, or an external orchestrator?

2. **How does the parent wait for a child?** koto has no built-in gate that blocks until a named child session is terminal. What's the intended mechanism: polling, a new gate type, or a different model (children notify the parent via context writes)?

3. **What is the right granularity for koto sessions?** Should each shirabe skill be one koto session, or should each phase within a skill be a state in a single session? The work-on template suggests single-session-per-skill is the pattern, but explore's multi-round loop and design's parallel decision agents push toward finer granularity.

4. **How do variables flow across sessions?** The PRD handoff writes a wip/ file. If parent->child handoff moves to koto context, does the parent write to the child's context store before init, or does the child read from the parent's context store during its own init?

5. **Can the dashboard show progress within a non-koto-backed skill?** Until prd, design, and plan are koto-backed, the dashboard can only show the work-on sessions. The parent-level view would be incomplete for the rest of the pipeline.

---

## Summary

The shirabe pipeline is a sequential chain of five skills (explore -> prd -> design -> plan -> work-on*N) where each skill produces a typed artifact that the next skill consumes; only /work-on is currently koto-backed, and the remaining four rely entirely on wip/ file naming conventions and agent memory for state persistence. Making the full pipeline composable in koto would require at minimum: a child-spawning mechanism, a gate type that blocks on child terminal status, and variable/template-name fields in the session snapshot for meaningful parent-level dashboard display. The biggest open question is whether parent sessions should observe or initiate child sessions, because koto currently supports neither direction — it records lineage via `parent_workflow` in the child's header but provides no signal back to the parent.
