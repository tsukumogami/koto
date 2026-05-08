---
status: Planned
upstream: docs/prds/PRD-local-dashboard.md
problem: |
  koto has no live visibility surface. The engine writes session state to JSONL files
  at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl` on every advance, but
  reading that state requires either raw JSON parsing or invoking `koto status` per
  session. For users monitoring 100–1000 parallel child sessions in a batch pipeline,
  this forces either manual polling loops or staying blind. A terminal UI must be
  added to the existing synchronous Rust binary that can read session state, derive
  hierarchy from parent-child headers, poll for changes, and render a live tree — all
  without introducing an async runtime.
decision: |
  Add a `koto dashboard [<name>]` subcommand backed by a ratatui TUI. The implementation
  splits into three layers: a data layer that reuses existing persistence functions
  (`derive_machine_state`, `derive_last_gate_evaluated`) to read and derive session
  state; an application state layer that holds the full session tree and handles
  expand/collapse, cursor position, and poll timing; and a rendering layer built on
  ratatui widgets. The event loop uses crossterm's synchronous `poll`/`read` API
  (no async) with a tick-based poll cycle. A `--once` flag bypasses the TUI entirely
  and writes plain-text output for scripting.
rationale: |
  ratatui with crossterm is the only TUI option that stays fully synchronous — it uses
  `std::io` and blocking-with-timeout event reads, matching koto's existing architecture.
  Reusing `src/engine/persistence.rs` functions for state derivation avoids duplicating
  logic and stays consistent with `koto status` behavior. The three-layer separation
  (data / app state / render) keeps TUI rendering code isolated from session logic,
  enabling unit tests of the data and state layers without a PTY.
---

# DESIGN: Local Dashboard

## Status

Planned

## Context and Problem Statement

koto sessions accumulate state in JSONL files — one file per session, written atomically
on every advance. The engine already provides functions to derive current state from these
files (`derive_machine_state`, `derive_state_from_log`, `derive_last_gate_evaluated` in
`src/engine/persistence.rs`), and `koto status` uses them to produce a one-shot JSON
snapshot. But there is no continuous view: users monitoring a batch run with 100+ parallel
child sessions must script their own polling or re-invoke `koto status` repeatedly.

The technical challenge is building a live terminal UI on top of a purely synchronous
codebase. koto has no async runtime (no tokio, no async-std), and adding one would be a
significant architectural change. The event loop for a TUI typically requires concurrent
I/O — waiting for keyboard input while also polling files. This must be accomplished with
synchronous primitives: a blocking-with-timeout event read from crossterm combined with a
wall-clock tick timer.

A secondary challenge is session hierarchy. Session files have no index; hierarchy is
reconstructed by reading all sessions and grouping by their `parent_workflow` header
field. For 100 sessions this requires 100 file header reads per poll cycle. The data layer
must be efficient enough to keep the UI responsive while keeping I/O proportional to the
number of changed files rather than total session count.

The design must also integrate cleanly with the existing CLI structure: a new `Dashboard`
variant in the `Command` enum, cleanly separated from existing session management code.

## Decision Drivers

- **No async runtime** (R19): the implementation must stay fully synchronous; crossterm's
  `poll()` with a timeout is the only viable event-loop primitive
- **Single binary** (R20): the dashboard extends the existing `koto` binary; no new
  binaries or background processes
- **Startup performance** (R16, R17): ≤1s for repo-wide with 100 sessions; ≤100ms for
  focused single-session view — data reading must be fast enough to meet these
- **Reuse existing persistence layer**: `derive_machine_state` and
  `derive_last_gate_evaluated` already implement terminal detection and gate result
  computation; the dashboard must not duplicate this logic
- **Testability without PTY**: the data layer and application state layer must be testable
  without a real terminal; only the rendering layer requires integration tests
- **V2 daemon compatibility**: `koto dashboard` as a top-level command leaves room for
  `koto daemon start/stop` in V2 without namespace collision
- **Graceful degradation**: truncated JSONL files, missing compiled templates, and unknown
  event types must not crash the dashboard (R15, R10)
- **Public codebase**: design doc, code, and tests must be written for external
  contributors; no internal references

## Considered Options

### D1: Event Loop Architecture

Three approaches to interleaving keyboard input and file polling in a synchronous binary:

**Option A — chosen: single-threaded tick loop with `crossterm::event::poll(50ms)`**

A single `loop { poll(...); tick++; if tick%N { poll_files() }; draw() }` structure.
`crossterm::event::poll(Duration::from_millis(50))` parks the OS thread for up to 50ms,
waking immediately when a key event arrives. File polling runs every N ticks (N =
poll_interval_ms / tick_rate_ms). No concurrency, no shared state, no locks.

*Strengths*: input latency ≤50ms with zero busy-waiting; single-threaded means no
synchronization; `signal-hook`'s `AtomicBool` is checked after each `poll()` for clean
shutdown; crossterm's `poll` is cross-platform (Linux, macOS, Windows).

*Weaknesses*: file I/O blocks the tick thread — if `poll_session_files` takes longer than
50ms, render cadence slips. In practice, 1000-session scan via `stat()` is <10ms on warm
cache, so this is acceptable.

**Option B — rejected: two threads with mpsc channel**

A background thread calls `crossterm::event::read()` (blocking, no timeout) and sends
events to the main thread via a channel. The main thread polls files and renders.

*Fatal flaw*: shutdown deadlock. When the user presses Ctrl+C, `signal-hook` sets the
`AtomicBool` — but the keyboard thread is blocked in `read()` with no timeout and no
way to be interrupted without platform-unsafe tricks. Unblocking requires synthetic event
injection or unsafe platform calls; neither is simpler than Option A. Also forces all
shared state behind `Arc<Mutex<_>>`.

**Option C — rejected: non-blocking `try_read()` with explicit sleep**

`crossterm::event::poll(Duration::ZERO)` + `read()` followed by `std::thread::sleep(50ms)`.
Structurally identical to A but with unconditional 50ms wakeups regardless of pending
events. Burns one unnecessary OS wakeup per tick during idle periods. No scenario where
this is preferable to Option A.

---

### D2: Session State Model

Three approaches to representing and refreshing session hierarchy in memory:

**Option C — chosen: `HashMap<String, CachedSession>` with mtime-based incremental diff**

An in-memory map keyed by session ID. On each poll cycle: stat all session files (~1–5µs
per file on warm cache), remove entries for sessions no longer on disk, re-read only files
whose `mtime` has advanced. The cache entry stores the parsed header, derived current
state, and the mtime used for validation.

*Strengths*: O(1) lookup for any session; diff detection is a single `HashMap` key
comparison; I/O is proportional to changed files, not total sessions; 1000-session stat
loop takes <10ms. Full event history is deferred: only loaded on demand for the focused
detail panel.

*Weaknesses*: stat loop for 1000 sessions is slightly more work than an inotify-style
approach, but inotify is Linux-specific and would complicate cross-platform support.

**Option A — rejected: full re-read on each tick**

Read the full event log for every session on every poll tick. Correct but O(n × avg_events).
For 1000 sessions × 20 events each, this is ~20,000 JSON line parses per tick — untenable
at scale, and with no foundation for lazy detail loading.

**Option B — noted: described identically to C**

The original framing of Options B and C converged on the same implementation: an mtime
cache with a HashMap backing. The design adopts Option C's framing because naming the data
structure explicitly makes the diff logic clearer.

---

### D3: TUI Layout

Three panel arrangement strategies for the terminal UI:

**Option A — chosen: single scrollable list with expandable 7-row detail panel**

Default view: full-height session list. Pressing Enter on any row slides a 7-row detail
panel up from the bottom (showing gate type, command, exit code, timing, 2–3 evidence
entries). Pressing Escape or Enter again collapses it.

*Strengths*: all pixels go to the session list during monitoring — the primary use case.
At 80×24, the list gets ~20 rows; at 80×24 with detail open, the list retains ~13 rows
(enough for context). The two modes map directly to the two use cases: monitoring
(list-only) and investigation (list + detail).

*Weaknesses*: detail panel obscures some list rows. Mitigated by keeping the selected
row visible above the panel separator.

**Option B — rejected: vertical split (~55% list / ~45% detail)**

At 80 columns, a 55/45 split yields ~44 chars for the session list — not wide enough
to show name, state, elapsed, and task counts without heavy truncation. The always-visible
detail panel wastes space when no session is selected (monitoring mode). On 120+ column
terminals this becomes attractive; reserved as a V2 wide-terminal mode.

**Option C — rejected: tabbed layout**

Tab switching loses the session list entirely while investigating. A failing session's
siblings and coordinator row provide important context ("is this isolated or systemic?")
that tabs eliminate. The tab bar consumes a row and adds a navigation concept not present
in the j/k/Enter/Escape/q model.

---

### D4: Module Organization

Three locations for the dashboard implementation within the codebase:

**Option A (with sub-files) — chosen: three sibling files under `src/cli/`**

```
src/cli/
├── mod.rs               # Add Dashboard variant to Command enum
├── dashboard_data.rs    # Session reading, hierarchy, aggregate counts
├── dashboard_state.rs   # Expand/collapse tree, cursor position, selection
└── dashboard_render.rs  # ratatui widgets, layout, row rendering
```

*Strengths*: follows the `batch.rs`/`batch_view.rs` precedent exactly. The split maps
to testability boundaries: `dashboard_data.rs` and `dashboard_state.rs` have no PTY
dependency; only `dashboard_render.rs` requires ratatui's `TestBackend`. Placing it under
`src/cli/` signals "this is a CLI command", not a domain concept.

**Option B — rejected: `src/dashboard/` top-level module**

Top-level modules in koto (`engine/`, `template/`, `session/`) own domain logic consumed
by multiple callers. The dashboard is consumed only by the CLI command dispatcher. Elevating
it implies a public API that doesn't exist, and breaks the established pattern where CLI
commands live in `src/cli/`.

**Option C — rejected: inline in `src/cli/mod.rs`**

`src/cli/mod.rs` is already 4000 lines. Adding a full TUI event loop would push it past
5000 and mix unrelated concerns. The codebase already avoids this: `batch.rs` (4484 lines),
`next.rs` (814 lines), and `batch_view.rs` (688 lines) each live in separate files.

## Decision Outcome

The four decisions compose without conflict:

The **single-threaded tick loop** (D1) owns the main thread throughout the dashboard's
lifetime. It drives the **mtime-based HashMap refresh** (D2) on every Nth tick — no
shared state, no locks needed because D1 is single-threaded. The refreshed session map
feeds the **application state** (D4: `dashboard_state.rs`), which the **rendering layer**
(D4: `dashboard_render.rs`) reads on every draw. User input — cursor movement, expand,
collapse, detail toggle — is handled by the state layer between ticks.

The **modal detail panel** (D3) determines which data the detail view requests. When the
user presses Enter, `dashboard_state.rs` sets `view_mode = ViewMode::Detail` and
`focused_session_id`. On the next render tick, `dashboard_render.rs` passes that ID to
`dashboard_data.rs`, which calls `persistence::read_events` and
`derive_last_gate_evaluated` on demand — the only time a full event read occurs outside
the poll cycle. This lazy read keeps startup fast: initial scan reads only headers and
derives state from the log tail; full event history loads only when inspected.

Terminal state detection — distinguishing "done" from "running" — relies on
`derive_machine_state` loading the compiled template from `MachineState.template_path`.
This is the same mechanism `koto status` uses, so the dashboard and status command stay
consistent.

The `--once` flag bypasses D1, D2, and D3 entirely: it performs a single poll cycle,
derives all session states, and writes tab-separated output to stdout. No ratatui
initialization occurs.

## Solution Architecture

### Component Overview

```
koto dashboard
│
├── dashboard_data.rs      (data layer)
│   ├── scan_sessions()    -- enumerate session IDs via SessionBackend::list()
│   ├── stat_and_diff()    -- mtime check, detect adds/removes
│   ├── read_session()     -- read_header + read_events + derive_state_from_log
│   └── read_detail()      -- read_events + derive_last_gate_evaluated (on demand)
│
├── dashboard_state.rs     (app state layer)
│   ├── DashboardAppState  -- cursor, view_mode, expand_set, poll timer
│   ├── SessionTree        -- HashMap<id, CachedSession> + roots Vec
│   ├── handle_key()       -- j/k/Enter/Esc/q key dispatch
│   └── tick()             -- advance timer, trigger poll if interval elapsed
│
└── dashboard_render.rs    (render layer)
    ├── render_frame()     -- top-level ratatui Frame draw fn
    ├── render_list()      -- Table widget: session rows with indent/state/elapsed/tasks
    └── render_detail()    -- Block + Paragraph: gate type, command, exit, evidence
