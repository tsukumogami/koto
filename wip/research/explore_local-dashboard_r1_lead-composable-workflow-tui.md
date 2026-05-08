# Lead: Composable Parent-Child Workflow TUI Visualization

## Findings

### Current data structures

The existing code is more capable than the lead description implies. `dashboard_state.rs` already has:

- `expanded: HashSet<String>` — tracks which roots have children shown
- `visible_rows()` — depth-first flattening with `indent_depth: usize` (0 = root, 1 = child)
- `TaskCounts { total, running, done, failed }` — aggregated per coordinator row
- `sorted_children()` — priority ordering: failed → running → pending → terminal-done
- `session_has_children()` — used to gate expand behavior

`dashboard_data.rs` has:

- `SessionTree { sessions: HashMap<String, CachedSession>, roots: Vec<String> }` — `roots` is computed by `rebuild_roots()`: a session is a root if it has no `parent_workflow` or its parent is not in the map. Orphaned children automatically become roots.
- `CachedSession.header.parent_workflow: Option<String>` — the link that constitutes the tree edge
- Three-state status per session: `is_terminal`, `is_blocked`, and `current_state`

`dashboard_render.rs` currently renders hierarchy only as indentation: `" ".repeat(indent_depth * 2)` prepended to the name. No tree-line characters (├──, └──, │) are used. The `tasks_cell` shows `"{done}/{total} done"` which loses the `running` and `failed` counts.

The current `visible_rows()` only goes one level deep. A child session is never expanded further; only `roots` can be in `expanded`.

### TUI patterns for tree data

**Indented list with ASCII/Unicode tree lines** (used by `tree`, ranger, Helix file picker)
- Connector chars: `│  `, `├──`, `└──`
- Works well in a table/list widget — each row is still a flat entry
- Width cost is typically 3–4 chars per depth level
- Ratatui's `Table` widget accepts per-cell content, so tree lines can be prefixed to the name cell without structural changes

**Collapsible subtrees with expand marker** (used by k9s pod list, lazygit branch log)
- Collapsed: `▶ root-name  [4 children: 1 failed]`
- Expanded: `▼ root-name` followed by indented children
- The marker doubles as affordance ("press space/l to expand")
- k9s uses `<space>` to expand/collapse; lazygit uses `<space>` to stage/unstage but `<left>/<right>` (h/l) for navigation
- Lazy variants only render visible rows, so large trees don't cause render overhead

**Split-pane vertical** (used by many file managers: lf, nnn, broot)
- Left pane: list of roots; right pane: children of focused root; detail pane: bottom
- Navigation: j/k in left pane moves root focus; l enters right pane; h returns
- Advantage: parent and children simultaneously visible without scrolling
- Disadvantage: requires horizontal space; conflicts with the existing vertical split (list + detail)

**Outline with depth indicators** (used by Orgmode, outline-mode editors)
- Depth shown by indentation only, no tree lines
- This is what the current code already does

### Status rollup model

For the target pipeline (explore → prd → design → plan → work-on), a parent workflow represents the overall pipeline and its children are sequential stages. Mixed-status means the pipeline is partially through:

- All children done → parent status = "complete"
- Any child failed → parent status = "blocked" (pipeline stalled)
- At least one child running, none failed → parent status = "running (N/M)"
- All children pending, none running → parent status = "waiting"

The existing `TaskCounts` struct captures the right aggregates (`failed`, `running`, `done`, `total`). The rendering currently compresses this to `"{done}/{total} done"`, dropping `running` and `failed` counts. A richer format like `1 failed | 2 running | 3/7 done` would communicate pipeline health at a glance.

For non-pipeline (parallel child) trees, the same rollup applies: any failure is the most important signal, followed by in-progress count.

### Keyboard navigation

Current bindings: `j`/`k` (move cursor), `Enter` (enter Detail view from List), `Esc` (return to List), `Enter` in Detail (toggle expand). This is functional but the expand affordance is hidden: expand is behind `Enter` while already in Detail mode, not something a user would discover.

