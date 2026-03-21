# Advocate: Fine-grained evidence-gated template

## Approach Description

Merge shirabe's work-on and just-do-it skills into a single koto template where every skill phase becomes an explicit workflow state. The agent-driven workflow enforces phase sequencing through koto's state machine, with structured evidence submission at each phase boundary using `koto next --with-data`. Branching at the staleness check routes directly to analysis without forcing an introspection state, using `accepts`/`when` conditions. This maximizes enforcement granularity — koto knows the exact phase at all times and prevents out-of-order execution.

## Investigation

### Phase Structure Mapping

The work-on skill has 7 sequential phases:
- Phase 0: Context Injection (ephemeral `IMPLEMENTATION_CONTEXT.md`, no artifact needed)
- Phase 1: Setup (creates `wip/issue_<N>_baseline.md`)
- Phase 2: Introspection (creates `wip/issue_<N>_introspection.md` if staleness check recommends)
- Phase 3: Analysis (creates `wip/issue_<N>_plan.md`)
- Phase 4: Implementation (working code + commits)
- Phase 5: Finalization (creates `wip/issue_<N>_summary.md`)
- Phase 6: Pull Request (CI monitoring until passing)

The just-do-it skill has 6 phases with the same structure but no introspection:
- Phase 1: Initial Jury (validation, no artifact)
- Phase 2: Research (lightweight context gathering, no artifact)
- Phase 3: Validation Jury (no artifact)
- Phase 4: Setup (branch creation)
- Phase 5: Implement (code changes)
- Phase 6: PR (creation and monitoring)

Both skills produce identical outcomes (PRs with passing CI) but differ in entry point and context gathering.

### State List (Fine-Grained Approach)

Merging into a single template creates 9 states for work-on and 8 for just-do-it (sharing the same merge):

**Universal states (both entry points):**
1. `setup` - Create feature branch, establish baseline
2. `analysis` - Research and planning (conditionally skip introspection)
3. `implementation` - Iterative coding with validation
4. `finalization` - Summary, cleanup, verification
5. `pr_creation` - Submit for review
6. `ci_monitor` - Wait for CI completion (terminal)

**Work-on only states:**
1. `context_injection` - Surface design context
2. `setup` (as above)
3. `introspection_check` - Determine if staleness-triggered introspection needed (routes based on evidence)

**Just-do-it only states:**
1. `initial_jury` - Validate task clarity
2. `research` - Lightweight context gathering
3. `validation_jury` - Confirm task scope

**Merged entry point (handles both workflows):**
- `entry_point` - Accept optional GitHub issue, route to context_injection (work-on) or initial_jury (just-do-it)

### State Transition Diagram (Fine-Grained Approach)

```
entry_point
  │
  ├─ when: {workflow_type: work-on}
  │    └─> context_injection
  │         └─> setup
  │             └─> introspection_check
  │                 ├─ when: {staleness: not_required}
  │                 │    └─> analysis
  │                 └─ when: {staleness: required}
  │                      └─> [hypothetical introspection state - SKIPPED]
  │                          └─> analysis
  │
  └─ when: {workflow_type: just-do-it}
       └─> initial_jury
            └─> research
                 └─> validation_jury
                     └─> setup
                          └─> analysis
                               └─> implementation
                                    └─> finalization
                                         └─> pr_creation
                                              └─> ci_monitor
                                                   └─> (terminal)
```

The staleness branch is the critical skip pattern: if `staleness_check_output` shows `introspection_recommended: false`, the agent transitions directly from `introspection_check` to `analysis`, bypassing any introspection state altogether.

### Evidence Schema per State

**entry_point accepts:**
```yaml
accepts:
  workflow_type:
    type: enum
    values: [work-on, just-do-it]
    required: true
  issue_number:
    type: string
    required: false  # Required if workflow_type: work-on
  task_description:
    type: string
    required: false  # Required if workflow_type: just-do-it
```

**context_injection accepts:**
```yaml
accepts:
  context_summary:
    type: string
    required: true
  design_dependencies: []  # Optional array of issue/design references
```