```

The three layers communicate through owned data (no shared references across layers):
`dashboard_data.rs` returns `SessionTree` updates, `dashboard_state.rs` merges them
into `DashboardAppState`, and `dashboard_render.rs` borrows `DashboardAppState` immutably
during each draw call.

### Key Data Structures

```rust
// dashboard_data.rs

struct CachedSession {
    header: StateFileHeader,            // workflow name, created_at, parent_workflow
    current_state: Option<String>,      // derived from event log
    is_terminal: bool,                  // loaded from compiled template
    mtime: std::time::SystemTime,       // for cache invalidation
    state_path: PathBuf,                // for re-reads
}

struct SessionTree {
    sessions: HashMap<String, CachedSession>,
    roots: Vec<String>,                 // session IDs with no parent_workflow
}

// dashboard_state.rs

enum ViewMode { List, Detail }

struct DashboardAppState {
    tree: SessionTree,
    cursor_idx: usize,                  // index into flattened visible rows
    view_mode: ViewMode,
    focused_id: Option<String>,         // set when ViewMode::Detail
    expanded: HashSet<String>,          // session IDs with children shown
    should_quit: bool,
    tick_count: u32,
    poll_every_n_ticks: u32,
    detail_cache: Option<DetailData>,   // lazily loaded, cleared on focus change
}