Reference patterns:
- **k9s**: `<space>` expands/collapses a namespace group; `j`/`k` move within the flat list; `<enter>` opens a resource
- **lazygit**: `j`/`k` navigate; `h`/`l` move between panels; `<space>` or `<enter>` act on selected item
- **tree command**: No navigation (static output); connector characters do the work visually
- **vim-style**: `zo`/`zc` open/close folds; `j`/`k` traverse all visible lines

A practical binding scheme for koto dashboard:
- `j`/`k` — move cursor through visible rows (current behavior, correct)
- `l` or `<space>` — expand the row under cursor (if it has children); do nothing if already expanded or leaf
- `h` — collapse the row under cursor (or its parent if already a child row)
- `<enter>` — open Detail pane for focused session (current behavior)
- `<esc>` — close Detail pane (current behavior)

This separates expand/collapse from the detail view transition, removing the current need to enter Detail mode just to expand.

### Edge cases

**Orphaned children** — already handled: `rebuild_roots()` promotes any session whose parent is absent into the root list. They will show as independent roots. If later the parent appears (unlikely but possible), `rebuild_roots()` would demote them. The UI should probably annotate orphan-promoted roots with a marker like `[orphan]` or a distinct color, so users know the parent is gone.

**Deeply nested (3+ levels)** — `visible_rows()` currently only emits depth 0 and depth 1. The `RowDescriptor.indent_depth` is a `usize` so it can represent any depth, but `sorted_children()` only looks at direct children of roots. The current data structure doesn't restrict depth, but the traversal does. Extending to full recursion requires changing `visible_rows()` to a recursive DFS and the `expanded` set must work for non-root sessions too. The `expanded` set is keyed by session ID, not depth, so it already supports arbitrary sessions.

**Cycles** — `parent_workflow` is a string reference, not a runtime pointer, so a cycle (A → parent B → parent A) would cause an infinite loop in recursive DFS. The data layer has no cycle detection. `rebuild_roots()` would include both A and B as roots because each would not find the other (each refers to the other as parent, so neither is absent from the map... actually wait: if A has parent B and B has parent A, then A.parent = B is in the sessions map, so A is not a root; B.parent = A is in the sessions map, so B is not a root. Both end up excluded from roots — an invisible cycle). This is a latent correctness bug.

**5 roots with 3–20 children** — at 5 roots × average 10 children, a fully expanded view would have 55 rows. On a typical 40-line terminal, this requires scrolling. The existing scrollbar handles this. Default view should be all roots collapsed, showing only 5 rows, with task counts giving the health summary. Users expand individually as needed.

## Implications

### What the list view should become

**Model: collapsible indented list with tree-line connectors**

Keep the existing 4-column `Table` widget. Change the name cell:

1. Add expand/collapse marker prefix for rows with children: `▶` (collapsed) or `▼` (expanded). Leaf rows and rows without children get no marker, or a non-interactive `·` glyph.
2. Add Unicode tree-line connectors for indented children: `├── ` for non-last children, `└── ` for the last child. Parent rows get no connector. This costs 4 characters per level.
3. Bind `l` / `<space>` to expand-on-cursor in List mode. Bind `h` to collapse-on-cursor or collapse-parent-of-current.

**Status rollup in the Tasks column**

Replace `"{done}/{total} done"` with a color-coded compact summary:
- `✗1 ◉2 ✓3/7` — one failed (red), two running (yellow), three done of seven total (green)
- Or text: `1F 2R 3/7` for terminals that don't support color reliably

**Detail pane unchanged** — the existing gate-detail pane stays as-is. What changes is how you reach it: navigate to any row (root or child) and press `<enter>`.

**Default view** — all roots collapsed. Roots with failed children show the failure in the Tasks column, guiding users to expand the relevant tree.

**Extend `visible_rows()` to arbitrary depth** — requires a recursive DFS. The `expanded` set (keyed by session ID) already supports this without structural change. Add cycle detection: track visited IDs during DFS traversal; skip any session whose ID is already in the visit stack.

### New key bindings

| Key | Action |
|-----|--------|
| `j` / `↓` | Move cursor down (current) |
| `k` / `↑` | Move cursor up (current) |
| `l` / `<space>` | Expand node under cursor |
| `h` | Collapse node under cursor (or parent if on a child) |
| `<enter>` | Open Detail pane for focused session (current, from List) |
| `<esc>` | Close Detail pane (current) |
| `r` | Force refresh (current) |
| `q` / `Ctrl+C` | Quit (current) |

