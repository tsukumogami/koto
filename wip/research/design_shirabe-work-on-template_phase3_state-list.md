# Phase 3 Research: Full State List with Gate-with-Evidence-Fallback

## Questions Investigated

- What shell commands can serve as deterministic gates for each phase of work-on and just-do-it?
- Which states are genuinely auto-advanceable vs. requiring agent evidence?
- Where do the two entry paths converge?
- How does the introspection skip pattern fit into the gate-with-evidence-fallback model?
- What evidence fields are meaningful decision records (not just boolean confirmations)?
- How does the just-do-it jury routing interact with koto's terminal state model?

---

## State List

### State 0: `entry`

**Path:** both
**Directive:**
If you have a GitHub issue number, this is a work-on workflow. If you have a free-form task description, this is a just-do-it workflow. Record which path you're taking and any initial parameters.

**Gate:** none
**Gate passes when:** n/a
**Evidence schema:**
```yaml
accepts:
  workflow_type:
    type: enum
    values: [work-on, just-do-it]
    required: true
  issue_number:
    type: string
    required: false  # required when workflow_type=work-on; validated downstream
  task_description:
    type: string
    required: false  # required when workflow_type=just-do-it
transitions:
  - target: jury_validation
    when:
      workflow_type: just-do-it
  - target: context_injection
    when:
      workflow_type: work-on
```
**Auto-advance:** no — the divergence point between paths; routing requires agent input
**Notes:** The conditional `required` constraint (issue_number required only for work-on) can't be enforced by the template format's per-field `required` flag. The directive must instruct the agent; the gate is evidence-only. Alternatively, collapse this state into the first path-specific state and use a template variable to set the initial state — but that requires two template files. Single template with an entry router is the cleaner approach given the design constraint.

---

### State 1a: `jury_validation`

**Path:** just-do-it only
**Directive:**
Assess the task description. Is it clear enough to implement without a design phase? Is it scoped to a single session? Apply the jury:
- `ready`: task is clear, scoped, implementable
- `needs_design`: task requires architecture decisions; recommend `/design` first
- `needs_breakdown`: task is too large; recommend `/plan` first
- `ambiguous`: task description is unclear; ask the user to clarify

Submit your assessment. If the result is anything other than `ready`, the workflow will terminate with a recommendation.

**Gate:** none
**Gate passes when:** n/a
**Evidence schema:**
```yaml
accepts:
  jury_verdict:
    type: enum
    values: [ready, needs_design, needs_breakdown, ambiguous]
    required: true
  rationale:
    type: string
    required: true
transitions:
  - target: jury_exit
    when:
      jury_verdict: needs_design
  - target: jury_exit
    when:
      jury_verdict: needs_breakdown
  - target: jury_exit
    when:
      jury_verdict: ambiguous
  - target: research
    when:
      jury_verdict: ready
```
**Auto-advance:** no — routing branches on jury verdict
**Notes:** `jury_exit` is a terminal state, not a failure state. The rationale field is required here because this is the primary decision record for why a task was rejected. The three non-ready verdicts all route to the same terminal state; the rationale field distinguishes them in the event log. Compiler mutual exclusivity: all four transitions share `jury_verdict` with disjoint values — valid.

---

### State 1b: `jury_exit`

**Path:** just-do-it only (non-ready verdicts)
**Directive:**
The jury determined this task isn't ready for direct implementation. Communicate the verdict and rationale to the user. Suggest the appropriate next skill based on the verdict recorded in the evidence.

**Gate:** none
**Terminal:** true
**Evidence schema:** none
**Auto-advance:** yes (terminal state — loop stops)
**Notes:** Terminal state, not evidence-gated. The event log already contains the jury verdict from `jury_validation`. This state exists so the directive can surface a useful message rather than leaving the agent in a stopped state with no instruction.

---

### State 2: `research`

**Path:** just-do-it only
**Directive:**
Gather lightweight context for the task: relevant files, recent git history, existing patterns in the codebase. Do not create any plan files yet. Write a one-paragraph summary of what you found. This is background research, not planning.

