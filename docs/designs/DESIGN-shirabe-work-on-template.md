---
status: Planned
problem: |
  shirabe's /work-on workflow exists and works, but it runs entirely through agent
  instructions. koto's template engine can't yet express it as a template because the
  engine lacks several platform capabilities: template variable substitution, default
  action execution for deterministic steps, mid-state decision capture, and
  gate-with-evidence-fallback. This design identifies the full set of platform
  capabilities koto needs to support the work-on workflow as a template, then designs
  the template that uses them.
decision: |
  A single koto template backing /work-on with a three-path model for deterministic
  steps: each step has a default action (koto executes automatically), an override path
  (user input changes the default), and a failure path (agent recovers). Agents are
  reached only on judgment states or when deterministic steps need override/failure
  handling. Safety constraint: only reversible actions execute by default; irreversible
  steps (PR creation) require agent confirmation. The template uses split topology,
  routes three input modes (GitHub issue, free-form, PLAN doc) through a 17-state
  machine, and requires four engine prerequisites (three needs-design): --var (template
  variable substitution), default action execution, mid-state decision capture, and
  gate-with-evidence-fallback.
rationale: |
  The automation-first principle — koto does deterministic work, agents do judgment work
  — requires koto to execute default actions, not just verify outcomes. The three-path
  model makes this concrete: each deterministic step specifies what happens by default,
  what overrides are recognized, and how failures are handled. The safety inversion
  (agents must opt-out rather than opt-in when koto executes by default) is addressed
  by the reversibility constraint — only undoable actions auto-execute. Four engine
  prerequisites (--var substitution, default action execution, mid-state decision
  capture, and gate-with-evidence-fallback) provide the platform capabilities. The SKILL.md orchestration wrapper (~55 lines) is eliminated because
  koto tracks state directly. Structured enforcement alone yields ~15-20% instruction
  reduction; with default action execution (Phase 0b), the reduction reaches ~42% of
  995 lines.
---

# DESIGN: shirabe work-on koto template

## Status

Planned

## Context and Problem Statement

shirabe's /work-on skill is a long agent skill with instructions covering every phase
of implementation: context gathering, branch creation, planning, coding, PR creation,
CI monitoring. Most of these phases involve deterministic operations — running scripts,
creating branches, polling APIs, checking file existence — that any program could
execute reliably. Embedding them in agent instructions keeps the agent's context
window loaded with procedural text for work the agent should never need to do itself.

koto's template engine is built to separate this correctly: states with command gates
auto-advance when conditions are met, executing verification without agent involvement.
Evidence states engage the agent only when interpretation is needed. The goal of a
/work-on koto template is not to give koto a description of what the agent currently
does — it is to identify every deterministic step in the workflow, move it from agent
instructions into koto's engine, and leave agents with only the states that genuinely
require creativity or judgment.

A second problem: the skill currently requires a GitHub issue number. Agents and users
often need to implement a small task without first creating an issue, and creating one
adds friction. Supporting free-form input is part of the same redesign — the koto
template defines the workflow structure, and the entry point determines which
context-gathering states run.

The measure of success is not "koto enforces the workflow" — it is "agent instructions
are shorter because koto does more."

This design produces two outputs. The primary output is the set of platform capabilities
koto needs to express the work-on workflow as a template — each capability gets its own
child design doc and blocks the template work. The secondary output is the template
design itself: the state machine, evidence schemas, and directive model that use those
capabilities. The template depends on the platform work; the platform capabilities are
independently useful and will benefit other templates beyond work-on.

## Decision Drivers

- **Automation-first**: every step that can be executed deterministically by koto must
  be — koto runs the command and checks the result, the agent is not asked to do it
- **Three paths for deterministic steps**: each automated step must handle a default
  (the action succeeds), overrides (the user changes the default), and failures (the
  action fails and the agent recovers). Deterministic doesn't mean unconditional —
  users provide context that changes what should happen
- **Reversibility determines safety**: automated steps execute by default only when the
  action is reversible. Irreversible or externally-visible actions (PR creation, posting
  comments) require agent confirmation. This addresses the opt-in/opt-out inversion —
  when koto acts by default, a missed override runs the default, so the default must
  be undoable
- **Agents for judgment only**: evidence states are reserved for decisions requiring
  interpretation, creativity, or nuance that command outputs cannot capture
- **Agent instructions shrink as a result**: states where koto auto-advances produce
  no agent work and need no agent-readable directives; the skill's instruction set
  gets shorter, not longer
- koto's CLI surface stays minimal — no new subcommands; new capabilities live inside
  the template schema (new state fields) and the engine (new evaluation logic)
- Both modes supported: issue-backed (GitHub issue) and free-form (task description,
  no issue), plus plan-backed (PLAN doc issue, routed as free-form)
- Session resumability: koto's event log handles mid-session interruption
- **Decisions are captured, not buried**: when agents make judgment calls during
  implementation (assumptions, tradeoffs, approach choices), those decisions must be
  captured as structured evidence and surfaced to the user — not buried in the agent's
  reasoning where only a diff reviewer might notice them
- Evidence schemas capture decisions, not completions — `{done: true}` evidence
  defeats enforcement

## Platform Capabilities Required

This is the primary output of the design: the full set of koto platform capabilities
needed to express the work-on workflow as a template. Each capability is either a
needs-design issue (gets its own child design doc during /plan) or a targeted engine
change (implementable directly).

### Capability Map

| Workflow feature | Platform gap | Status |
|---|---|---|
| Issue-specific gate commands (`{{ISSUE_NUMBER}}` in `test -f`, `check-staleness.sh`) | Template variables (`--var`) | needs-design |
| Directive text references to issue numbers and artifact paths | Variable substitution in directives (fold into `--var` scope) | needs-design |
| Deterministic steps auto-execute (context injection, branch setup, staleness check, CI polling) | Default action execution | needs-design |
| CI monitoring polls repeatedly until pass/timeout | Polling/retry execution model (fold into default action scope) | needs-design |
| Default action commands reference `{{ISSUE_NUMBER}}` | Variable substitution in default actions (fold into `--var` scope) | needs-design |
| Gate output available to agent on fallback (staleness check structured data) | Gate output capture (fold into default action scope) | needs-design |
| Agent decisions during implementation captured as structured evidence | Mid-state decision capture | needs-design |
| Gate failure on a state with `accepts` falls through to evidence collection | Gate-with-evidence-fallback | targeted engine change |
| Workflow names in state file paths must be safe | Workflow name validation | targeted engine change |

### Phase 0a: Template variables (`--var` support) — needs-design

The `--var` feature enables issue-specific gate commands (`{{ISSUE_NUMBER}}` substitution)
and artifact-path gates (`{{ARTIFACT_PREFIX}}`). It spans CLI, event storage, runtime
evaluation, and input sanitization.

Scope for the child design:
- `koto init` accepts `--var KEY=VALUE` (repeatable). Values are stored in the
  `WorkflowInitialized` event's `variables` field (already defined, currently always
  empty).
- At gate evaluation time, `{{KEY}}` in gate command strings is substituted from the
  stored variables map. Substitution happens at runtime, not compile time — the compiled
  template remains variable-agnostic.
- Variable substitution applies to directive text too, not just gate commands. When a
  deterministic state's gate fails and the directive is surfaced, references to
  `{{ISSUE_NUMBER}}` or `{{ARTIFACT_PREFIX}}` in the directive text must be resolved.
  The `done_blocked` state's directive references issue-specific recovery paths; the
  override/failure directives on deterministic states reference issue-specific artifacts.
