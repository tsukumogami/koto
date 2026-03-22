---
status: Proposed
problem: |
  shirabe's /work-on skill requires a GitHub issue number. Agents and users often want
  to implement a small task without first creating an issue — a branch, a PR, CI green,
  done. Without issue-backed context, /work-on has no way to load design context or run
  a staleness check. The skill either requires an issue or provides no structured
  guidance at all. koto's template engine can enforce the workflow phase structure for
  both cases in a single template, making the issue optional while preserving enforcement.
decision: |
  A single koto template backing /work-on with 15 states and two modes: issue-backed
  (GitHub issue number provided) and free-form (task description provided, no issue).
  The modes share all states from analysis onward and diverge only in their context-
  gathering phases. States use the gate-with-evidence-fallback pattern: command gates
  auto-advance when outcomes are mechanically verifiable, and fall back to requiring
  agent evidence (with rationale) when they fail. Two engine changes unlock this —
  gate failure on states with an accepts block routes to NeedsEvidence rather than
  hard-stopping, and the GateBlocked CLI response carries the expects schema when a
  fallback is available. Template variables supply issue-specific values at init time
  via a new --var flag on koto init.
rationale: |
  Gate-with-evidence-fallback captures what the other approaches miss. Fine-grained
  evidence at every state produces ceremony-heavy templates where most evidence fields
  don't affect routing. Coarse-grained checkpoints lose enforcement at the staleness
  branch, the critical decision point. Pure auto-advancing states give the agent no
  instruction and produce no audit record. Gate-with-evidence-fallback keeps evidence
  where it matters: at genuine branching decisions and at gates that can fail. Evidence
  fields capture the agent's decision as structured enums and a rationale string, not
  generic completion flags.
---

# DESIGN: shirabe work-on koto template

## Status

Proposed

## Context and Problem Statement

shirabe's /work-on skill implements a GitHub issue: gather context, plan, implement,
create a PR, verify CI, done. The skill currently requires a GitHub issue number to
load design context and run a staleness check. Agents often need to implement a small
task without an existing issue — a config tweak, a doc fix, a quick refactor — and
creating an issue first adds friction with no benefit.

The skill needs a free-form mode: accept a task description instead of an issue number,
skip the GitHub context and staleness phases, and run a lightweight validation step
instead. The implementation phases are identical in both modes.

koto enforces workflow phase structure through a state machine template. A koto template
for /work-on would: enforce that the agent completes each phase before advancing, persist
progress across sessions, and make the workflow auditable. /work-on is a natural first
koto template because it's linear (one workflow, one session), maps cleanly to koto's
state machine model, and requires no external integrations — the agent handles all
external actions (git, GitHub, CI) within state directives.

## Decision Drivers

- koto's surface must stay minimal — no new subcommands or integration runner config
  required for this template to work
- The agent handles all external actions; koto enforces phase order via evidence-gated
  transitions
- The template must support both modes: issue-backed (GitHub issue number) and free-form
  (task description, no issue)
- Session resumability: koto's event log handles mid-session interruption without
  additional state management
- The staleness/introspection check must be able to route directly to analysis without
  forcing introspection
- Evidence schemas must capture agent decisions, not just confirm completion — `{done:
  true}` evidence that can be submitted regardless of what actually happened defeats the
  enforcement purpose

## Considered Options

### Decision 1: Template granularity and gate model

The core question is where evidence gates belong. Put them everywhere and the template
enforces every transition, but agents end up submitting boilerplate evidence that doesn't
carry any decision content. Put them only at coarse checkpoints and the template loses
visibility at the transitions that actually branch — particularly the staleness check,
where the routing decision (introspect or skip) is entirely within the agent's judgment.

The third path is to use command gates for mechanically verifiable outcomes and fall back
to evidence only when the gate can't verify the work or when the agent needs to override.
This keeps evidence at genuine decision points while letting deterministic checks handle
the rest.

#### Chosen: Gate-with-evidence-fallback

States with command gates auto-advance when the gate passes. When a gate fails — because
the work isn't done yet or the agent is intentionally deviating — koto surfaces the
state's `accepts` schema and requires the agent to submit a decision record. The decision
record uses meaningful enum fields (e.g., `branch_action: created|reused_existing`) and
a `rationale` string, not `{done: true}`. The rationale is stored permanently in the
event log.

