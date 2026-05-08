# Lead: ratatui TUI Patterns for Rich Detail Pane Design

## Findings

### Current Layout

The dashboard uses a simple vertical split (`Layout::vertical`) with `Constraint::Min(0)` for the list and `Constraint::Length(8)` for the detail pane — a fixed 8-row strip at the bottom. The main list occupies all available height; the detail strip only appears when `ViewMode::Detail` is active.

The detail pane is rendered with a single `Paragraph` widget using `Text::from(lines)`. There is no scrollbar, no scroll state, and no scroll position tracking. The pane is gated entirely on `DetailData`, which today requires a `GateEvaluated` event to exist in the current epoch. If none exists, the pane shows a static string ("No gate evaluations recorded" or "No state history"). Sessions that use only evidence submission currently show nothing useful.

### ratatui Version and Available Widgets

The project uses `ratatui = "0.29"`. In 0.29, the widget inventory includes:

- `Paragraph` — wraps `Text` (a `Vec<Line>`), supports `wrap`, `scroll((row, col))` for scrollable content, and `alignment`. Scroll is set per-render call using an offset stored in app state. This is the primary widget for free-form text blocks.
- `Table` + `TableState` — renders rows with column widths defined by `Constraint`. Used already for the session list. Good for key-value pairs when you want fixed-width label columns.
- `List` + `ListState` — simpler single-column widget with selectable items. Less appropriate here than `Table`.
- `Tabs` — renders a row of tab titles and reports the active tab index. The caller renders only the active tab's content; ratatui does not manage content switching itself.
- `Scrollbar` + `ScrollbarState` — a `StatefulWidget` that draws a scrollbar track and thumb. Used already in the session list. Works with any container area; the caller manages `position` and `content_length`.
- `Block` — wraps any widget with borders and a title. Can carry title spans with styling.
- `Gauge` — renders a progress bar, useful for batch completion ratios.
- `Sparkline`, `BarChart`, `Chart` — visualization widgets; not relevant here.
- `Canvas` — raw drawing; not relevant.

### Scrollable Text Pattern

The standard ratatui scrollable-text pattern:

```rust
// In app state:
pub detail_scroll: u16,

// In render:
let paragraph = Paragraph::new(text)
    .scroll((state.detail_scroll, 0))
    .block(block);
f.render_widget(paragraph, area);

// Show scrollbar when content exceeds area:
let mut scrollbar_state = ScrollbarState::new(content_len)
    .position(state.detail_scroll as usize);
f.render_stateful_widget(
    Scrollbar::new(ScrollbarOrientation::VerticalRight),
    area, &mut scrollbar_state,
);
```

The scroll offset must be clamped in `handle_key` to `content_len.saturating_sub(visible_rows)`. This is all explicit; ratatui has no internal scroll management beyond the offset parameter.

### Tabbed Panels Pattern

Tabs in ratatui work like a radio-button header with manual content dispatch:

```rust
// In app state:
pub detail_tab: usize,  // 0 = Summary, 1 = History, 2 = Remaining

// In render:
let tab_titles = vec!["Summary", "History", "Remaining"];
let tabs = Tabs::new(tab_titles)
    .select(state.detail_tab)
    .highlight_style(Style::default().fg(Color::Yellow));
f.render_widget(tabs, tab_header_area);

// Then dispatch on detail_tab to render different content
match state.detail_tab {
    0 => render_summary(...),
    1 => render_history(...),
    _ => render_remaining(...),
}
```

Each tab gets its own scroll state. The tab header consumes about 1 row height. Keyboard handling adds Tab/Shift-Tab (or 1/2/3) to cycle between tabs.

### Key-Value Table Pattern

For structured evidence fields, a `Table` with two columns (label, value) reads naturally:

```rust
let rows: Vec<Row> = fields.iter().map(|(k, v)| {
    Row::new(vec![
        Cell::from(k.as_str()).style(bold),
        Cell::from(v.to_string()),
    ])
}).collect();

let table = Table::new(rows, [Constraint::Length(20), Constraint::Min(0)])
    .block(block);
f.render_widget(table, area);
```

When values are long JSON blobs, the right column wraps if `Paragraph` is used inside a custom widget. For the simple case (short values), `Table` is sufficient. For deep-nested JSON, rendering the value as pretty-printed text inside `Paragraph` is more readable.

### Timeline / History Pattern

Transition history is naturally a list of timestamped entries. The ratatui pattern is a `Paragraph` with one `Line` per event, using `Span::styled` for color-coding:

```rust
Line::from(vec![
    Span::styled("2026-05-08T10:23Z", dim_style),
    Span::raw(" "),
    Span::styled("gather", bold_style),
    Span::raw(" -> "),
    Span::styled("analyze", current_style),
    Span::raw(" [evidence]"),
])
```

Color-coding by event type (green=transition, yellow=evidence, red=gate-fail, cyan=rewind) makes the timeline scannable. For large histories (200+ events), this list must be scrollable — the Paragraph scroll pattern applies here too.