**Gate:** none
**Gate passes when:** n/a
**Evidence schema:**
```yaml
accepts:
  context_gathered:
    type: enum
    values: [sufficient, insufficient]
    required: true
  context_summary:
    type: string
    required: true
transitions:
  - target: setup
    when:
      context_gathered: sufficient
  - target: setup
    when:
      context_gathered: insufficient
```
**Auto-advance:** no — agent must submit context summary before proceeding
**Notes:** Both transitions target `setup`, making this functionally an auto-advancing state with mandatory evidence capture. The `insufficient` value is preserved because it's meaningful in the event log — it records that the agent couldn't fully gather context, which is useful if the implementation later encounters unexpected complexity. An alternative design omits the enum and makes this a pure auto-advance state with the directive text as the only record. Judgment call: keep the context summary as required evidence since it's the only structured record of pre-implementation research in the just-do-it path.

---

### State 3: `context_injection`

**Path:** work-on only
**Directive:**
Load the GitHub issue context. Read the issue body, comments, linked design docs, and any referenced IMPLEMENTATION_CONTEXT.md. Identify the key design decisions already made and the work the issue specifies. Summarize what you found.

**Gate:**
```yaml
gates:
  issue_accessible:
    type: command
    command: "gh issue view ${ISSUE_NUMBER} --json number --jq .number > /dev/null 2>&1"
    timeout: 15
```
**Gate passes when:** GitHub CLI can read the issue (auth configured, issue exists)
**Evidence schema (fallback):**
```yaml
accepts:
  context_loaded:
    type: enum
    values: [loaded, issue_not_accessible, context_incomplete]
    required: true
  context_summary:
    type: string
    required: true
  design_references:
    type: string
    required: false
transitions:
  - target: setup
```
**Auto-advance:** yes, if gate passes — agent reads context per directive, then calls `koto next` to advance
**Notes:** The gate verifies the issue is accessible before the agent spends time on context gathering. If the gate fails (no auth, issue closed/missing), the agent submits `context_loaded: issue_not_accessible` with explanation in `context_summary`. The single unconditional transition means no routing decision — the evidence is a decision record, not a routing signal. `design_references` captures links to design docs, useful for audit.

---

### State 4: `setup`

**Path:** both (convergence begins here for just-do-it; work-on arrives via context_injection)
**Directive:**
Create a feature branch for this work. Branch naming: `issue-<N>-<slug>` for work-on, `task-<slug>` for just-do-it. Establish a baseline: run existing tests, check current state. Create `wip/issue_<N>_baseline.md` (work-on) or `wip/task_<slug>_baseline.md` (just-do-it) documenting what you found.

**Gate:**
```yaml
gates:
  on_feature_branch:
    type: command
    command: "git rev-parse --abbrev-ref HEAD | grep -vE '^(main|master|develop)$'"
    timeout: 5
  baseline_file_exists:
    type: command
    command: "test -f wip/*_baseline.md"
    timeout: 5
```
**Gate passes when:** current branch is not a protected branch AND a baseline file exists
**Evidence schema (fallback):**
```yaml
accepts:
  branch_created:
    type: enum
    values: [created, reused_existing]
    required: true
  branch_name:
    type: string
    required: true
  baseline_outcome:
    type: enum
    values: [clean, existing_failures, build_broken]
    required: true
  rationale:
    type: string
    required: false
transitions:
  - target: staleness_check
    when:
      workflow_type: work-on
  - target: analysis
    when:
      workflow_type: work-on
```
**Auto-advance:** yes, if both gates pass — the gates verify the branch exists and the baseline file was created
**Notes:** Two separate gate commands because they check different things. `baseline_outcome` is a meaningful enum: `existing_failures` means tests were already failing before the agent touched anything, which matters for implementation. `build_broken` is a stop condition — the agent should record this and surface it. The routing after setup diverges again: work-on paths to `staleness_check`, just-do-it paths directly to `analysis`. This requires the `workflow_type` evidence from the `entry` state to still be in scope — which it is, since evidence merges across the epoch. Alternative: use two separate "setup complete" states. Judgment call: single setup state with evidence-routed exit keeps the template smaller.

