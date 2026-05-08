# Explore Scope: local-dashboard

## Visibility

Public

## Core Question

The current `koto dashboard` is a minimal TUI that shows session names, current state names, and elapsed time — it surfaces almost none of the information a user actually needs to understand what their workflows are doing. The question is: what should a fully functional koto dashboard look like, covering the full spectrum from quick snapshot (where am I?) to deep investigation (how did I get here, what's been accomplished, what's remaining), and including composable parent-child workflow relationships where the entire shirabe pipeline (explore→prd→design→plan→work-on) would eventually be a single navigable workflow tree?

## Context

- The existing dashboard renders a flat list (session name, state, elapsed, status bucket) and a detail pane that only shows gate evaluation data — empty for evidence-based sessions.
- koto's event log already records transitions, evidence submissions, gate evaluations, and directed transitions; the raw material exists.
- Sessions have `parent_workflow` headers that form a tree, but the list is rendered flat.
- Agents currently have no way to attach human-readable context to a workflow (intent summary, step narrative, rationale).
- The shirabe plugin runs complex multi-phase workflows but doesn't use koto yet; the user envisions the full pipeline being expressible as composable koto workflows.
- A "snapshot view" should convey current status + accumulated context at a glance; a "deep dive" should let users trace the full history.
- New CLI-level augmentation may be needed (e.g., `koto annotate` or an `intent` field on `koto init`) so agents can feed readable context into the state file.

## In Scope

- What information a user needs from a workflow at any lifecycle stage
- How to visualize parent-child (composable) workflow relationships in the TUI
- What the detail pane should show for evidence-based workflows (most shirabe-style work)
- What CLI augmentations would let agents attach narrative context (intent, step summaries)
- How the shirabe pipeline maps to composable koto workflows as a concrete example use case
- How to represent "what's remaining" (incomplete states, next expected steps)
- The event log data that already exists and how it should be rendered

## Out of Scope

- Re-architecting the koto state machine or event format
- Web/browser-based visualization
- Real-time remote streaming of sessions across machines
- Specific shirabe workflow implementation details (out of scope for this PRD)

## Research Leads

1. **What information does a user actually need from a running or completed workflow?**
   Map the user's mental model across three modes: quick-check (is it running/stuck/done?), status review (what state is it in, what evidence was submitted, any issues?), and deep investigation (full history, why did it transition, what context was provided). This shapes the information hierarchy for the UI.

2. **What data does koto's event log already expose that could enrich the dashboard?**
   Audit every `EventPayload` variant — `WorkflowInitialized`, `Transitioned`, `EvidenceSubmitted`, `GateEvaluated`, `DirectedTransition`, `Rewound`, etc. — for dashboard-useful fields. Identify what's currently unused in the render layer. The gap between "data exists" and "data is shown" is the low-hanging fruit.

3. **How do other pipeline and workflow monitoring tools (GitHub Actions, Temporal, Airflow, Prefect) present workflow state, history, and relationships?**
   Look for patterns that work well at the scale of 5–50 concurrent sessions: what information hierarchies they use, how they handle nested/parent-child pipelines, how they show progress through a DAG vs. a linear state machine.

4. **How should composable parent-child workflow relationships be visualized in a terminal UI?**
   The current flat list loses all hierarchy. Investigate TUI patterns for tree-shaped data (collapsible trees, indented lists, split views). Consider how a root workflow's status should summarize its children and how navigation should work when a workflow spawns sub-workflows.

5. **What CLI augmentations would let agents attach narrative context to workflows?**
   Agents know *why* they are doing something — the intent, the rationale, the summary of what each step accomplished. Explore what new CLI surface (`koto init --intent`, `koto annotate`, a context-setting event type, or metadata in the state file header) would let agents persist this in a way the dashboard can display. Look at how other tools (Temporal, Linear, GitHub Actions) attach human-readable summaries to automated work.

6. **What does the full shirabe explore→prd→design→plan→work-on pipeline look like structurally, and what would a user need to see if it were one composable koto workflow?**
   Read the shirabe skill files to map the phases, their inputs/outputs, and inter-phase dependencies. Model it as a koto workflow tree: what would the parent and child sessions be, what evidence would be submitted at each node, what would "stuck" look like vs. "progressing"? This is the concrete target use case that should drive the dashboard design.

7. **What are the right TUI design patterns for a richer detail pane that serves both gate-based and evidence-based sessions?**
   The current detail pane is gate-specific. Investigate ratatui capabilities and established TUI patterns for structured information display: scrollable text, key-value tables, timeline views, tabbed panels. The detail pane needs to work for a session with 2 events and one with 200.
