---
status: Draft
problem: |
  koto's state machine is per-workflow with no cross-workflow awareness. When an
  agent workflow needs to fan out over a collection -- issues in a plan, repos in
  a release, research leads in an exploration -- the agent must build an external
  orchestrator that duplicates what koto already tracks. This creates two sources
  of truth, loses koto's gate/override/decision infrastructure for parent-level
  coordination, and breaks resumability across the hierarchy.
goals: |
  Agents can express multi-level workflow hierarchies natively in koto. A parent
  workflow spawns children, waits for them through the gate system, queries their
  results, and routes based on outcomes -- all using the same koto next loop they
  already know. The hierarchy supports at least 3 levels of nesting and resumes
  correctly after interruption at any level.
source_issue: 127
---

# PRD: Hierarchical multi-level workflows

## Status

Draft

## Problem Statement

AI coding agents running koto-backed workflows regularly need to coordinate
multiple independent sub-workflows. A plan workflow processes N issues, each
going through analysis, implementation, review, and QA. An exploration workflow
fans out M research agents, each following a structured data-gathering template.
A release workflow coordinates across repos, each with its own plan.

Today, these patterns require the agent to:
- Manage a queue of items outside koto (a manifest file, a JSON array in context)
- Track per-item state manually (which items started, which finished, which failed)
- Reconcile koto's event log with external state when resuming after interruption
- Forgo koto's gates, overrides, and decisions for parent-level coordination

The result is fragile, non-resumable orchestration code duplicated across every
skill that needs fan-out.

## Goals

1. A parent workflow can spawn child workflows that run their own templates
   independently
2. The parent can wait for children using the same gate mechanism it already
   uses for other blocking conditions
3. The parent can query child state and read child results without side effects
4. The hierarchy is resumable -- interrupting at any level and resuming
   produces the correct next step
5. Existing single-workflow templates continue to work without changes

## Scenarios

### Scenario 1: Implementation plan fan-out

An agent runs a plan workflow that processes N issues, each through a multi-phase
implementation template. Issues may have dependency ordering (issue B depends on
issue A completing first).

**Parent template** (`plan-workflow.md`):

```yaml
states:
  run_plan:
    directive: |
      Read the plan file and identify all issues to implement.
      For each issue, create a child workflow:

      ```bash
      koto init plan.issue-<N> --parent plan --template implement-issue.md \
        --var ISSUE_NUMBER=<N> --var ISSUE_TITLE="<title>"
      ```

      Spawn a child agent for each issue that has no unmet dependencies.
      Issues whose dependencies haven't completed yet will be spawned later.

      When the gate unblocks, check if any completed children unblocked
      dependent issues. If so, spawn those children and call koto next again.
    gates:
      children-done:
        type: children-complete
        completion: "terminal"
    transitions:
      - target: summarize
        when:
          gates.children-done.all_complete: true
      - target: handle_failures
        when:
          gates.children-done.pending: 0

  handle_failures:
    directive: |
      Some child workflows failed. Check which:

      ```bash
      koto workflows --children plan
      ```

      For each failed child, read its state:

      ```bash
      koto status plan.issue-<N>
      koto context get plan.issue-<N> error_summary
      ```

      Decide whether to retry, skip, or escalate.
    accepts:
      resolution:
        type: enum
        values: [retry, skip, escalate]
    transitions:
      - target: wait_for_children
        when:
          resolution: retry
      - target: summarize
        when:
          resolution: skip
      - target: blocked
        when:
          resolution: escalate

  summarize:
    directive: |
      All children completed (or were skipped). Gather results:

      ```bash
      for child in $(koto workflows --children plan --json | jq -r '.[].name'); do
        koto context get "$child" summary
      done
      ```

      Compile the overall plan completion summary.
    terminal: true
```

**Child template** (`implement-issue.md`):

