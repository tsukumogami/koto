---
name: work-on
version: "1.0"
description: >
  Implementation workflow for issue-backed and free-form tasks. Split topology:
  issue-backed mode routes through context_injection, setup, and staleness_check;
  free-form mode routes through task_validation, research, and post_research_validation.
  Both paths converge at analysis and share all subsequent states. Requires two
  variables: ISSUE_NUMBER (issue-backed only) and ARTIFACT_PREFIX (always). Self-loops
  use conditional when blocks (scope_changed_retry, partial_tests_failing_retry,
  creation_failed_retry) to avoid triggering cycle detection.
initial_state: entry

variables:
  ISSUE_NUMBER:
    description: GitHub issue number for issue-backed workflows
    required: false
  ARTIFACT_PREFIX:
    description: >
      Prefix for wip/ artifact filenames. Set at koto init time:
      issue_<N> for issue-backed, task_<slug> for free-form.
    required: true

states:
  entry:
    accepts:
      mode:
        type: enum
        values: [issue_backed, free_form]
        required: true
      issue_number:
        type: string
        description: GitHub issue number (required for issue_backed mode)
      task_description:
        type: string
        description: Task description (required for free_form mode)
    transitions:
      - target: context_injection
        when:
          mode: issue_backed
      - target: task_validation
        when:
          mode: free_form

  context_injection:
    gates:
      context_artifact:
        type: command
        command: "test -f wip/issue_{{ISSUE_NUMBER}}_context.md"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        description: Override type or failure reason
    transitions:
      - target: setup_issue_backed

  task_validation:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
      rationale:
        type: string
        description: Reasoning behind the validation verdict
    transitions:
      - target: research
        when:
          verdict: proceed
      - target: validation_exit
        when:
          verdict: exit

  validation_exit:
    terminal: true

  research:
    accepts:
      context_gathered:
        type: enum
        values: [sufficient, insufficient]
      context_summary:
        type: string
        description: Summary of research findings and codebase observations
    transitions:
      - target: post_research_validation

  post_research_validation:
    accepts:
      verdict:
        type: enum
        values: [ready, needs_design, exit]
        required: true
      rationale:
        type: string
        description: Reasoning behind the validation verdict
      revised_scope:
        type: string
        description: Narrowed scope when task can proceed with adjustments
    transitions:
      - target: setup_free_form
        when:
          verdict: ready
      - target: validation_exit
        when:
          verdict: needs_design
      - target: validation_exit
        when:
          verdict: exit

  setup_issue_backed:
    gates:
      branch_and_baseline:
        type: command
        command: "test \"$(git rev-parse --abbrev-ref HEAD)\" != \"main\" && test -f wip/{{ARTIFACT_PREFIX}}_baseline.md"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        description: Override type or failure reason
    transitions:
      - target: staleness_check

  setup_free_form:
    gates:
      branch_and_baseline:
        type: command
        command: "test \"$(git rev-parse --abbrev-ref HEAD)\" != \"main\" && test -f wip/{{ARTIFACT_PREFIX}}_baseline.md"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        description: Override type or failure reason
    transitions:
      - target: analysis

  staleness_check:
    gates:
      staleness_fresh:
        type: command
        command: "check-staleness.sh --issue {{ISSUE_NUMBER}} | jq -e '.introspection_recommended == false'"
    accepts:
      staleness_signal:
        type: enum
        values: [fresh, stale_requires_introspection, override, blocked]
        required: true
      detail:
        type: string
        description: Override reason or failure detail
    transitions:
      - target: introspection
        when:
          staleness_signal: stale_requires_introspection
      - target: done_blocked
        when:
          staleness_signal: blocked
      - target: analysis

  introspection:
    gates:
      introspection_artifact:
        type: command
        command: "test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md"
    accepts:
      introspection_outcome:
        type: enum
        values: [approach_unchanged, approach_updated, issue_superseded]
        required: true
      rationale:
        type: string
        description: What changed or why the issue is superseded
    transitions:
      - target: done_blocked
        when:
          introspection_outcome: issue_superseded
      - target: analysis

  analysis:
    gates:
      plan_artifact:
        type: command
        command: "test -f wip/{{ARTIFACT_PREFIX}}_plan.md"
    accepts:
      plan_outcome:
        type: enum
        values: [plan_ready, blocked_missing_context, scope_changed_retry, scope_changed_escalate]
        required: true
      approach_summary:
        type: string
        description: Summary of the implementation approach
      decisions:
        type: string
        description: >
          JSON array of decision records, each with choice, rationale, and
          alternatives_considered fields. Captures non-obvious judgment calls
          made during analysis.
    transitions:
      - target: implementation
        when:
          plan_outcome: plan_ready
      - target: analysis
        when:
          plan_outcome: scope_changed_retry
      - target: done_blocked
        when:
          plan_outcome: scope_changed_escalate
      - target: done_blocked
        when:
          plan_outcome: blocked_missing_context

  implementation:
    gates:
      code_committed:
        type: command
        command: "test \"$(git rev-parse --abbrev-ref HEAD)\" != \"main\" && test \"$(git log --oneline main..HEAD | wc -l)\" -gt 0 && go test ./... 2>/dev/null"
    accepts:
      implementation_status:
        type: enum
        values: [complete, partial_tests_failing_retry, partial_tests_failing_escalate, blocked]
        required: true
      rationale:
        type: string
        description: What was accomplished or what is blocking progress
      decisions:
        type: string
        description: >
          JSON array of decision records, each with choice, rationale, and
          alternatives_considered fields. Captures non-obvious judgment calls
          made during implementation.
    transitions:
      - target: finalization
        when:
          implementation_status: complete
      - target: implementation
        when:
          implementation_status: partial_tests_failing_retry
      - target: done_blocked
        when:
          implementation_status: partial_tests_failing_escalate
      - target: done_blocked
        when:
          implementation_status: blocked

  finalization:
    gates:
      summary_and_tests:
        type: command
        command: "test -f wip/{{ARTIFACT_PREFIX}}_summary.md && go test ./... 2>/dev/null"
    accepts:
      finalization_status:
        type: enum
        values: [ready_for_pr, deferred_items_noted, issues_found]
        required: true
    transitions:
      - target: implementation
        when:
          finalization_status: issues_found
      - target: pr_creation

  pr_creation:
    accepts:
      pr_status:
        type: enum
        values: [created, creation_failed_retry, creation_failed_escalate]
        required: true
      pr_url:
        type: string
        description: URL of the created pull request
    transitions:
      - target: ci_monitor
        when:
          pr_status: created
      - target: pr_creation
        when:
          pr_status: creation_failed_retry
      - target: done_blocked
        when:
          pr_status: creation_failed_escalate

  ci_monitor:
    gates:
      ci_passing:
        type: command
        command: "gh pr checks $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number --jq '.[0].number // empty') --json state --jq '[.[] | select(.state != \"SUCCESS\")] | length == 0' | grep -q true"
    accepts:
      ci_outcome:
        type: enum
        values: [passing, failing_fixed, failing_unresolvable]
        required: true
      rationale:
        type: string
        description: What was fixed or why CI failures are unresolvable
    transitions:
      - target: done_blocked
        when:
          ci_outcome: failing_unresolvable
      - target: done

  done:
    terminal: true

  done_blocked:
    terminal: true
