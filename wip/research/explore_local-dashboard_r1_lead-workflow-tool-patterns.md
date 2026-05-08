# Lead: Workflow Tool Patterns for Pipeline State Visualization

## Findings

### GitHub Actions

**Information hierarchy:**
GitHub Actions uses a three-level hierarchy: workflow run → jobs → steps. Each level is independently expandable and has its own status. The run-level view shows duration, trigger (push/PR/manual), and a job grid where each job is a named box colored by status (green/red/yellow/grey). Clicking a job reveals a collapsible step list. Steps show name, duration, and a real-time log tail when expanded.

**What works:**
- The job grid gives a fast spatial summary. Failed jobs are immediately visible without reading text.
- Step logs use a persistent expand/collapse state — you can open two steps at once and scroll freely without losing context.
- Duration is shown at every level (run total, per job, per step). This lets users instantly identify the bottleneck.
- Status uses both color and an icon (checkmark, X, spinner, circle) so the display degrades gracefully on monochrome terminals.
- A "re-run failed jobs" affordance is always visible on completed runs — the history view and the action panel share screen space.

**What's missing at the step level:**
- No "what's remaining" count — if a job has 12 steps and is on step 4, you don't see "8 remaining" without scrolling.
- Logs aren't searchable in-browser without extensions.
- Parent-child relationships (called workflows, reusable workflow calls) are rendered awkwardly: the called workflow appears as a single step in the parent job, and you have to navigate away to see its internal state.

**Information primacy:** Status > Name > Duration. Everything else is secondary.

---

### Temporal Workflow Engine (UI and CLI)

Temporal is the closest analog to koto: it's a state-machine-like workflow engine where workflows have typed events, activities can fail and retry, and the full history is the source of truth.

**Workflow detail view:**
The Temporal Web UI shows a workflow's history as a chronological event list. Each event has a type (WorkflowExecutionStarted, ActivityTaskScheduled, ActivityTaskCompleted, TimerStarted, etc.), a timestamp, and an expandable payload. The current state is inferred from the last event type.

**What works:**
- Event history as the canonical source of truth is powerful for debugging. You can reconstruct exactly why the workflow is in its current state by reading the event list top to bottom.
- The "pending activities" panel shows what's currently running or waiting — this is the "what's remaining right now" view, separate from the full history.
- Child workflows appear in the history as `StartChildWorkflowExecution` events, and the UI deep-links directly to the child workflow's detail page. Parent-child navigation is first-class.
- The "stack trace" view (for blocked workflows) shows the exact line of workflow code that is waiting. This is the "why is it stuck" answer without reading logs.

**What doesn't work well:**
- The event list becomes unreadable for long-running workflows with thousands of events. Temporal introduced "continue as new" specifically to truncate history, which means the dashboard can lose older history entirely.
- The DAG/visual graph view Temporal added shows activity dependencies but is hard to read for dynamic workflows where branches depend on runtime data.
- Status is stored as an enum (Running, Completed, Failed, Terminated, Continued-As-New, TimedOut) — coarse-grained enough to fit on a list but too coarse for quick diagnosis.

**Information primacy:** Current status → last event type → pending activities → full event history.

---

### Apache Airflow

Airflow manages DAGs (directed acyclic graphs) of tasks. Its UI is oriented around the DAG run, not the individual task.

**DAG run view:**
The primary view is a grid: columns are DAG run dates, rows are tasks. Each cell is colored by task status. This gives a "runs over time × tasks" matrix, which is very effective for spotting patterns (task X always fails, run every Tuesday takes longer).

**Tree view:**
Shows one DAG run as a tree with parent-child task relationships. Each node shows status and can be expanded to show logs. The tree makes upstream/downstream dependencies visible.

**Graph view:**
Shows the DAG structure with edges. Nodes are colored by status for the selected run. This is the "what's remaining" view — you can see which tasks are still pending downstream of the currently-running task.

**What works:**
- Separating "structure" (graph) from "run history" (grid) is a useful dual-view pattern.
- Task-level filtering (show only failed, show only running) dramatically reduces noise at scale.
- The "clear task" action (re-run from a specific point) is surfaced inline on the task detail — the monitoring view and the control surface are integrated.

**What doesn't work:**
- Airflow's UI is web-only and assumes a browser. TUI tooling doesn't exist.
- The grid view breaks down with more than ~15 tasks — columns become too narrow.
- Dynamic tasks (TaskFlow API, mapped tasks) don't render well in the static graph view because the graph structure is only known at runtime.

---

### Prefect

Prefect is closer to Airflow but has a stronger emphasis on the run detail view rather than the DAG structure.