### Available Data in the Event Log

Reading `src/engine/types.rs`, the event log carries more than the current detail pane surfaces:

| Event type | Dashboard-relevant fields |
|---|---|
| `WorkflowInitialized` | `template_path`, `variables` |
| `Transitioned` | `from`, `to`, `condition_type`, `skip_if_matched` |
| `EvidenceSubmitted` | `state`, `fields` |
| `GateEvaluated` | `gate`, `outcome`, `timestamp`, `output` |
| `GateOverrideRecorded` | `gate`, `rationale`, `override_applied`, `actual_output` |
| `Rewound` | `from`, `to`, `rationale` |
| `DirectedTransition` | `from`, `to`, `rationale` |
| `DecisionRecorded` | `state`, `decision` |
| `DefaultActionExecuted` | `state`, `command`, `exit_code`, `stdout`, `stderr` |
| `WorkflowCancelled` | `state`, `reason` |
| `ContextAdded` | `key`, `hash`, `size` |

The template file (`CompiledTemplate`) provides: state names, `directive` (the agent instruction), `details`, `terminal` flag, `accepts` (evidence schema), `gates`. This is what powers a "remaining states" view.

### What `read_detail` Currently Returns vs. What Is Needed

Current `DetailData` fields:
- `gate_name`, `command`, `result`, `elapsed`, `evidence: Vec<EvidenceEntry>`

Missing for a universal detail pane:
- Current state name and directive (from template)
- Transition history (list of timestamped from→to pairs with condition type)
- Evidence per state (not just current epoch, or at minimum all evidence in current epoch with full field display)
- Gate evaluations with full output (not just most-recent)
- Whether the session is blocked/terminal (already in `CachedSession` but not passed to render)
- Session creation timestamp, template name
- Remaining states (requires reading compiled template)

---

## Implications

### Proposed Layout

The current fixed-8-row strip is too shallow for useful detail. Recommended: switch to a **horizontal split** (list left ~40%, detail right ~60%) rather than a vertical strip. This matches the description in the exploration context and is more conventional for a master-detail TUI. If a vertical split is retained, the detail pane height should be `Min(0)` not `Length(8)` — at least half the terminal height.

**Concrete detail pane structure (vertical sub-layout within the right pane):**

```
+------------------------------------------+
| Session: my-workflow   [running]  12m34s |  <- 1 row header block title
+------------------------------------------+
| [Summary] [History] [Remaining]          |  <- 1 row Tabs widget
+------------------------------------------+
|                                          |
|  (tab-specific scrollable content)       |
|                                          |
+------------------------------------------+
|  j/k scroll  Tab: switch tab  Esc: back  |  <- 1 row status/help line
+------------------------------------------+
```

**Summary tab (default):**
- Current state name (large/bold)
- Directive text (Paragraph, wrapped)
- Evidence submitted at current state (key-value Table, newest entry)
- Gate result if present (PASS/FAIL with color, gate name, elapsed)

**History tab:**
- Scrollable list of all state transitions, newest-first
- Each entry: `timestamp | from → to [condition_type]`
- Interleaved evidence/gate events in context
- Color-coded by event type
- Scrollbar when content exceeds visible area

**Remaining tab:**
- States not yet visited, derived from template graph traversal
- Shown as a `List` of state names with their directives
- Only meaningful when `template_path` is accessible and parseable
- Graceful fallback: "Template not available" when template file is missing

### Scaling: 2 events vs. 200 events

For 2-event sessions (just initialized + one transition), Summary tab shows minimal content cleanly; History tab has 2 lines without a scrollbar. For 200-event sessions, Summary still shows only current-state data (always bounded); History tab scrolls with a scrollbar. The tab design naturally isolates the unbounded data to a specific tab rather than dumping all 200 events into one view.

### Data Layer Changes Required

`DetailData` needs to be replaced or substantially extended. A new `RichDetailData` struct:

```rust
pub struct RichDetailData {
    pub session_id: String,
    pub workflow_name: String,
    pub created_at: String,
    pub template_name: Option<String>,      // from CompiledTemplate.name

    // Current state summary
    pub current_state: Option<String>,
    pub current_directive: Option<String>,  // from template
    pub is_terminal: bool,
    pub is_blocked: bool,

    // Evidence at current state (current epoch, all entries, newest-first)
    pub current_evidence: Vec<EvidenceEntry>,

    // Most recent gate result at current state (if any)
    pub latest_gate: Option<GateSummary>,

    // Full transition history (all epochs, chronological)
    pub history: Vec<HistoryEntry>,

    // States not yet reached (derived from template)
    pub remaining_states: Vec<RemainingState>,
}

pub struct GateSummary {
    pub gate_name: String,
    pub result: String,   // "PASS" or "FAIL"
    pub elapsed: Duration,
    pub command: Option<String>,
    pub override_rationale: Option<String>,
}

pub struct HistoryEntry {
    pub timestamp: String,
    pub event_type: String,      // "transition", "evidence", "gate", "rewind", etc.
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub label: String,            // human-readable one-line summary
}

pub struct RemainingState {
    pub name: String,
    pub directive: String,
    pub is_terminal: bool,
}
```