struct DetailData {
    session_id: String,
    gate_type: String,
    command: Option<String>,
    result: String,
    elapsed: Duration,
    evidence: Vec<EvidenceEntry>,
}
```

### Event Loop

```rust
let tick_rate = Duration::from_millis(50);
let shutdown = /* AtomicBool from signal-hook */;
let mut state = DashboardAppState::new(poll_interval_ms);

loop {
    if crossterm::event::poll(tick_rate)? {
        match crossterm::event::read()? {
            Event::Key(key) => state.handle_key(key),
            Event::Resize(w, h) => state.handle_resize(w, h),
            _ => {}
        }
    }
    state.tick_count = state.tick_count.wrapping_add(1);
    if state.tick_count % state.poll_every_n_ticks == 0 {
        dashboard_data::refresh(&mut state.tree, backend)?;
    }
    terminal.draw(|f| dashboard_render::render_frame(f, &state))?;
    if shutdown.load(Ordering::Relaxed) || state.should_quit {
        break;
    }
}
```

### Session Hierarchy Construction

Sessions are grouped into a tree by `parent_workflow` header field:
- A session with no `parent_workflow` is a root.
- A session whose `parent_workflow` matches another session's `id` is a child of that session.
- Epoch-branched sessions (IDs containing `~`) are filtered from the main list by default.

The `roots` Vec is rebuilt whenever the session map changes (inserts or removes). Within
each parent, children are sorted: failed sessions first, then running, then pending/blocked,
then terminal. Coordinator rows (sessions with `>= 1` child) show aggregate task counts.

### `--once` Mode

When `--once` is passed, the run function:
1. Calls `dashboard_data::refresh_once(backend)` — a single stat+read pass, no caching.
2. Walks the flattened tree in display order.
3. Writes one tab-separated line per session to stdout:

```
<name>\t<current_state>\t<elapsed>\t<status_bucket>
```

Where `status_bucket` is one of: `running`, `done`, `failed`, `blocked`, `unknown`.

4. Exits with code 0.

No ratatui or crossterm initialization occurs in `--once` mode.

### Integration Points

| Integration | Location | Notes |
|---|---|---|
| `SessionBackend::list()` | `session/local.rs` | Returns `Vec<SessionInfo>` with ID + created_at |
| `persistence::read_header()` | `engine/persistence.rs` | First-line parse only; cheap for stat hits |
| `persistence::read_events()` | `engine/persistence.rs` | Full event log; used on mtime change |
| `derive_state_from_log()` | `engine/persistence.rs` | Returns `Option<String>` (current state name) |
| `derive_machine_state()` | `engine/persistence.rs` | Loads compiled template; detects terminal |
| `derive_last_gate_evaluated()` | `engine/persistence.rs` | Gate result for detail panel |
| `Command` enum | `cli/mod.rs` | Add `Dashboard(DashboardArgs)` variant |

### New Dependencies

Two crates are added to `Cargo.toml`:

| Crate | Version | Purpose |
|---|---|---|
| `ratatui` | `0.29` | TUI widget rendering (Table, Paragraph, Block, Layout, Scrollbar) |
| `crossterm` | `0.28` | Cross-platform terminal: raw mode, alternate screen, event poll/read |

Both are pure Rust with no system library requirements. `crossterm` is already an indirect
dependency of `ratatui`, so the effective addition is one new direct dependency.

## Implementation Approach

### Phase 1: Scaffolding

Add `Dashboard(DashboardArgs)` to the `Command` enum in `src/cli/mod.rs`. Add the three
new module files with empty pub functions:

- `src/cli/dashboard_data.rs` — `pub fn refresh(tree: &mut SessionTree, backend: &dyn SessionBackend) -> anyhow::Result<()>`
- `src/cli/dashboard_state.rs` — `pub struct DashboardAppState` with all fields
- `src/cli/dashboard_render.rs` — `pub fn render_frame(f: &mut Frame, state: &DashboardAppState)`

Add `ratatui` and `crossterm` to `Cargo.toml`. Verify `cargo build` succeeds before
writing any logic.

### Phase 2: Data Layer

Implement `dashboard_data.rs` fully:

1. `scan_sessions(backend)` — calls `backend.list()`, returns a `Vec<(String, PathBuf)>`
   of (session_id, state_file_path) pairs, filtering out epoch-branched names (containing `~`).
2. `stat_and_diff(tree, session_paths)` — compute adds (IDs in scan but not in HashMap),
   removes (IDs in HashMap but not in scan), and existing (to stat for mtime change).
3. `read_session(path)` — calls `read_header`, `read_events`, `derive_state_from_log`,
   and `derive_machine_state`. Returns a `CachedSession`. Treats any parse error as
   `current_state = None`, `is_terminal = false`.
4. `refresh(tree, backend)` — orchestrates the diff and selective re-reads, rebuilds
   `roots` if the session set changed.
5. `read_detail(path)` — calls `read_events` + `derive_last_gate_evaluated`. Returns
   `DetailData`. Called on demand, not cached in the poll cycle.

Unit tests in `#[cfg(test)]` at the bottom of the file: test `scan_sessions` with a
mock file tree, test `stat_and_diff` logic with synthetic mtimes, test `read_session`
with fixture JSONL files (reuse test fixtures from `persistence.rs` tests).