---

## entry

Determine the workflow mode and provide the initial context for this task.

**Issue-backed mode**: you have a GitHub issue number. Submit evidence with
`mode: issue_backed` and include the `issue_number` field.

**Free-form mode**: you have a task description but no issue. Submit evidence with
`mode: free_form` and include the `task_description` field.

**Plan-backed tasks** (from a PLAN document) use free-form mode. The skill layer
extracts the goal and acceptance criteria from the PLAN doc and provides them as
the task description.

Evidence schema:
- `mode`: `issue_backed` or `free_form`
- `issue_number`: GitHub issue number (issue-backed only)
- `task_description`: what to build (free-form only)

## context_injection

This state extracts context from the GitHub issue into a local artifact. The gate
checks for `wip/issue_{{ISSUE_NUMBER}}_context.md`. When the gate passes (artifact
exists), the workflow auto-advances.

If the gate fails, you are here because extraction did not produce the expected
artifact. Common causes:
- The issue is inaccessible (private repo, wrong number, network error)
- `extract-context.sh` failed or is not available

Submit `status: completed` after manually creating the context artifact, `status:
override` if providing context through a different mechanism, or `status: blocked`
if the issue cannot be reached.

## task_validation

Assess whether this free-form task description is clear enough and appropriately
scoped for direct implementation.

