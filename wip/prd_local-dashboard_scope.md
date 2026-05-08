# /prd Scope: local-dashboard

## Problem Statement

The `koto dashboard` command renders a minimal TUI that shows session names, current state names, a broken elapsed column (always 0), and a detail pane that is empty for evidence-based sessions — the dominant usage pattern. Users cannot see what a workflow is trying to accomplish, what evidence has been submitted, what rationale was given for decisions or transitions, what states remain, or how sessions relate to each other in parent-child trees. The gap between what koto's event log already records and what the dashboard surfaces is large, and the current layout (vertical 8-row strip, gate-only detail pane) cannot accommodate the information a user needs.

## Initial Scope

### In Scope

- Three-tier information hierarchy: quick-check (list view), status review (detail pane summary), and history (scrollable event timeline)
- Horizontal split layout: session list (~40% width) and detail pane (~60% width) side by side
- Tabbed detail pane: Summary tab (current state, directive, latest evidence, gate status), History tab (full event timeline, scrollable), Remaining tab (states not yet visited, derived from template)
- Surface existing but unused event data: transition history, `DecisionRecorded` events, `DirectedTransition.rationale`, `Rewound.rationale`, `GateOverrideRecorded.rationale`, `DefaultActionExecuted` output, `ContextAdded` artifacts
- Fix the broken elapsed column (currently always 0s)
- Tree visualization for parent-child sessions: collapsible with `├─`/`└─` connectors, status rollup (parent shows worst child status), expose expand/collapse in List mode (`l`/`h` keys)
- `intent: Option<String>` on `StateFileHeader` — set via `koto init --intent "..."` — for human-readable session labeling in the list view
- `template_name: Option<String>` on `StateFileHeader` — written from the compiled template's `name` field at init time — for session type display without disk reads on every refresh
- Optional `summary` field on `EvidenceSubmitted` events — agents pass it via `--with-data` — for one-sentence step narratives in the detail pane
- `--once` scripting mode enhancements: include `intent` and `template_name` in tab-separated output columns
- Progress indicator: "N states remaining" derived from the template graph traversal

### Out of Scope

- Full raw event log viewer (falls back to `koto query --events`)
- Engine changes for composable child-spawning or child-terminal gate type (future work)
- Web or browser-based visualization
- Real-time remote streaming across machines
- Session filtering / search (`/` filter) — deferred to a follow-on issue
- Rewind epoch navigation (archived `~`-named sessions) — deferred

## Research Leads

1. **What acceptance criteria should govern the Summary tab?**: The Summary tab needs to serve both gate-based and evidence-based sessions equally well. Research what fields are universally meaningful vs. session-type-specific, and how to handle sessions with no evidence, no gate, or both.

2. **What is the right behavior for the Remaining tab when the template file is unavailable?**: The Remaining tab reads the compiled template from disk. If the template cache was cleared or the session was archived from another machine, the template is missing. What should the tab show? Graceful fallback wording and behavior need to be specified.

3. **How should `intent` display in the list view when it's long?**: A session name column has fixed width. If `intent` replaces or supplements the session name, truncation behavior needs requirements (ellipsis, max character count, tooltip vs. scrolling).

4. **What are the exact `--once` output columns after adding intent and template_name?**: The current format is `session_id\tstate\telapsed\tstatus_bucket`. Adding `intent` and `template_name` changes the column count and affects all downstream consumers. The format change needs a clear specification with backwards-compatibility guidance.

5. **How should parent-child status rollup work for mixed terminal states?**: A parent session whose children have mixed outcomes (some `done`, one `done_blocked`, one still running) should show a composite status. The rollup rules need to be specified for all combinations.

## Coverage Notes

The exploration answered the "what is broken and what is missing" questions thoroughly. The PRD should focus on:
- Precise acceptance criteria for each improvement area (what counts as correct behavior?)
- Priority ordering (which improvements are MVP vs. nice-to-have?)
- Interaction model for the new key bindings (complete keyboard reference)
- Backwards-compatibility requirements for `StateFileHeader` and `EvidenceSubmitted` schema changes (what do older state files do when `intent` is absent?)
- Metric for success: how will we know the dashboard is "fully functional"?

Areas the exploration did NOT fully answer:
- Whether `intent` replaces the session name in the list or supplements it
- Exact `--once` column spec after additions
- The full keyboard reference (all modes, all keys)
- Priority ordering between the five improvement areas

## Decisions from Exploration

- **Scope to monitoring tool**: The dashboard targets quick-check and status-review. Deep investigation falls back to `koto query`. No full event log viewer in the dashboard.
- **Horizontal split layout**: Confirmed. The current vertical-strip layout is a placeholder.
- **Composable pipeline via existing `parent_workflow`**: Tree visualization uses existing `parent_workflow` pointers in session headers. Engine-level child-spawning and child-terminal gate types are out of scope for this PRD.
- **`intent` + `template_name` belong in StateFileHeader**: Additive optional fields following the established `#[serde(default, skip_serializing_if = "Option::is_none")]` pattern. No schema version bump required.
- **`summary` on `EvidenceSubmitted` is a convention, not a required field**: Agents opt in. The dashboard renders it if present, falls back to raw field display if absent.