### Phase 3: Application State Layer

Implement `dashboard_state.rs`:

1. `DashboardAppState::new(poll_interval_ms)` — initializes all fields; computes
   `poll_every_n_ticks = max(1, poll_interval_ms / 50)`.
2. `handle_key(key)` — maps crossterm `KeyCode` to state mutations:
   - `j` / `Down` → increment cursor (capped at visible row count - 1)
   - `k` / `Up` → decrement cursor (floored at 0)
   - `Enter` → if `ViewMode::List`: set `ViewMode::Detail`, `focused_id`, load detail.
     If `ViewMode::Detail` and session has children: toggle expand/collapse.
   - `Escape` → set `ViewMode::List`, clear `focused_id`, clear `detail_cache`.
   - `q` / `Ctrl+C` → set `should_quit = true`.
   - `r` → force refresh on next tick (set `tick_count` to trigger poll).
3. `visible_rows()` — returns a `Vec<RowDescriptor>` in display order: roots first, then
   children if the parent is in `expanded`, recursively. Each `RowDescriptor` carries
   indent depth, session ID, display name, state, elapsed, and task counts.
4. `handle_resize(w, h)` — update terminal size in state (used by render layer for
   truncation decisions).

Unit tests: test cursor movement, expand/collapse toggle, detail mode transitions,
`visible_rows` ordering with a synthetic `SessionTree`.