**setup accepts:**
```yaml
accepts:
  branch_name:
    type: string
    required: true
  baseline_established:
    type: boolean
    required: true
```

**introspection_check accepts:**
```yaml
accepts:
  staleness_signal:
    type: enum
    values: [fresh, stale_requires_introspection, stale_no_introspection]
    required: true
  staleness_details:
    type: string
    required: false
```

**analysis accepts:**
```yaml
accepts:
  plan_created:
    type: boolean
    required: true
  analysis_summary:
    type: string
    required: true
  file_count:
    type: integer
    required: true
```

**implementation accepts:**
```yaml
accepts:
  commits_pushed:
    type: boolean
    required: true
  tests_passing:
    type: boolean
    required: true
  implementation_complete:
    type: boolean
    required: true
```

**finalization accepts:**
```yaml
accepts:
  summary_created:
    type: boolean
    required: true
  wip_cleaned:
    type: boolean
    required: true
  quality_verified:
    type: boolean
    required: true
```

**pr_creation accepts:**
```yaml
accepts:
  pr_url:
    type: string
    required: true
  pr_number:
    type: integer
    required: true
```

**ci_monitor accepts:**
```yaml
accepts:
  ci_status:
    type: enum
    values: [passing, failing, pending]
    required: true
  final_check_timestamp:
    type: string
    required: false
```

### Mutual Exclusivity for Introspection Branch

The `introspection_check` state's transitions:

```yaml
transitions:
  - target: analysis
    when:
      staleness_signal: fresh
  - target: analysis
    when:
      staleness_signal: stale_no_introspection
  - target: [hypothetical introspection state]
    when:
      staleness_signal: stale_requires_introspection
```

Compiler validates: these three transitions are provably exclusive because they all share the `staleness_signal` field with disjoint values (fresh vs stale_no_introspection vs stale_requires_introspection).

However, the SKIPPED introspection state reveals the design decision: we don't model introspection as a state at all. The introspection agent runs as a task subprocess during the `introspection_check` state's directive execution, not as a koto state. The agent completes introspection, writes the artifact, and then submits evidence that routing the transition. This keeps introspection off the critical path in koto's state machine.

### Evidence Submission Flow

**Work-on entry point:**

```
Agent invokes skill with issue #71
  │
  └─> koto next --with-data '{"workflow_type":"work-on","issue_number":"71"}'
       └─ Enters context_injection state
            └─ (Context extraction script runs in directive)
                 └─ Agent reads IMPLEMENTATION_CONTEXT.md
                      └─ Agent submits: koto next --with-data '{"context_summary":"..."}'
                           └─ Enters setup state
                                └─ (Branch creation in directive)
                                     └─ Agent submits: koto next --with-data '{"branch_name":"...","baseline_established":true}'
                                          └─ Enters introspection_check state
                                               └─ (Staleness script runs in directive)
                                                    └─ Agent submits based on output:
                                                         koto next --with-data '{"staleness_signal":"fresh"}'
                                                         └─ Routes to analysis (skipping introspection)
```

**Just-do-it entry point:**

```
Agent invokes skill with task description
  │
  └─ koto next --with-data '{"workflow_type":"just-do-it","task_description":"..."}'
       └─ Enters initial_jury state
            └─ (Jury evaluation in directive)
                 └─ Agent submits jury result
                      └─ Enters research state
                           └─ (Lightweight research in directive)
                                └─ Agent submits: koto next --with-data '{"research_complete":true}'
                                     └─ Enters validation_jury state
                                          └─ (Final scope check)
                                               └─ Agent submits approval
                                                    └─ Enters setup state
```

Both paths converge at `analysis`.

## Strengths

1. **Maximum enforcement granularity**: Every phase is an explicit state. koto knows exactly which phase is executing. Mid-session interruption is unambiguous — check `koto status` and see the exact state.

2. **Evidence-driven routing eliminates special cases**: The staleness branch uses standard `accepts`/`when` machinery. No custom skip logic in the agent code. The compiler validates mutual exclusivity of branching conditions at compile time.

