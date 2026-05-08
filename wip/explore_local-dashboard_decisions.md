# Exploration Decisions: local-dashboard

## Round 1

- **Scope boundary**: The dashboard is a monitoring tool, not a complete observability surface. Deep investigation (raw event log) remains the domain of `koto query`. The dashboard adds three information layers (quick-check, status review, history tab) but does not replace `koto query` for raw event access.
- **Layout**: The detail pane should be a horizontal split (list ~40% left, detail ~60% right), not the current vertical 8-row strip. The vertical strip was a placeholder; the horizontal split is the stated design intent confirmed by research.
- **Composable pipeline scope**: The PRD should cover what's achievable with the current koto model (parent_workflow tree visualization, variable surfacing from WorkflowInitialized, template name from header) and note the engine additions needed for the full shirabe pipeline (child-spawning, child-terminal gate). Implementation of the composable pipeline engine is out of scope for this PRD.
- **Template name in header**: Add `template_name: Option<String>` to `StateFileHeader` alongside `intent: Option<String>`. This enables meaningful session labeling in the dashboard list without reading the compiled template from disk on every refresh.
- **Crystallize to PRD**: The findings are sufficient and consistent. The shape is clear requirements work (what the dashboard should show, what the CLI should accept, what the data layer should expose). No architectural decisions are contested enough to need a design doc first.