### Phase 4: Render Layer

Implement `dashboard_render.rs`:

1. `render_frame(f, state)` — top-level function called by `terminal.draw()`. Decides
   layout: if `ViewMode::List`, one full-height block. If `ViewMode::Detail`, vertical
   split: `Constraint::Min(0)` for the list, `Constraint::Length(8)` for the detail panel.
2. `render_list(f, area, state)` — builds a ratatui `Table` from `state.visible_rows()`.
   Columns: `Name` (left-aligned, fills remaining width), `State` (12 chars), `Elapsed`
   (9 chars), `Tasks` (10 chars, blank for leaf sessions). Highlights the cursor row.
   Attaches a `Scrollbar` when row count exceeds visible area.
3. `render_detail(f, area, state)` — renders the detail panel as a titled `Block`
   containing a `Paragraph`. Title: ` <session-name>: detail `. Content: gate type,
   command (if `command_gate`), result (PASS/FAIL), elapsed since gate evaluation, and
   2–3 evidence entries newest-first. If no detail data loaded: shows "Loading…".

The render layer never calls persistence functions directly. It reads only from
`DashboardAppState`.

### Phase 5: Entry Point and `--once` Mode

In `src/cli/mod.rs`, add the `Dashboard` command dispatch to the `run()` function:

```rust
Command::Dashboard(args) => dashboard_data::run(args, &backend),
```