- Input sanitization: variable values containing shell metacharacters must be rejected or
  safely quoted at `koto init` time to prevent command injection. The child design should
  specify the safe character set and rejection behavior.
- Workflow name validation: names are incorporated into state file paths
  (`koto-<name>.state.jsonl`) and must be validated against a strict pattern to prevent
  path traversal. This is a targeted engine change that can be implemented alongside
  `--var` or separately.

### Phase 0b: Default action execution — needs-design

The three-path model (Decision 6+8) requires koto to execute a default action for
deterministic states — not just verify the outcome. This is the engine capability that
makes the default path work: koto runs a command on state entry, captures the result,
then evaluates the gate.

Scope for the child design:
- How the template specifies the default action per state (command string, working
  directory, environment).
- How output is captured and made available for gate evaluation. Gate output capture
  is required for states like `staleness_check`, where the gate command produces
  structured data the agent needs to see on fallback.
- How override evidence prevents default execution (the agent or skill layer submits
  override evidence before the action runs).
- The reversibility constraint: the engine should support marking actions as
  requiring confirmation, so irreversible actions can be flagged in the template schema.
- Interaction with `--var` substitution (default action commands reference
  `{{ISSUE_NUMBER}}`). Phase 0b's design should coordinate with Phase 0a on the
  substitution interface, though implementation can proceed in parallel once the
  interface is agreed.
- The execution model must handle both one-shot commands (`context_injection`,
  `setup_issue_backed`, `setup_free_form`, `staleness_check`) and repeated evaluation
  (`ci_monitor`). The `ci_monitor` state needs polling/retry semantics — run a command
  repeatedly until it passes or times out — which is fundamentally different from the
  other four deterministic states that run once and check. The child design should
  specify how the template declares polling behavior and how the engine manages
  retry intervals and timeout.

### Phase 0c: Mid-state decision capture — needs-design

During judgment states like `implementation` and `analysis`, agents make non-obvious
choices — assumptions about API behavior, tradeoff decisions, approach pivots. Today
these are buried in the agent's reasoning and only visible if a reviewer reads the diff.
The decision capture mechanism makes these choices structured evidence that koto records
and surfaces to the user.

The core platform capability is incremental submission: accepting evidence mid-state
without triggering a transition. Batch submission at transition time is already possible
(the agent includes a `decisions` array alongside the routing evidence), but incremental
capture lets agents record decisions as they happen rather than reconstructing them at
the end.

Scope for the child design:
- How decisions are submitted mid-state without triggering a state transition. The agent
  stays in `implementation` but accumulates decision records in the event log.
- The decision record schema: at minimum `choice`, `rationale`, and
  `alternatives_considered`. May align with or adapt the `/decision` skill's output
  format for consistency.
- How koto surfaces accumulated decisions to the user — as part of `koto next` output,
  as a separate query command, or both.
- Whether the template can require decision capture (e.g., a minimum count before
  completion evidence is accepted) or whether it's advisory.

This is a cross-cutting engine concern — other templates beyond work-on will benefit
from the same mechanism.

### Phase 1: Gate-with-evidence-fallback — targeted engine change

When a gate fails on a state that also has an `accepts` block, the engine should fall
through to `NeedsEvidence` instead of returning `GateBlocked`. The CLI then handles
`NeedsEvidence` by including the `expects` schema and setting `agent_actionable: true`.
This provides the override and failure paths for every deterministic state and the
resume path for judgment-with-verification states.

Deliverables:
- `src/engine/advance.rs`: When evaluating gates, if any gate fails and the current state
  has an `accepts` block, skip the hard `GateBlocked` return and fall through to
  `NeedsEvidence`. The existing transition resolution logic already handles this case
  correctly once reached.