3. **Workflow auditing and resumability**: The event log shows exactly when each phase transitioned. If a phase failed, the event log records what evidence was submitted. No ambiguity about whether introspection completed or was skipped.

4. **Clean separation of concerns**: koto enforces sequencing, the agent executes directives. The agent knows exactly which state it's in and what evidence to submit next. Each state's `accepts` block is self-documenting.

5. **No new koto concepts required**: Uses only `accepts`/`when` (already designed), `integration` field (already designed), and the auto-advancement engine (already designed). No new subcommands, no new CLI flags beyond existing `--with-data`.

6. **Git history integration**: Each transition becomes an explicit commit (agent creates commits as artifacts during implementation phase). The event log maps to git commits, creating an auditable chain.

7. **Handles both entry points elegantly**: A single template with conditional entry states, not two separate skills. Future entry points (e.g., `/triaged-issue`, `/bulk-task`) extend the template by adding new entry states.

## Weaknesses

1. **Verbose template definition**: 9 states × 7+ fields per `accepts` block = hundreds of lines of YAML. The hello-koto example is 33 lines; this template is 500+. Template authors need discipline to keep it maintainable.

2. **Evidence explosion**: Every state requires its own evidence submission. The work-on skill's Phase 3 (analysis) produces a `wip/issue_<N>_plan.md` file, but koto never sees it — the agent must extract key fields and submit them as evidence. If analysis produces 10 different decision points, all 10 must be in the `accepts` schema and submitted.

3. **Over-specification of introspection skip**: The staleness check is deterministic (`check-staleness.sh` returns JSON). But the agent still must invoke the script, parse it, and submit the result. The template can't express "automatically route based on script output" — it must route based on agent-submitted evidence of what the script said. This is technically correct but adds a layer of indirection.

4. **No template-level loop or retry**: If implementation fails, the template has no way to model "loop back to analysis." The agent must manually call `koto next --to analysis` to rework. Work-on currently supports resuming from any phase by checking artifact existence; this template enforces forward-only movement unless the agent manually backtracks.

5. **Jury decision outcomes not modeled**: Just-do-it's Phase 1 (initial jury) produces a 3-agent decision (needs-design / needs-breakdown / ready), but only one path (ready) enters the template. If an agent votes "needs-design," the skill stops with a routing message. This template would need separate `jury_needs_design` and `jury_needs_breakdown` states, duplicating the jury logic.

6. **Integration closure unavailability**: The template's CI monitoring state (`ci_monitor`) could use an `integration` field to invoke a koto-integrated CI checker, but the integration runner is deferred. The agent must check CI status manually and submit evidence, defeating the automation value of koto.

7. **Atomic PR creation**: The PR creation state doesn't block on passing CI — it creates the PR, then routes to `ci_monitor`. If PR creation fails (GitHub API error, branch protection), the state machine gets stuck. No gate system to enforce preconditions before PR creation.

## Deal-Breaker Risks

1. **Agent subagent orchestration**: Work-on's Phase 2 (introspection) and Phase 3 (analysis) spawn subagents to save context tokens. The template itself has no way to express "spawn a subagent and block until it finishes and writes `wip/issue_<N>_plan.md`." The koto template would need to model subagent invocation, but koto's integration runner (deferred) doesn't handle that.

   **Mitigation**: The agent running the template could remain responsible for spawning subagents during directive execution, not treating it as a koto responsibility. But this means koto sees an external interaction it can't control or verify.

2. **Evidence bloat from non-deterministic steps**: Phase 4 (implementation) is iterative — the agent may commit multiple times, run tests multiple times, fix failures, and re-run. The template's implementation state accepts `{commits_pushed: bool, tests_passing: bool, implementation_complete: bool}`, but the actual iteration history lives in git commits, not in koto. koto sees only the final evidence submission, not the iteration count or failure recovery.

   **Mitigation**: If iteration history matters for auditing, the agent must submit intermediate evidence (e.g., `attempt_count: 3, failures_recovered: 2`). This turns iteration detail into schema bloat.