Implement the entry point in `dashboard_data.rs` (or a thin `dashboard.rs` file):

1. If `args.once`: call `refresh_once(backend)`, format rows as tab-separated, print,
   return.
2. Otherwise: set up crossterm raw mode + alternate screen (RAII guard for cleanup),
   create `Terminal`, register `signal-hook` SIGINT/SIGTERM handler, enter the tick loop.
   On loop exit: clean up raw mode + alternate screen.

The RAII guard must restore terminal state even if the tick loop returns an `Err`.

### Phase 6: Tests and CI

Add integration tests under `test/` (following the existing Gherkin + fixture pattern):

1. `--once` output format: invoke binary with a fixture session directory, assert
   tab-separated output matches expected rows.
2. `Dashboard` command recognition: assert `koto dashboard --help` exits 0.

Add at least one render-layer unit test using ratatui's `TestBackend`:
```rust
let backend = TestBackend::new(80, 24);
let mut terminal = Terminal::new(backend)?;
terminal.draw(|f| render_frame(f, &mock_state))?;
let buffer = terminal.backend().buffer().clone();
// assert cell content at known positions
```

## Security Considerations

**File system access**: the dashboard reads from `~/.koto/sessions/<repo-id>/` — a
directory owned and writable only by the running user (mode `0700`, enforced by
`ensure_koto_root()` in `session/local.rs`). The dashboard opens files with `File::open`
(read-only). No writes to the session directory occur.