```yaml
states:
  analyze:
    directive: |
      Read issue #{{ISSUE_NUMBER}} ("{{ISSUE_TITLE}}") and analyze what
      needs to change.
    accepts:
      analysis:
        type: string
        required: true
    transitions:
      - target: implement
        when: {}

  implement:
    directive: |
      Implement the changes identified in the analysis.
    accepts:
      outcome:
        type: enum
        values: [success, failure]
    transitions:
      - target: review
        when:
          outcome: success
      - target: failed
        when:
          outcome: failure

  review:
    directive: |
      Run tests and review the implementation.
    accepts:
      review_result:
        type: enum
        values: [pass, fail]
    transitions:
      - target: done
        when:
          review_result: pass
      - target: implement
        when:
          review_result: fail

  done:
    directive: |
      Store the completion summary in context:

      ```bash
      koto context add {{WORKFLOW_NAME}} summary '{"issue": {{ISSUE_NUMBER}}, "status": "complete"}'
      ```
    terminal: true

  failed:
    directive: |
      Store the failure summary in context:

      ```bash
      koto context add {{WORKFLOW_NAME}} error_summary '{"issue": {{ISSUE_NUMBER}}, "error": "..."}'
      ```
    terminal: true
```

**Agent command sequence:**

```bash
# Parent agent initializes the plan
koto init plan --template plan-workflow.md --var PLAN_FILE=plan.md

# First koto next: gate blocks because no children exist yet
koto next plan
# -> action: gate_blocked, category: temporal
# -> directive: "Read the plan file and identify all issues..."
# -> blocking_conditions: [{name: "children-done", type: "children-complete",
#      output: {total: 0, completed: 0, pending: 0, all_complete: false}}]

# Agent reads the directive and spawns children
koto init plan.issue-1 --parent plan --template implement-issue.md \
  --var ISSUE_NUMBER=1 --var ISSUE_TITLE="Add header parser"
koto init plan.issue-2 --parent plan --template implement-issue.md \
  --var ISSUE_NUMBER=2 --var ISSUE_TITLE="Fix validation bug"
# Issue 3 not spawned yet -- depends on issue 1

# Re-check: gate still blocks (children pending)
koto next plan
# -> action: gate_blocked, category: temporal
# -> output: {total: 2, completed: 0, pending: 2, ...}

# [Time passes, child agents run their loops independently]

# Child 1 reaches terminal state -- spawn issue 3 (was blocked on issue 1)
koto init plan.issue-3 --parent plan --template implement-issue.md \
  --var ISSUE_NUMBER=3 --var ISSUE_TITLE="Use new parser in routes"

# Re-check: still pending
koto next plan
# -> gate_blocked, output: {total: 3, completed: 1, pending: 2, ...}

# [More time passes, all children complete]

koto next plan
# -> gates pass, advances to summarize (terminal)
```

### Scenario 2: Multi-repo release coordination

A release workflow coordinates across 3 repos. Each repo has its own release
checklist template. The top-level workflow needs all repos to pass their
checklists before cutting the final release.

**Parent template** (`release-coordinator.md`):

```yaml
states:
  coordinate:
    directive: |
      Create a child workflow for each repo's release checklist:

      ```bash
      koto init release.koto --parent release \
        --template repo-checklist.md --var REPO=koto
      koto init release.niwa --parent release \
        --template repo-checklist.md --var REPO=niwa
      koto init release.tsuku --parent release \
        --template repo-checklist.md --var REPO=tsuku
      ```

      Spawn an agent per repo to run through the checklist.
    gates:
      repos-ready:
        type: children-complete
        completion: "terminal"
    transitions:
      - target: cut_release
        when:
          gates.repos-ready.all_complete: true

  cut_release:
    directive: |
      All repos passed their checklists. Read results:

      ```bash
      koto context get release.koto release_version
      koto context get release.niwa release_version
      koto context get release.tsuku release_version
      ```

      Create the coordinated release.
    terminal: true
```

**Agent command sequence:**

```bash
# Top-level agent
koto init release --template release-coordinator.md --var VERSION=0.7.0

# First call: gate blocks (no children yet), directive tells agent what to spawn
koto next release
# -> gate_blocked, category: temporal, directive: "Create a child workflow..."

# Spawn per-repo children
koto init release.koto --parent release --template repo-checklist.md --var REPO=koto
koto init release.niwa --parent release --template repo-checklist.md --var REPO=niwa
koto init release.tsuku --parent release --template repo-checklist.md --var REPO=tsuku

# Check status without side effects
koto status release.koto
# -> {"name": "release.koto", "current_state": "run_tests", "is_terminal": false}

koto status release.niwa
# -> {"name": "release.niwa", "current_state": "done", "is_terminal": true}

# Re-check parent gate
koto next release
# -> gate_blocked, category: temporal, 1 of 3 complete

# Override a stuck repo checklist
koto overrides record release --gate repos-ready \
  --rationale "tsuku repo has a known flaky test, manually verified it passes"

# Parent advances (override + 2 complete = all resolved)
koto next release
# -> gates pass, advances to cut_release
```

