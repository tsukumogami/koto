<!-- decision:start id="deterministic-step-three-path-model" status="assumed" -->
### Decision: Functional Behavior Model for Deterministic Workflow Steps

**Context**

The shirabe work-on template has 17 states. Six of those states have deterministic default behavior: `context_injection`, `setup_issue_backed`, `setup_free_form`, `staleness_check`, `finalization`, and `ci_monitor`. Decision 8 established the gate-only model (koto verifies via gates, agents run commands per directives) and Decision 6 (revised) established two-tier directives (full directives for judgment states, error-fallback for auto-advance states).

Neither decision addressed how the template handles three real-world scenarios for these deterministic states: (1) the default behavior succeeds normally, (2) the user provides an override that changes the default (e.g., "use my existing branch," "skip staleness check"), (3) the default behavior fails. The user feedback that prompted this decision also raised a safety concern: if future on_entry hooks allow koto to execute commands by default, the failure mode inverts from "nothing happens" (agent opt-in) to "unwanted action" (agent must opt-out).

The three concerns that must be resolved: execution responsibility (who runs the default), the override model (how user overrides reach the system before irreversible action), and safety constraints (what happens when an override is missed and the default runs anyway).

Four alternatives were evaluated through adversarial validation: (a) directive-driven three-path where the agent handles everything per directive text, (b) gate-with-evidence-fallback where gates handle the default and evidence schemas capture overrides/failures, (c) wrapper-mediated override where the skill wrapper intercepts before koto, and (d) state decomposition where each deterministic step becomes multiple states in the machine.

**Assumptions**

- Gate-with-evidence-fallback is not yet implemented in the engine but is planned work (identified in prior gate mechanics research). The template design depends on this capability shipping before or alongside the template. If it doesn't ship, the degradation is graceful: `GateBlocked` responses still carry the directive text, and agents can act on it without structured evidence submission.
- The skill wrapper (shirabe) will translate user override signals from invocation arguments into koto evidence. The wrapper is a convenience layer, not the primary override mechanism. The template is self-describing -- it works without the wrapper, just with less ergonomic override detection.
- Variable substitution (`--var`) is not implemented. Gate commands that reference issue numbers must use workarounds (glob patterns, wrapper-set environment variables) until `--var` ships.
- The safety inversion concern (opt-in vs. opt-out) does not apply for this release because Decision 8 chose gate-only. No commands are auto-executed by koto. The model defined here is designed to remain correct when on_entry hooks ship later, but the safety constraint is prospective, not immediate.

**Chosen: Gate-With-Evidence-Fallback (Two-Path State Model)**

Each deterministic state has two components in the template: a gate that verifies the expected outcome, and an `accepts` block that captures override or failure evidence when the gate doesn't pass.

**Default path.** The agent runs the command described in the directive (or the wrapper runs it before calling koto). The gate evaluates. If the gate passes, koto auto-advances. The agent never sees the `accepts` schema. The event log records a normal auto-transition.

**Override path.** The user provides input that changes the expected behavior. Two sub-flows:

- *Pre-flight override*: The skill wrapper detects the override from user invocation (e.g., `--branch my-branch`), applies it to the environment, and calls `koto next`. If the environment matches the gate condition despite the override, the gate passes and koto auto-advances normally. Example: user says "use my existing branch" and the gate checks "are we on a non-main branch?" -- the gate passes.

- *Evidence override*: The gate fails because the override produced a state the gate doesn't recognize, or the agent wants to explicitly signal an override. The engine returns the `accepts` schema (gate-with-evidence-fallback behavior). The agent submits evidence: `{ "status": "override", "detail": "used_existing_branch" }`. The conditional transition routes based on this evidence.

**Failure path.** The default command failed. The gate fails. The engine returns the `accepts` schema with the error-fallback directive. The agent reads the directive, diagnoses the failure, and submits evidence: `{ "status": "blocked", "reason": "git checkout failed: branch name conflict" }`. The conditional transition routes to a recovery or blocked state.

**Directive content model.** Deterministic states use the error-fallback directive from Decision 6 (revised): 3-6 lines covering what went wrong, what to do, and what evidence to submit. The directive is only displayed when the gate fails. On the default path, the agent never reads it.

**Evidence schema.** Each deterministic state's `accepts` block follows a standard pattern:

```yaml
accepts:
  status:
    type: enum
    values: [completed, override, blocked]
    required: true
  detail:
    type: string
    required: false
    description: "Override type or failure reason"
```