**Path traversal**: session IDs are validated by `validate_session_id()` in
`session/local.rs` before use as path components. The dashboard reads from paths
constructed by `SessionBackend::list()` and `session_dir()`, not from user-supplied
strings, so path traversal is not a concern for the list view. The `--name` argument
is passed through `validate_session_id()` before constructing any path.

**Terminal state restoration**: raw mode and alternate screen must be restored on all
exit paths — normal return, `?`-propagated errors, and SIGINT/SIGTERM. A RAII guard
wrapping `disable_raw_mode()` and `execute!(stdout, LeaveAlternateScreen)` handles this.
Without it, a panic or early error would leave the user's terminal in raw mode.

**No network access**: the dashboard reads only local files. It does not initiate any
network connections. The `rust-s3` dependency used by `CloudBackend` is not invoked.

**Binary injection**: evidence entries and session names are user-controlled strings
displayed via ratatui's `Paragraph` widget. ratatui renders to a cell buffer before
writing to the terminal; it does not interpret escape sequences in content strings.
Malicious content in a session name or evidence entry cannot inject terminal escape
sequences.

**Dependency surface**: adding `ratatui` and `crossterm` increases the dependency tree.
Both are widely used, pure-Rust crates with no unsafe I/O beyond the terminal operations
they're explicitly designed for. Their transitive dependencies should be reviewed on
addition and pinned in `Cargo.lock`.

**Signal handling**: `signal-hook` installs handlers for SIGINT and SIGTERM that set an
`AtomicBool`. The dashboard checks this flag at the bottom of each tick and exits cleanly.
No custom signal handlers are registered beyond the existing koto pattern.

## Consequences

### Positive

- Users monitoring batch pipelines with 100+ sessions gain a live, hierarchical view
  without scripting.
- The data and state layers are fully testable without a PTY; only the render layer
  requires `TestBackend`.
- Reusing `derive_machine_state`, `derive_state_from_log`, and `derive_last_gate_evaluated`
  ensures the dashboard shows the same state as `koto status`, with no divergence risk.
- The `--once` flag makes the dashboard composable in shell scripts and CI pipelines.
- The three-file split under `src/cli/` follows established precedent and keeps the
  implementation findable without structural exceptions.
- No daemon, no background process, no IPC: the dashboard is just another CLI command
  with a clean exit.

### Negative

- Two new direct dependencies (`ratatui`, `crossterm`) increase binary size and build
  time. `ratatui` 0.29 + `crossterm` 0.28 add approximately 0.5MB to the release binary
  and ~15s to a clean build on a mid-range workstation.
- The tick loop runs at 20 Hz regardless of whether the user is actively watching. During
  long idle periods this performs ~2 `stat()` calls per second (one per 10th tick) with
  no net benefit. A future improvement would add an adaptive idle slowdown.
- The detail panel height is fixed at 7 rows. Sessions with many evidence entries require
  a separate scroll mechanism (or a "N more" indicator) to surface all entries.
- Terminal state restoration requires careful RAII discipline. Any code path that returns
  before the guard drops will leave the terminal in raw mode. Code reviewers should watch
  for early returns in the entry point function.

### Mitigations

- Binary size growth is acceptable given the user value. The `lto = true` and `strip = true`
  release profile settings already minimize size; the `ratatui` addition fits within this
  budget.
- The `--once` flag provides a zero-TUI fallback for users in non-interactive environments
  (CI, scripts, or terminals that don't support alternate screen).
- The fixed detail panel height is documented as a V2 enhancement; the initial
  implementation adds a `↓ N more` indicator when evidence count exceeds 3.
- The RAII cleanup guard pattern is the standard approach in ratatui applications; the
  implementation should follow the ratatui template verbatim.
