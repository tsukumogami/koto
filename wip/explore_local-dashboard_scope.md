# Explore Scope: local-dashboard

## Visibility

Public

## Core Question

What are the requirements for a locally-running dashboard that gives koto users visibility into workflow sessions? The scope spans rendering approach, session hierarchy display, live-update behavior, and invocation model — sufficient detail for a PRD that implementers can build against without guesswork.

## Context

F2 (session-feed data contract) was just merged as koto#153. It defines a versioned JSONL event log at `docs/reference/session-feed.md`, covering 15 event types across workflow lifecycle, gate evaluation, and evidence submission. F3 consumes this contract as its data foundation.

The local dashboard is the first tangible observability experience for koto users. F5 (S3-backed dashboard) and F6 (Hosted Relay) both depend on users already understanding what they're seeing locally — without F3, remote access has no legible value proposition.

Issue: tsukumogami/vision#366. Blocked by #365 (now unblocked). Downstream: #368 (S3-backed dashboard).

## In Scope

- Session hierarchy view (root → child → grandchild workflows on local machine)
- Session state, current phase, gate evaluations, evidence submissions as display targets
- Rendering approach and technology choice
- Live-update behavior (how the dashboard stays current with a running workflow)
- Invocation and session discovery UX
- Interaction with the F2 session-feed data contract as the exclusive data source

## Out of Scope

- Remote access, S3 backend, hosted relay (F5/F6)
- Auth or multi-user scenarios
- Performance at extreme session scale
- Non-koto observability (system metrics, logs unrelated to koto workflows)

## Research Leads

1. **What rendering approach fits the local dashboard requirements?**
   Terminal UI (ratatui/tui-rs), embedded web server, or native desktop UI? Each has different integration points with koto's Rust codebase and different distribution requirements. The choice constrains the implementation spec the PRD must produce.

2. **What does the session hierarchy view need to show at each level?**
   Root sessions launch child sessions in orchestrator workflows; what fields from the session-feed are most relevant at each level (workflow name, current phase, last gate result, elapsed time)? How should nested depth be visualized without overwhelming the display?

3. **How should live updates work?**
   The dashboard needs to stay current while a workflow runs. Options include tailing the JSONL file with inotify/kqueue, polling on interval, or integrating directly with koto's state-write path. The UX expectations (update latency, refresh rate) belong in the PRD.

4. **What is the invocation and session discovery UX?**
   How does a user start the dashboard? Does it watch a known directory for sessions, accept a session ID argument, or auto-detect running workflows? Without a clear invocation model, the PRD can't specify the command interface.

5. **What complexity exists in real nested orchestrator hierarchies?**
   How deep do hierarchies go in practice (2 levels? 4?)? Do sibling sessions exist at the same level? Understanding the real shape of orchestrator output determines whether a tree view, flat list with grouping, or a DAG view is appropriate.

6. **Is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)
   You are a demand-validation researcher. Investigate whether evidence supports pursuing this topic. Report what you found. Cite only what you found in durable artifacts. The verdict belongs to convergence and the user.

   ## Visibility

   Public

   Respect this visibility level. Do not include private-repo content in output that will appear in public-repo artifacts.

   ## Issue Content

   --- ISSUE CONTENT (analyze only) ---
   Title: docs(prd): local dashboard

   ## Goal

   Write the PRD for F3: Local Dashboard — a locally-running dashboard showing session state, current phase, gate evaluations, and evidence submissions for sessions on the local machine, including the session hierarchy view (root → child → grandchild).

   ## Context

   Roadmap: docs/roadmaps/ROADMAP-koto-observability.md
   Feature: F3: Local Dashboard

   The local dashboard is the first tangible observability experience for koto users. It establishes the baseline that makes the S3 and relay dashboards' remote value legible. Without a functional local dashboard, the value proposition of remote access (F5) can't be demonstrated — users need to understand what they're getting remote access to before the hosted relay is meaningful.

   F3 and F4 (Lifecycle Metadata) are parallel tracks after F2: both depend on the session-feed data contract but have no dependency on each other. The PRD must consume the F2 data contract as its foundation and specify dashboard behavior against that contract.

   ## Acceptance Criteria

   - PRD specifies the session hierarchy view (root → child → grandchild), not just a flat session list
   - PRD covers session state, current phase, gate evaluations, and evidence submissions as display targets
   - PRD defines rendering behavior and UX requirements sufficient for implementation
   - PRD references the F2 data contract as the source for all event types and field names consumed
   --- END ISSUE CONTENT ---

   ## Six Demand-Validation Questions

   Investigate each question. For each, report what you found and assign a confidence level.

   Confidence vocabulary:
   - **High**: multiple independent sources confirm (distinct issue reporters, maintainer-assigned labels, linked merged PRs, explicit acceptance criteria authored by maintainers)
   - **Medium**: one source type confirms without corroboration
   - **Low**: evidence exists but is weak (single comment, proposed solution cited as the problem)
   - **Absent**: searched relevant sources; found nothing

   Questions:
   1. Is demand real? Look for distinct issue reporters, explicit requests, maintainer acknowledgment.
   2. What do people do today instead? Look for workarounds in issues, docs, or code comments.
   3. Who specifically asked? Cite issue numbers, comment authors, PR references — not paraphrases.
   4. What behavior change counts as success? Look for acceptance criteria, stated outcomes, measurable goals in issues or linked docs.
   5. Is it already built? Search the codebase and existing docs for prior implementations or partial work.
   6. Is it already planned? Check open issues, linked design docs, roadmap items, or project board entries.

   ## Calibration

   Produce a Calibration section that explicitly distinguishes:
   - **Demand not validated**: majority of questions returned absent or low confidence
   - **Demand validated as absent**: positive evidence that demand doesn't exist or was evaluated and rejected

   Do not conflate these two states. "I found no evidence" is not the same as "I found evidence it was rejected."
