<!-- decision:start id="directive-structure-automation-first" status="assumed" -->
### Decision: Directive Structure Under the Automation-First, Gate-Only Model

**Context**

Decision 8 established the gate-only model for the shirabe work-on template: agents run scripts per their state directives; koto gates verify results; auto-advancement happens when gates pass. This creates two distinct populations of states in the template.

The first population never reaches an agent on the happy path. `implementation` auto-advances when tests pass. `ci_monitor` auto-advances when CI is green. `staleness_check` auto-advances via piped gate when staleness is not recommended. Agents only land on these states when something has gone wrong — tests are broken, CI is failing, the staleness script produced unexpected output.

The second population always requires agent judgment. `entry`, `task_validation`, `research`, `post_research_validation`, `introspection`, `analysis`, `finalization`, and `pr_creation` each require interpretation or creative work that no gate can substitute for. Agents read and execute their directives on every visit.

The previous Decision 6 answer — concise directives with resume-oriented preambles — was designed before the automation-first principle was established. It assumed agents read every state's directive. With gate-only, that assumption no longer holds, and the question is whether the directive structure should reflect this in the template schema.

The instruction audit found that the largest instruction reduction comes from eliminating the skill's orchestration wrapper (resume detection, state tracking, phase dispatch logic — ~55 lines in SKILL.md that reconstruct koto's state machine from scratch). The reduction from directive length is secondary. This context matters: the directive structure decision is about how to format per-state instructions, not about the primary source of instruction bloat.

**Assumptions**

- The three auto-advance states (`implementation`, `ci_monitor`, `staleness_check`) always reach agents via the failure path in exceptional cases. Empty directives on failure paths are not acceptable — agents need actionable guidance about what went wrong and how to recover.
- Template standalone-readability remains a constraint. A template author reading the file should understand what each state does without running the skill.
- The resume preamble (re-read artifacts, check git state before continuing) is only meaningful for states where prior work persists across sessions. Judgment states like `analysis` and `implementation` have prior artifacts to re-read; auto-advance states that agents only see on failure do not — the failure context is the entry point, not a resumable work session.
- Decision 8's gate-only model is final for this release. On_entry actions are a separate engine issue.
- The background note that "the largest reduction is NOT from directive length" is accurate: eliminating SKILL.md's orchestration wrapper is orthogonal to this decision and proceeds regardless of which directive structure is chosen.

**Chosen: (b) Two-Tier Directives**

Judgment states receive a full concise directive (10-25 lines covering what the agent should accomplish, key artifacts, evidence guidance) plus a resume preamble (3-6 lines directing the agent to re-read relevant wip artifacts and check git state before continuing work). These states are visited by agents on every workflow execution.

Auto-advance states receive a short error-fallback directive only (3-6 lines). The error-fallback directive follows the pattern: what went wrong, what to do, what evidence to submit. For `implementation`, this reads roughly: "Tests are failing. Read the failure output, identify the root cause, fix the issue, re-run tests. When tests pass, submit `tests_passed: true`. If the issue is unresolvable in this session, submit `blocked_reason: <description>`." For `ci_monitor`, the same pattern: CI is failing, diagnose by type (test/lint/build/flaky), fix or escalate. For `staleness_check`, the piped gate already handles the fresh path; the fallback directive tells the agent what to do when staleness is detected.

The two-tier convention is documented in the template header with a one-line explanation: judgment states have full directives; auto-advance states have error-fallback directives only. This gives template authors a clear authoring model.

**Rationale**

Option (b) directly encodes the automation-first principle in the template schema. A reader scanning the template can identify which states are judgment-required and which are auto-advance just from directive length and structure. This is information that belongs in the template, not only in design documentation.

The error-fallback directive for auto-advance states is better than a full directive for the failure case. When tests are broken in `implementation`, an agent doesn't need guidance on how to implement code — they need guidance on diagnosing and recovering from a broken state. A full concise directive mixing implementation guidance with failure recovery guidance would bury the actionable content.

The resume preamble in option (a)'s uniform format would be applied to auto-advance states unnecessarily. An agent landing on `implementation` because tests broke doesn't have a prior work session to resume — the failure condition is the entry point. The preamble adds lines without adding meaning.

Option (a) works but misses the chance to express the automation-first principle in the template format. The extra directive content for auto-advance states is harmless but represents a claim about what agents see that's only true on failure paths. The template should be honest about which states agents encounter routinely.

Option (c) is eliminated: empty directives on failure paths leave agents without actionable guidance. The failure path is exceptional but not rare — CI failures, test regressions, and staleness signals are expected workflow events. An agent receiving an empty directive on these states would have no structured recovery instructions.

Option (d) addresses SKILL.md orchestration bloat, not template directive structure. It's the correct approach for eliminating the ~55-line resume detection and dispatch logic. It's compatible with option (b) — the skill's orchestration wrapper is removed because koto tracks state, while the template uses two-tier directives because agents have different relationships to judgment states vs. auto-advance states. These are orthogonal choices.

**Alternatives Considered**

- **(a) Uniform concise directives**: every state gets the same 10-25 line directive and resume preamble regardless of whether agents visit it on the happy path. Consistent format, low authoring complexity. Rejected because it applies the resume preamble where it has no meaning (failure-path entry states), and because the uniform format doesn't signal which states require agent judgment — information that should be legible in the template.

- **(c) No directives for auto-advance states**: the smallest template, but agents on failure paths receive no actionable guidance. Eliminated. The failure path is an expected event, not an edge case; directives there are not optional.

- **(d) Uniform directives, eliminate orchestration wrapper**: addresses SKILL.md bloat, not directive structure. Compatible with and complementary to option (b). The real instruction reduction from koto state tracking comes from SKILL.md changes, not from directive formatting. This insight doesn't answer the question of how to structure directives — it clarifies that the stakes of this decision are lower than they might appear.

**Consequences**

Template authors follow a two-tier convention:
- Judgment states: full concise directive (what to accomplish, key artifacts, evidence schema) + resume preamble (what to re-read, what to check before continuing)
- Auto-advance states: error-fallback directive only (what went wrong, what to do, what evidence to submit)

The `implementation` directive shrinks from a would-be 15-20 line implementation guide to a 4-5 line failure-recovery directive. This is the correct abstraction: the skill's phase-4-implementation.md carries the implementation procedure; the template directive carries the failure recovery instructions that koto can't express as a gate transition.

The `ci_monitor` directive becomes a 4-5 line CI failure triage guide rather than a CI monitoring guide — because on the happy path, koto handles CI monitoring with no agent involvement.

The `staleness_check` directive becomes a 4-5 line staleness-detected response guide. On the fresh path, the piped gate auto-advances and the directive is never read.

Template standalone-readability is preserved: the error-fallback directives describe what the state does (implicitly — by describing the failure condition) even if the description is shorter than a full directive.

The two-tier structure makes visible in the template itself which states are automation targets for future on_entry hooks and which states are permanently judgment-required. This aids future template maintenance.
<!-- decision:end -->