**Flow run view:**
Shows a timeline (Gantt-style) of task runs within a flow run. Each task is a horizontal bar; width represents duration; color represents status. The timeline immediately shows parallelism (tasks whose bars overlap ran concurrently) and sequential bottlenecks (a narrow bar followed by a long one).

**What works:**
- The Gantt view is the best "at a glance" summary for complex flows: you see duration, parallelism, and where time was spent.
- Subflow runs (nested flows) appear as expandable rows in the Gantt, preserving hierarchy while keeping the top-level view clean.
- The artifact panel shows custom outputs agents or tasks emitted — this is Prefect's equivalent of koto evidence.

**What doesn't work:**
- Gantt charts require pixel-level precision. They don't translate to terminal UIs.
- No keyboard navigation; entirely mouse-driven.

---

### TUI tools: k9s (Kubernetes)

k9s is a terminal UI for Kubernetes. It manages 5–500+ concurrent resources (pods, deployments, services) with keyboard-only navigation.

**Information hierarchy:**
The primary view is a filterable, sortable table of resources. Each row: name, status, ready count, restarts, age. Status uses short codes and color (Running=green, Pending=yellow, Error=red, Terminating=grey). The table fits ~30 rows on a standard terminal.

**Navigation patterns:**
- `/` to filter by name — the list narrows in real time. This is the primary way to reduce noise from 500 to 5 items.
- `Enter` to drill into a resource (e.g., pod detail showing containers, events, resource usage).
- `l` to tail logs directly from the resource row — no navigation required.
- `d` to describe (YAML dump of the full resource spec).
- Breadcrumbs at the top show current context: cluster > namespace > resource type.
- `Escape` goes up one level. Navigation is a stack, not a tree.

**What works:**
- The table model with live updates (resources change color in place as status changes) works at 5–50 items without overwhelming the user.
- Keyboard shortcuts are shown in a footer bar — discoverable without documentation.
- The filter persists across refreshes. If you're watching a specific pod, filtering by name keeps it in view even as other pods scroll past.
- Color as a primary signal means a user can assess "is anything broken" in under one second by scanning for red rows.

**What doesn't work:**
- k9s has no parent-child visualization for hierarchical resources (e.g., Deployment → ReplicaSet → Pod is three separate screens). You navigate between them with different commands, not by expanding a tree.
- Nested relationships are implied by naming conventions, not rendered visually.

---

### TUI tools: lazygit

lazygit is a Git TUI. Its primary panel shows branches/commits/files in a three-column layout.

**Information design:**
- Three persistent panes: left (branches), middle (commits for selected branch), right (diff for selected commit).
- Status lines use compact notation: `✔ 3` (staged), `✗ 2` (unstaged), `?1` (untracked).
- Color distinguishes tracked/untracked/staged/conflicted without needing to read labels.
- The "commit graph" (merge history) is rendered with ASCII box-drawing characters (`─ │ ╭ ╰`) — effective tree visualization in a terminal.

**What works:**
- The three-pane layout shows primary list, secondary detail, and tertiary content simultaneously. Navigation between panes is `Tab`/arrow keys.
- The ASCII commit graph is the best example of tree visualization in a terminal: it degrades gracefully as branches merge, and it uses no color for the graph lines themselves (relying on text for status).
- Context changes as you select different items — the right pane always reflects what's under the cursor, making exploration feel like browsing.

**What doesn't work:**
- Three-pane layouts require wide terminals. At < 100 columns, panes become too narrow to be useful.
- Lazygit doesn't handle more than ~500 commits before scrolling becomes impractical.

---

### htop

htop shows process state (running, sleeping, stopped, zombie) for 5–500 processes simultaneously.

**What works:**
- Header section shows aggregate metrics (CPU, memory, load) — the "system health at a glance" before drilling into individual processes.
- Color-coded bars (green=user, red=kernel, blue=memory) use visual encoding that doesn't require reading.
- Sorting by any column (CPU, memory, PID, name) lets the user put the most relevant items at the top of the list.
- `F5` shows a tree view of processes — parent-child relationships rendered as an indented, collapsible tree. This is the clearest terminal example of hierarchical state.

**htop's tree view pattern:**
Parent process at column 0, children indented 2 spaces, grandchildren 4 spaces. If a parent is collapsed, children disappear from the list. The expand/collapse toggle is `space` on the selected row. This works at 5–50 items; it becomes unwieldy above 100.

---

### Common Patterns Across Effective Tools

**Pattern 1: Status before content.**
Every effective tool uses status (color + icon) as the first signal, before the user reads any text. The visual scan of a list should answer "is anything broken?" in under 500ms. Text provides the "what" and "why" after the user has identified the problem.

**Pattern 2: Three information levels.**
Effective tools expose exactly three levels of detail:
1. List view — name, status, one key metric (duration, restarts, state name).
2. Detail view — current state, recent events, pending items, key metadata.
3. History/log view — full event sequence, raw output, timestamps.

