# Phase 2 Research: UX Perspective

## Lead 1: TUI Layout for Session Hierarchy

### Findings

**Standard terminal constraints:**
An 80×24 terminal provides roughly 22 usable rows (minus status bar and title). This supports approximately 15–20 session rows without scrolling, using one row per session. For deeper hierarchies or large batch outputs, scrolling or expansion is required.

**Hierarchical display conventions (k9s, gitui, lazygit, bottom):**
Mature terminal UIs consistently use:
- Tree indentation (2–4 spaces per level) to signal parent/child relationships
- Inline aggregate rows for groups (e.g., "10/50 running" on the parent row)
- A cursor-based navigation model (j/k or arrow keys move focus, Enter expands)
- Fixed-position header/status rows with a scrollable main viewport

**Recommended layout structure (80×24):**
```
┌──────────────────────────────────────────────────────────────────────────────┐
│ koto dashboard · repo: my-project · Updated: 0.3s ago                        │  (header, 1 row)
├──────────────────────────────────────────────────────────────────────────────┤
│  NAME                          STATE          TIME    STATUS                 │  (column headers, 1 row)
│  orchestrator                  exploring      4h 12m  running                │
│  ├── prd                       in-progress    45m     running                │
│  ├── design                    pending        --      waiting                │
│  ╰── plan                      pending        --      waiting                │
│  batch-work                    coordinating   2h 08m  running                │
│  ╰── [1000 tasks] 847✓ 12✗ 141⋯                                            │  (aggregate row)
│                                                                               │
│                                                                               │
│                                                                               │
├──────────────────────────────────────────────────────────────────────────────┤
│ [j/k] Move  [Enter] Expand  [g] Gates  [r] Refresh  [q] Quit  [?] Help      │  (footer, 1 row)
└──────────────────────────────────────────────────────────────────────────────┘
```

**Per-row field recommendations:**
- Session name (with tree connector prefix: `├──`, `╰──`)
- Current state name (from last `transitioned.to` event)
- Elapsed time since session creation (human-readable: `4h 12m`)
- Status indicator: `running` / `terminal` / `blocked` (red) / `failed` (red)

**Key binding conventions (matching TUI ecosystem):**
| Key | Action |
|-----|--------|
| `j` / `↓` | Move cursor down |
| `k` / `↑` | Move cursor up |
| `g` / `Home` | Jump to first entry |
| `G` / `End` | Jump to last entry |
| `Enter` | Expand/collapse children or drill into detail view |
| `e` | Toggle evidence panel |
| `r` | Force refresh |
| `q` | Quit |
| `?` | Show key bindings help |
| `Ctrl+C` | Exit (always works) |

### Implications for Requirements

1. **Viewport model:** Main panel scrolls; header (title + column labels) and footer (key hints) are fixed. The viewport shows as many sessions as fit, with cursor tracking.
2. **Hierarchy rendering:** Depth is shown via ASCII tree connectors. At depth 3 (root → batch coord → tasks), the full tree is visible in one scroll without requiring a second navigation step.
3. **Aggregate row for large batches:** When a batch coordinator has >1 child, its children are collapsed behind an aggregate row by default. The aggregate row shows: `[N tasks] X✓ Y✗ Z⋯` where X=success, Y=failed, Z=pending/running.
4. **Color coding:** Green = running, Yellow = pending/blocked, Red = failed, Gray = terminal-success, Dim = skipped.

---

## Lead 2: Handling 1000+ Sibling Batches

### Findings

**The scaling problem:**
1000 children cannot fit in a 24-row terminal. Showing all by default creates:
- O(1000) rows that require scrolling past to reach the parent's gate state
- Visual noise that obscures the summary (failed count)
- Slow initial render and high poll overhead

**Pattern from orchestration tools:**
GitHub Actions (1000+ jobs), k9s (1000+ pods), and Jenkins (parallel stages) all use the same pattern:
1. **Default: aggregate summary row** — collapsed, showing counts
2. **On-demand expansion** — Enter or click to expand full list
3. **Failure prominencing** — failed items sort to top or are color-highlighted

**Recommended UX for large batches:**
- Default collapsed aggregate row: `[1000 tasks] 847 done · 12 failed · 141 pending`
- Expand with Enter → shows a paginated/scrollable sublist
- In the sublist: failed tasks sorted first (or filtered to "failures only" by default)
- Pagination: show 20 children at a time with `n`/`p` for next/previous page

**Failure prominencing is the key UX requirement:**
Users running a 1000-task batch are NOT asking "are all tasks running?" — they're asking "are any tasks failing?" The dashboard must answer this immediately:
- The aggregate row must show the failed count in **red** when non-zero
- Expanding to the child list must sort failed tasks first (before pending/running)
- Each task row must show its skip reason or failure mode inline

**Drill-down UX for a failing task:**
When a user navigates into a failing child session, the focused view shows:
1. Current state name + elapsed time
2. Gate status panel (which gate is failing and why)
3. Recent evidence timeline (last 3-5 `evidence_submitted` events)
4. Skip reason chain (if skipped, why — following `skipped_because_chain`)

### Implications for Requirements

1. **Children are collapsed by default for any batch with >5 children** — shows aggregate row instead.
2. **Aggregate row minimum fields:** total count, success count, failed count (red if >0), pending/running count.
3. **Expanding a large batch** shows the child list sorted by status priority: failed first, then running, then pending, then success/skipped.
4. **Evidence timeline** (from `evidence_submitted` events) shown in the detail panel for any session with recent evidence.
5. **Skip reason chain** surfaced in the detail panel when a task is in a skipped state.

---

## Summary

**Aggregation-first for large batches:** With up to 1000 siblings, the dashboard collapses children behind an aggregate row (total/success/failed/pending). Users expand on demand. Failed tasks sort to the top on expansion.

**Failure visibility as primary UX goal:** Users come to the dashboard to answer "what failed?" — not "what's running?" Failed counts appear in red in the aggregate row and failed children sort first on expansion.

**Template coupling and gate scanning are UX requirements:** Without a `workflow_completed` event or pre-computed gate summaries, the dashboard must load compiled templates (to determine terminal status) and scan Tier 2 events (to compute last gate result per state). These are non-negotiable for the core UX goal of immediate orientation — users need to know instantly whether a session is complete and what its gates look like.