### Scenario 3: Exploration fan-out

An exploration workflow fans out research agents, each running a structured
research template. The parent converges their findings.

**Parent template** (`exploration.md`):

```yaml
states:
  discover:
    directive: |
      Define the research leads. For each lead, spawn a child:

      ```bash
      koto init explore.lead-<name> --parent explore \
        --template research-lead.md --var LEAD="<question>" --var TOPIC="<topic>"
      ```

      Wait for all research agents to complete.
    gates:
      leads-done:
        type: children-complete
        completion: "terminal"
        name_filter: "explore."
    transitions:
      - target: converge
        when:
          gates.leads-done.all_complete: true

  converge:
    directive: |
      All research leads completed. Read their findings:

      ```bash
      koto workflows --children explore
      ```

      For each child, read the summary:

      ```bash
      koto context get explore.lead-<name> findings_summary
      ```

      Synthesize findings and decide: explore further or crystallize?
    accepts:
      decision:
        type: enum
        values: [explore_further, crystallize]
    transitions:
      - target: discover
        when:
          decision: explore_further
      - target: produce
        when:
          decision: crystallize

  produce:
    directive: |
      Write the final artifact based on accumulated findings.
    terminal: true
```

**Agent command sequence:**

```bash
# Parent agent scopes the exploration
koto init explore --template exploration.md --var TOPIC="caching strategy"

# First call: gate blocks (no children yet), directive explains what to spawn
koto next explore
# -> gate_blocked, category: temporal
# -> directive: "Define the research leads. For each lead, spawn a child..."

# Spawn 3 research leads
koto init explore.lead-prior-art --parent explore \
  --template research-lead.md --var LEAD="How do others solve this?"
koto init explore.lead-constraints --parent explore \
  --template research-lead.md --var LEAD="What are our constraints?"
koto init explore.lead-feasibility --parent explore \
  --template research-lead.md --var LEAD="Is approach X feasible?"

# [Research agents run independently]

# Parent checks progress
koto next explore
# -> gate_blocked, category: temporal
# -> output: {total: 3, completed: 1, pending: 2, children: [...]}

# All leads finish
koto next explore
# -> gates pass, advances to converge

# Parent reads child results
koto context get explore.lead-prior-art findings_summary
koto context get explore.lead-constraints findings_summary
koto context get explore.lead-feasibility findings_summary

# Parent decides to explore further
koto next explore --with-data '{"decision": "explore_further"}'
# -> back to scope state, spawn more leads for round 2
```

### Scenario 4: Resuming a hierarchy after interruption

An agent is interrupted mid-hierarchy. On resume, it needs to discover the
hierarchy state and continue from where it left off.

**Agent command sequence:**

```bash
# Agent was interrupted while running a plan with 5 child issues
# On resume, discover what exists:

koto workflows --roots
# -> [{"name": "plan", "parent_workflow": null, ...}]

koto workflows --children plan
# -> [
#   {"name": "plan.issue-1", "parent_workflow": "plan", ...},
#   {"name": "plan.issue-2", "parent_workflow": "plan", ...},
#   {"name": "plan.issue-3", "parent_workflow": "plan", ...}
# ]

# Check parent state
koto status plan
# -> {"name": "plan", "current_state": "wait_for_children", "is_terminal": false}

# Check each child
koto status plan.issue-1
# -> {"current_state": "done", "is_terminal": true}

koto status plan.issue-2
# -> {"current_state": "implement", "is_terminal": false}

koto status plan.issue-3
# -> {"current_state": "analyze", "is_terminal": false}

# Resume the parent -- gate evaluates, shows progress
koto next plan
# -> gate_blocked, category: temporal
# -> output: {total: 3, completed: 1, pending: 2, ...}

# Resume child 2 (was in the middle of implementing)
koto next plan.issue-2
# -> evidence_required at implement state

# Resume child 3
koto next plan.issue-3
# -> evidence_required at analyze state
```

