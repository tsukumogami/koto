# Decision 3: ratatui Widget Layout

## Chosen: Option A
Single scrollable session list with an on-demand detail panel that slides up from the bottom when the user presses Enter on a selected row.

## Rationale

The primary use case is monitoring a batch with up to 1000 child sessions. In that scenario, the user needs to see as many rows as possible at once: aggregate coordinator rows with their fail counts, plus individual child rows scrolling below. Option A gives every pixel of terminal height to the session list in list-only mode. Option B sacrifices roughly half the horizontal space permanently — on an 80-column terminal the left pane is only ~44 chars wide, which can't comfortably show a session name, status, elapsed time, and fail counts without truncation. That width constraint makes B a poor fit for the minimum supported size in R21 and leaves the detail panel wasted space whenever the user is just monitoring.

Option A's modal switch is a natural fit for the investigation workflow. The user scans the aggregate row, sees a non-zero fail count, navigates to a specific child, and presses Enter to open the detail panel. The panel occupies roughly the bottom third of the screen (~7-8 rows), leaving the top two-thirds (~14-15 rows) for the list. That split gives enough context (the rows above and below the selected session remain visible) while fitting the gate state and last few evidence entries without scrolling. Pressing Escape or Enter again collapses the panel and returns to full-height list mode. The two modes map cleanly to the two primary use cases: list-only for monitoring, list+detail for investigation.

Option C (tabbed) was the second serious candidate because it avoids the width problem and gives each panel full screen real estate. It fails on the transition between use cases: switching tabs loses the session list entirely, which means the user must context-switch rather than glance up at the list while reading gate output. Tabs also add an extra key binding (Tab to switch) and an extra UI element (tab bar consuming a row) without improving information density over Option A.

## Rejected Options

### Option B: Vertical split (side-by-side)
Rejected primarily due to the 80-column constraint. A 55/45 split yields ~44 chars for the session panel and ~35 chars for the detail panel. Session rows require: indent (up to 4 chars for depth), name (~20-30 chars), state, elapsed, fail count — that doesn't fit cleanly in 44 chars without heavy truncation. The detail panel at 35 chars is barely wide enough for gate type labels. The always-visible detail panel wastes space during monitoring when no session is selected. On wider terminals (120+) this layout becomes attractive and could be offered as a V2 wide-terminal mode.

### Option C: Tabbed layout
Rejected because tab switching disconnects the user from the session list while investigating. A failing session's context (its siblings, the coordinator row above it) helps the user understand whether a failure is isolated or systemic. Losing that view on the "Detail" tab forces the user to flip back and forth. The tab bar also consumes a row and adds a navigation concept that isn't present in the simple j/k/Enter/Escape/q model targeted by the key binding requirements.

## Layout Wireframe

### List-only mode (default)

```
┌─ koto dashboard ──────────────────── repo: my-project ─── 500ms ─┐
│ SESSION                        STATE          ELAPSED   TASKS     │
│ root-workflow                  running        12m 04s             │
│  └ batch-coordinator           running         8m 33s   87/100    │
│     ├ task-a                   completed       1m 12s             │
│     ├ task-b                   completed       0m 58s             │
│  ▶  ├ task-c                   failed          2m 07s             │  ← selected
│     ├ task-d                   running         0m 43s             │
│     ├ task-e                   pending                            │
│     ├ task-f                   pending                            │
│     ├ task-g                   pending                            │
│     ├ task-h                   pending                            │
│     ├ task-i                   blocked                            │
│     └ task-j                   blocked                            │
│                                                                   │
│                                                                   │
│                                                                   │
│                                                                   │
│                                                                   │
│                                                                   │
│                                                                   │
│                                                                   │
└─ j/k:navigate  Enter:detail  q:quit ────────────────────────────-┘
```

80 columns × 24 rows. ~20 session rows available after header + status bar.

### List+detail mode (Enter pressed on task-c)

```
┌─ koto dashboard ──────────────────── repo: my-project ─── 500ms ─┐
│ SESSION                        STATE          ELAPSED   TASKS     │
│ root-workflow                  running        12m 04s             │
│  └ batch-coordinator           running         8m 33s   87/100    │
│     ├ task-a                   completed       1m 12s             │
│     ├ task-b                   completed       0m 58s             │
│  ▶  ├ task-c                   failed          2m 07s             │
│     ├ task-d                   running         0m 43s             │
│     ├ task-e                   pending                            │
│     ├ task-f                   pending                            │
│     ├ task-g                   pending                            │
│     ├ task-h                   pending                            │
│     ├ task-i                   blocked                            │
├─ task-c: detail ──────────────────────────────────────────────────│
│ Gate: command_gate             FAIL                               │
│  cmd: cargo test --lib                                            │
│  exit: 1  (2m 07s ago)                                           │
│ Evidence:                                                         │
│  [12:04:01] submitted: test-output (12 lines)                    │
│  [12:02:14] submitted: build-log (4 lines)                       │
└─ Esc:close  j/k:navigate  q:quit ──────────────────────────────-─┘
```

Detail panel occupies 7 rows (separator + 6 content rows). Session list retains 13 rows.

## Implementation Notes

Key ratatui widgets:

- **`Table`** for the session list — supports column alignment, row highlighting, and scrolling via `TableState`. Preferred over `List` because session rows have multiple aligned columns (name, state, elapsed, task counts).
- **`Block`** with `Borders::ALL` or `Borders::TOP` for the detail panel separator. A titled `Block` (e.g., `Block::default().title(" task-c: detail ")`) makes the panel self-labeling.
- **`Paragraph`** inside the detail panel for gate info and evidence entries. Wraps long lines automatically.
- **`Layout::vertical`** with `Constraint::Min` for the list and `Constraint::Length(N)` for the detail panel. When detail is hidden, use a single `Constraint::Percentage(100)` constraint so the list fills the frame.
- **`Scrollbar`** (ratatui built-in) on the session list for large batches.
- Mode is a simple enum (`enum ViewMode { List, Detail }`) on the app state struct. `Enter` toggles to `Detail`; `Escape` returns to `List`.

Detail panel height: 7-8 rows is sufficient for one gate entry (type, command, result, timing) plus 2-3 evidence timeline entries. If a session has more evidence, add a scroll indicator ("↓ 3 more").

## Assumptions

- Terminal minimum is 80×24 as specified in R21; layouts are designed for this floor.
- The detail panel is read-only; no interactive controls inside it beyond close.
- Gate display shows only the most recent `gate_evaluated` event for the selected session's current state. Historical gate results are out of scope for this panel.
- Evidence entries are shown newest-first (descending chronological order) to surface the most relevant entry without scrolling.
- The session list column layout (name, state, elapsed, tasks) fits in 78 chars at the minimum terminal width. Name column will truncate with `…` if needed.
- Aggregate task-count column (`TASKS`) is shown only for batch coordinator rows; it's blank for leaf sessions.
- A fixed detail panel height of 7 rows is used initially. Adaptive height (based on gate output length) is a V2 enhancement.