Read the task description provided at entry. Check for:
- Ambiguous or missing requirements that would make implementation guesswork
- Scope that clearly exceeds a single implementation session
- Requests that need a design document before code can be written

If the task is clear and reasonably scoped, submit `verdict: proceed` with your
rationale. If the task is not ready, submit `verdict: exit` with rationale
explaining what the user should do instead (narrow the scope, create an issue,
write a design doc).

Evidence schema:
- `verdict`: `proceed` or `exit`
- `rationale`: reasoning behind your assessment

## validation_exit

The task was not ready for direct implementation. Communicate the verdict and
rationale to the user. Suggest concrete next steps: create a GitHub issue with
clearer requirements, write a design document, narrow the scope to a single
change, or split into smaller tasks. If the verdict was `needs_design`, explain
which aspects need design work before implementation can proceed.

## research

Gather context about the codebase relevant to this free-form task. Read code,
check existing patterns, understand the architecture around the area you will
modify. Focus on:
- Files and packages that will be touched
- Existing patterns and conventions to follow
- Dependencies and interfaces that constrain the approach
- Tests that cover the affected area

Submit `context_gathered` with a `context_summary` describing what you found.
The summary carries forward to inform the post-research validation and
subsequent analysis.

Evidence schema:
- `context_gathered`: `sufficient` or `insufficient`
- `context_summary`: description of findings and relevant codebase observations

## post_research_validation

Reassess the task against what research revealed about the current codebase.

With the codebase context from research, evaluate whether the task is still
appropriate for direct implementation. Check for:
- Misconceptions in the original task description now visible with codebase context
- Dependencies or prerequisites not apparent from the task description alone
- Scope that research reveals to be larger than initially expected
- Existing code that already solves the problem or conflicts with the approach

Submit `verdict: ready` to proceed, `verdict: needs_design` if a design doc is
needed first (routes to validation_exit), or `verdict: exit` if the task should
not proceed (routes to validation_exit). Include `revised_scope` when the task
can proceed with a narrowed scope.

Evidence schema:
- `verdict`: `ready`, `needs_design`, or `exit`
- `rationale`: reasoning informed by research findings
- `revised_scope`: (optional) narrowed scope description

## setup_issue_backed

This state creates a feature branch and baseline file for issue-backed workflows.
The gate checks that you are not on main and that
`wip/{{ARTIFACT_PREFIX}}_baseline.md` exists. When the gate passes, the workflow
auto-advances to staleness_check.

If the gate fails, common causes:
- Still on the main branch (branch creation failed or was skipped)
- Baseline file was not created (build or test infrastructure issue)
- An existing branch was reused but the baseline is missing

Submit `status: completed` after creating the branch and baseline manually,
`status: override` if reusing an existing branch (include branch name in detail),
or `status: blocked` if setup cannot proceed.

## setup_free_form

This state creates a feature branch and baseline file for free-form workflows.
The gate checks that you are not on main and that
`wip/{{ARTIFACT_PREFIX}}_baseline.md` exists. When the gate passes, the workflow
auto-advances to analysis.