## Requirements

### Functional

**R1. Parent-child lineage.** `koto init <name> --parent <parent-name>` creates
a child workflow linked to the named parent. The parent must exist at init time.
The relationship is recorded in the child's state file header.

**R2. Child discovery.** `koto workflows --children <parent>` returns only
workflows whose parent is the named workflow. `koto workflows --roots` returns
only workflows with no parent. `koto workflows --orphaned` returns workflows
whose parent no longer exists.

**R3. Children-complete gate.** A `children-complete` gate type checks whether
child workflows have reached their completion condition. The gate fails
(blocks advancement) while children are pending, and passes when all children
are complete.

**R4. Configurable completion.** The `children-complete` gate accepts a
`completion` field specifying what "complete" means: `"terminal"` (child
reached a terminal state, default), with `"state:<name>"` and
`"context:<key>"` reserved for future releases.

**R5. Child filtering.** The `children-complete` gate accepts a `name_filter`
field that restricts which children are evaluated. Only children whose name
starts with the filter prefix are included. Without a filter, all children of
the parent are included.

**R6. Structured gate output.** The `children-complete` gate produces a
fixed-shape JSON output with aggregate fields (`total`, `completed`, `pending`,
`all_complete`) and a per-child array (`children`) with each child's name,
current state, and completion status. The aggregate fields are available for
`when`-clause routing via `gates.<gate-name>.*`.

**R7. Temporal blocking signal.** When a `children-complete` gate blocks, the
`blocking_conditions` response includes a `category: "temporal"` field,
distinguishing it from corrective blocking conditions where the agent needs to
fix something. Existing gate types use `category: "corrective"`.

**R8. Read-only status inspection.** `koto status <name>` returns the
workflow's current state, template info, and terminal status without evaluating
gates, running actions, or advancing state.

**R9. Cross-workflow context reads.** A parent agent can read a child's context
store via `koto context get <child-session> <key>`. This is how children pass
results back to the parent.

**R10. Override support.** The `children-complete` gate supports the existing
override mechanism (`koto overrides record`). The override default represents
"pretend all children are done."

**R11. Advisory lifecycle.** When a parent is cancelled, cleaned up, or
rewound, the command's JSON response includes a `children` array listing
affected child workflows and their states. koto does not automatically cascade
these operations to children.

**R12. Hierarchy resumability.** Interrupting at any level of the hierarchy and
resuming via `koto next` produces the correct state. Parent gates re-evaluate
child status on each call. Child workflows resume independently.

### Non-functional

**R13. Backward compatibility.** Existing workflows without `--parent` continue
to work without changes. The `parent_workflow` field defaults to absent in
state file headers. No schema version bump required.

**R14. Nesting depth.** The hierarchy supports at least 3 levels (grandparent
-> parent -> child). All operations (discovery, gate evaluation, status) work
at any depth.

**R15. Session scan performance.** Gate evaluation scans all sessions in the
repo-id scope. The design assumes under 50 sessions per scope. Performance
degradation at higher counts is acceptable for MVP.

### Skill updates

**R16. koto-user skill update.** The koto-user skill must document the
`children-complete` gate type in the action dispatch table and handling
guidance: how `gate_blocked` responses with `category: "temporal"` differ from
corrective blocks, how to read `children` array output, when to poll vs take
action, and how to use `koto status` for side-effect-free child inspection.
The `--parent` flag on `koto init`, `--roots`/`--children`/`--orphaned` flags
on `koto workflows`, and the `koto status` command must be added to the
command reference. The override flow section must cover overriding
`children-complete` gates.

**R17. koto-author skill update.** The koto-author skill must document the
`children-complete` gate type in the template authoring guide: the `completion`
and `name_filter` fields, gate type schema, the single-state pattern (directive
+ gate on the same state), and how `gates.<name>.*` routing works for child
outcome-dependent transitions. The compiler validation section must cover
`children-complete` field validation. Template examples should show the
fan-out pattern (parent template + child template pair).

