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
  A single koto template backing /work-on with 17 states and two modes: issue-backed
  (GitHub issue number provided) and free-form (task description provided, no issue).
  The modes share all states from analysis onward and diverge through mode-specific
  context-gathering phases. The template uses a split topology: two separate setup
  states (setup_issue_backed and setup_free_form) eliminate the epoch-scoped mode
  re-submission that a single setup state requires. Context injection gates on the
  existence of wip/IMPLEMENTATION_CONTEXT.md to ensure extraction happened before
  setup. Free-form mode has two validation states — pre-research and post-research —
  to catch misconceived tasks at both the description stage and the codebase-discovery
  stage. Self-looping states use retry/escalate enum variants with explicit escalation
  paths to done_blocked. Directives are concise with resume-oriented preambles; the
  skill wrapper supplements complex phases with full procedure.
rationale: |
  The split topology eliminates the most confusing element of the previous design:
  requiring agents to re-submit mode at setup despite having submitted it at entry.
  Two separate setup states make routing self-documenting without requiring epoch-scoped
  evidence knowledge. Gating context_injection on the IMPLEMENTATION_CONTEXT.md artifact
  fixes the panel finding that the accessibility-only gate allowed skipping context
  extraction entirely. Two free-form validation states reflect the just-do-it skill's
  two-jury structure: the pre-research check catches obviously-wrong tasks cheaply, the
  post-research check catches tasks that codebase discovery reveals to be misconceived.
  Retry/escalate variants in self-looping states give agents a structured escalation
  path with a clear audit record. Concise directives with resume preambles solve the
  reorientation gap while keeping the template maintainable.
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

### Decision 1: Workflow mode routing topology

The template supports two modes: issue-backed (GitHub issue number provided) and
free-form (task description only). The modes share all states from analysis onward
but diverge through their context-gathering phases. koto's evidence is epoch-scoped —
each state transition clears the current evidence, so routing fields cannot carry
forward automatically between states.

Source code investigation confirmed that `--var` CLI support is not implemented
(`variables: HashMap::new()` hardcoded in the init handler) and `{{VAR_NAME}}` gate
substitution is not implemented. These findings eliminated option (c) as currently viable.

#### Chosen: Split topology — two separate setup states

The template uses an entry state that accepts mode evidence and routes to diverged paths:
`context_injection` for issue-backed, `task_validation` for free-form. Each path
terminates in a mode-specific setup state: `setup_issue_backed` transitions
unconditionally to `staleness_check`; `setup_free_form` transitions unconditionally to
`analysis`. No mode re-submission is required — routing is implicit in which setup state
the agent is in. Both paths merge at `analysis` and share all subsequent states.

Key assumptions: the `--var` CLI flag and `{{VAR_NAME}}` gate substitution will be
implemented in a future koto release; when both ship, the template should migrate toward
init-time mode determination (option c below), reducing from ~17 to ~14 states. The two
setup states will have distinct directive content covering mode-specific preparation work.

#### Alternatives Considered

**Entry state with single setup state and mode re-submission (a)**: Mode submitted at
`entry` routes to diverged paths; mode re-submitted at single `setup` determines the
post-setup routing target. Rejected because re-submission requires contributors to
understand epoch-scoped evidence — a non-obvious engine property that creates an unbounded
maintenance cost every time someone reads or extends the template.

**Init-time `--var` flag (c)**: Mode encoded at `koto init` via `--var MODE=issue-backed`,
entry state eliminated, initial state determined at init. Rejected because both `--var`
CLI support and `{{VAR_NAME}}` gate substitution are not implemented in the current engine.
This is the target architecture for a future template version.

**Two separate templates (d)**: Separate `work-on-issue.md` and `work-on-freeform.md`
files. Rejected because it duplicates approximately 12 shared states, violates the
duplication constraint, and recreates the divergence problem that motivated this design.

---

### Decision 2: Context injection depth

The `context_injection` state backs Phase 0 of /work-on, which runs `extract-context.sh`
to create `wip/IMPLEMENTATION_CONTEXT.md`. This file carries design rationale forward —
Phase 4 (implementation) explicitly references it. The original design gated on issue
accessibility (`gh issue view {{ISSUE_NUMBER}}`), which checks reachability but doesn't
verify that context extraction happened. A panel review identified this as a core gap:
"The entire context injection purpose is lost."

#### Chosen: Gate on context artifact existence; extraction is the state's work