If the gate fails, common causes:
- Still on the main branch (branch creation failed or was skipped)
- Baseline file was not created (build or test infrastructure issue)
- An existing branch was reused but the baseline is missing

Submit `status: completed` after creating the branch and baseline manually,
`status: override` if reusing an existing branch (include branch name in detail),
or `status: blocked` if setup cannot proceed.

## staleness_check

This state assesses whether the codebase has changed significantly since the issue
was opened. The gate runs `check-staleness.sh --issue {{ISSUE_NUMBER}}` and pipes
through jq to check `introspection_recommended == false`. When fresh (gate passes),
the workflow auto-advances to analysis.

If the gate fails, you are here because the staleness check found significant
changes or could not complete.

Submit `staleness_signal: fresh` if you have confirmed the issue context is still
current, `staleness_signal: stale_requires_introspection` if the codebase has
changed enough to warrant re-reading the issue against current code,
`staleness_signal: override` if the user says to skip the staleness check, or
`staleness_signal: blocked` if the check cannot complete.

Evidence schema:
- `staleness_signal`: `fresh`, `stale_requires_introspection`, `override`, or `blocked`
- `detail`: explanation of the signal or override reason

## introspection

Re-read the issue against the current codebase to determine whether the original
approach is still valid. The gate checks for
`wip/issue_{{ISSUE_NUMBER}}_introspection.md`. On resume, if the artifact already
exists, the gate auto-advances.

Perform a fresh reading of the issue requirements against the current state of the
code. Check for:
- Requirements that have been partially or fully addressed by other changes
- Assumptions in the issue that no longer hold given current code
- New constraints or dependencies introduced since the issue was filed
- Whether the issue has been superseded by other work

Write your findings to `wip/issue_{{ISSUE_NUMBER}}_introspection.md`, then submit
evidence.

Submit `introspection_outcome: approach_unchanged` if the original approach is
still valid, `approach_updated` if the approach needed adjustments (describe what
changed in rationale), or `issue_superseded` if the issue is no longer relevant
(routes to done_blocked).

Evidence schema:
- `introspection_outcome`: `approach_unchanged`, `approach_updated`, or `issue_superseded`
- `rationale`: what changed or why the issue is superseded

## analysis

Research the codebase and create an implementation plan. The gate checks for
`wip/{{ARTIFACT_PREFIX}}_plan.md`. On resume, if the plan already exists, the
gate auto-advances.

Read the context artifacts (baseline, issue context or task description, and
introspection results if they exist). Study the relevant code, identify the
files to modify, and create a plan covering:
- Specific files and functions to change
- Order of changes and dependencies between them
- Test strategy: which tests to add, modify, or verify
- Risk areas and assumptions being made

Write the plan to `wip/{{ARTIFACT_PREFIX}}_plan.md`, then submit evidence.

If scope changes are discovered during analysis, you may self-loop with
`scope_changed_retry` up to 3 times to revise the plan. After 3 retries, switch
to `scope_changed_escalate` to route to done_blocked. Submit
`blocked_missing_context` if analysis cannot proceed without information that
is not available.

Capture non-obvious decisions in the `decisions` field: scope narrowing choices,
dependency ordering rationale, or assumptions about behavior that could be wrong.

Evidence schema:
- `plan_outcome`: `plan_ready`, `blocked_missing_context`, `scope_changed_retry`,
  or `scope_changed_escalate`
- `approach_summary`: summary of the implementation approach
- `decisions`: (optional) JSON array of `{choice, rationale, alternatives_considered}`

## implementation

Write code, commit changes, and ensure tests pass. The gate checks: not on main,
commits exist beyond main, and `go test ./...` passes. On resume, if all gate
conditions are met, the gate auto-advances.