---

### State 5: `staleness_check`

**Path:** work-on only
**Directive:**
Assess whether the codebase has changed significantly since this issue was opened. Check: git log since issue creation date, any merges to the files this issue will touch, any superseding issues or PRs. Determine if introspection is warranted.

**Gate:**
```yaml
gates:
  staleness_script:
    type: command
    command: "scripts/check-staleness.sh ${ISSUE_NUMBER}"
    timeout: 30
```
**Gate passes when:** script exits 0 AND outputs `{"introspection_recommended": false}` — but koto gates only check exit code, not output content. The gate here is weaker: it verifies the script ran successfully.
**Evidence schema (fallback or always):**
```yaml
accepts:
  staleness_signal:
    type: enum
    values: [fresh, stale_skip_introspection, stale_requires_introspection]
    required: true
  staleness_details:
    type: string
    required: false
transitions:
  - target: introspection
    when:
      staleness_signal: stale_requires_introspection
  - target: analysis
    when:
      staleness_signal: fresh
  - target: analysis
    when:
      staleness_signal: stale_skip_introspection
```
**Auto-advance:** no — this is a genuine branch point; the agent must classify the staleness
**Notes:** This is one of the most important evidence states. Even if the gate passes (script ran), the agent must still interpret the output and submit a verdict. The routing decision here is agent judgment, not mechanical — the script may recommend introspection, but the agent decides. `stale_skip_introspection` captures "stale but introspection not warranted" as a distinct record from `fresh`. Compiler mutual exclusivity: `fresh` and `stale_skip_introspection` both route to `analysis` — this works because they share the `staleness_signal` field with different values.

---

### State 6: `introspection`

**Path:** work-on only (reached when staleness_signal=stale_requires_introspection)
**Directive:**
The codebase has changed significantly since this issue was opened. Run a deep introspection pass: re-read the issue, check what changed, identify whether the implementation approach needs updating. Write findings to `wip/issue_<N>_introspection.md`. Update your understanding before proceeding to analysis.

**Gate:**
```yaml
gates:
  introspection_file_exists:
    type: command
    command: "test -f wip/issue_${ISSUE_NUMBER}_introspection.md"
    timeout: 5
```
**Gate passes when:** introspection artifact file exists
**Evidence schema (fallback):**
```yaml
accepts:
  introspection_outcome:
    type: enum
    values: [approach_unchanged, approach_updated, issue_superseded]
    required: true
  rationale:
    type: string
    required: true
transitions:
  - target: analysis
```
**Auto-advance:** yes, if gate passes — the artifact file is the verification
**Notes:** `issue_superseded` is a meaningful stopping case: if introspection reveals the issue was already addressed by other work, the agent should surface this. However, modeling a terminal exit from this state adds complexity. Simplest approach: route `issue_superseded` to a dedicated `introspection_exit` terminal state, or document that the agent should call `koto cancel` if the issue is superseded. For phase 1, route unconditionally to `analysis` and let the rationale field carry the superseded signal — the analysis state will surface it.

---

### State 7: `analysis`

**Path:** both (full convergence point)
**Directive:**
Research and create an implementation plan. Read relevant code, understand the interfaces, identify risks. Write the plan to `wip/issue_<N>_plan.md` (work-on) or `wip/task_<slug>_plan.md` (just-do-it). The plan must include: approach, files to modify, test strategy, edge cases. Review it before submitting.