The `context_injection` directive instructs the agent to run `extract-context.sh`. The
gate is `test -f wip/IMPLEMENTATION_CONTEXT.md`. On the first `koto next` call, the gate
fails (file doesn't exist); the agent runs the script, then calls `koto next` again — the
gate passes and koto auto-advances to `setup_issue_backed`. The fixed path requires no
`--var` support and matches the real skill's convention.

Key assumption: `extract-context.sh` creates `wip/IMPLEMENTATION_CONTEXT.md` at a fixed
path. If the real script uses a parameterized path, the gate path and directive references
need revision.

#### Alternatives Considered

**Gate on issue accessibility only (a)**: Current design. Even if `--var` shipped, the
gate auto-advances on issue existence without verifying extraction happened. An agent can
skip context extraction entirely with no consequence in the state machine. Rejected.

**Separate context_extraction state after accessibility check (c)**: Stronger enforcement
but the accessibility gate requires unimplemented `--var`. Extract-context.sh fails
naturally on inaccessible issues, making an explicit accessibility state unnecessary until
`--var` ships. Rejected as premature.

**Fold context work into analysis directive (d)**: Removes koto enforcement entirely —
context loading becomes a suggestion with no structural guarantee. Rejected.

---

### Decision 3: Free-form validation sequence

The free-form path needs a mechanism to reject tasks not ready for direct implementation.
The original design had a single `task_validation` state before research, which assesses
only the task description — not the codebase state that research reveals. Two independent
panel reviewers identified the same gap: an agent that discovers a misconception during
research had no clean exit path other than `done_blocked`.

#### Chosen: Lightweight pre-research check + post-research validation

Two validation states in the free-form path:

1. `task_validation` (before research) — binary gate on description quality:
   `verdict: enum[proceed, exit]` with `rationale: string`. Catches obviously-wrong tasks
   (ambiguous description, clearly oversized scope) without spending research effort.

2. `post_research_validation` (after research, before setup) — ternary routing decision
   informed by codebase findings: `verdict: enum[ready, needs_design, exit]` with
   `rationale: string` and optional `revised_scope: string`. Catches tasks that research
   reveals to be misconceived relative to current codebase state.

Both states route to `validation_exit` (terminal). The free-form path becomes:
`task_validation` → `research` → `post_research_validation` → `setup_free_form`. This
matches just-do-it's jury → research → validation-jury → setup structure without requiring
multi-agent jury mechanics.

#### Alternatives Considered

**Pre-research validation only (a)**: Current design. Provides no exit path when research
reveals a task is misconceived. An agent in this situation can only route to `done_blocked`
— a permanent terminal rather than a graceful "not ready" signal. Rejected.

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
The original design's three-value enum missed Clarify and Amend, and left `issue_superseded`
without a routing target.

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

Key assumption: the introspection gate (`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`)
requires `--var` to work precisely. Until `--var` ships, this gate always fails and the
state operates as evidence-only (gate-with-evidence-fallback handles this correctly —
the agent submits evidence directly). This is consistent with the cross-validated behavior.

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

### Decision 6: Directive content model

Template directives must carry enough instruction for agents to execute each state
correctly, including after a session interruption. The original design specified evidence
schemas but wrote no directive text. The real /work-on skill has 50-130 lines of procedure
per major phase stored in `references/phases/` files.

#### Chosen: Concise directives with resume-oriented preambles; wrapper injection for complex phases

Each state's directive has two parts:

1. **Action summary** (10-25 lines): what the agent should accomplish in this state, key
   artifact paths, and evidence schema guidance. Sufficient for a template author to
   understand the state's purpose without reading SKILL.md.

2. **Resume preamble** (3-6 lines): explicit instructions to re-read relevant wip
   artifacts and check git state before continuing. Included for any state where prior
   work affects what the agent should do next.

For the two most complex states (`analysis`, `implementation`), the skill wrapper
supplements the directive with the full phase file content before the agent begins work.
The template directive orients the agent to workflow context; the injected phase file
provides procedural detail. Template maintenance: update SKILL.md phase files when
procedure changes; update template directives only when workflow structure changes.
These are different change triggers.

#### Alternatives Considered

**Self-contained full procedure (a)**: Embeds all 50-130 lines of procedure per phase.
Duplicates procedure from SKILL.md phase files, requires synchronized updates in two
places, produces an 800+ line template difficult to author and review. Rejected.

**Short directives referencing SKILL.md phase files (c)**: A directive that says "follow
phase-4-implementation.md." Requires reading external files to understand the template
— violates standalone readability. Rejected.

**Very short trigger directives with full wrapper injection (d)**: Template becomes an
empty scaffold. Agents running koto directly receive useless directives. Option (b) uses
selective wrapper injection for complex phases without sacrificing template readability.
Rejected as the primary approach.

---

## Decision Outcome

**Chosen: gate-with-evidence-fallback, split topology, artifact-gated context injection,
two-stage free-form validation, collapsed introspection outcomes, retry/escalate
self-loops, concise directives with resume preambles**

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

Five states have command gates enabling auto-advancement: `context_injection` (context
artifact), `introspection` (introspection artifact, evidence-only until `--var` ships),
`analysis` (plan file), `finalization` (summary + tests), and `ci_monitor` (CI checks).
When gates pass, `koto next` advances without asking for anything. When gates fail on
states with `accepts` blocks, koto surfaces the expects schema and the agent submits a
decision record. Five states are always evidence-gated: `entry`, `task_validation`,
`post_research_validation`, `staleness_check`, and `pr_creation`. `research` is also
evidence-gated with an unconditional transition (no routing decision needed).

Two engine changes are needed. First, the advancement loop in `src/engine/advance.rs`
must fall through to `NeedsEvidence` when a gate fails on a state that also has an
`accepts` block, rather than unconditionally returning `GateBlocked`. Second, the
`GateBlocked` CLI response in `src/cli/next_types.rs` and `src/cli/mod.rs` must carry
the `expects` schema and set `agent_actionable: true` when a fallback is available. The
`--var` flag on `koto init` must also be implemented to support issue-specific gate
commands — until then, gates referencing `{{ISSUE_NUMBER}}` fall through to evidence
fallback unconditionally, which degrades auto-advancement but doesn't break the workflow.

### Rationale

The split topology eliminates the most confusing element of the previous design: requiring
agents to re-submit mode at setup despite having already submitted it at entry. Two
separate setup states make routing self-documenting — the path you're on determines which
setup state you're in, with unconditional transitions to the appropriate next state. No
epoch-scoped evidence knowledge is required.

Gating `context_injection` on the IMPLEMENTATION_CONTEXT.md artifact fixes the panel
finding that the original accessibility-only gate allowed skipping context extraction
entirely. Two free-form validation states reflect the just-do-it skill's actual two-jury
structure. Retry/escalate variants in self-looping states give agents a structured
escalation path with a clear audit record. Concise directives with resume preambles solve
the reorientation gap the workflow practitioner identified while keeping the template
maintainable. The gate-with-evidence-fallback model itself remains: agents making
reasonable judgment calls (reusing a branch, skipping introspection on a fresh codebase)
record their decisions rather than getting blocked.

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
  │   staleness_check ─── stale_requires_introspection ──► introspection[G/E]
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

`[G]` = has command gate (auto-advances when gate passes).
`[G/E]` = gate-with-evidence-fallback (gate fails → evidence fallback).
States always evidence-gated (no gate): `entry`, `task_validation`,
`post_research_validation`, `staleness_check`, `pr_creation`.
`research` is evidence-gated with unconditional transition.

`done_blocked` is reachable from: `analysis` (`blocked_missing_context` and
`scope_changed_escalate`), `implementation` (`blocked` and `partial_tests_failing_escalate`),
`pr_creation` (`creation_failed_escalate`), `ci_monitor` (`failing_unresolvable`),
and `introspection` (`issue_superseded`).

### State Definitions

**`entry`** — routes issue-backed vs free-form mode. Evidence: `mode: enum[issue_backed,
free_form]`, `issue_number: string` (issue-backed only), `task_description: string`
(free-form only).

**`context_injection`** — creates context artifact for issue-backed workflows. Gate:
`test -f wip/IMPLEMENTATION_CONTEXT.md`. Directive instructs the agent to run
`extract-context.sh`, which reads the GitHub issue and linked design docs and writes the
context file. Gate passes once the file exists. No evidence fallback needed — the gate
is a hard block until the artifact is produced. On resume: check if file already exists
before re-running the script.

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
task can proceed with a narrowed scope). `exit` routes to `validation_exit`.
On resume: re-read the research context summary before assessing.

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