Follow the plan from `wip/{{ARTIFACT_PREFIX}}_plan.md`. For each change:
1. Make the code change
2. Run relevant tests to verify
3. Commit with a clear message

Re-read the plan periodically to stay on track. Check git log and git status to
understand what has already been committed if resuming a previous session.

If tests fail after changes, you may self-loop with `partial_tests_failing_retry`
up to 3 times to fix them. After 3 failed retries, switch to
`partial_tests_failing_escalate` to route to done_blocked. Submit `blocked` if
implementation cannot proceed due to external dependencies or missing information.

Capture non-obvious judgment calls in the `decisions` field: assumptions about
API behavior, tradeoff choices between approaches, or pivots from the original
plan. These decisions are recorded in the event log and surfaced to the user.

Evidence schema:
- `implementation_status`: `complete`, `partial_tests_failing_retry`,
  `partial_tests_failing_escalate`, or `blocked`
- `rationale`: what was accomplished or what is blocking
- `decisions`: (optional) JSON array of `{choice, rationale, alternatives_considered}`

## finalization

Create a summary file and verify everything is clean. The gate checks for
`wip/{{ARTIFACT_PREFIX}}_summary.md` and that tests pass. On resume, if both
conditions are met, the gate auto-advances.

Review all changes made during implementation:
1. Run the full test suite and verify all tests pass
2. Check for any temporary code, debug output, or incomplete changes
3. Write a summary to `wip/{{ARTIFACT_PREFIX}}_summary.md` covering:
   - What was changed and why
   - Any deferred items or known limitations
   - Test coverage for the changes
4. Clean up wip/ artifacts that should not be in the PR

Submit `finalization_status: ready_for_pr` when everything is clean,
`deferred_items_noted` when proceeding with documented limitations, or
`issues_found` to return to implementation for fixes.

Evidence schema:
- `finalization_status`: `ready_for_pr`, `deferred_items_noted`, or `issues_found`

## pr_creation

Create the pull request. This is an irreversible externally-visible action that
requires your judgment -- there is no automated gate.

Check if a PR already exists for this branch:
`gh pr list --head $(git rev-parse --abbrev-ref HEAD)`

If no PR exists, create one with a clear title and description covering the
changes, test results, and any deferred items. If a PR already exists, submit
`pr_status: created` with the existing PR URL.

On failure, you may self-loop with `creation_failed_retry` up to 3 times. After
3 retries, switch to `creation_failed_escalate` to route to done_blocked.

Evidence schema:
- `pr_status`: `created`, `creation_failed_retry`, or `creation_failed_escalate`
- `pr_url`: URL of the created or existing pull request

## ci_monitor

Wait for CI checks to pass on the pull request. The gate polls CI status and
auto-advances when all checks succeed.

If the gate fails, check the CI output:
- If failures are fixable (test flakes, lint issues), fix them, push, and submit
  `ci_outcome: failing_fixed`
- If all checks pass after your review, submit `ci_outcome: passing`
- If failures are not resolvable (infrastructure issues, unrelated failures in
  shared CI), submit `ci_outcome: failing_unresolvable` with rationale

Evidence schema:
- `ci_outcome`: `passing`, `failing_fixed`, or `failing_unresolvable`
- `rationale`: what was fixed or why failures are unresolvable

## done

The workflow is complete. The PR has been created and CI is passing.

## done_blocked

The workflow reached a blocking condition that requires human intervention.
This state is reachable from multiple points in the workflow: analysis
(scope too large or missing context), implementation (persistent test failures
or external blockers), pr_creation (repeated creation failures), ci_monitor
(unresolvable CI failures), and introspection (issue superseded).

If the blocker has been resolved externally, use `koto rewind <name>` to walk
back to the originating state. `koto rewind` rewinds one step per call; call
it repeatedly to reach a non-adjacent origin state. For example, if blocked
from ci_monitor, one rewind reaches pr_creation; from analysis, one rewind
reaches the previous state in the path.
