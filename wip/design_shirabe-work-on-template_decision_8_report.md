<!-- decision:start id="shirabe-work-on-template-on-entry-actions" status="assumed" -->
### Decision: on_entry Actions in the shirabe work-on Template

**Context**

The shirabe work-on skill has 995 lines across 10 files. Approximately 42% (~420 lines) are eliminable if koto handles deterministic steps. The largest targets are resume detection and dispatch logic (~55 lines that reconstruct koto's state machine from scratch), branch creation and test sequences (~35 lines), and artifact cleanup (~35 lines). The question is whether the template design should include on_entry actions — commands executed automatically by koto on state entry with stdout capture and evidence injection — and if so, which states should use them.

Code inspection confirms that koto has no existing on_entry mechanism. Gates are the only execution path today. Adding full on_entry actions requires changes to TemplateState schema, a new action executor, stdout capture, output-to-evidence mapping, integration into the advance_until_stop() loop, and variable substitution (a prerequisite). Variable substitution is currently stored in the event log but not rendered at runtime — all code paths write an empty HashMap for variables. The combined scope is 2-3 engineer-weeks.

A critical finding during research: `staleness_check` routing can be handled with an existing piped gate command (`check-staleness.sh | jq -e '.introspection_recommended == false'`). This was identified as a design oversight. The gate determines which path to take; the agent reads the full script output in the destination state when it needs to act on staleness concerns. This is not a capability gap — it is an unexploited capability.

**Assumptions**

- Variable substitution is not implemented at runtime (confirmed by code inspection — `HashMap::new()` in all WorkflowInitialized event payloads in the codebase).
- The 1-2 week estimate for on_entry engine changes does not include variable substitution; the combined scope is 2-3 weeks.
- The shirabe work-on template is scoped for the current release cycle; blocking on unimplemented engine features is not acceptable for this release.
- The primary value of migrating to a koto template is state machine externalization (eliminating resume reconstruction logic), which requires zero engine changes.
- Template revision to add on_entry fields is mechanical and straightforward when the engine capability ships.

**Chosen: Gate-Only Model (option B)**

The work-on template uses koto's existing gate capabilities exclusively. Agents run scripts per their state directives; gates verify outcomes; evidence routing drives conditional transitions. No new engine capability is required. Specific design:

- `staleness_check` state: a piped gate command (`check-staleness.sh | jq -e '.introspection_recommended == false'`) determines routing. If the gate passes (staleness not recommended), the workflow auto-advances. If the gate fails (staleness is recommended), the workflow routes to an `introspection_recommended` state where the agent reads the full script output and acts on it.
- `context_injection` state: agent runs `extract-context.sh`, reads the output, proceeds. Gate verifies the output file exists.
- `setup_issue_backed` / `setup_free_form` states: agent creates branch and runs baseline script per directive. Gate verifies branch existence.
- `pr_creation` state: agent runs `gh pr create` and reports the PR URL. Gate verifies PR existence for resume.
- `implementation` and `ci_monitor` states: already auto-advance with existing gate capabilities (confirmed in prior research).

On_entry actions are scoped as a separate koto engine issue, tracking the five states and their intended automation commands, to be filed alongside this template design.

**Rationale**

The automation-first principle — "everything koto CAN do deterministically, it SHOULD do" — applies to koto's current capabilities. A template that maximizes automation within today's engine is more valuable than a template blocked on a 2-3 week engine sprint. The gate-only model captures the largest single reduction (the ~55-line resume reconstruction block) immediately, and the staleness_check oversight is corrected by using the piped gate.

Four validator positions were developed and cross-examined. All four converged: option D (deferred schema) is fully dominated by B; option C (exit-code-only) is dominated by B given that staleness routing works with gates today; option A (full on_entry) is the correct long-term design but not appropriate as a blocking prerequisite for this template release. Validator A's position explicitly converged to B during cross-examination: "the operative choice is does this template design require on_entry? The answer should be no."

The residual limitation of gate-only — agents manually run context_injection and PR creation scripts — is real but acceptable. These are single-step directive instructions, not multi-step reconstructions of system state.

**Alternatives Considered**

- **Full on_entry actions (option A)**: koto autonomously runs five states with stdout capture and evidence injection. Maximum reduction (~420 lines). Rejected for this release because it requires 2-3 weeks of unimplemented engine work (on_entry executor, stdout capture, output-to-evidence mapping, variable substitution) before the template can ship. Endorsed as the correct long-term direction via a separate engine issue.

- **Minimal on_entry exit-code-only (option C)**: fire-and-forget command execution, no stdout capture. Captures branch creation automation but not staleness routing or context injection. Rejected because staleness routing is achievable with a piped gate command today, removing the key motivation for this middle option. C is also a partial implementation that creates pressure for a second engine sprint without delivering the high-value targets.

- **Deferred schema (option D)**: template includes on_entry fields as no-ops with engine treating them as future-intent markers. Rejected because directives must serve agents running today: a directive that implies "koto runs X" when koto doesn't run X creates confusion and incorrect agent behavior. D is dominated by B (cleaner) or A (more correct).

**Consequences**

- The template ships without a dependency on engine work. Agents using the shirabe work-on skill see immediate reduction in skill complexity (resume detection/dispatch logic eliminated, staleness check simplified to gate routing).
- The template design must explicitly use the piped gate for staleness_check (not agent-driven staleness evidence). This is a design requirement, not optional.
- A koto engine issue for on_entry actions should be filed, specifying the five target states and their intended commands. This prevents template ossification by creating a tracked path for future upgrade.
- When on_entry ships, the template revision is mechanical: move script calls from agent directives into on_entry fields, remove the corresponding directive steps.
- Context_injection and pr_creation remain agent-driven. This is a known and accepted limitation.
<!-- decision:end -->