Navigation between levels uses consistent keys (Enter to go deeper, Escape to go up).

**Pattern 3: "What's running now" vs. "what happened" are separate panels.**
Temporal separates pending activities from event history. Airflow separates the graph (structure) from the grid (history). These are distinct mental models and should be distinct UI areas. Mixing them creates confusion.

**Pattern 4: Filters are essential at 5–50 items.**
At 10 items, a filter feels unnecessary. At 30 items, it's essential. Every effective tool at this scale has a live filter (k9s `/`, GitHub Actions job search). The filter should narrow the list without navigating away.

**Pattern 5: Hierarchical data in terminals uses indentation, not graphics.**
Collapsible indented trees (htop F5, lazygit commit graph, Temporal child workflow links) are the standard TUI pattern. ASCII box-drawing characters (`├─`, `└─`) are universally understood. Parent-child state should summarize children: a parent is "running" if any child is running, "failed" if any child failed.

**Pattern 6: Duration at every level.**
htop shows per-process CPU time. GitHub Actions shows job and step duration. Temporal shows activity duration. Users consistently want to know "how long has this been running" and "how long did that take" at every level of the hierarchy.

**Pattern 7: Keyboard shortcuts in a persistent footer.**
k9s puts a shortcut bar at the bottom. lazygit shows context-sensitive shortcuts for the active pane. htop uses an F-key footer. The pattern is universal: shortcuts are always visible, context-sensitive, and take up 1–2 rows at the bottom of the terminal.

---

### Anti-Patterns

**Anti-pattern 1: Flat lists for hierarchical data.**
Rendering parent-child workflows as a flat list (current koto dashboard) loses all structural information. Users can't tell which sessions are related, which parent is stalled waiting for a child, or what the aggregate status of a workflow tree is.

**Anti-pattern 2: Log tailing as the only detail view.**
Raw logs are the last resort, not the first. Tools that only show logs (no structured event history, no state summary) force the user to parse text to understand state. Structured events with human-readable summaries are far more useful than log lines.