**Gate:**
```yaml
gates:
  plan_file_exists:
    type: command
    command: "test -f wip/*_plan.md"
    timeout: 5
```
**Gate passes when:** a plan file exists
**Evidence schema (fallback):**
```yaml
accepts:
  plan_outcome:
    type: enum
    values: [plan_ready, blocked_missing_context, scope_changed]
    required: true
  approach_summary:
    type: string
    required: true
  rationale:
    type: string
    required: false
transitions:
  - target: implementation
    when:
      plan_outcome: plan_ready
  - target: analysis
    when:
      plan_outcome: scope_changed
  - target: done_blocked
    when:
      plan_outcome: blocked_missing_context
```
**Auto-advance:** yes, if gate passes and agent submits `plan_outcome: plan_ready`
**Notes:** `scope_changed` creates a self-loop — the agent re-enters analysis after updating the plan. This is a rare case (scope expands mid-analysis) but the loop is valid because the next call to `koto next` will re-evaluate the gate. The `blocked_missing_context` path routes to a terminal state to record a clean stop rather than leaving the workflow abandoned. The `approach_summary` field is a required decision record: it captures the core approach in the event log independent of the plan file content.

---

### State 8: `implementation`

**Path:** both
**Directive:**
Implement the plan. Write code, run tests, iterate. Commit as you go with descriptive commit messages. When implementation is complete and tests pass, submit evidence. Do not submit until all tests pass and the implementation matches the plan.

**Gate:**
```yaml
gates:
  on_feature_branch:
    type: command
    command: "git rev-parse --abbrev-ref HEAD | grep -vE '^(main|master|develop)$'"
    timeout: 5
  has_commits:
    type: command
    command: "git log --oneline main..HEAD | grep -c ."
    timeout: 5
  tests_pass:
    type: command
    command: "go test ./... 2>&1 | tail -1 | grep -q '^ok\\|^---'"
    timeout: 120
```
**Gate passes when:** still on feature branch, has at least one commit beyond main, tests pass
**Evidence schema (fallback):**
```yaml
accepts:
  implementation_status:
    type: enum
    values: [complete, partial_tests_failing, blocked]
    required: true
  commit_count:
    type: string
    required: false
  rationale:
    type: string
    required: true
transitions:
  - target: finalization
    when:
      implementation_status: complete
  - target: implementation
    when:
      implementation_status: partial_tests_failing
  - target: done_blocked
    when:
      implementation_status: blocked
```
**Auto-advance:** yes, if all three gates pass — the gates verify the work is done mechanically
**Notes:** The `tests_pass` gate command is language-specific (Go shown here). The template will need a configurable test command, likely via a template variable. The `partial_tests_failing` self-loop allows the agent to retry without koto involvement — the agent calls `koto next` which re-evaluates gates, and if tests still fail, returns `GateBlocked`. The self-loop exists for the evidence-fallback path where the agent submits partial completion and then keeps working. `commit_count` is optional evidence — useful for audit but not required for routing.

---

### State 9: `finalization`

**Path:** both
**Directive:**
Prepare for PR submission. Clean up `wip/` files no longer needed. Create `wip/issue_<N>_summary.md` documenting what was implemented, what was deferred, and any known issues. Verify the implementation against the original issue or task description. Run a final check: tests pass, no debug artifacts, commits are clean.

**Gate:**
```yaml
gates:
  summary_file_exists:
    type: command
    command: "test -f wip/*_summary.md"
    timeout: 5
  tests_still_pass:
    type: command
    command: "go test ./... 2>&1 | tail -1 | grep -q '^ok\\|^---'"
    timeout: 120
```
**Gate passes when:** summary file exists AND tests still pass
**Evidence schema (fallback):**
```yaml
accepts:
  finalization_status:
    type: enum
    values: [ready_for_pr, deferred_items_noted, issues_found]
    required: true
  rationale:
    type: string
    required: false
transitions:
  - target: pr_creation
    when:
      finalization_status: ready_for_pr
  - target: pr_creation
    when:
      finalization_status: deferred_items_noted
  - target: implementation
    when:
      finalization_status: issues_found
```
**Auto-advance:** yes, if both gates pass
**Notes:** `deferred_items_noted` routes to `pr_creation` because the agent explicitly acknowledged deferred scope — the PR will note those items. `issues_found` routes back to `implementation` for a fix cycle. The summary file gate ensures the agent created the artifact before advancing.

---

### State 10: `pr_creation`