This splits states into two categories. States that represent deterministic outcomes
(branch exists, file was created, tests pass, CI is green) get command gates; if the gate
passes, the agent called `koto next` and koto advanced without asking for anything.
States that represent agent judgment (staleness assessment, task scope validation) are
always evidence-gated because there's no command that returns the right answer.

The implementation requires two engine changes to koto, described in Solution Architecture.

#### Alternatives Considered

**Fine-grained evidence at every state**: Evidence gates on every state transition
(~9 states with `accepts` blocks), with fields like `baseline_established: true`,
`context_loaded: true`, and `commits_pushed: true`. Rejected because most evidence fields
don't affect routing — they're just confirmation that the agent did something. A template
that requires 9 `--with-data` submissions per workflow, most of which carry no routing
information, adds ceremony without adding enforcement value.

**Coarse-grained checkpoint states**: 3-4 large checkpoint states covering multiple
skill phases internally. Simple template, low overhead. Rejected because the staleness
check — the most important branch in the workflow — happens inside a Setup checkpoint
with no koto visibility. Agents could skip introspection silently. The audit trail would
show "Setup completed" without recording whether staleness was assessed.

---

### Decision 2: Gate fallback opt-in mechanism

When a state has both `gates` and `accepts`, the koto engine currently hard-stops at gate
failure without consulting `accepts`. Changing this behavior requires a mechanism to
distinguish "this gate is a hard prerequisite" from "this gate is a fast path with an
evidence fallback."

#### Chosen: Implicit convention — co-presence of gates and accepts implies fallback

If a state has both `gates` and `accepts`, gate failure routes to evidence (NeedsEvidence)
rather than hard-stopping. If a state has only `gates` and no `accepts`, gate failure
remains a hard stop. This is backward-compatible: existing templates with gate-only states
behave identically. Template authors don't need to add a new YAML key; the semantics are
self-documenting from the template structure.

#### Alternatives Considered

**Explicit opt-in flag on the state or gate**: Add a `fallback: evidence` field on the
gate block or a `gate_mode: fallback` field on the state. Rejected because it adds a
new YAML key with no benefit over the implicit convention — the presence of an `accepts`
block already signals that the template author intends evidence to be submitted here.
The implicit model is both simpler and harder to misuse.

---

### Decision 3: Template variable substitution

Several gate commands need issue-specific values, particularly the GitHub issue number
used in `gh issue view` and introspection artifact paths. Template variables (`--var`)
are the natural mechanism, but `koto init` doesn't currently implement the `--var` flag
— it exists in documentation but not in the engine.

#### Chosen: Add --var support to koto init as part of this work

Template variable substitution (`{{VAR_NAME}}` in directive text and gate commands) is
a required dependency for this template to work correctly. This design treats it as a
Phase 1 prerequisite rather than a workaround to defer. The alternative — static gate
commands that use shell introspection to find artifact paths (e.g., `test -f
wip/*_baseline.md`) — works for many states but can't verify issue-specific GitHub
access before the agent spends time loading context.

#### Alternatives Considered

**Static template with shell globbing in gates**: Replace `{{ISSUE_NUMBER}}` references
with shell commands that discover the relevant path (e.g., `test -f wip/*_baseline.md`
instead of `test -f wip/issue_{{ISSUE_NUMBER}}_baseline.md`). Rejected as the primary
approach because it makes gate commands brittle (glob matches the first file in wip/,
even one from a different workflow), and it doesn't solve the `gh issue view` gate which
genuinely needs the issue number. Acceptable as a fallback for individual states but not
as the overall strategy.

---

### Decision 4: How the optional issue input is handled

The two modes of /work-on — issue-backed and free-form — differ only in their first few
phases. The question is whether to express this as a single template with mode-specific
states, or as two separate templates that share most of their content.

#### Chosen: Single template with a routing entry state

A single `entry` state captures `mode: enum [issue-backed, free-form]` plus mode-specific
parameters (issue number or task description) and routes to the appropriate first state.
The two paths diverge through their context-gathering states, then converge at `setup`
and from there share all remaining states.