3. **Jury routing back to template state**: If just-do-it's Phase 3 validation jury votes "scope too large," the skill stops and recommends `/work-on` instead. In a template model, this would be a transition to a `scope_too_large_stop` state (terminal). But the skill's actual behavior is to present the recommendation to the user (via `AskUserQuestion`) and wait for confirmation before stopping. A template can't express "ask the user a question and branch on their response" — that's outside koto's model.

   **Mitigation**: Model jury outcomes as transitions rather than user interaction. Accept that the template is less interactive than the skill.

4. **Resume from arbitrary phase**: Work-on's resume logic checks for artifacts and commits to determine the current phase. With a template, resume means checking `koto status`, which always shows the true current state. But if an agent's session is interrupted mid-phase (e.g., partway through analysis), the artifact file exists (`wip/issue_<N>_plan.md` is partially written) but the state is still `analysis` with no evidence submitted yet. The template can't distinguish "analysis in progress" from "analysis complete." 

   **Mitigation**: The agent must always complete a phase before submitting evidence to transition. Incomplete work is not represented in koto's state machine.

5. **Blocker: Evidence Required vs Gate Blocked**: If an agent forgets to submit evidence (e.g., leaves `issue_number` blank in the entry state), koto's `dispatch_next` returns `EvidenceRequired` — it doesn't auto-advance because the `accepts` field is required. This is correct behavior, but the agent must know to invoke `koto next --with-data` again with the missing field. The template can't prevent the agent from trying a bare `koto next` call.

   **Mitigating design**: The directive for each state could instruct the agent on what evidence to submit next. But this duplicates information already in the compiled template's `expects` field. The agent's koto skill wrapper (if built) could parse `expects` and enforce it.

## Implementation Complexity

### Effort Estimate

- **Template authoring**: 2-3 days. Writing, validating mutual exclusivity, testing with `koto template validate`.
- **Agent skill migration**: 5-7 days. Adapt work-on and just-do-it logic to call `koto next --with-data` at each phase boundary instead of internal phase management. Extract evidence from artifacts and format for submission.
- **Evidence payload design**: 2-3 days. Finalize `accepts` schemas, validate that all phase outputs map to submittable evidence, test round-trip submission and routing.
- **Integration testing**: 3-5 days. Full end-to-end tests for both work-on and just-do-it entry points, staleness routing, jury outcomes, resume from arbitrary states.
- **Total**: ~2-3 weeks of focused work.

### Risk Vectors

1. **Compiler edge cases**: The pairwise mutual exclusivity check for `when` conditions is O(n^2). With 3-4 transitions per state, this is fine. But if evidence routing explodes (10+ conditional transitions), compilation could get slow. The compiler rejects non-deterministic templates, which is safe but might require redesigning a template that seems correct.

2. **Agent-side evidence extraction**: The agent must parse the plan file (`wip/issue_<N>_plan.md`), extract structured fields, and format them for `--with-data`. If the plan format changes, the extraction logic breaks. This creates coupling between the skill and the template that the current split-skill design avoids.

3. **Template versioning**: If the template evolves (e.g., adding a new phase), existing workflows at old states need a migration path. The template format has no versioning mechanism beyond `format_version: 1` in the compiled JSON. Changing the template without a version bump leaves in-flight workflows in ambiguous states.

## Summary

This fine-grained evidence-gated template approach offers maximum enforcement granularity and clean evidence-driven routing through koto's state machine, making phase sequencing explicit and auditable. However, it trades off skill code simplicity for template complexity — the template becomes 500+ lines of YAML with exhaustive `accepts` blocks, and the agent code must extract evidence from phase artifacts and submit it manually at each boundary. The staleness check elegantly uses `when` conditions to skip the introspection state, but jury outcomes and iterative implementation details don't map cleanly to state transitions, requiring either bloat in evidence fields or loss of auditability. The approach is viable but best suited for straightforward linear workflows; it risks becoming unwieldy if branching and retry logic expand.