**Path:** both
**Directive:**
Create a pull request. Title should describe the change clearly. Body should reference the issue (if work-on), summarize the approach, and note any deferred items. After creating the PR, record the PR URL.

**Gate:**
```yaml
gates:
  on_feature_branch:
    type: command
    command: "git rev-parse --abbrev-ref HEAD | grep -vE '^(main|master|develop)$'"
    timeout: 5
  branch_pushed:
    type: command
    command: "git status -sb | grep -q '\\[ahead\\|\\[behind'"
    timeout: 10
```
**Gate passes when:** still on feature branch AND branch has been pushed to remote (or has commits to push)
**Evidence schema (fallback):**
```yaml
accepts:
  pr_status:
    type: enum
    values: [created, creation_failed]
    required: true
  pr_url:
    type: string
    required: false
  rationale:
    type: string
    required: false
transitions:
  - target: ci_monitor
    when:
      pr_status: created
  - target: pr_creation
    when:
      pr_status: creation_failed
```
**Auto-advance:** no — PR creation requires the agent to actually call `gh pr create`; there's no pre-creation gate that proves the PR exists
**Notes:** The self-loop on `creation_failed` allows retry. The `branch_pushed` gate is imprecise — a better gate would be `gh pr view --json number --jq .number > /dev/null 2>&1` to check if a PR already exists for this branch, which would allow auto-advance if a PR was already created (e.g., after a resume). This is a good example of a gate that's close but not quite right — the evidence fallback path handles the actual PR creation action.

---

### State 11: `ci_monitor`

**Path:** both
**Directive:**
Monitor CI until all checks pass. Poll `gh pr checks <PR-NUMBER>` periodically. If a check fails, diagnose the failure, fix it, push a new commit, and continue monitoring. Do not submit evidence until CI is fully green.

**Gate:**
```yaml
gates:
  ci_passing:
    type: command
    command: "gh pr checks --json state --jq '[.[] | .state] | all(. == \"SUCCESS\")' | grep -q true"
    timeout: 30
```
**Gate passes when:** all CI checks report SUCCESS state
**Evidence schema (fallback):**
```yaml
accepts:
  ci_outcome:
    type: enum
    values: [passing, failing_fixed, failing_unresolvable]
    required: true
  rationale:
    type: string
    required: false
transitions:
  - target: done
    when:
      ci_outcome: passing
  - target: done
    when:
      ci_outcome: failing_fixed
  - target: done_blocked
    when:
      ci_outcome: failing_unresolvable
```
**Auto-advance:** yes, if CI gate passes — the gate is the definitive check here
**Notes:** This is the strongest gate in the template: it mechanically verifies CI status via the GitHub API. When the gate passes, koto auto-advances to `done` without requiring evidence. When CI is still running or failing, the gate blocks and the agent waits. The `failing_fixed` value in the evidence schema captures "I had to fix something and re-push" — meaningful for audit. `failing_unresolvable` routes to the blocked terminal state.

---

### State 12: `done`

**Path:** both
**Terminal:** true
**Directive:**
The workflow is complete. The PR has been created and CI is passing.

**Gate:** none
**Evidence schema:** none
**Auto-advance:** yes (terminal — loop stops)

---

### State 13: `done_blocked`

**Path:** both
**Terminal:** true
**Directive:**
The workflow reached a blocking condition that requires human intervention. Review the event log (`koto query`) to understand what was blocked and why.

**Gate:** none
**Evidence schema:** none
**Auto-advance:** yes (terminal — loop stops)
**Notes:** Consolidates all "blocked exit" cases: `blocked_missing_context` from analysis, `blocked` from implementation, `failing_unresolvable` from CI. A single blocked terminal state keeps the template clean; the rationale in the upstream evidence events carries the distinction.

---

## Convergence Point

The two paths converge at **State 7: `analysis`**.

- work-on path: `entry` → `context_injection` → `setup` → `staleness_check` → [`introspection` →] `analysis`
- just-do-it path: `entry` → `jury_validation` → [`jury_exit` (terminal)] → `research` → `setup` → `analysis`