koto's evidence is epoch-scoped: each state transition clears the current evidence, so
`mode` can't carry forward automatically from `entry` to `setup`. The `setup` state
therefore includes `mode` in its own `accepts` schema, requiring the agent to re-submit
it alongside the branch and baseline evidence. This is a general pattern for any routing
field that spans multiple states: re-submit it at the state where the routing decision
is made.

#### Alternatives Considered

**Two separate templates**: `work-on.md` (issue-backed) and `work-on-free.md`
(free-form), each with their own initial states, converging on shared states from
analysis onward. The shared states would need to be duplicated or split into a third
included template, which koto doesn't support. Rejected because it reintroduces
duplication that koto's single-template model is meant to eliminate.

## Decision Outcome

**Chosen: gate-with-evidence-fallback, implicit convention, --var support, single template**

The template has 15 states across two converging modes. The issue-backed mode:
`entry` → `context_injection` → `setup` → `staleness_check` → (optional) `introspection`
→ `analysis` → `implementation` → `finalization` → `pr_creation` → `ci_monitor` → `done`.
The free-form mode: `entry` → `task_validation` → (if not ready: `validation_exit`) →
`research` → `setup` → `analysis` (and from there, identical to issue-backed).

The `entry` state is always evidence-gated: it captures `mode` and mode-specific
parameters, then routes to the correct first state. Both modes reach `setup` via different
routes. koto's evidence is epoch-scoped — evidence submitted in one state is only accessible
within that state's epoch and is cleared on transition. `setup` therefore includes
`mode` in its own `accepts` schema, requiring the agent to re-submit it alongside the
branch and baseline evidence. This makes `setup` always evidence-gated (not just
gate-with-fallback), since the routing decision requires `mode` regardless of whether
the gates pass. From `analysis` onward, both modes are identical.

Five states have command gates that enable auto-advancement when the work is mechanically
verifiable: `context_injection` (issue accessibility), `introspection` (artifact file),
`analysis` (plan file), `finalization` (summary file + tests), and `ci_monitor` (all CI
checks passing). When their gates pass, `koto next` advances without asking for anything.
When gates fail, koto surfaces the state's `accepts` schema and the agent submits a
decision record with a meaningful enum field and optional rationale. Five states are always
evidence-gated because they represent genuine branching decisions or require routing
evidence: `entry`, `task_validation`, `setup`, `staleness_check`, and `pr_creation`.

Two engine changes are needed before the template can be used. First, the advancement
loop in `src/engine/advance.rs` must fall through to `NeedsEvidence` when a gate fails
on a state that also has an `accepts` block, rather than unconditionally returning
`GateBlocked`. Second, the `GateBlocked` CLI response in `src/cli/next_types.rs` and
`src/cli/mod.rs` must carry the `expects` schema and set `agent_actionable: true` on
blocking conditions when a fallback is available. The `--var` flag on `koto init` must
also be implemented to support issue-number substitution in gate commands.

### Rationale

The gate-with-evidence-fallback model reflects what enforcement actually means in an
agent workflow. Agents aren't trying to skip phases — they're making judgment calls about
what a phase requires in context. An agent continuing in an existing branch rather than
creating a new one, or skipping introspection on a fresh codebase, is making a reasonable
decision that should be recorded, not blocked. Command gates handle the mechanical cases
(did the artifact get created? do tests pass?) while evidence captures the judgment calls.
The result is a template that enforces the workflow shape without becoming an obstacle for
agents that know what they're doing.

## Solution Architecture

### Overview

The template is a koto state machine that enforces the /work-on workflow structure for
both modes. When an agent starts the skill, it initializes a workflow from the template
and calls `koto next <name>` in a loop to get directives and advance state. koto enforces
sequencing: an agent can't reach `analysis` without passing through `staleness_check`
(issue-backed) or `task_validation` (free-form), and can't reach `ci_monitor` without
passing through `pr_creation`.

### Components

**`shirabe/koto-templates/work-on.md`** — the template file. 15 states with directives,
gate commands, and evidence schemas. Lives in shirabe's plugin directory and is copied
to `.koto/templates/work-on.md` in the project on first use.