**Anti-pattern 3: Visual progress bars for state machines.**
Progress bars imply linear progress toward 100%. State machines don't have a fixed endpoint or linear progression. Airflow's grid works because tasks are fixed at design time. For dynamic workflows (like koto's evidence-driven state machines), showing "current state" and "next expected states" is more accurate than a progress bar.

**Anti-pattern 4: Updating the full screen on every tick.**
Tools that redraw the entire display on every data update (every second) cause visual noise — the user's eyes reset constantly. k9s and lazygit update individual rows in place; the display is stable until something actually changes. For a TUI, stable display is more important than real-time accuracy.

**Anti-pattern 5: No "what's remaining" signal.**
GitHub Actions fails here: you can see what's done and what's running, but not how many steps are left. Temporal shows pending activities. Airflow's graph shows downstream nodes. Users ask "how much longer?" more than "what ran?". The remaining-work signal is more actionable than the completed-work signal.

**Anti-pattern 6: Collapsing parent status hides child failures.**
If a parent workflow shows "running" while a child has already failed, the user misses the failure until they happen to drill into the child. Parent status should immediately reflect child failure — a parent is in the worst state of any child.

---

## Implications

**For koto's TUI dashboard:**

1. **Replace the flat list with a collapsible tree.** Sessions with a `parent_workflow` should be indented under their parent. A parent row should aggregate child status (worst-case wins). `Space` or `Enter` expands/collapses the subtree. This directly addresses the scope's "composable parent-child" requirement.

2. **Use a three-level navigation model.** Session list (level 1) → session detail (level 2) → event history (level 3). `Enter` drills down, `Escape` goes up. The detail pane (level 2) should show: current state, last transition reason, evidence submitted so far, pending gates. The history pane (level 3) should show the full event log.

3. **Status encoding: color + short code, not text.** Each row should show a 1–2 character status code (e.g., `RUN`, `DONE`, `FAIL`, `WAIT`, `STUK`) with a color. Text status labels like "running" waste horizontal space and are slower to scan than a colored code.

4. **"What's remaining" field in the detail pane.** Koto templates define states; the engine knows which states haven't been visited. The detail pane should show a list of unvisited states downstream of the current state. For linear state machines this is straightforward; for branching machines, show the set of possible next states.

5. **Persistent live filter.** A `/` filter that narrows the session list by name, state, or status is essential for 10+ sessions. The filter should apply at the tree level (filtering a parent hides it and its children if none match, or shows only matching children).

6. **Duration at every row.** Each session row should show elapsed time. Each event in the history view should show a timestamp and delta from the previous event. Duration is always useful; it's the most common "is this stuck?" signal.

7. **Separate "current state" from "event history" in the detail pane.** The top of the detail pane shows the snapshot (current state, last activity, pending items). Below a divider is the event log (chronological, expandable, timestamped). These answer different questions and should be visually distinct.

8. **Footer with context-sensitive shortcuts.** A 1-row footer showing the available keys for the current focus area (list vs. detail vs. history). Keys change when the user moves between panes. At minimum: navigation, filter, expand/collapse, quit.

9. **ASCII tree rendering for parent-child.** Use `├─` and `└─` for children, consistent with lazygit/htop conventions. Indent each level 2 characters. Show the parent's aggregate status as the worst child status. A collapsed parent with a failed child should show `FAIL` status, not the parent's own state.

10. **Don't use progress bars for state machine progress.** Instead, show "state X of N" where N is the count of known states in the template, or show current state name + "N states remaining" derived from the template's state list.

---

## Surprises

- **Temporal's "pending activities" panel is the most actionable part of its UI**, not the event history. The history is for debugging; pending activities tell you what's happening right now. This distinction (current vs. historical) is sharper in Temporal than in any other tool, and it maps directly to what koto needs: the "what is this session doing right now" answer should be immediately visible, not buried in an event list.

- **k9s handles parent-child through sequential navigation, not tree visualization.** Despite being a mature, well-regarded TUI, k9s doesn't show Deployment→ReplicaSet→Pod as a tree — it navigates between them as separate views. This suggests that collapsible tree views in TUIs are harder than they appear, and a simpler approach (e.g., a parent "summary row" that when selected replaces the list with children) might be more practical than a full inline tree.

- **The Gantt/timeline view (Prefect) is the most information-dense layout for workflow runs**, but it requires pixel-level rendering and doesn't translate to terminals at all. The terminal equivalent — showing timestamps and durations in a table — is much less spatial but is the right tradeoff for a TUI.

- **Color is a crutch that fails in certain environments.** Several tools (Temporal, Airflow) rely heavily on color for status, but provide no icon-based fallback. k9s and htop both use both color AND short text codes, which is more accessible. For a TUI, assuming the user has a color terminal is reasonable, but providing text codes as a primary signal (not just color) is better practice.

- **GitHub Actions' reusable workflow problem** — called workflows appearing as opaque steps — is exactly the problem koto faces with parent-child sessions. GitHub's handling is poor (you have to navigate away to see child details), which confirms this is a real design challenge, not a solved problem. The Temporal approach (deep-link to child, parent history shows the child event) is better.

---

## Open Questions

1. **How does ratatui handle collapsible tree rendering?** Is there a standard widget for indented, expand/collapse tree views, or does this need to be built from scratch? The answer affects how realistic a full tree view is in the first iteration vs. a simpler indented-list approach.

2. **How does koto's event log expose "what states are remaining"?** The engine knows the template's state list. Does `koto query` expose the full template state list alongside the current state, or only the current state? If not, the "remaining states" signal would require a template compilation step in the dashboard.

3. **What's the right behavior when a parent session is collapsed and a child transitions to FAIL?** Does the collapsed parent row immediately update to show FAIL status? Does it auto-expand? Auto-expansion on failure would be useful but potentially disruptive if the user has many collapsed trees.

4. **How should sessions without a parent be visually distinguished from sessions that are roots of a tree vs. standalone sessions?** A session with children is a "root"; a session with no parent and no children is standalone. These may warrant different visual treatment (e.g., a tree indicator icon on roots).

5. **Does the filter need to search across the full tree, or only top-level names?** If a child session matches a filter but the parent doesn't, should the parent be shown (with only the matching child visible)? This is a non-trivial UX decision with real complexity in the implementation.

6. **How should "stuck" be defined and displayed?** A session that hasn't transitioned in N minutes might be stuck, or it might be waiting on human input. The distinction matters for the status signal. Is there a koto concept of "waiting for human" vs. "waiting for agent" that could inform this?

7. **What should the detail pane show for a session in a terminal/final state?** When a workflow is DONE or FAILED, the "what's remaining" concept doesn't apply. The detail pane may need a different layout for completed vs. active sessions.

---

## Summary

Effective workflow monitoring tools at the 5–50 session scale share three structural decisions: they separate current state from historical events into distinct panels, they use color-plus-short-code for status (not text labels), and they expose "what's remaining" as a first-class signal rather than an afterthought. For koto's TUI specifically, the highest-leverage changes are replacing the flat session list with a collapsible indented tree (using `├─`/`└─` conventions), adding a three-level navigation model (list → detail → history), and ensuring parent sessions immediately reflect child failure status. The biggest open question is whether ratatui provides tree-widget primitives that make collapsible tree rendering tractable in the first iteration, or whether a simpler indented-list-with-manual-expand approach is the right starting point.