The `read_detail` function needs to read all events (it already does), build history from all transition/evidence/gate events, and read the compiled template for `remaining_states` and directives. Template reading already exists in `is_terminal_state` — that pattern can be reused.

### App State Changes

`DashboardAppState` needs:
- `detail_tab: usize` — active tab index (0=Summary, 1=History, 2=Remaining)
- `detail_scroll: u16` — scroll offset for the active tab's content
- Per-tab scroll offsets (or reset on tab switch): simplest is a `[u16; 3]` array indexed by tab

`ViewMode` is fine as-is. Keyboard additions: Tab key to cycle tabs, j/k within detail view to scroll content.

### Layout Change (vertical → side-by-side)

```rust
// Replace the current vertical split with horizontal:
let chunks = Layout::horizontal([
    Constraint::Percentage(40),
    Constraint::Percentage(60),
]).split(f.area());
render_list(f, state, chunks[0]);
render_detail(f, state, chunks[1]);
```

This matches the stated goal (list left ~40%, detail right ~60%) and gives the detail pane much more vertical space for the tab content.

---

## Surprises

**The layout described in the exploration context does not match the code.** The context says "list on the left (~40% width) and a detail panel on the right (~60% width)" but the code uses a **vertical** split with the detail pane as a fixed 8-row strip at the **bottom**. The render mode switch is already built (`ViewMode::List` / `ViewMode::Detail`) but Detail mode just shrinks the list vertically rather than splitting horizontally. Either the design changed after the description was written, or the implementation is a placeholder that never evolved to horizontal.

**`read_detail` returns `None` for any session with no gate evaluations.** This means evidence-only sessions (the second interaction pattern mentioned in the context) currently show "No gate evaluations recorded" even when they have submitted evidence. The evidence is collected inside `read_detail` but the function returns `None` if there is no `GateEvaluated` event — the evidence collection code is dead for gate-free sessions.

**The `EVIDENCE_DISPLAY_CAP` of 3 is a rendering constant, not a data constraint.** The data layer already fetches all evidence; only the render layer truncates it. Removing the cap and adding scroll is purely a rendering change.

**The compiled template path is already available** via `StateFileHeader.template_path` (from `WorkflowInitialized` event). The `is_terminal_state` helper already reads and parses the compiled template. Reusing this for directive lookup and remaining-states computation is straightforward — no new persistence needed.

---

## Open Questions

1. **Horizontal vs. vertical split**: Should the layout change to a true side-by-side split, or is the bottom-strip approach intentional to maximize list visibility? The context description says side-by-side but the code says vertical. This needs a product decision before layout work starts.

2. **History depth limit**: For sessions with 200+ events, building a full `HistoryEntry` list on every poll cycle (every `poll_every_n_ticks` ticks) is fine if the history list is built lazily and only when in Detail mode. But should the data layer cap history at N entries for display, with a note that earlier events are truncated? Or always show all?

3. **Template availability**: If the compiled template file is missing (e.g., template was updated or cache was cleaned), the Remaining tab cannot show anything. The Summary directive also falls back to None. Is "directive from template" worth the complexity, or should it be deferred?

4. **Tab key conflict**: `Tab` is the natural key for switching tabs. But `crossterm` also maps Tab to `KeyCode::Tab`. Verify that there is no conflict with any existing binding (there isn't currently, but the handle_key match needs updating).

5. **Detail pane refresh when focused session changes**: Currently `detail_cache` is set to `None` when the cursor moves in Detail mode, and the cache is only refreshed on the next poll tick. For large sessions, that means a blank pane for up to `poll_every_n_ticks * 50ms`. Should the refresh be immediate (triggered on cursor movement) or deferred to the next tick?

6. **RichDetailData construction cost**: Reading and parsing the compiled template JSON on every refresh tick for every focused session could be expensive if the template is large. Should template data be cached separately (parsed once per template hash)?

---

## Summary

The current detail pane is hardwired to gate-evaluated sessions via a `DetailData` struct that returns `None` for evidence-only sessions, and the layout is a fixed 8-row vertical strip rather than the side-by-side split described in the design intent. A tabbed design (Summary | History | Remaining) using `ratatui::Tabs` for the header and `Paragraph::scroll()` with `Scrollbar` for each tab's content naturally handles both 2-event and 200-event sessions because unbounded data lives in the History tab while Summary remains bounded. The data layer must move from the current gate-centric `DetailData` to a new `RichDetailData` struct that always populates current-state info and transition history from the full event log regardless of whether any gates exist. The biggest open question is whether to resolve the layout discrepancy (vertical strip vs. horizontal split) before or alongside the data layer rework, since both are required changes but the layout change has a wider test surface impact.