Remove `Enter` in Detail mode as expand toggle — it overlaps with a natural "confirm/open" semantic. Move expand/collapse to List mode with `l`/`h`.

### Layout: stay with 2-pane

The current vertical split (list + detail) is the right default. A 3-pane horizontal split (roots | children | detail) would require knowing terminal width and adds navigation complexity. The indented-list approach fits the existing layout with no structural changes to `render_frame`.

## Surprises

1. **Much of the tree infrastructure is already built.** `expanded`, `visible_rows()`, `RowDescriptor.indent_depth`, `TaskCounts`, and `sorted_children()` all exist and are tested. The gap is rendering (tree-line chars, expand markers) and the key bindings (expand/collapse only works from Detail mode, not List mode).

2. **The current tasks format drops the most important signal.** `"{done}/{total} done"` hides `failed` and `running`. A coordinator with 1 failed child renders the same Tasks column shape as one with all children running — the operator has to expand and scan children to see failures. This should be fixed regardless of the tree visualization work.

3. **`visible_rows()` is hardcoded to one level deep.** The method calls `sorted_children(root_id)` once; there's no recursion. Deep trees (3+ levels) would require a code change to `visible_rows()`, not just to state.

4. **Cycle detection is absent and cycles create invisible sessions.** Two sessions that each reference the other as parent would both be excluded from `roots` and never appear in the dashboard at all. This is a silent data loss bug.

5. **Expand affordance is hidden behind Detail mode.** Pressing `Enter` opens Detail, and then `Enter` again in Detail toggles expand. This is non-discoverable; most users would expect to press something in List mode to expand a tree node.

## Open Questions

1. **Should the parent workflow itself be a real koto session or a virtual coordinator?** In the target pipeline, does the root "feature-pipeline" have its own template and state file, or does it exist only as a logical grouping derived from the `parent_workflow` pointers on children? This affects whether the root row shows a real `current_state` or just aggregate counts.

2. **How does expand state survive a data refresh?** Currently, `expanded` is in-memory. If a root session disappears and reappears (unlikely but possible during refresh), its expand state would persist stale. Should `clamp_expanded()` clean up entries for sessions no longer in the tree?

3. **Should tree lines be computed per-refresh or per-render?** Currently `visible_rows()` does the flattening; the render layer formats the name. Tree-line connectors (whether a child is "last") require knowing sibling position, which is available in `visible_rows()` but not in the render layer. Either `RowDescriptor` needs a `is_last_sibling` field, or the render layer needs the full sibling list.

4. **Is 3+ level depth realistic in practice?** The target pipeline is one level deep (root spawns sequential children). If 3+ levels never occur in practice, the cycle detection and recursive DFS complexity may not be worth implementing now. Worth checking whether the spawn mechanism allows a child to itself spawn grandchildren.

5. **What should the `h` key do when the cursor is on a root that's already collapsed?** Options: (a) do nothing, (b) move cursor to parent (no parent for roots), (c) wrap to jump up past siblings. Lazygit uses `h` to navigate to the parent panel entirely; in a single-pane list this doesn't translate directly.

6. **Should orphaned children be visually distinguished?** They're already treated as roots by `rebuild_roots()`, but a user who sees them won't know the parent is gone. An `[orphan]` annotation or a dim style would clarify the situation without extra navigation.

## Summary

The state and data layers already have most of the tree infrastructure (expand set, depth-first flattening, task counts, sorted children), but the render layer only uses space-based indentation and the key bindings bury expand/collapse behind Detail mode instead of exposing it directly in List mode. The main implication is that the changes needed are narrower than a redesign: add Unicode tree-line connectors and an expand-marker glyph to the name cell, move expand/collapse to `l`/`h` in List mode, fix the Tasks column to surface `failed` and `running` counts, and extend `visible_rows()` to recurse beyond one level with cycle detection. The biggest open question is whether the root "feature-pipeline" workflow is a real koto session with its own state (enabling a meaningful `current_state` in the root row) or a virtual grouping that exists only in the `parent_workflow` pointer, because that determines whether the root row conveys actionable status or is purely a summary container.
