<!-- decision:start id="detail-pane-data-loading" status="assumed" -->
### Decision: Detail pane data loading in the always-visible horizontal split layout

**Context**

The dashboard currently uses two view modes (`List` and `Detail`). The detail pane only loads
data when the user presses Enter to enter Detail mode. The new horizontal split design makes
the detail pane always visible — 40% list / 60% detail — so it must show meaningful content
for the focused session without requiring a keypress.

`read_detail()` in `dashboard_data.rs` performs a full JSONL parse of the focused session's
state file on every call. It scans all events twice: once to find the current-epoch boundary,
once to collect gate evaluations and evidence. For 500 events this is fast (well under 20ms),
but the operation is unconditionally repeated on every poll tick under the current model.
`refresh()`, by contrast, uses mtime-based diffing to skip re-reading sessions whose files
haven't changed. The detail layer has no equivalent guard.

The rendering is synchronous (ratatui renders on each tick). There is no background loading.
The 200ms full-refresh budget applies to the entire tick — list refresh plus detail load.
A typical user has 5-15 sessions; global scope could bring more.

**Assumptions**

- A JSONL parse of 500 events completes in under 20ms on developer hardware. If payloads are
  significantly larger, the per-tick cost may need revisiting.
- The expensive History and Remaining tabs are rendered lazily (only when active), not on every
  tick. If all tab data is loaded simultaneously on each refresh, a per-tab lazy loading layer
  would be needed on top of this decision.
- `focused_id` will be extended to always track the cursor position, not only when in Detail
  mode. This is a prerequisite for any option that loads detail data for the focused session.

**Chosen: Tick-based load with mtime guard on the focused session**

On each poll tick, after `refresh()` completes, check whether the focused session's state file
has a newer mtime than the last detail load. If yes (or if `focused_id` changed since the last
load), call `read_detail()` and update `detail_cache`. If the file is unchanged, skip the parse
and keep the cached data.

Implementation requires two small additions to `DashboardAppState`:
- `detail_cache_mtime: Option<SystemTime>` — the mtime at last successful detail read
- `detail_cache_session: Option<String>` — the session ID the cache belongs to (invalidates
  when the cursor moves to a different session)

The guard logic in the tick loop becomes:
1. Determine focused session from cursor position (always, not only in Detail mode)
2. If `focused_id` changed from `detail_cache_session`, clear cache and read immediately
3. If `focused_id` is unchanged, compare `detail_cache_mtime` against current file mtime;
   skip read if equal

**Rationale**

The mtime guard eliminates redundant JSONL parsing during the common case — a user reading a
session that hasn't been written to since the last poll. This mirrors the pattern already used
by `refresh()` for the session list, keeping the data layer consistent.

The tick-based trigger (rather than cursor-move trigger) is sufficient. The poll interval
defaults to 500ms. A user moving the cursor will see updated data within one poll interval,
which is imperceptible during casual navigation. Triggering a read on every cursor keystroke
adds I/O during rapid j/k scrolling with no practical UX benefit.

Loading only the focused session (not all visible sessions) keeps the budget predictable: one
JSONL parse per tick at most, regardless of how many sessions are visible. With the mtime guard,
idle sessions cost only a cheap `stat()` syscall per tick.

**Alternatives Considered**

- **Load focused session on every polling tick (no mtime guard)**: Simple and correct, but
  performs a full JSONL parse on every tick even when the session file hasn't changed. Wasteful
  during the common case where the user is reading a static session. Rejected in favor of the
  mtime guard, which costs one extra field and one syscall.

- **Load on cursor move plus tick refresh**: Triggers `read_detail()` on every j/k keypress in
  addition to the polling tick. Provides the freshest possible data at cursor arrival, but
  causes redundant reads during rapid navigation (multiple reads within one poll interval).
  The 500ms polling interval is fast enough that this optimization doesn't improve perceived
  responsiveness. Rejected as unnecessary complexity.

- **Eager load all visible sessions**: Calls `read_detail()` for every session in
  `visible_rows()` on each tick. At 15 sessions × ~10ms per parse = ~150ms, this approaches
  the 200ms budget before accounting for list refresh. At global scope with more sessions it
  would breach the budget. Rejected as not viable.

- **Retain List/Detail mode distinction (layout change only)**: The split pane is always visible
  in the new layout, but shows a "press Enter to view details" placeholder in List mode.
  Preserves the current data loading model unchanged. Rejected because an always-visible pane
  that requires a keypress to populate is confusing — it defeats the purpose of the split layout.

**Consequences**

- The detail pane shows content for the focused session without any user action beyond moving
  the cursor, meeting the always-visible pane requirement.
- Redundant JSONL parsing is eliminated for idle sessions — the common case while a user is
  reading a session.
- The 200ms budget is comfortably met: at most one `stat()` syscall plus one conditional JSONL
  parse per tick, not N parses.
- `DashboardAppState` gains two small fields (`detail_cache_mtime`, `detail_cache_session`).
- `focused_id` must be maintained in List mode (currently only set when entering Detail mode);
  this is a prerequisite change and touches `handle_key()` cursor movement for List mode.
- The ViewMode distinction can be retained for other purposes (e.g., controlling which pane
  receives keyboard focus) or simplified — this decision does not require removing it.
<!-- decision:end -->