Transitions use `when` conditions on the `status` field:
- `status: completed` -> next state (agent manually confirmed completion when gate couldn't verify)
- `status: override` -> next state (agent signals override was applied)
- `status: blocked` -> blocked/recovery state or `done_blocked`

**Safety model (current release).** Agents opt-in to every action. koto's gates only verify after the fact. If the agent misses a user override and runs the default command anyway, the worst case is: the default runs (e.g., a new branch is created when the user wanted to use an existing one). This is recoverable -- the agent or user can fix the branch. The gate doesn't auto-execute anything.

**Safety model (future with on_entry hooks).** When on_entry hooks ship, the engine would run the default command before gate evaluation. If the agent missed an override, koto would auto-execute the default. The template's `accepts` block provides the override mechanism: the wrapper submits override evidence before calling `koto next`, which makes the conditional transition fire before the on_entry hook executes. The hook's execution should be conditional on no override evidence being present. This is a future engine design constraint, not a template-level concern.

**Rationale**

Alternative 1 (directive-driven three-path) was eliminated because it contradicts Decision 6's two-tier directive model. Three-path directives grow to 15-25 lines, defeating the purpose of short error-fallback directives for auto-advance states. It also produces no audit trail for overrides -- koto's event log can't distinguish "gate passed because default succeeded" from "gate passed because override aligned with gate conditions."

Alternative 3 (wrapper-mediated override) was rejected as the primary model because it moves three-path logic outside the template. The template becomes unable to describe its own override behavior. A different wrapper using the same template wouldn't know about override support. However, the wrapper has a legitimate role as a convenience layer: translating user invocation arguments into evidence that feeds koto's model. This role is preserved in the chosen approach.

Alternative 4 (state decomposition) was rejected for uniform application because it doubles the state count (17 to 29+). The decomposition adds structural plumbing states that don't carry workflow semantics. The observation that only 2-3 states genuinely benefit from decomposition (the setup states) is valid but doesn't justify a different structural convention for a subset of states. The evidence model handles those cases without decomposition.

Alternative 5 (parameterized states with pre-gate evidence) was rejected because it requires changing the engine's evaluation order from gates-then-transitions to transitions-then-gates. This is a deeper engine change than gate-with-evidence-fallback, and the benefit (single-round overrides) is small.

The gate-with-evidence-fallback model is the correct fit because:
1. It uses koto's existing template primitives (gates, accepts, when conditions) in combination
2. It's consistent with Decision 6's two-tier directives (error-fallback for auto-advance states)
3. It produces a structured audit trail for overrides and failures via evidence events
4. It degrades gracefully if the engine change isn't ready (GateBlocked still surfaces the directive)
5. It upgrades cleanly to on_entry hooks (add the hook, keep gates and evidence schemas unchanged)

**Alternatives Considered**

- **Directive-Driven Three-Path**: Agent handles all three paths per directive text. No engine changes. Rejected because it contradicts Decision 6's two-tier directives, creates 15-25 line directives for states that should have 3-6 lines, and produces no override audit trail. The directive becomes the only carrier of three-path logic, which is invisible to koto's state machine.

- **Wrapper-Mediated Override**: Skill wrapper intercepts user input, applies overrides to the environment, runs default commands, then calls koto. No engine changes. Rejected as the primary model because it moves three-path logic outside the template, breaking self-description. Accepted as a convenience layer that translates user overrides into evidence for the gate-with-evidence-fallback model.

- **State Decomposition**: Each deterministic step becomes a check/execute/recover state group. Full audit trail via state path. Rejected because it doubles the state count without proportional value. Only setup states genuinely benefit; applying it uniformly creates noise states that carry no workflow semantics.

- **Parameterized States (Pre-Gate Evidence)**: Agent submits override evidence before gate evaluation. Requires engine evaluation order change (transitions before gates). Rejected because the engine change is deeper than gate-with-evidence-fallback, and single-round override interaction is not a high-priority requirement.

**Consequences**

The template's deterministic states use a uniform pattern: gate + accepts + error-fallback directive. On the default path, agents never interact with these states (gates pass, auto-advance). On override and failure paths, agents see a short directive and submit structured evidence.

The engine must implement gate-with-evidence-fallback behavior: when a gate fails on a state that has both gates and an `accepts` block, the engine surfaces the `accepts` schema instead of returning a bare `GateBlocked` response. This is an engine issue that must be tracked and delivered alongside or before the template. The degradation path (no fallback behavior, bare `GateBlocked` with directive text) is functional but loses structured evidence.

The skill wrapper's role is defined: detect user overrides from invocation arguments, translate them into environment changes or evidence data, and feed them to koto. The wrapper is not the owner of three-path logic -- the template is. The wrapper is a user-facing convenience that makes the evidence submission ergonomic.

Evidence schemas on deterministic states follow the standard `status` enum pattern (completed/override/blocked). Template authors don't need to design custom evidence schemas for each deterministic state -- the pattern is reusable.

Per-state behavior under this model:

| State | Gate Condition | Override Evidence | Failure Evidence | Error-Fallback Directive |
|---|---|---|---|---|
| `context_injection` | `test -f wip/issue_N_context.md` | `override: "user_provided_context"` -- user supplied additional context | `blocked: "script_failed"` -- extract-context.sh failed | "Context extraction failed. Run extract-context.sh manually or provide context via evidence." |
| `setup_issue_backed` | Branch is not main + baseline file exists | `override: "existing_branch"` -- user specified branch | `blocked: "branch_creation_failed"` | "Branch setup failed. Check git state, resolve conflicts, retry or submit blocked status." |
| `setup_free_form` | Branch is not main + baseline file exists | `override: "existing_branch"` | `blocked: "branch_creation_failed"` | Same as issue-backed variant |
| `staleness_check` | Piped gate: `check-staleness.sh \| jq -e '.introspection_recommended == false'` | `override: "skip_staleness"` -- user explicitly skips | `blocked: "script_failed"` | "Staleness check indicates introspection needed. Run introspection or submit override to skip." |
| `finalization` | Summary file exists + `go test ./...` passes | `override: "custom_summary"` -- user provided summary | `blocked: "tests_failing"` | "Finalization checks failed. Tests not passing or summary missing. Diagnose and fix." |
| `ci_monitor` | `gh pr checks` all pass | `override: "ci_acceptable"` -- user accepts current CI state | `blocked: "ci_unresolvable"` | "CI checks failing. Diagnose by failure type. Fix or submit blocked if unresolvable." |
<!-- decision:end -->