**`staleness_check`** — assesses codebase freshness since the issue was opened. Always
evidence-gated (command gates can't inspect script output content). Directive instructs
agent to run check-staleness.sh and read the `introspection_recommended` field from its
YAML output. Evidence: `staleness_signal: enum[fresh, stale_requires_introspection]`,
`staleness_details: string`. `fresh` or `stale_skip_introspection` routes to `analysis`;
`stale_requires_introspection` routes to `introspection`.

**`introspection`** — re-reads the issue against the current codebase via a sub-agent.
Gate: `test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md` (requires `--var`; operates
as evidence-only until `--var` ships). Evidence fallback:
`introspection_outcome: enum[approach_unchanged, approach_updated, issue_superseded]`,
`rationale: string`. `approach_unchanged` and `approach_updated` route to `analysis`.
`issue_superseded` routes to `done_blocked`. The `approach_updated` value covers both
Clarify and Amend outcomes from the sub-agent's internal loop.

**`analysis`** — researches and creates implementation plan. Gate:
`test -f wip/issue_{{ISSUE_NUMBER}}_plan.md` (issue-backed, requires `--var`) or
`ls wip/task_*_plan.md 2>/dev/null | grep -q .` (free-form, uses shell expansion).
Evidence fallback: `plan_outcome: enum[plan_ready, blocked_missing_context,
scope_changed_retry, scope_changed_escalate]`, `approach_summary: string`.
`scope_changed_retry` self-loops (up to 3 iterations). `scope_changed_escalate` and
`blocked_missing_context` route to `done_blocked`. On resume: re-read the plan file if
it exists, check git log for any prior work in this branch.

**`implementation`** — writes code and commits. Gates: on feature branch, has commits
beyond main, tests pass (`go test ./...`). Evidence fallback:
`implementation_status: enum[complete, partial_tests_failing_retry,
partial_tests_failing_escalate, blocked]`, `rationale: string`. `partial_tests_failing_retry`
self-loops (up to 3 iterations). `partial_tests_failing_escalate` and `blocked` route to
`done_blocked`. Directive includes escalation threshold: switch from `_retry` to
`_escalate` after 3 failed submissions. On resume: re-read the plan file, check git log
and git status to identify what was already committed.

**`finalization`** — cleanup, summary file, final verification. Gates: summary file
exists, tests pass. Evidence fallback: `finalization_status: enum[ready_for_pr,
deferred_items_noted, issues_found]`. `issues_found` routes to `implementation`
to address blocking issues. On resume: check for existing summary file and test results.

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
koto init work-on-71 --template .koto/templates/work-on.md --var ISSUE_NUMBER=71
```
Creates `koto-work-on-71.state.jsonl` in the current directory. Returns
`{"name": "work-on-71", "state": "entry"}`. Note: `--var` flag requires Phase 1
engine changes; until implemented, the workflow still functions via evidence fallback
for gates referencing `{{ISSUE_NUMBER}}`.

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
If found, it calls `koto next <name>` to resume at the current state, and re-reads any
existing wip/ artifacts and git log before acting on the directive. If not found, it
copies the template to `.koto/templates/work-on.md` (from the plugin directory) if it
doesn't exist, calls `koto init` to start fresh, then `koto next <name>` to enter `entry`.

The agent loops: read directive from `koto next`, do the work, call `koto next` (bare or
with `--with-data`) to advance. koto's evidence is epoch-scoped: each state transition
clears the current evidence, so only evidence submitted in the current state is accessible
for routing. `mode` is captured at `entry` for routing to the mode-specific first state
(`context_injection` or `task_validation`). The two setup states route unconditionally to
their respective post-setup states, so no mode re-submission is needed.

For complex states (`analysis`, `implementation`), the skill wrapper injects the full
phase procedure (from `references/phases/`) before the agent reads the directive.
Template directives carry concise action summaries and resume preambles — the phase files
carry full procedural detail.

wip/ artifact files created during the workflow:
- `wip/IMPLEMENTATION_CONTEXT.md` (issue-backed, created by extract-context.sh)
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
- `shirabe/koto-templates/work-on.md`: the 17-state template with all directives, gate
  commands, and evidence schemas as specified in Solution Architecture. Each state
  directive follows the two-part structure: action summary + resume preamble. Gate
  commands referencing `{{ISSUE_NUMBER}}` fall through to evidence fallback until Phase 1
  `--var` support is confirmed working.
- `koto template compile shirabe/koto-templates/work-on.md`: must pass with no errors.
  The compiler validates mutual exclusivity of transitions and rejects non-deterministic
  routing. Write YAML front-matter and markdown headings in lockstep (state name
  mismatches produce compile errors).
- Verify that all conditional self-loops use a `when` condition (unconditional self-loops
  trigger cycle detection in the engine); document this constraint in the template header.

### Phase 3: Shirabe skill integration

Update the /work-on skill to drive koto.

Deliverables:
- Updated /work-on skill instructions: on invocation, check `koto workflows` for a
  `work-on-*` workflow in cwd. If found, re-read existing wip/ artifacts and git log,
  then resume via `koto next`. If not found, copy the template to `.koto/templates/work-on.md`
  (from the plugin directory) if it doesn't exist, then call `koto init`.
- The skill accepts an optional issue number. When provided, initializes with
  `--var ISSUE_NUMBER=<N>` and submits `mode: issue_backed` at `entry`. When omitted,
  initializes without `--var` and submits `mode: free_form` with a task description.
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
`test -f`, `ls ... | grep -q`, `gh pr checks`, and `go test ./...`. No commands are
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
  The agent can't reach `ci_monitor` without a PR existing, or `analysis` without passing
  through `staleness_check` (issue-backed) or `post_research_validation` (free-form).
- Evidence fields are decision records. The event log shows not just that a phase
  completed, but what decision the agent made and why — useful for debugging and audit.
- Session resume is supported. Each state's directive includes a resume preamble
  instructing the agent to re-read relevant wip/ artifacts and git state before continuing.
  Calling `koto next <name>` after a session interruption returns the current directive.
- The two modes share 8 states from `analysis` onward. The split topology adds two setup
  states but eliminates the epoch-scoped mode re-submission of the previous design,
  which was the primary maintainability concern.
- Command gates auto-advance through mechanical checks without agent overhead. When tests
  pass and artifacts exist, `koto next` advances through `analysis`, `implementation`,
  and `finalization` without a `--with-data` call.
- Self-loop escalation paths give agents a structured exit from retry loops. The event
  log records escalation decisions, distinguishing "agent gave up after N attempts" from
  "agent encountered an unrelated blocker."
- Context injection is verified. The `context_injection` gate confirms
  `wip/IMPLEMENTATION_CONTEXT.md` was created before setup begins, ensuring design
  context reaches implementation regardless of session interruptions.

### Negative

- Two engine changes are required before gate-with-evidence-fallback activates. The
  template can be written and compiled, but gate failure routes to hard-stop until the
  advancement loop is patched.
- The `--var` flag must be implemented for gate commands to reference `{{ISSUE_NUMBER}}`.
  Until then, gates referencing the issue number fall through to evidence fallback
  unconditionally (degraded auto-advancement, but workflow remains functional).
- The staleness check always requires agent evidence. Command gates can only check exit
  codes, not script output content, so the routing decision (introspect or skip) must
  always be submitted by the agent.
- The introspection gate operates as evidence-only until `--var` ships. Agents
  self-report whether the introspection artifact exists rather than having koto verify it.
- Test commands in gates are language-specific (`go test ./...`). Non-Go projects need
  a different test command; a `TEST_COMMAND` template variable (defaulting to `go test ./...`)
  is the planned mitigation.
- The 17-state template is authoring-heavy with no tooling support. The compiler reports
  one error at a time, and state name mismatches between YAML front-matter and markdown
  headings produce compile errors. Authors should write states in lockstep.
- `koto rewind` rewinds one step per call. Recovering `done_blocked` to a non-adjacent
  originating state requires multiple calls. The directive must make this explicit.

### Mitigations

- The engine changes are targeted (two files, one new flag). They don't affect existing
  templates with gate-only states, which continue to hard-block on gate failure.
- Until `--var` is implemented, the introspection gate fails unconditionally and the
  state uses evidence-only path. The gate-with-evidence-fallback pattern handles this
  correctly — agents submit evidence directly. Once `--var` ships, the gate becomes
  active without template changes.
- The staleness check limitation is inherent to command gates; it's documented in the
  template directive. Future work could add an output-matching gate type
  (e.g., `type: command_output`) to close this gap.
- Add `TEST_COMMAND` as a template variable with a default of `go test ./...`, making
  it configurable without changing the template structure.
- `koto rewind` is CLI-callable (confirmed in source). The one-step-per-call behavior
  means recovery from `done_blocked` to a non-adjacent state requires N rewind calls,
  where N is the number of states traversed. The directive lists the specific rewind
  count for each path that reaches `done_blocked`.