- `src/cli/next_types.rs`: The `NeedsEvidence` response carries `expects` (populated via
  `derive_expects` from the state's `accepts` block) and `agent_actionable: true`.
- `src/cli/mod.rs`: No changes needed to the `GateBlocked` arm — states with both gates
  and accepts will never reach `GateBlocked` because the engine falls through to
  `NeedsEvidence` first.
- Tests: add engine tests for gate-failure-with-fallback behavior and CLI output shape.

### Workflow name validation — targeted engine change

Workflow names are incorporated into state file paths (`koto-<name>.state.jsonl`).
Names must be validated at `koto init` time against a strict pattern to prevent path
traversal (e.g., `../../../etc/koto.state.jsonl`). Currently listed in Security
Considerations but required as a prerequisite for safe operation with user-provided
workflow names. Can be implemented alongside `--var` (Phase 0a) or independently.

### Acknowledged Limitations

These are known gaps that are out of scope for this design but documented so template
authors and future designs can account for them:

- **Artifact lifecycle after rewind**: koto doesn't manage artifact cleanup when
  rewinding. If an agent rewinds from `finalization` back to `implementation`, the
  summary artifact isn't cleaned up. Template authors should know that artifacts and
  koto state can diverge after rewind.
- **Progress reporting mid-state**: there is no mechanism for agents to report progress
  without triggering a transition. An agent in `implementation` that wants to signal
  "3 of 5 files updated" has no koto-level way to do so. Out of scope for this design.
- **Path-specific rewind counts in `done_blocked`**: directives are static text, but
  the rewind distance from `done_blocked` varies by originating state (1 step from
  `ci_monitor`, up to 5+ from `analysis`). The template author must enumerate all
  paths in the directive or use generic guidance like "rewind until you reach state X."
  Variable substitution in directives (Phase 0a) doesn't solve this because the origin
  isn't a template variable — it's runtime state.

### Parallelism and Dependencies

Phase 0a, Phase 0b, Phase 0c, and Phase 1 can all be designed in parallel. Each Phase 0
item gets its own design doc. Phase 0b depends on Phase 0a's substitution interface
(default action commands reference `{{VAR}}`), so Phase 0b's design should coordinate
with Phase 0a on that interface. Implementation can still proceed in parallel once the
interface is agreed.

All platform capabilities are prerequisites for Phase 2 (the template).

## Considered Options

### Decision 1: Workflow mode routing topology

The template supports two modes: issue-backed (GitHub issue number provided) and
free-form (task description only). The modes share all states from analysis onward
but diverge through their context-gathering phases. koto's evidence is epoch-scoped —
each state transition clears the current evidence, so routing fields cannot carry
forward automatically between states.

The `--var` CLI flag and `{{VAR_NAME}}` gate substitution are prerequisites for this
design (see Implementation Approach, Phase 0). Option (c) — init-time mode determination
— was evaluated but requires additional engine capabilities beyond `--var` substitution
(either `--initial-state` support or var-based transition routing). Those are separate
concerns from gate substitution and would add scope to the prerequisite engine work
without proportional benefit.

#### Chosen: Split topology — two separate setup states

The template uses an entry state that accepts mode evidence and routes to diverged paths:
`context_injection` for issue-backed, `task_validation` for free-form. Each path
terminates in a mode-specific setup state: `setup_issue_backed` transitions
unconditionally to `staleness_check`; `setup_free_form` transitions unconditionally to
`analysis`. No mode re-submission is required — routing is implicit in which setup state
the agent is in. Both paths merge at `analysis` and share all subsequent states. The two
setup states will have distinct directive content covering mode-specific preparation work.

#### Alternatives Considered

**Entry state with single setup state and mode re-submission (a)**: Mode submitted at
`entry` routes to diverged paths; mode re-submitted at single `setup` determines the
post-setup routing target. Rejected because re-submission requires contributors to
understand epoch-scoped evidence — a non-obvious engine property that creates an unbounded
maintenance cost every time someone reads or extends the template.

**Init-time `--var` flag (c)**: Mode encoded at `koto init` via `--var MODE=issue-backed`,
entry state eliminated, initial state determined at init. Rejected because it requires
either `--initial-state` CLI support (koto selects the starting state at init) or
var-based transition routing (transitions conditioned on stored vars, not just evidence).
Both are distinct engine features beyond `--var` gate substitution and would expand the
prerequisite scope without clear benefit — the entry state is lightweight and the split
topology already avoids mode re-submission.

**Two separate templates (d)**: Separate `work-on-issue.md` and `work-on-freeform.md`
files. Rejected because it duplicates approximately 12 shared states, violates the
duplication constraint, and recreates the divergence problem that motivated this design.

---

### Decision 2: Context injection depth

The `context_injection` state backs Phase 0 of /work-on, which runs `extract-context.sh`
to create `wip/issue_<N>_context.md`. This file carries design rationale forward —
Phase 4 (implementation) explicitly references it. Gating on issue accessibility alone
(`gh issue view {{ISSUE_NUMBER}}`) checks reachability but doesn't verify that context
extraction happened — an agent could skip extraction with no consequence in the state
machine.

#### Chosen: Gate on context artifact existence; extraction is the state's work

The default action is `extract-context.sh --issue <N>` (koto executes on state entry).
The gate is `test -f wip/issue_{{ISSUE_NUMBER}}_context.md`. The `{{ISSUE_NUMBER}}`
substitution uses the `--var` flag passed at `koto init` time. When the action succeeds
and the artifact exists, the gate auto-advances without agent involvement. On override
(user provides additional context) or failure (script fails), the agent submits evidence.

Key assumption: `extract-context.sh` will be updated in Phase 3 (shirabe integration) to
accept an issue number argument and write to `wip/issue_<N>_context.md`. The numbered path
follows the established `wip/issue_<N>_<artifact>.md` convention used by all other /work-on
artifacts and eliminates the concurrency risk of a fixed shared path.

#### Alternatives Considered

**Gate on issue accessibility only (a)**: The gate auto-advances on issue existence
without verifying extraction happened. An agent can skip context extraction entirely
with no consequence in the state machine. Rejected.

**Separate context_extraction state after accessibility check (c)**: Stronger enforcement
but adds a state. Extract-context.sh fails naturally on inaccessible issues, making an
explicit accessibility state unnecessary — the artifact gate already catches the failure.
Rejected as unnecessary complexity.

**Fold context work into analysis directive (d)**: Removes koto enforcement entirely —
context loading becomes a suggestion with no structural guarantee. Rejected.

---

### Decision 3: Free-form validation sequence

The free-form path needs a mechanism to reject tasks not ready for direct implementation.
A single validation state before research would assess only the task description — not
the codebase state that research reveals. An agent that discovers a misconception during
research needs a clean exit path other than `done_blocked`.

#### Chosen: Lightweight pre-research check + post-research validation

Two validation states in the free-form path:

1. `task_validation` (before research) — binary gate on description quality:
   `verdict: enum[proceed, exit]` with `rationale: string`. Catches obviously-wrong tasks
   (ambiguous description, clearly oversized scope) without spending research effort.

2. `post_research_validation` (after research, before setup) — ternary routing decision
   informed by codebase findings: `verdict: enum[ready, needs_design, exit]` with
   `rationale: string` and optional `revised_scope: string`. Catches tasks that research
   reveals to be misconceived relative to current codebase state. `needs_design` routes
   to `validation_exit` (same as `exit`, but the rationale captures that a design doc is
   needed before implementation can proceed).

Both `exit` and `needs_design` route to `validation_exit` (terminal). The free-form path
becomes:
`task_validation` → `research` → `post_research_validation` → `setup_free_form`. This
matches just-do-it's jury → research → validation-jury → setup structure without requiring
multi-agent jury mechanics.

#### Alternatives Considered

**Pre-research validation only (a)**: Provides no exit path when research reveals a task
is misconceived. An agent in this situation can only route to `done_blocked` — a permanent
terminal rather than a graceful "not ready" signal. Rejected.

**Post-research validation only (b)**: Removes the early filter. Research takes real agent
effort; a lightweight pre-research gate avoids that cost for tasks obviously wrong at the
description level. Rejected.

**No validation state (d)**: Agent self-assesses scope during analysis. `validation_exit`
must be reachable from a defined state; implicit agent routing provides no structured audit
record. Rejected.

---

### Decision 4: Introspection outcome model

The `introspection` state backs Phase 2 of /work-on, where a sub-agent re-reads the issue
against the current codebase. The real skill's Phase 2 produces four outcomes: Proceed,
Clarify (needs user input), Amend (update issue scope), and Re-plan (issue superseded).
A three-value enum would miss Clarify and Amend, and leave `issue_superseded` without a
routing target.

#### Chosen: Collapse Clarify/Amend into approach_updated; route issue_superseded to done_blocked

The `introspection_outcome` enum has three values:
- `approach_unchanged` → `analysis`
- `approach_updated` → `analysis` (covers both Clarify and Amend outcomes completed
  inside the sub-agent's Task invocation)
- `issue_superseded` → `done_blocked` (explicit terminal routing, fixing the undefined
  routing gap)

Clarify and Amend are sub-phases internal to the sub-agent — it handles any user
interaction internally, writes the introspection artifact, and returns a macro-level
outcome. The `rationale` string captures what happened: "Clarified with user: requirement
changed; approach updated" or "Amended issue scope: removed stale auth section."

The introspection gate (`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`) uses
`--var` substitution to check for the issue-specific artifact. When the artifact exists,
the gate auto-advances; when it doesn't, gate-with-evidence-fallback surfaces the
evidence schema for the agent to submit directly.

#### Alternatives Considered

**Expand to 5-value enum (a)**: Add `needs_clarification` and `needs_amendment` enum
values. Without valid routing targets for these values, the expansion creates new
stuck-workflow paths. Rejected.

**Separate user_interaction state (b)**: Add `awaiting_clarification` or
`awaiting_amendment` states reachable from `introspection`. Models sub-agent internal
behavior at the koto workflow level — the sub-agent is the right place for user
interaction during introspection; koto tracks only the macro outcome. Rejected.

**Two-value enum (d)**: `proceed_with_analysis: true/false`. Loses the
`approach_unchanged` vs. `approach_updated` audit distinction and doesn't clarify how
Clarify/Amend map to the binary. Rejected.

---

### Decision 5: Error recovery model for self-loops and blocked states

Self-looping states (`analysis`, `implementation`, `pr_creation`) have no loop count, so
agents can cycle indefinitely. `done_blocked` is terminal with no recovery path, meaning
even a recoverable blocker (like a flaky CI test) requires re-initializing the entire
workflow. koto's `when`-conditions use exact JSON equality — range conditions like
`loop_count >= 3` cannot be expressed as transition conditions. Escalation must use a
distinct enum value.

#### Chosen: Retry/escalate enum variants with koto rewind for done_blocked

Each self-looping state gains `_retry` and `_escalate` variants for the retry-eligible
evidence value, plus an escalation transition to `done_blocked`:
- `analysis`: `scope_changed_retry` (self-loop) and `scope_changed_escalate` → `done_blocked`
- `implementation`: `partial_tests_failing_retry` (self-loop) and
  `partial_tests_failing_escalate` → `done_blocked`
- `pr_creation`: `creation_failed_retry` (self-loop) and `creation_failed_escalate` →
  `done_blocked`

Each state's directive specifies the escalation threshold (default: 3 failed retries) and
instructs the agent to switch from `_retry` to `_escalate` after that threshold.

`done_blocked` remains terminal. Its directive gains `koto rewind` instructions: if the
blocker has been resolved externally, run `koto rewind <name>` repeatedly — once per
state to traverse back — to step back to the originating state. (`koto rewind` rewinds
exactly one step per call; it does not accept a named target state.)

#### Alternatives Considered

**loop_count evidence field (a)**: Routing logic is identical to the chosen option (still
requires an escalate enum value). `loop_count` becomes a supplementary audit field rather
than the routing mechanism. Can be added as an optional enhancement; not required for
structural escalation. Rejected as the primary approach.

**Resumable blocked state (c)**: Replace `done_blocked` with a non-terminal state.
Routing `continue` back to the appropriate origin state requires multiple blocked states
or origin encoding. koto rewind achieves the same recovery at zero template cost.
Rejected as over-engineered.

**Documentation only (d)**: Directive text describing escalation thresholds without
structural routing changes. Rejected given the low cost of option (b)'s enum variants,
which produce auditable escalation records.

---

### Decision 6+8 (merged): Functional behavior model for deterministic steps

Each workflow step that has a deterministic default behavior must handle three scenarios:
the default case (everything works as expected), user overrides (the user provided input
that changes the default), and failures (the default behavior fails). Former Decision 6
(directive tiers) and Decision 8 (automation ceiling) are coupled — the directive model
follows directly from who is responsible for each path — so they're merged here.

A safety concern shapes this decision: today, agents opt in to every action (worst case
if the agent does nothing: nothing happens). If koto executes deterministic steps by
default, the agent must opt out to prevent unwanted actions (worst case if the agent
misses an override: koto runs something it shouldn't have). The design must address
this inversion.

#### Chosen: Three-path model with reversibility-based safety

**Default path.** Each deterministic step specifies a default action and expected
outcome. When no override is provided and the action succeeds, the workflow advances
automatically — the agent is not involved. koto executes the action and verifies the
outcome.

**Override path.** The user provides input that changes the default (e.g., "use my
existing branch," "skip staleness check," "here's additional context"). The skill layer
detects the override and communicates it to koto before the default action runs. The
workflow routes based on the override evidence rather than executing the default.

**Failure path.** The default action fails or produces unexpected results. koto surfaces
the failure to the agent with the state's evidence schema. The agent diagnoses the
failure and either recovers (submits `completed` or `override` evidence) or escalates
(submits `blocked` evidence routing to `done_blocked`).

**Evidence schema.** Each deterministic state's `accepts` block follows a standard
pattern:

```yaml
accepts:
  status:
    type: enum
    values: [completed, override, blocked]
  detail:
    type: string
    description: "Override type or failure reason"
```

Transitions use `when` conditions on `status`:
- `completed` → next state (agent confirmed completion when automation couldn't verify)
- `override` → next state (override applied, default skipped)
- `blocked` → recovery state or `done_blocked`

This standard pattern applies to truly deterministic states (`context_injection`,
`setup_issue_backed`, `setup_free_form`, `staleness_check`, `ci_monitor`). Judgment
and judgment-with-verification states use domain-specific evidence schemas tailored
to their routing needs (e.g., `plan_outcome`, `introspection_outcome`).

**Safety constraint: reversibility determines execution policy.** Only reversible actions
execute by default; irreversible or externally-visible actions (PR creation) require
agent confirmation. All current deterministic states are reversible:

| State | Default action | Reversibility | Known overrides | Common failures |
|---|---|---|---|---|
| `context_injection` | Run extract-context.sh, create wip file | File can be overwritten | User provides additional context | Script fails, issue inaccessible |
| `setup_issue_backed` | Create feature branch, run baseline | Branch can be deleted | User specifies existing branch | Branch name conflict, build broken |
| `setup_free_form` | Create feature branch, run baseline | Branch can be deleted | User specifies existing branch | Branch name conflict, build broken |
| `staleness_check` | Run check-staleness.sh, evaluate output | Read-only | User says "skip, issue is current" | Script fails, network error |
| `ci_monitor` | Poll CI status | Read-only | User says "CI acceptable" | CI flaky, unresolvable failures |

**Directive content model (follows from three-path model).** Two directive tiers matching
the agent's interaction pattern:

**Tier 1 — judgment and judgment-with-verification states** (`entry`, `task_validation`,
`post_research_validation`, `introspection`, `analysis`, `implementation`, `finalization`,
`pr_creation`, `validation_exit`, `done_blocked`):
Full directive (10-25 lines) covering what to accomplish, artifact paths, evidence
schema, and resume guidance. These states always involve agent work — either pure judgment
or judgment with a verification gate that enables resume.

**Tier 2 — deterministic states (exception path only)** (`context_injection`,
`setup_issue_backed`, `setup_free_form`, `staleness_check`, `ci_monitor`): Override and
failure guidance (3-8 lines). Only displayed when the default didn't execute or failed.
Covers: recognized overrides, common failure recovery, evidence schema. On the happy
path, the agent never sees these directives.

The SKILL.md orchestration wrapper (~55 lines of resume detection and phase dispatch) is
eliminated. koto tracks state; the skill calls `koto next` in a loop and injects full
phase files for `analysis` and `implementation` before the agent begins work.

**Engine requirements.** The mechanism by which koto executes default actions is a
`needs-design` engine issue (alongside `--var` support). This design specifies WHAT
happens at each step; the engine design specifies HOW koto executes it. The
gate-with-evidence-fallback pattern (Phase 1 prerequisite) provides the override and
failure paths. The default execution capability is additional prerequisite scope.

#### Alternatives Considered

**Verify-only / gate-only model (a)**: koto only verifies outcomes via gates; agents
do all work per directive. Overrides handled naturally by the agent (it reads user
context and adapts). Rejected because it contradicts the automation-first principle —
agents perform deterministic work that koto should handle. The directive tells the agent
to run a script, which is the wrong layer for deterministic execution.

**Wrapper-mediated override (b)**: The skill wrapper intercepts user input, applies
overrides, runs default commands, then calls koto. Rejected as the primary model because
it moves three-path logic outside the template — a different wrapper using the same
template wouldn't know about override support. Acceptable as a convenience layer that
translates user invocation arguments into koto evidence.

**State decomposition (c)**: Each deterministic step becomes a check/execute/recover
state group. Full audit trail via state path. Rejected because it doubles the state
count (17 to 29+) without proportional value. Only 2-3 states genuinely benefit.

**Execute-all without safety constraint (d)**: koto executes all steps by default,
including externally-visible ones. Rejected because a missed override on PR creation
or comment posting has consequences that can't be undone quietly. The reversibility
constraint exists precisely for this case.

---

### Decision 7: Plan-backed issue support

The /plan workflow creates PLAN documents with plan-only issues (no GitHub issue numbers).
Each issue has a goal, acceptance criteria, complexity, dependencies, and an upstream
design doc reference. Users need `/work-on PLAN-<topic>#N` to work on a single plan
issue interactively, without running the full batch via /implement.

The key structural finding: the free-form path in this template already routes correctly
for plan-backed issues. Free-form goes `entry → task_validation → research →
post_research_validation → setup_free_form → analysis`, skipping staleness entirely.
Plan-backed issues should also skip staleness (they just came from /plan). The
two validation states serve plan-backed well: task_validation verifies extracted AC is
actionable; post_research_validation checks codebase readiness.

#### Chosen: Free-form mode absorbs plan-backed; skill layer handles PLAN doc parsing

No template changes. The skill layer detects `/work-on PLAN-<topic>#N` input, reads the
PLAN doc issue at that sequence number, and populates `task_description` with the issue's
goal, acceptance criteria, and design doc references before initializing the koto workflow
as free-form mode. The template sees a rich task description and routes identically to
any other free-form invocation.

Key assumption: Phase 3 (shirabe integration) updates the /work-on skill to accept
`PLAN-<topic>#N` syntax and implement PLAN doc parsing. Execution quality depends on
the skill layer faithfully preserving AC from the PLAN doc.

#### Alternatives Considered

**Third template mode with dedicated states (a)**: Add `plan_context_extraction` +
`setup_plan_backed` states (17 → 19). Provides koto-level enforcement that the PLAN doc
was read. Rejected because PLAN docs are local files (always readable, no network risk),
the free-form path already routes correctly, and adding a third mode adds 2 states of
ceremony without behavioral difference. The principle: templates enforce workflow phases;
skills translate input formats.

**Delegate plan-backed to /implement (c)**: Document plan-only issues as out of scope
for /work-on. Rejected because /implement runs a full plan batch — it doesn't serve
single-issue interactive work. Users should not need a different skill to work on a
plan issue vs. a GitHub issue.

---

*Decision 8 (automation ceiling: which states auto-execute and the reversibility
constraint) has been merged into Decision 6+8 above.*

---

## Decision Outcome

**Chosen: automation-first three-path model, split topology, artifact-gated context
injection, piped staleness gate, two-stage free-form validation, collapsed introspection
outcomes, retry/escalate self-loops, reversibility-based safety, plan-backed via
free-form**

The template has 17 states across two converging modes.

**Issue-backed path:** `entry` → `context_injection` → `setup_issue_backed` →
`staleness_check` → (optional) `introspection` → `analysis` → `implementation` →
`finalization` → `pr_creation` → `ci_monitor` → `done`

**Free-form path:** `entry` → `task_validation` → (if exit: `validation_exit`) →
`research` → `post_research_validation` → (if exit: `validation_exit`) →
`setup_free_form` → `analysis` (and from there, identical to issue-backed)

Both modes share 8 states from `analysis` onward. The modes diverge through their
context-gathering phases and each have a dedicated setup state with unconditional
transitions — no epoch-scoped mode re-submission required.

**State classification:**

| State | Type | Behavior | Agent involvement |
|---|---|---|---|
| `context_injection` | deterministic | koto runs extract-context.sh, verifies artifact | Override or failure only |
| `setup_issue_backed` | deterministic | koto creates branch, runs baseline, verifies | Override or failure only |
| `setup_free_form` | deterministic | koto creates branch, runs baseline, verifies | Override or failure only |
| `staleness_check` | deterministic | koto runs check-staleness.sh, evaluates via piped gate | Override or failure only |
| `ci_monitor` | deterministic | koto polls CI status, verifies all pass | Override or failure only |
| `introspection` | judgment w/ verification gate | Agent re-reads issue against codebase; gate checks artifact exists on resume | Always (gate enables skip on resume) |
| `analysis` | judgment w/ verification gate | Agent creates implementation plan; gate checks plan file exists on resume | Always (gate enables skip on resume) |
| `implementation` | judgment w/ verification gate | Agent writes code and commits; gate checks branch state on resume | Always (gate enables skip on resume) |
| `finalization` | judgment w/ verification gate | Agent creates summary and verifies; gate checks summary exists on resume | Always (gate enables skip on resume) |
| `entry` | judgment | Agent submits mode evidence | Always |
| `task_validation` | judgment | Agent assesses task, submits verdict | Always |
| `post_research_validation` | judgment | Agent reassesses after research | Always |
| `pr_creation` | judgment (irreversible) | Agent creates PR, submits evidence | Always |
| `research` | judgment | Unconditional transition | Always |

Three state categories:

- **Deterministic**: koto executes a default action and verifies the outcome. The
  three-path model (default/override/failure) applies. Agents interact only on override
  or failure. Tier 2 directives (3-8 lines). The standard `status: completed|override|blocked`
  evidence pattern applies to these states.
- **Judgment with verification gate**: the agent does the primary work, but koto has a
  gate that checks for completion artifacts. On first execution, the agent always acts.
  On resume, the gate auto-advances past completed work. The three-path model does not
  apply — there is no default action. Tier 1 directives (10-25 lines). Evidence schemas
  are domain-specific.
- **Judgment**: the agent always acts, no gate. Tier 1 directives (10-25 lines). Evidence
  schemas are domain-specific.

The SKILL.md orchestration wrapper (~55 lines of resume detection and phase dispatch) is
eliminated. koto tracks state; the skill calls `koto next` in a loop.

The platform capabilities required for this template are detailed in the Platform
Capabilities Required section above. In summary: four engine capabilities are
prerequisites (three needs-design, one targeted change), plus workflow name validation.
Each needs-design item gets its own child design doc during /plan.

### Rationale

Structured enforcement alone (orchestration wrapper eliminated, structured evidence,
koto-managed sequencing) yields ~15-20% agent instruction reduction. With default action
execution (Phase 0b), the reduction reaches ~42% (420 of 995 lines). See Decisions 1-7
for the reasoning behind each design choice.

## Solution Architecture

### Overview

The template is a koto state machine that enforces the /work-on workflow structure for
both modes. When an agent starts the skill, it initializes a workflow from the template
and calls `koto next <name>` in a loop to get directives and advance state. koto enforces
sequencing: an agent can't reach `analysis` without passing through `staleness_check`
(issue-backed) or `task_validation` (free-form), and can't reach `ci_monitor` without
passing through `pr_creation`.

### Components

**`shirabe/koto-templates/work-on.md`** — the template file. 17 states with directives,
gate commands, and evidence schemas. Lives in shirabe's plugin directory and is copied
to `.koto/templates/work-on.md` in the project on first use.

**koto engine** (changes to existing Rust files):
- `src/engine/advance.rs`: gate-with-evidence-fallback logic — when a gate fails on a
  state with an `accepts` block, fall through to `NeedsEvidence` instead of returning
  `GateBlocked`
- `src/cli/mod.rs` (init command): implement `--var KEY=VALUE` flag (repeatable). Store
  the resulting `HashMap<String, String>` in the `variables` field of the
  `WorkflowInitialized` event (already defined, currently always empty). Substitute
  `{{KEY}}` in gate command strings at gate evaluation time by reading from the stored
  variables map — this avoids a compiler change and keeps the compiled template
  variable-agnostic

**shirabe /work-on skill** (updated): calls `koto init` on first run, loops `koto next`
for directives, submits evidence via `koto next --with-data`, injects full phase
procedure for complex states (`analysis`, `implementation`) before the agent begins
work, and resumes via `koto workflows` + `koto next` on session restart.

### State Machine

```
entry (evidence: mode)
  │
  ├─ issue-backed mode:
  │   context_injection[G]
  │        │
  │   setup_issue_backed[G/E]
  │        │
  │   staleness_check[G/E] ─── stale_requires_introspection ──► introspection[G/E]
  │        │                                                      │
  │        └──────────────── fresh/stale_skip ───────────────────┘
  │                                    │
  │                                    ▼
  └─ free-form mode:            analysis[G/E] ◄─ scope_changed_retry (self-loop)
      task_validation                  │                          │
          │                 implementation[G/E] ◄─ partial_tests_failing_retry
    ┌─────┴──────────┐               │ │              (self-loop)
 validation_exit  research        finalization[G/E]    done_blocked (terminal)
 (terminal)           │                │
              post_research_validation  pr_creation ◄─ creation_failed_retry
                      │                │              (self-loop)
                ┌─────┴──────┐    ci_monitor[G/E]
         validation_exit  setup_free_form    │        \
         (terminal)         │[G/E]         done     done_blocked
                       (converges       (terminal)  (terminal)
                       to analysis)
```

`[G]` = has command gate (auto-advances when gate passes, unconditional transition).
`[G/E]` = gate-with-evidence-fallback (gate fails → evidence fallback).
`staleness_check` uses a piped gate (`check-staleness.sh | jq -e`), annotated `[G/E]`.
States always evidence-gated (no gate): `entry`, `task_validation`,
`post_research_validation`, `pr_creation`.
`research` is evidence-gated with unconditional transition.

`done_blocked` is reachable from: `analysis` (`blocked_missing_context` and
`scope_changed_escalate`), `implementation` (`blocked` and `partial_tests_failing_escalate`),
`pr_creation` (`creation_failed_escalate`), `ci_monitor` (`failing_unresolvable`),
and `introspection` (`issue_superseded`).

### State Definitions

**`entry`** — routes issue-backed vs free-form mode. Evidence: `mode: enum[issue_backed,
free_form]`, `issue_number: string` (issue-backed only), `task_description: string`
(free-form only).

**`context_injection`** — creates context artifact for issue-backed workflows. Default
action: `extract-context.sh --issue <N>` (koto executes). Gate:
`test -f wip/issue_{{ISSUE_NUMBER}}_context.md`. On the default path, koto runs the
script and the gate verifies the artifact — agent not involved. Evidence fallback:
`status: enum[completed, override, blocked]`, `detail: string`. Override: user provides
additional context alongside the issue. On resume: check if file already exists.

**`task_validation`** — assesses whether the free-form task description is clear and
appropriately scoped for starting work. Always evidence-gated. Evidence:
`verdict: enum[proceed, exit]`, `rationale: string`. `exit` routes to `validation_exit`.
On resume: re-read the original task description before assessing.

**`validation_exit`** — terminal state for tasks not ready for direct implementation.
Directive instructs the agent to communicate the verdict with the rationale and suggest
the next step (create an issue, write a design doc, narrow the scope, etc.).

**`research`** — lightweight context gathering for free-form tasks. Evidence:
`context_gathered: enum[sufficient, insufficient]`, `context_summary: string`.
Unconditional transition to `post_research_validation`. On resume: check for any
research notes or codebase observations already made in the session.

**`post_research_validation`** — reassesses the task against what research revealed
about the current codebase. Always evidence-gated. Evidence: `verdict: enum[ready,
needs_design, exit]`, `rationale: string`, `revised_scope: string` (optional, when
task can proceed with a narrowed scope). Both `exit` and `needs_design` route to
`validation_exit` — the rationale distinguishes them (e.g., "task needs a design doc
before implementation"). On resume: re-read the research context summary before
assessing.

**`setup_issue_backed`** — creates feature branch and baseline file for issue-backed
workflows. Gate: branch is not main/master, baseline file exists. Evidence fallback:
`branch_action: enum[created, reused_existing]`, `branch_name: string`,
`baseline_outcome: enum[clean, existing_failures, build_broken]`. Unconditional
transition to `staleness_check`.

**`setup_free_form`** — creates feature branch and baseline file for free-form workflows.
Gate: branch is not main/master, baseline file exists. Evidence fallback:
`branch_action: enum[created, reused_existing]`, `branch_name: string`,
`baseline_outcome: enum[clean, existing_failures, build_broken]`. Unconditional
transition to `analysis`.

**`staleness_check`** — assesses codebase freshness since the issue was opened. Default
action: `check-staleness.sh --issue {{ISSUE_NUMBER}}` (koto executes). Gate:
piped evaluation via `| jq -e '.introspection_recommended == false'` (jq exit code:
0 = fresh → `analysis`, 1 = stale → evidence fallback). Evidence fallback:
`status: enum[completed, override, blocked]`, `staleness_signal:
enum[fresh, stale_requires_introspection]`, `detail: string`.
`stale_requires_introspection` routes to `introspection`. Override: user says "skip
staleness, issue is current."

**`introspection`** — re-reads the issue against the current codebase via a sub-agent.
Gate: `test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`. Evidence fallback:
`introspection_outcome: enum[approach_unchanged, approach_updated, issue_superseded]`,
`rationale: string`. `approach_unchanged` and `approach_updated` route to `analysis`.
`issue_superseded` routes to `done_blocked`. The `approach_updated` value covers both
Clarify and Amend outcomes from the sub-agent's internal loop.

**`analysis`** — researches and creates implementation plan. Gate:
`test -f wip/{{ARTIFACT_PREFIX}}_plan.md`. The `{{ARTIFACT_PREFIX}}` variable is set at
`koto init` time: `issue_71` for issue-backed, `task_add-retry-logic` for free-form
(see Key Interfaces).
Evidence fallback: `plan_outcome: enum[plan_ready, blocked_missing_context,
scope_changed_retry, scope_changed_escalate]`, `approach_summary: string`,
`decisions: array[{choice: string, rationale: string, alternatives_considered: string}]`
(optional, same pattern as `implementation` — captures non-obvious choices made during
analysis such as scope narrowing decisions or dependency ordering).
`scope_changed_retry` self-loops (up to 3 iterations). `scope_changed_escalate` and
`blocked_missing_context` route to `done_blocked`. On resume: re-read the plan file if
it exists, check git log for any prior work in this branch.

**`implementation`** — writes code and commits. Gates: on feature branch, has commits
beyond main, tests pass (`go test ./...`). Evidence fallback:
`implementation_status: enum[complete, partial_tests_failing_retry,
partial_tests_failing_escalate, blocked]`, `rationale: string`,
`decisions: array[{choice: string, rationale: string, alternatives_considered: string}]`
(optional, submitted with each evidence). When agents make non-obvious judgment calls
during implementation — assumptions about API behavior, tradeoff choices, approach
pivots — those decisions must be captured in the `decisions` array rather than buried
in the agent's reasoning. koto records them in the event log and surfaces them to the
user deterministically. The mechanism for mid-state decision capture is a cross-cutting
engine concern (see Phase 0c in Implementation Approach).
`partial_tests_failing_retry` self-loops (up to 3 iterations).
`partial_tests_failing_escalate` and `blocked` route to `done_blocked`. Directive
includes escalation threshold: switch from `_retry` to `_escalate` after 3 failed
submissions. On resume: re-read the plan file, check git log and git status to identify
what was already committed.

**`finalization`** — cleanup, summary file, final verification. Gates:
`test -f wip/{{ARTIFACT_PREFIX}}_summary.md` and tests pass. Evidence fallback:
`finalization_status: enum[ready_for_pr, deferred_items_noted, issues_found]`.
`issues_found` routes to `implementation` to address blocking issues. On resume: check
for existing summary file and test results.

**`pr_creation`** — creates the pull request. Always evidence-gated (no gate can verify
a PR was created before the action happens). Evidence: `pr_status: enum[created,
creation_failed_retry, creation_failed_escalate]`, `pr_url: string`. `creation_failed_retry`
self-loops. `creation_failed_escalate` routes to `done_blocked`. On resume: check if a
PR already exists for the current branch via `gh pr list --head $(git rev-parse --abbrev-ref HEAD)`.

**`ci_monitor`** — waits for CI to pass. Gate:
```
gh pr checks $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number \
  --jq '.[0].number // empty') --json state \
  --jq '[.[] | select(.state != "SUCCESS")] | length == 0' | grep -q true
```
The `// empty` guard handles the case where no PR is found (brief window after PR
creation). Evidence fallback: `ci_outcome: enum[passing, failing_fixed,
failing_unresolvable]`, `rationale: string`. `failing_unresolvable` routes to
`done_blocked`. Evidence fallback also serves as the retry mechanism for the brief
indexing window.

**`done`** — terminal. Workflow complete.

**`done_blocked`** — terminal. Records a blocking condition requiring human intervention.
Directive includes recovery instructions: "If the blocker has been resolved externally,
run `koto rewind <name>` once per step to walk back to the originating state. (`koto rewind`
rewinds one step per call; call it repeatedly to reach a non-adjacent origin state.)"
Reachable from multiple states via explicit escalation paths.

### Key Interfaces

**Initialize a workflow (issue-backed):**
```
koto init work-on-71 --template .koto/templates/work-on.md \
  --var ISSUE_NUMBER=71 --var ARTIFACT_PREFIX=issue_71
```
Creates `koto-work-on-71.state.jsonl` in the current directory. Returns
`{"name": "work-on-71", "state": "entry"}`. The `--var` flag stores the variable in
the workflow's `WorkflowInitialized` event; `{{ISSUE_NUMBER}}` and `{{ARTIFACT_PREFIX}}`
are substituted into gate commands at evaluation time.

**Initialize a workflow (free-form):**
```
koto init work-on-add-retry-logic --template .koto/templates/work-on.md \
  --var ARTIFACT_PREFIX=task_add-retry-logic
```
The `ARTIFACT_PREFIX` variable enables shared gates across both modes. No `ISSUE_NUMBER`
is set — issue-specific gates (`staleness_check`, `introspection`) are only reachable
in issue-backed mode.

**Get directive and advance:**
```
koto next work-on-71
```
Returns `{"action": "execute", "state": "<state>", "directive": "<text>", "expects": {...}}`.
For states with command gates that pass, koto auto-advances through them and stops at
the next evidence-required or terminal state. For gate-with-fallback states where the
gate fails, the engine falls through to `NeedsEvidence` — returning the directive with
`expects` populated and `agent_actionable: true`.

**Submit evidence:**
```
koto next work-on-71 --with-data '{"plan_outcome": "plan_ready", "approach_summary": "..."}'
```
Validates against the state's `accepts` schema. On success, appends `evidence_submitted`
event and advances. On failure (exit code 2), returns `{"error": {"code":
"invalid_submission", "details": [...]}}`.

**Discover active workflows:**
```
koto workflows
```
Scans cwd for `koto-*.state.jsonl` files. Returns array with name and current state.

**Resume after interruption:**
```
koto workflows          # find active work-on-* workflow
koto next work-on-71    # get current directive
```

### Data Flow

**Session start:** the skill checks `koto workflows` for an active `work-on-*` workflow.
If found, it calls `koto next <name>` to resume, re-reading wip/ artifacts and git log
first. If not found, it copies the template to `.koto/templates/work-on.md` (from the
plugin directory) if needed, calls `koto init`, then `koto next <name>` to enter `entry`.

**Session loop:** read directive from `koto next`, do the work, call `koto next` (bare or
with `--with-data`) to advance. The koto state file (`koto-<name>.state.jsonl`) is
committed to the feature branch alongside wip/ artifacts, enabling resume across sessions.

**Artifact files** created during the workflow (naming uses `ARTIFACT_PREFIX` — see Key
Interfaces):
- `wip/issue_<N>_context.md` (issue-backed only, created by extract-context.sh)
- `wip/<prefix>_baseline.md`
- `wip/issue_<N>_introspection.md` (issue-backed, stale path only)
- `wip/<prefix>_plan.md`
- `wip/<prefix>_summary.md`

For free-form workflows, the artifact slug is derived from the workflow name passed to
`koto init` (e.g., `work-on-add-retry-logic` produces `task_add-retry-logic_plan.md`).
The skill layer sets `ARTIFACT_PREFIX=task_<slug>` at init time.

## Implementation Approach

### Phase 0a: Template variables (`--var` support) — needs-design

See Platform Capabilities Required section. Gets its own design doc during /plan.

### Phase 0b: Default action execution — needs-design

See Platform Capabilities Required section. Gets its own design doc during /plan.

### Phase 0c: Mid-state decision capture — needs-design

See Platform Capabilities Required section. Gets its own design doc during /plan.

### Phase 1: Gate-with-evidence-fallback engine change

See Platform Capabilities Required section for the full specification and deliverables.
Independent of `--var` (Phase 0) and implementable in parallel.

### Phase 2: Template file

Write the template and validate it compiles cleanly. Reference `plugins/hello-koto/` for
the YAML syntax used in koto templates — specifically how `gates:`, `accepts:`, and
conditional `transitions:` with `when:` blocks are expressed in front-matter.

Deliverables:
- `shirabe/koto-templates/work-on.md`: the 17-state template with all directives, gate
  commands, and evidence schemas as specified in Solution Architecture. Judgment states
  get Tier 1 directives (action summary + resume preamble, 10-25 lines). Koto-gated
  states get Tier 2 error-fallback directives (3-8 lines: "koto should have advanced
  past this state automatically; if you see this, [specific fallback action]"). Gate
  commands referencing `{{ISSUE_NUMBER}}` use `--var` substitution from Phase 0.
- `koto template compile shirabe/koto-templates/work-on.md`: must pass with no errors.
  The compiler validates mutual exclusivity of transitions and rejects non-deterministic
  routing. Write YAML front-matter and markdown headings in lockstep (state name
  mismatches produce compile errors).
- Verify that all conditional self-loops use a `when` condition (unconditional self-loops
  trigger cycle detection in the engine); document this constraint in the template header.

### Phase 3: Shirabe skill integration

Update the /work-on skill to drive koto.

Deliverables:
- Updated /work-on skill instructions: remove the ~55-line orchestration wrapper
  (resume detection logic, phase dispatch, session management). Replace with: on
  invocation, check `koto workflows` for a `work-on-*` workflow in cwd. If found,
  re-read existing wip/ artifacts and git log, then resume via `koto next`. If not
  found, copy the template to `.koto/templates/work-on.md` (from the plugin directory)
  if it doesn't exist, then call `koto init`. koto tracks state; the skill calls
  `koto next` in a loop.
- The skill accepts three input forms: (1) a GitHub issue number — initializes with
  `--var ISSUE_NUMBER=<N> --var ARTIFACT_PREFIX=issue_<N>` and submits
  `mode: issue_backed`; (2) a free-form task description — initializes with
  `--var ARTIFACT_PREFIX=task_<slug>` (slug derived from workflow name) and submits
  `mode: free_form`; (3) a plan issue reference (`PLAN-<topic>#N`) — reads the PLAN doc
  at that sequence number, extracts goal, acceptance criteria, and design doc references,
  constructs a `task_description` from these fields, then initializes as `mode: free_form`
  with `ARTIFACT_PREFIX=task_<slug>`.
- Phase injection: before the agent begins work in `analysis` or `implementation` states,
  the skill wrapper reads and injects the corresponding phase procedure file
  (`references/phases/phase-3-analysis.md` or `phase-4-implementation.md`).
- Evidence submission loop: when `koto next` returns `expects` with fields, the skill
  instructions guide the agent to submit the correct evidence schema. When `koto next`
  returns `action: done`, the skill is complete.
- Error handling: on `invalid_submission` (exit code 2), re-read the `details` array,
  fix the evidence, and resubmit without retrying the same payload.
- Session stop hook: extend the existing koto Stop hook to mention work-on specifically
  when a `koto-work-on-*` workflow is active, and include the current state name in the
  reminder message.

### Phase 4: Documentation

Deliverables:
- Update `koto-skills` AGENTS.md to reflect the actual CLI signatures: positional `name`
  argument (not `--name` flag), `--var` flag, accurate `koto next` response shapes.
- Add a worked example to AGENTS.md showing the work-on workflow from `koto init` through
  `done`.
- Update the hello-koto template if any API contracts changed in Phase 1.

## Security Considerations

**Download verification**: koto does not download binaries. The template file is a
local markdown file read from disk. Not applicable.

**Execution isolation**: Gate commands and default actions run shell commands in the
user's working directory with the user's credentials. This is the same trust model as
running the commands manually. Gate commands are limited to: `git rev-parse`, `git log`,
`test -f`, `ls ... | grep -q`, `gh pr checks`, and `go test ./...`. Default actions
include: `extract-context.sh`, `check-staleness.sh`, `git checkout -b`, and baseline
scripts. All commands are static strings in the compiled template — no commands are
constructed from untrusted input at evaluation time (with the exception of `--var`
substitution, covered below).

**Default action safety**: The three-path model inverts the failure mode from opt-in
(agent must act) to opt-out (agent must prevent). The reversibility constraint limits
the blast radius: all default actions in this template are reversible (file creation,
branch creation, read-only checks). The child design for default action execution
(Phase 0b) must enforce this constraint — irreversible actions should not be executable
as defaults without explicit confirmation.

The `--var` flag (Phase 0 prerequisite) allows caller-controlled strings to be
substituted into gate commands at evaluation time. If a variable value contains shell
metacharacters (e.g., `; rm -rf ~`), it could be injected into the gate command.
Sanitization must happen at `koto init` time, before storing variables in the
`WorkflowInitialized` event: reject values containing characters outside a safe set
(alphanumeric, hyphens, dots, slashes) or quote and escape them. The compiled template
remains variable-agnostic; substitution happens at runtime from the stored variables map.
The Phase 0 child design must specify the exact sanitization approach.

Additionally, workflow names are incorporated into state file paths
(`koto-<name>.state.jsonl`). Names must be validated at `koto init` time against a
strict pattern to prevent path traversal (e.g., `../../../etc/koto.state.jsonl`). This
is listed as a prerequisite in Platform Capabilities Required.

**Supply chain risks**: The template is shipped as part of the shirabe plugin. Trust
in the template is the same as trust in the shirabe plugin itself. No external content
is fetched at workflow runtime. The koto cache stores compiled template JSON keyed by
content hash; a modified template produces a different hash and a new cache entry, so
cached templates are not silently stale. Low residual risk: the `ci_monitor` gate
trusts the GitHub API's CI status response; a compromised GitHub API could return false
SUCCESS and cause the workflow to complete without verified CI. This risk is pre-existing
(the same threat applies to any tool that reads `gh pr checks`) and not introduced by
this design.

**User data exposure**: The event log (`koto-<name>.state.jsonl`) is written to the
project working directory. It contains evidence submitted by the agent, which may include
issue summaries, PR URLs, and rationale strings. The file is committed to the feature
branch and visible to anyone with repository access. Agents should not include secrets
or credentials in evidence fields; the skill instructions should make this explicit.
No data is transmitted outside the local machine by koto itself.

## Consequences

### Positive

- Agent instructions shrink ~15-20% from structured enforcement alone (orchestration
  wrapper eliminated, structured evidence, koto-managed sequencing). With default action
  execution (Phase 0b), the reduction reaches ~42% (420 of 995 lines eliminable) as
  deterministic steps execute without agent involvement on the happy path.
- Five deterministic states auto-advance on the happy path (`context_injection`,
  `setup_issue_backed`, `setup_free_form`, `staleness_check`, `ci_monitor`). Agents never
  see their directives when the default action succeeds. Four judgment-with-verification
  states (`introspection`, `analysis`, `implementation`, `finalization`) auto-advance on
  resume when the agent's work from a prior session is already complete.
- The three-path model makes override and failure handling explicit per state. The
  event log records whether a state completed via default, override, or recovery —
  useful for debugging and workflow audit.
- User overrides are first-class: "use my existing branch" or "skip staleness" are
  structured evidence submissions, not implicit agent behavior. Different skill wrappers
  can support overrides using the same template.
- The reversibility constraint provides a principled safety boundary: deterministic states
  only auto-execute reversible actions. Irreversible steps (PR creation) require agent
  confirmation.
- Phase order is enforced without additional state management in the /work-on skill.
  The agent can't reach `ci_monitor` without a PR, or `analysis` without passing
  through `staleness_check` (issue-backed) or `post_research_validation` (free-form).
- Session resume is supported. `koto next <name>` returns the current directive after
  interruption without any orchestration wrapper.
- The split topology eliminates epoch-scoped mode re-submission. Two setup states with
  unconditional transitions make routing self-documenting.

### Negative

- Four prerequisite engine capabilities plus workflow name validation are required. Three
  are `needs-design` issues (Phase 0a `--var` + Phase 0b default action execution +
  Phase 0c decision capture). Phase 1 (gate fallback) and workflow name validation are
  targeted changes. Each needs-design item gets its own design doc during /plan.
- Phase 0b (default action execution) and Phase 0c (decision capture) are the largest
  unknowns. Their scope depends on the child designs.
- Test commands in gates are language-specific (`go test ./...`). Non-Go projects need
  a different test command.
- The 17-state template is authoring-heavy with no tooling support. The compiler reports
  one error at a time; state name mismatches produce compile errors.
- `koto rewind` rewinds one step per call. Recovering from `done_blocked` to a
  non-adjacent originating state requires multiple calls.

### Mitigations

- All Phase 0 items and Phase 1 can be designed and implemented in parallel (see
  Platform Capabilities Required for the dependency between Phase 0a and 0b).
- The child designs can scope to the minimum needed for this template — they don't need
  to be general-purpose engine features in the first release. Phase 0c (decision capture)
  is cross-cutting but can start with the work-on template's needs.
- Add `TEST_COMMAND` as a template variable with a default of `go test ./...`, making
  it configurable without changing the template structure.
- `koto rewind` is CLI-callable (confirmed in source). The directive for `done_blocked`
  enumerates recovery paths or uses generic guidance ("rewind until you reach state X").
  See Acknowledged Limitations in Platform Capabilities Required for the static-directive
  constraint.