Both paths arrive at `analysis` with the same preconditions: a feature branch exists, a baseline file exists, and any pre-analysis context has been gathered. From `analysis` onward, the states are identical.

The `setup` state is shared but uses `workflow_type` evidence (still in scope from `entry`) to route to different post-setup destinations: work-on goes to `staleness_check`, just-do-it goes directly to `analysis`.

---

## Implications for Design

**Gate reliability varies significantly by state.** The strongest gates (`ci_passing`, `tests_pass`, `on_feature_branch`) are mechanically reliable. The weakest gates (`branch_pushed`, `plan_file_exists`) are necessary but incomplete — they verify artifact existence, not correctness. Template authors must accept that gates verify completion signals, not quality.

**The `workflow_type` evidence travels across states.** The `entry` state captures `workflow_type`, and `setup` uses it for routing. This works because evidence merges within the epoch — the `workflow_type` value submitted at `entry` is still accessible at `setup`. This is a consequence of koto's last-write-wins evidence merging model and is worth documenting explicitly in the template.

**The staleness_check state cannot auto-advance even with a passing gate.** The gate can verify the script ran, but the script's output (recommend introspection or not) cannot be checked by the gate — gates only check exit codes. The agent must always submit evidence here. This is a fundamental limit of command gates: they're boolean pass/fail, not output-matching.

**Jury routing produces a terminal state, not a redirect.** The `jury_exit` terminal state with a directive is the right model for just-do-it's non-ready verdicts. Using `koto cancel` from within a directive is awkward; a purpose-built terminal state with a message directive is cleaner.

**Test command gates are language-specific.** The `tests_pass` and `tests_still_pass` gates use `go test ./...` — the right answer is a template variable (`TEST_COMMAND`) with a configurable default. This is a template authoring detail, not a design problem.

**Self-loops require careful gate design.** States with self-loop transitions (`analysis` on `scope_changed`, `implementation` on `partial_tests_failing`) rely on the gate re-evaluating on re-entry. This works correctly with koto's advancement loop: gate evaluation happens on every `koto next` call, so a re-entered state with a passing gate auto-advances out.

---

## Surprises

**The gate-with-evidence-fallback pattern nearly eliminates pure auto-advancing states.** In the auto-advancing advocate's design, most states had no evidence. In the gate-with-evidence-fallback design, even states that auto-advance when gates pass still have evidence schemas for the fallback path. This means nearly every state has a defined `accepts` block — the difference from fine-grained is that these blocks are only invoked when the gate fails, not on every transition.

**`workflow_type` must be in scope at `setup` for routing to work.** This works with koto's evidence merging model, but it's a non-obvious dependency. The template should document that `entry` evidence persists to `setup`. If a future koto version scopes evidence to individual state epochs, this routing breaks.

**The introspection state is reachable from a gate-passing setup state.** The `staleness_check` state cannot auto-advance regardless of gate outcome because the routing decision is agent judgment. This means work-on has at least one mandatory evidence submission between `entry` and `analysis`, even in the happy path. That's appropriate — staleness assessment is a genuine decision record.

**The `done_blocked` terminal state consolidates multiple failure modes.** Three different states can route here (`analysis`, `implementation`, `ci_monitor`). The event log preserves the distinction via the evidence in the state that transitioned to `done_blocked`. This is cleaner than three separate blocked terminal states.

---

## Summary

The merged template has 13 states (including 3 terminal states), with the two paths diverging at `entry` and converging at `analysis` (State 7). Gate-with-evidence-fallback applies cleanly: 6 states have command gates that enable auto-advancement when the work is mechanically verifiable (`context_injection`, `setup`, `introspection`, `analysis`, `finalization`, `ci_monitor`), while 4 states are always evidence-gated because they represent genuine branching decisions (`entry`, `jury_validation`, `staleness_check`, `pr_creation`). The critical finding is that command gates check exit codes only — script output cannot route transitions — so the `staleness_check` state must always require agent evidence regardless of whether a staleness script ran successfully, making it a permanent evidence gate rather than a gate-with-fallback state.