**koto engine** (two changes to existing Rust files):
- `src/engine/advance.rs`: gate-with-evidence-fallback logic in the advancement loop
- `src/cli/next_types.rs` + `src/cli/mod.rs`: extend GateBlocked response to carry
  `expects` and set `agent_actionable: true` when a fallback is available
- `src/cli/mod.rs` (init command): implement `--var KEY=VALUE` flag (repeatable). Store
  the resulting `HashMap<String, String>` in the `variables` field of the
  `WorkflowInitialized` event (already defined, currently always empty). Substitute
  `{{KEY}}` in gate command strings at gate evaluation time by reading from the stored
  variables map — this avoids a compiler change and keeps the compiled template
  variable-agnostic

**shirabe /work-on skill** (updated): calls `koto init` on first run, loops `koto next`
for directives, submits evidence via `koto next --with-data`, and resumes via
`koto workflows` + `koto next` on session restart.

### State Machine

```
entry (evidence: mode)
  │
  ├─ issue-backed mode:
  │   context_injection[G] → setup* → staleness_check
  │                                        │
  │                             ┌──────────┴──────────────┐
  │                        fresh/stale_skip         stale_requires_introspection
  │                             │                          │
  │                             │                  introspection[G]
  │                             │                          │
  │                             └──────────┬───────────────┘
  │                                        ▼
  └─ free-form mode:               analysis[G] ◄─── scope_changed (self-loop)
      task_validation                    │                 │
          │                     implementation[G] ◄─── partial_tests_failing
    ┌─────┴──────────────┐             │ │
 validation_exit      research      finalization[G]    done_blocked
 (terminal)              │                │              (terminal)
                        setup*        pr_creation ◄─── creation_failed
                         │                │
                         │           ci_monitor[G]
                         │                │        \
                         │              done      done_blocked
                         │           (terminal)   (terminal)
                         │
                      (converges to analysis)
```

`[G]` = has command gate (auto-advances when gate passes).
`*` = always evidence-gated (routing requires `mode` re-submission).

States always evidence-gated: `entry`, `task_validation`, `setup*`, `staleness_check`,
`pr_creation`.

`done_blocked` is reachable from three states: `analysis` (`plan_outcome:
blocked_missing_context`), `implementation` (`implementation_status: blocked`), and
`ci_monitor` (`ci_outcome: failing_unresolvable`).

### State Definitions

**`entry`** — routes issue-backed vs free-form mode. Evidence: `mode: enum[issue-backed,
free-form]`, `issue_number: string` (issue-backed mode), `task_description: string`
(free-form mode).

**`task_validation`** — assesses whether the free-form task description is clear and
appropriately scoped for direct implementation. Evidence: `verdict: enum[ready,
needs_design, needs_breakdown, ambiguous]`, `rationale: string`.

**`validation_exit`** — terminal state for tasks that aren't ready for direct
implementation. Directive instructs the agent to communicate the verdict and suggest the
appropriate next step (create an issue, write a design doc, etc.).

**`research`** — lightweight context gathering for free-form tasks. Evidence:
`context_gathered: enum[sufficient, insufficient]`, `context_summary: string`.

**`context_injection`** — loads GitHub issue context. Gate: `gh issue view
{{ISSUE_NUMBER}} --json number --jq .number`. Evidence fallback: `context_loaded:
enum[loaded, issue_not_accessible, context_incomplete]`, `context_summary: string`.

**`setup`** — creates feature branch and baseline file. Always evidence-gated. Gates:
branch is not main/master, baseline file exists. Evidence: `mode:
enum[issue-backed, free-form]` (re-submitted here for epoch-local routing), `branch_created:
enum[created, reused_existing]`, `branch_name: string`, `baseline_outcome: enum[clean,
existing_failures, build_broken]`. Gate failure triggers evidence fallback; the routing
to `staleness_check` (issue-backed) or `analysis` (free-form) is determined by `mode`
in this state's evidence epoch.

