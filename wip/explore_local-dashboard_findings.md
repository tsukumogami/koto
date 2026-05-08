# Exploration Findings: local-dashboard

## Core Question

The current `koto dashboard` is a minimal TUI showing session names, current state names, and elapsed time. It surfaces almost none of the information a user needs to understand what their workflows are doing. What should a fully functional koto dashboard look like, covering the full spectrum from quick snapshot to deep investigation, including composable parent-child workflow relationships?

## Round 1

### Key Insights

**1. The elapsed column is always 0 (koto-event-log-data)**
`RowDescriptor.elapsed` is hardcoded to `Duration::from_secs(0)` in `visible_rows()`. Every session shows "0s" elapsed. This is a broken feature, not a missing one.

**2. Three fully-implemented derivation functions are dead code in the dashboard (koto-event-log-data)**
`derive_decisions()`, `derive_overrides()`, and `derive_visit_counts()` exist and are tested in `persistence.rs` but are never called from the dashboard layer. Wiring them in requires no new event types.

**3. The detail pane returns None for evidence-only sessions (koto-event-log-data, ratatui-detail-pane)**
`read_detail()` requires a `GateEvaluated` event to return data. Sessions that only submit evidence — the dominant usage pattern for shirabe-style workflows — show "No gate evaluations recorded." Evidence is collected inside `read_detail()` but the function exits before returning it if no gate exists.

**4. Significant narrative is already persisted but invisible (cli-augmentation, user-information-needs)**
`DecisionRecorded.decision.rationale`, `DirectedTransition.rationale`, `Rewound.rationale`, and `GateOverrideRecorded.rationale` are all stored in the event log but rendered nowhere in the dashboard. These contain the richest human-readable context in the entire log.

**5. The layout is a vertical strip, not the intended horizontal split (ratatui-detail-pane)**
The code uses `Constraint::Length(8)` for a fixed 8-row strip at the bottom. The stated design intent (40% list / 60% detail side-by-side) is unimplemented. The current detail area is too small for useful content.

**6. Tree infrastructure exists but is incomplete and misbound (composable-workflow-tui)**
`expanded`, `visible_rows()`, `RowDescriptor.indent_depth`, `TaskCounts`, and `sorted_children()` are all in `dashboard_state.rs`. The gaps are: tree-line rendering is spaces-only (no `├─`/`└─`), expand/collapse is bound to Detail mode rather than List mode, `visible_rows()` is hardcoded to 1 level deep, and cycle detection is absent (cycles produce sessions invisible to the dashboard).

**7. Three information layers map to three UI areas (user-information-needs, workflow-tool-patterns)**
Effective monitoring tools at 5–50 items expose exactly: (1) list view — name, status, one key metric; (2) detail view — current state, recent evidence, directive, gate; (3) history view — full event sequence. Current koto dashboard collapses (2) and (3) into a 7-row panel that serves neither.

**8. "What's remaining" is computable and high-value (user-information-needs, workflow-tool-patterns)**
The compiled template is already read for terminal detection on every refresh cycle. The state graph is parseable from `template_path` in `WorkflowInitialized`. "States remaining" and the current state's directive text are derivable with no new I/O.

**9. `intent` and `template_name` belong in the state file header (cli-augmentation, shirabe-pipeline)**
Session names like `task_session-feed-issue-1` are opaque in the dashboard list. Adding `intent: Option<String>` (set at `koto init` time with `--intent "..."`) and `template_name: Option<String>` (written from the compiled template's `name` field) to `StateFileHeader` follows the established additive-field pattern and requires no schema version bump.

**10. The shirabe pipeline's composable future requires engine additions not present today (shirabe-pipeline)**
Parent session spawning of children, a child-terminal gate type, and variable surfacing are not in the current koto model. Only `/work-on` is koto-backed today. The dashboard can support the tree view via existing `parent_workflow` pointers but cannot visualize the full pipeline until the engine adds these capabilities. The PRD should acknowledge this boundary.

### Tensions

**Monitoring tool vs. full observability surface**
The user information needs research points toward a complete event log viewer for deep investigation. The existing `koto query` CLI provides this today as raw JSON. A decision was made to scope the dashboard as a monitoring tool with a rich detail pane (Summary + History + Remaining tabs), not a replacement for `koto query`. Deep investigation falls back to the CLI.

**Immediate fixes vs. structural redesign**
Many gaps (elapsed, detail pane content, tree rendering, key bindings) are small fixes to existing infrastructure. But the layout change (vertical → horizontal split), the `DetailData → RichDetailData` replacement, and the header field additions are structural. Both are needed but they are not the same class of work.

**Short-term vs. long-term composable pipeline**
The shirabe pipeline composability vision is rich and well-mapped, but none of explore/prd/design/plan are koto-backed today. The dashboard work should be scoped to current capabilities (parent_workflow tree, variable surfacing) with a documented path for future engine work, not blocked on the engine additions.

### Gaps

- No investigation of filter/search UX (the `/` filter pattern from k9s) — deferred to implementation
- No investigation of `--once` scripting output enhancements (e.g., adding intent, template name columns)
- No investigation of session name generation conventions vs. intent field interaction

### Decisions
- Scope to monitoring tool, not full log viewer
- Horizontal split layout
- Composable pipeline support via existing `parent_workflow` (engine additions are future work)
- `intent` + `template_name` in StateFileHeader
- Crystallize to PRD

## Decision: Crystallize

The findings are sufficient across all seven research areas. The problem is well-understood, the requirements are shaped, and no architectural questions require a design doc. Routing to PRD.

## Accumulated Understanding

The koto dashboard has two classes of problems: **broken basics** (elapsed column always 0, detail pane empty for evidence-based sessions, tree hierarchy not rendered visually) and **missing features** (information hierarchy, narrative context, remaining states, horizontal split layout). Both are addressable with dashboard-side and data-layer work; no koto engine changes are required for the core improvements.

The data model is already rich. Fifteen event types capture transitions, evidence, gate evaluations, decisions, overrides, rewinds, context artifacts, and cancellations. The dashboard currently surfaces a small fraction of this. The primary work is: fix the data layer to surface all relevant events (remove the GateEvaluated guard, wire in existing derivation functions), replace the gate-centric `DetailData` with a universal `RichDetailData`, implement the horizontal split layout with a tabbed detail pane (Summary / History / Remaining), and add `intent` + `template_name` to the state file header for legible session identification.

For composable workflows: the tree infrastructure is 80% built in `dashboard_state.rs`. The render layer needs tree-line connectors and proper key bindings. The data layer needs variables surfaced from the `WorkflowInitialized` event so child sessions can display their issue numbers or task descriptions. Future engine work (child-spawning, child-terminal gate) would complete the pipeline visualization, but is out of scope for this PRD.

The result is a dashboard that: shows sessions as a navigable tree with status rollup, lets users drill into any session to see its current state + directive + evidence + gate status, provides a scrollable history view of the full event log within the dashboard, shows what states remain in the workflow, and labels sessions with human-readable intent rather than opaque machine names.
