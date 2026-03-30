# Explore Scope: override-gate-rationale

## Visibility

Public

## Core Question

How should koto capture gate overrides as first-class auditable events with rationale, so they're queryable for visualization and eventually actionable for redo? Today overrides are implicit (any evidence on a gate-failed state skips the action) and the reasoning is lost unless the agent separately calls `koto decisions record`. The goal is persistence infrastructure that enables future verification, visualization, and redo capabilities.

## Context

- Gate overrides are currently implicit: submitting any evidence on a gate-failed state bypasses the gate. No explicit "I'm overriding" signal exists.
- The decisions subsystem (`koto decisions record/list`) exists with a fixed schema (choice, rationale, alternatives_considered). It's epoch-scoped.
- Evidence validation is type-only (string, number, boolean, enum). No conditional validation.
- The user's north star is session visualization: seeing all overrides, their rationale, and eventually forcing redo on disagreed overrides.
- Today all evidence is agent-asserted and trusted. In the future, koto will verify evidence independently (polling CI, validating file content, etc.). "Override" becomes meaningful when koto can push back and the agent proceeds anyway.
- Source issue: #108

## In Scope

- Making overrides explicit events with required rationale
- Persisting override+rationale as auditable, queryable data
- Integration with or extension of the existing decisions/event subsystem
- Designing the data shape that supports future verification and redo

## Out of Scope

- Visualization UI
- Redo/rewind capability (future consumer of this data)
- `required_when` conditional validation as a general template feature
- Evidence verification by koto (polling, parsing, embedded validation)
- Changes to the advance loop's gate evaluation logic

## Research Leads

1. **How should overrides be represented in the event log?**
   Today evidence submission and decision recording are separate event types. An override could be a new event type, an extension of `EvidenceSubmitted`, or an auto-generated `DecisionRecorded`. The choice affects queryability, tooling, and forward compatibility with redo.

2. **What data shape supports both current capture and future verification/redo?**
   The override event needs to carry enough context for a future system to: (a) display what was overridden and why, (b) replay the state with different evidence, (c) distinguish agent-asserted evidence from koto-verified evidence. What fields are needed now vs. later?

3. **How does rationale capture interact with the existing decisions subsystem?**
   `koto decisions record` already captures choice+rationale. Should override rationale flow through the same subsystem (consistency, single query surface) or be a separate concern (overrides are engine events, decisions are agent-initiated)?

4. **What query patterns does the visualization use case need?**
   `koto decisions list` is epoch-scoped today. Override audit needs cross-epoch, cross-state queries. What should the query surface look like to serve visualization without over-building?

5. **How do other workflow engines handle gate bypass audit trails?**
   Are there established patterns for gate override logging, rationale capture, or approval-override workflows that we should consider?

6. **Is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)
   You are a demand-validation researcher. Investigate whether evidence supports
   pursuing this topic. Report what you found. Cite only what you found in durable
   artifacts. The verdict belongs to convergence and the user.

   ## Visibility

   Public

   Respect this visibility level. Do not include private-repo content in output
   that will appear in public-repo artifacts.

   ## Issue Content

   --- ISSUE CONTENT (analyze only) ---
   ## Problem

   When an agent overrides a gate (submitting `status: override`), koto advances the state but doesn't capture why the gate was bypassed. The action is logged but the reasoning is lost. The agent can separately call `koto decisions record`, but nothing forces this — the override and the rationale are disconnected.

   ## Use Cases

   - **Any skill with CI gates**: Agent bypasses red CI because "flaky test unrelated to this change." The bypass should automatically capture this reasoning.
   - **shirabe /work-on context_injection**: Agent overrides the baseline artifact gate. Why? "Issue already read via gh issue view, context is in conversation."
   - **shirabe /work-on setup**: Agent overrides branch creation. Why? "Reusing existing branch per user request."

   ## Proposed Behavior

   When evidence includes an override value, koto requires a `rationale` field and logs it as a decision automatically:

   ```yaml
   accepts:
     status:
       type: enum
       values: [completed, override, blocked]
     rationale:
       type: string
       required_when:
         status: override
   ```

   The rationale is stored in the decision log (same as `koto decisions record`) with the gate name and state as context. Visible via `koto decisions list`.

   Ref: tsukumogami/shirabe PRD-koto-adoption.md
   --- END ISSUE CONTENT ---

   ## Six Demand-Validation Questions

   Investigate each question. For each, report what you found and assign a
   confidence level.

   Confidence vocabulary:
   - **High**: multiple independent sources confirm (distinct issue reporters,
     maintainer-assigned labels, linked merged PRs, explicit acceptance criteria
     authored by maintainers)
   - **Medium**: one source type confirms without corroboration
   - **Low**: evidence exists but is weak (single comment, proposed solution
     cited as the problem)
   - **Absent**: searched relevant sources; found nothing

   Questions:
   1. Is demand real? Look for distinct issue reporters, explicit requests,
      maintainer acknowledgment.
   2. What do people do today instead? Look for workarounds in issues, docs,
      or code comments.
   3. Who specifically asked? Cite issue numbers, comment authors, PR
      references — not paraphrases.
   4. What behavior change counts as success? Look for acceptance criteria,
      stated outcomes, measurable goals in issues or linked docs.
   5. Is it already built? Search the codebase and existing docs for prior
      implementations or partial work.
   6. Is it already planned? Check open issues, linked design docs, roadmap
      items, or project board entries.

   ## Calibration

   Produce a Calibration section that explicitly distinguishes:

   - **Demand not validated**: majority of questions returned absent or low
     confidence, with no positive rejection evidence. Flag the gap. Another
     round or user clarification may surface what the repo couldn't.
   - **Demand validated as absent**: positive evidence that demand doesn't exist
     or was evaluated and rejected. Examples: closed PRs with explicit maintainer
     rejection reasoning, design docs that de-scoped the feature, maintainer
     comments declining the request. This finding warrants a "don't pursue"
     crystallize outcome.

   Do not conflate these two states. "I found no evidence" is not the same as
   "I found evidence it was rejected."