## Acceptance Criteria

- [ ] `koto init child --parent parent --template child.md` creates a child
  workflow with `parent_workflow` in its state file header
- [ ] `koto init child --parent nonexistent` fails with an error naming the
  missing parent
- [ ] `koto workflows --children parent` returns only children of that parent
- [ ] `koto workflows --roots` excludes workflows that have a parent
- [ ] `koto workflows --orphaned` returns workflows whose parent was cleaned up
- [ ] A template with a `children-complete` gate blocks when children are pending
- [ ] A template with a `children-complete` gate passes when all children reach
  terminal state
- [ ] `children-complete` gate output includes `total`, `completed`, `pending`,
  `all_complete`, and `children` array
- [ ] `gates.<name>.all_complete` is usable in transition `when` clauses
- [ ] `name_filter` restricts which children the gate evaluates
- [ ] `children-complete` with zero matching children fails (no vacuous pass)
- [ ] `blocking_conditions` for `children-complete` includes `category: "temporal"`
- [ ] `koto status <name>` returns current state without advancing
- [ ] `koto status <name>` reports `is_terminal: true` for terminal states
- [ ] `koto context get <child> <key>` works from the parent agent's perspective
- [ ] `koto overrides record <name> --gate <gate>` works for `children-complete`
- [ ] `koto cancel <parent>` response includes child workflow names and states
- [ ] `koto session cleanup <parent>` does not delete child sessions
- [ ] Hierarchy with 3 levels (A -> B -> C) works for init, gate, status, and
  discovery
- [ ] Resuming a parent after interruption re-evaluates child status correctly
- [ ] Existing templates without `--parent` continue to work unchanged
- [ ] koto-user skill documents `children-complete` gate handling, temporal
  blocking category, `koto status`, and hierarchy discovery commands
- [ ] koto-user command reference includes `--parent` on init,
  `--roots`/`--children`/`--orphaned` on workflows, and `koto status`
- [ ] koto-author skill documents `children-complete` gate type with
  `completion` and `name_filter` fields
- [ ] koto-author skill shows the single-state fan-out pattern (directive +
  gate on same state) in template examples
- [ ] koto-author compiler validation section covers `children-complete` fields

## Out of Scope

- **Agent process management.** koto tracks workflow relationships but does not
  launch, monitor, or terminate child agent processes. The parent agent manages
  child agents through its own mechanism (Claude Agent tool, subprocesses, etc.).
- **Automatic cascade.** koto never automatically cancels, cleans up, or rewinds
  children when a parent lifecycle event occurs. This is advisory only.
- **Child-to-parent queries.** Children cannot query parent state through koto.
  If a child needs parent context, the parent passes it as init-time variables.
- **Sibling-to-sibling queries.** Siblings cannot discover or query each other
  through koto.
- **Completion modes beyond "terminal".** `"state:<name>"` and `"context:<key>"`
  are reserved in the schema but not implemented in this release.
- **Per-child lifecycle policies.** Temporal-style per-child close policies
  (terminate, cancel, abandon) are deferred.
- **CloudBackend optimization.** S3 session listing doesn't read headers today.
  Hierarchy filtering on the cloud backend may require downloading headers.
  Optimization is deferred.

## Known Limitations

- **Session scan on every gate evaluation.** The `children-complete` gate calls
  `backend.list()` on every evaluation, reading all session headers. This is
  O(N) in total sessions, not just children. Acceptable under 50 sessions.
- **Self-declared lineage.** Any agent can claim any workflow as its parent.
  There is no authorization mechanism. This is consistent with koto's single-user
  trust model but unsuitable for multi-tenant environments.
- **Orphan accumulation.** Advisory-only lifecycle means agents that don't clean
  up children leave orphaned state files. `--orphaned` makes them discoverable
  but doesn't prevent them.
- **Name recycling risk.** If a parent is cleaned up and a new workflow reuses
  its name, orphaned children from the original parent silently become children
  of the new workflow.

## Downstream Artifacts

- [DESIGN-hierarchical-workflows](../designs/DESIGN-hierarchical-workflows.md) --
  technical architecture (retrofit, design written first)