**`staleness_check`** — assesses codebase freshness since the issue was opened. Always
evidence-gated (command gates can't inspect script output). Evidence:
`staleness_signal: enum[fresh, stale_skip_introspection, stale_requires_introspection]`,
`staleness_details: string`.

**`introspection`** — re-reads the issue against the current codebase. Gate:
`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`. Evidence fallback:
`introspection_outcome: enum[approach_unchanged, approach_updated, issue_superseded]`,
`rationale: string`.

**`analysis`** — researches and creates implementation plan. Gate:
`test -f wip/issue_{{ISSUE_NUMBER}}_plan.md` (issue-backed) or `test -f wip/task_*_plan.md`
(free-form). Note: glob-based gates can match artifacts from prior workflows; once
`--var` is available, both modes should use anchored paths. Evidence
fallback: `plan_outcome: enum[plan_ready, blocked_missing_context, scope_changed]`,
`approach_summary: string`. Self-loop on `scope_changed`. `blocked_missing_context`
routes to `done_blocked`.

**`implementation`** — writes code and commits. Gates: on feature branch, has commits
beyond main, tests pass. Evidence fallback: `implementation_status: enum[complete,
partial_tests_failing, blocked]`, `rationale: string`. Self-loop on
`partial_tests_failing`. `blocked` routes to `done_blocked`.

**`finalization`** — cleanup, summary file, final verification. Gates: summary file
exists, tests pass. Evidence fallback: `finalization_status: enum[ready_for_pr,
deferred_items_noted, issues_found]`.

**`pr_creation`** — creates the pull request. Always evidence-gated (no gate can prove
a PR was created before the action happens). Evidence: `pr_status: enum[created,
creation_failed]`, `pr_url: string`. Self-loop on `creation_failed`.

**`ci_monitor`** — waits for CI to pass. Gate: `gh pr checks $(gh pr list --head
$(git rev-parse --abbrev-ref HEAD) --json number --jq '.[0].number') --json state
--jq '[.[] | select(.state != "SUCCESS")] | length == 0' | grep -q true`. The gate
explicitly resolves the PR number from the current branch to avoid ambiguity when
multiple PRs exist. Note: there is a brief window after `pr_creation` where the PR
may not yet be indexed by the GitHub API; if the gate fails immediately after PR
creation, the evidence fallback allows the agent to wait and retry. Evidence fallback:
`ci_outcome: enum[passing, failing_fixed, failing_unresolvable]`, `rationale: string`.
`failing_unresolvable` routes to `done_blocked`.

**`done`** — terminal. Workflow complete.

**`done_blocked`** — terminal. Records a blocking condition requiring human intervention.
Reachable from `analysis` (missing context), `implementation` (blocked), and `ci_monitor`
(unresolvable failure).

### Key Interfaces

**Initialize a workflow (issue-backed):**
```
koto init work-on-71 --template .koto/templates/work-on.md --var ISSUE_NUMBER=71
```
Creates `koto-work-on-71.state.jsonl` in the current directory. Returns
`{"name": "work-on-71", "state": "entry"}`.

**Initialize a workflow (free-form):**
```
koto init work-on-add-retry-logic --template .koto/templates/work-on.md
```
No `--var` needed for free-form mode since no issue-specific gate commands apply.

**Get directive and advance:**
```
koto next work-on-71
```
Returns `{"action": "execute", "state": "<state>", "directive": "<text>", "expects": {...}}`.
For states with command gates that pass, koto auto-advances through them and stops at
the next evidence-required or terminal state. For gate-with-fallback states where the
gate fails, returns `GateBlocked` with `expects` populated and `agent_actionable: true`.

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

On session start, the skill checks `koto workflows` for an active `work-on-*` workflow.
If found, it calls `koto next <name>` to resume at the current state. If not found, it
calls `koto init` to start fresh, then `koto next <name>` to enter `entry`.

The agent loops: read directive from `koto next`, do the work, call `koto next` (bare or
with `--with-data`) to advance. koto's evidence is epoch-scoped: each state transition
clears the current evidence, so only evidence submitted in the current state is accessible
for routing. `mode` is captured at `entry` for routing to the first mode-specific state,
then re-submitted at `setup` for routing to the post-setup state (`staleness_check` for
issue-backed, `analysis` for free-form). This re-submission is required — the `setup`
evidence schema includes `mode` as a required field.

wip/ artifact files created during the workflow:
- `wip/issue_<N>_baseline.md` (issue-backed) or `wip/task_<slug>_baseline.md` (free-form)
- `wip/issue_<N>_introspection.md` (issue-backed, stale path only)
- `wip/issue_<N>_plan.md` (issue-backed) or `wip/task_<slug>_plan.md` (free-form)
- `wip/issue_<N>_summary.md` (issue-backed) or `wip/task_<slug>_summary.md` (free-form)

The koto state file (`koto-<name>.state.jsonl`) is committed to the feature branch
alongside wip/ artifacts, enabling resume in a new session by checking out the branch
and calling `koto next`.

## Implementation Approach

### Phase 1: Engine changes

These changes unlock the gate-with-evidence-fallback pattern and enable template
variables in gate commands. They're prerequisites for the template.

Deliverables:
- `src/engine/advance.rs`: When evaluating gates, if any gate fails and the current state
  has an `accepts` block, skip the hard `GateBlocked` return and fall through to
  `NeedsEvidence`. The existing transition resolution logic already handles this case
  correctly once reached.
- `src/cli/next_types.rs`: Add an `expects` field to the `GateBlocked` response variant,
  populated via `derive_expects` when the state has an `accepts` block.
- `src/cli/mod.rs` (GateBlocked arm): Set `agent_actionable: true` on blocking conditions
  when the state has both gates and accepts. Populate the `expects` field.
- `src/cli/mod.rs` (init command): Add `--var KEY=VALUE` flag (repeatable). Store in the
  `WorkflowInitialized` event's `variables` field. At gate evaluation time, substitute
  `{{KEY}}` in gate command strings by reading from the stored variables map. Sanitize
  variable values at `koto init` time: reject or quote values containing shell
  metacharacters to prevent command injection.
- `src/cli/mod.rs` (init command): Validate the workflow name against a strict pattern
  (`^[a-zA-Z0-9][a-zA-Z0-9-]*$`) to prevent path traversal in state file paths.
- Tests: add engine tests for gate-failure-with-fallback behavior, CLI output shape for
  the new GateBlocked-with-fallback response, `--var` substitution, and workflow name
  validation rejection.

### Phase 2: Template file

Write the template and validate it compiles cleanly. Reference `plugins/hello-koto/` for
the YAML syntax used in koto templates — specifically how `gates:`, `accepts:`, and
conditional `transitions:` with `when:` blocks are expressed in front-matter.

Deliverables:
- `shirabe/koto-templates/work-on.md`: the 15-state template with all directives, gate
  commands, and evidence schemas as specified in Solution Architecture. Gate commands
  referencing `{{ISSUE_NUMBER}}` are stubbed as no-ops until Phase 1 `--var` support
  is confirmed working; the evidence fallback path handles those states in the interim.
- `koto template compile shirabe/koto-templates/work-on.md`: must pass with no errors.
  The compiler validates mutual exclusivity of transitions and rejects non-deterministic
  routing.

### Phase 3: Shirabe skill integration

Update the /work-on skill to drive koto.

Deliverables:
- Updated /work-on skill instructions: on invocation, check `koto workflows` for a
  `work-on-*` workflow in cwd. If found, resume via `koto next`. If not found, copy the
  template to `.koto/templates/work-on.md` (from the plugin directory) if it doesn't
  exist, then call `koto init`.
- The skill accepts an optional issue number. When provided, initializes with
  `--var ISSUE_NUMBER=<N>` and submits `mode: issue-backed` at `entry`. When omitted,
  initializes without `--var` and submits `mode: free-form` with a task description.
- Evidence submission loop: when `koto next` returns `expects` with fields, the skill
  instructions guide the agent to submit the correct evidence schema. When `koto next`
  returns `action: done`, the skill is complete.
- Error handling: on `invalid_submission` (exit code 2), re-read the `details` array,
  fix the evidence, and resubmit without retrying the same payload.
- Session stop hook: extend the existing koto Stop hook to mention work-on specifically
  when a `koto-work-on-*` workflow is active.

### Phase 4: Documentation

Deliverables:
- Update `koto-skills` AGENTS.md to reflect the actual CLI signatures: positional `name`
  argument (not `--name` flag), `--var` flag (new), accurate `koto next` response shapes.
- Add a worked example to AGENTS.md showing the work-on workflow from `koto init` through
  `done`.
- Update the hello-koto template if any API contracts changed in Phase 1.

## Security Considerations

**Download verification**: koto does not download binaries. The template file is a
local markdown file read from disk. Not applicable.

**Execution isolation**: Command gates run shell commands in the user's working directory
with the user's credentials. This is the same trust model as running the gate commands
manually. The gate commands in this template are limited to: `git rev-parse`, `git log`,
`test -f`, `gh issue view`, `gh pr checks`, and `go test ./...`. No commands are
constructed from untrusted input at gate evaluation time, because gate commands are
static strings in the compiled template.

The `--var` flag introduced in Phase 1 allows caller-controlled strings to be substituted
into gate commands at evaluation time. If a variable value contains shell metacharacters
(e.g., `; rm -rf ~`), it could be injected into the gate command. Sanitization must
happen at `koto init` time, before storing variables in the `WorkflowInitialized` event:
reject values containing characters outside a safe set (alphanumeric, hyphens, dots,
slashes) or quote and escape them. The compiled template remains variable-agnostic;
substitution happens at runtime from the stored variables map.

Additionally, workflow names are incorporated into state file paths (`koto-<name>.state.jsonl`).
Names must be validated at `koto init` time against a strict pattern to prevent path
traversal (e.g., `../../../etc/koto.state.jsonl`).

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

- Phase order is enforced without additional state management in the /work-on skill.
  The agent can't reach `ci_monitor` without a PR existing, or `analysis` without
  passing through `staleness_check` (issue-backed) or `task_validation` (free-form).
- Evidence fields are decision records. The event log shows not just that a phase
  completed, but what decision the agent made and why — useful for debugging and audit.
- Session resume is free. Calling `koto next <name>` after a session interruption
  returns the current directive, regardless of how the session ended.
- The two modes share 9 of 15 states. Adding the free-form mode requires only 3 new
  states (`task_validation`, `validation_exit`, `research`) with no changes to the
  shared implementation path.
- Command gates auto-advance through mechanical checks without agent overhead. When
  tests pass and artifacts exist, `koto next` advances through `analysis`,
  `implementation`, and `finalization` without a `--with-data` call.

### Negative

- Two engine changes are required before the template works. The template can be written
  and compiled, but the gate-with-evidence-fallback behavior won't activate until the
  advancement loop is patched.
- The `--var` flag must be implemented for gate commands to reference `{{ISSUE_NUMBER}}`.
  Without it, gates that reference the issue number fall through to evidence fallback
  unconditionally, which degrades auto-advancement but doesn't break the workflow.
- The staleness check always requires agent evidence, even when the staleness script
  runs successfully. Command gates can only check exit codes, not script output content,
  so the routing decision (fresh/stale) must always be submitted by the agent.
- koto's evidence is epoch-scoped: each state transition clears the current evidence.
  Routing fields like `mode` can't carry forward automatically from `entry` to `setup`.
  The design handles this by requiring `mode` to be re-submitted in `setup`'s evidence
  schema. Any future state that needs routing information from an earlier state must
  follow the same pattern: include the routing field in its own `accepts` block.
- Test commands in gates are language-specific (`go test ./...`). Templates for non-Go
  projects need a different test command; ideally a `TEST_COMMAND` template variable.

### Mitigations

- The engine changes are targeted (two files, one new flag). They don't affect existing
  templates with gate-only states, which continue to hard-block on gate failure.
- Until `--var` is implemented, gates that reference `{{ISSUE_NUMBER}}` can be replaced
  with glob-based equivalents (`test -f wip/issue_*_introspection.md`) as a temporary
  workaround. This is less precise but keeps the template functional.
- The staleness check limitation is inherent to command gates; it's documented in the
  template directive and evidence schema. Future work could add an output-matching gate
  type (e.g., `type: command_output`) to close this gap.
- The epoch-scoped evidence pattern (re-submit routing fields at each state that needs
  them) is documented in the template's header comment. It's a straightforward authoring
  rule once understood: if a state routes differently based on earlier decisions, include
  those fields in its own accepts schema.
- Add `TEST_COMMAND` as a template variable with a default of `go test ./...`, making it
  configurable without changing the template structure.
