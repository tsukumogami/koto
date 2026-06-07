---
status: Implemented
upstream: docs/prds/PRD-local-dashboard.md
problem: |
  koto had no live visibility surface. The engine writes session state to JSONL
  files at ~/.koto/sessions/ on every advance, but reading that state required
  raw JSON parsing or per-session koto status invocations. For users monitoring
  100–1000 parallel child sessions in a batch pipeline this forced manual polling
  loops or staying blind entirely.

  Beyond the missing TUI, a partial implementation revealed five broken behaviors:
  elapsed time was hardcoded to zero; the detail pane gate guard blocked
  evidence-only sessions from showing content; session discovery was scoped to the
  current working directory's repo hash, making it useless for monitoring parallel
  workflows across multiple workspaces; tree rendering had no connectors and was
  capped at one level deep; and the layout was a fixed 8-row vertical strip rather
  than the intended horizontal split. Three new capabilities — tabbed detail pane,
  session identity fields (intent, template_name), and global session scope — also
  needed architectural integration.
decision: |
  Add a koto dashboard subcommand backed by a ratatui TUI using a three-layer
  architecture: a data layer that reads session state from JSONL files; an
  application state layer that holds the session tree, cursor, expand/collapse, and
  detail cache; and a rendering layer built on ratatui widgets. The event loop uses
  crossterm's synchronous poll/read API with a 50ms tick.

  Session storage moves to a flat ~/.koto/sessions/<name>/ namespace by removing
  the repo-id hash from LocalBackend, making the dashboard globally scope-aware
  without touching the SessionBackend trait. Intent becomes a first-class field set
  at koto init --intent and updatable mid-workflow via koto session update <name>
  --intent, with updates appended as IntentUpdated events to avoid read-modify-write
  races with a running engine. The always-visible horizontal split detail pane stays
  within the 200ms refresh budget using an mtime guard on the focused session. A
  one-time migration helper handles existing per-repo sessions on first startup.

  The --once flag bypasses the TUI entirely and writes 6 tab-separated columns to
  stdout for scripting.
rationale: |
  ratatui with crossterm is the only TUI option that stays fully synchronous —
  std::io and blocking-with-timeout event reads match koto's existing architecture
  without requiring tokio or async-std.

  Every chosen approach reuses an existing pattern. The mtime-based HashMap refresh
  makes I/O proportional to changed files, not total session count. Flat session
  storage reuses list() unchanged — no trait changes required. O_APPEND event
  appending for intent updates reuses append_event and avoids read-modify-write
  races given that normal koto next runs hold no file lock. The mtime guard on the
  detail pane mirrors the session list refresh, keeping the always-visible split
  within budget: at most one JSONL parse fires per tick regardless of how many
  sessions are visible. The #[serde(default, skip_serializing_if)] pattern on new
  schema fields mirrors every other optional field on StateFileHeader, ensuring
  existing sessions deserialize cleanly.

  The three-layer split (data / app state / render) keeps TUI rendering isolated
  from session logic, enabling unit tests of the data and state layers without a PTY.
---

# DESIGN: Local Dashboard

## Status

Implemented

> **Note:** Superseded in part by DESIGN-session-legibility (PR #166): the `--once` output now has 8 columns (adds `idle`, `liveness`) and the list is attention-ordered, not health-severity sorted.

## Context and Problem Statement

koto sessions accumulate state in JSONL files — one file per session, written
atomically on every advance. The engine provides functions to derive current state
from these files (`derive_machine_state`, `derive_state_from_log`,
`derive_last_gate_evaluated` in `src/engine/persistence.rs`), and `koto status`
uses them for a one-shot JSON snapshot. But there was no continuous view: users
monitoring a batch run with 100+ parallel child sessions had to script their own
polling or re-invoke `koto status` repeatedly.

The core technical challenge is building a live terminal UI on top of a purely
synchronous codebase. koto has no async runtime (no tokio, no async-std). The
event loop must interleave keyboard input and file polling using only synchronous
primitives: a blocking-with-timeout event read from crossterm combined with a
wall-clock tick timer.

Session hierarchy adds another layer of complexity. Session files have no index;
hierarchy is reconstructed by reading all sessions and grouping by their
`parent_workflow` header field. For 100 sessions this requires 100 file header
reads per poll cycle. The data layer must be efficient enough to keep the UI
responsive while keeping I/O proportional to the number of changed files rather
than total session count.

A partial implementation exposed five broken behaviors before the dashboard was
complete: elapsed time hardcoded to zero, a gate guard in the detail pane that
blocked evidence-only sessions, session discovery scoped to the current working
directory's repo hash, shallow tree rendering with no connectors, and a fixed
8-row vertical strip rather than the intended horizontal split. These were
repaired alongside three new capabilities: a tabbed detail pane, session identity
fields (intent, template_name), and global session scope.

## Decision Drivers

- **No async runtime**: the implementation must stay fully synchronous;
  crossterm's `poll()` with a timeout is the only viable event-loop primitive
- **Single binary**: the dashboard extends the existing `koto` binary; no new
  binaries or background processes
- **Startup performance**: ≤1s for repo-wide with 100 sessions; ≤100ms for a
  focused single-session view
- **Reuse existing persistence layer**: `derive_machine_state` and
  `derive_last_gate_evaluated` already implement terminal detection and gate
  result computation; the dashboard must not duplicate this logic
- **Testability without PTY**: the data layer and application state layer must be
  testable without a real terminal; only the rendering layer requires
  `TestBackend`
- **Additive schema safety**: all changes to `StateFileHeader` and
  `EvidenceSubmitted` must follow `#[serde(default, skip_serializing_if =
  "Option::is_none")]`; existing sessions must deserialize cleanly
- **Concurrent write safety**: `koto session update --intent` writes to a state
  file the engine may be reading and writing concurrently; the strategy must not
  corrupt the state file or event log
- **Refresh performance**: a full refresh cycle must complete under 200ms for a
  session with 500 events; the always-visible horizontal split means detail data
  may need to load on every cursor move
- **Terminal width adaptation**: three layout modes — ≥80 columns horizontal
  split, <80 columns list-only, <40 columns "terminal too narrow" message
- **Graceful degradation**: truncated JSONL files, missing compiled templates, and
  unknown event types must not crash the dashboard
- **V2 daemon compatibility**: `koto dashboard` as a top-level command leaves room
  for `koto daemon start/stop` in V2 without namespace collision
- **Global scope as cloud foundation**: session discovery must be
  working-directory-independent so the same scope model extends to cloud storage
  without redesigning discovery

## Considered Options

### D1: Event Loop Architecture

**Option A — chosen: single-threaded tick loop with `crossterm::event::poll(50ms)`**

A single `loop { poll(...); tick++; if tick%N { poll_files() }; draw() }` structure.
`crossterm::event::poll(Duration::from_millis(50))` parks the OS thread for up to
50ms, waking immediately when a key event arrives. File polling runs every N ticks.
No concurrency, no shared state, no locks.

Input latency is ≤50ms with zero busy-waiting. `signal-hook`'s `AtomicBool` is
checked after each `poll()` for clean shutdown. crossterm's `poll` is
cross-platform (Linux, macOS, Windows). The one risk is that file I/O blocking the
tick thread slips render cadence, but a 1000-session scan via `stat()` is <10ms on
warm cache, well within budget.

**Option B — rejected: two threads with mpsc channel**

A background thread calls `crossterm::event::read()` (blocking, no timeout) and
sends events to the main thread via a channel. Fatal flaw: shutdown deadlock. When
the user presses Ctrl+C, `signal-hook` sets the `AtomicBool`, but the keyboard
thread is blocked in `read()` with no timeout and no way to be interrupted without
platform-unsafe tricks. Also forces all shared state behind `Arc<Mutex<_>>`.

**Option C — rejected: non-blocking `try_read()` with explicit sleep**

`crossterm::event::poll(Duration::ZERO)` + `read()` followed by
`std::thread::sleep(50ms)`. Structurally identical to Option A but with
unconditional 50ms wakeups regardless of pending events. No scenario where this is
preferable.

---

### D2: Session State Model

**Chosen: `HashMap<String, CachedSession>` with mtime-based incremental diff**

An in-memory map keyed by session ID. On each poll cycle: stat all session files
(~1–5µs per file on warm cache), remove entries for sessions no longer on disk,
re-read only files whose `mtime` has advanced. The cache entry stores the parsed
header, derived current state, derived intent, and the mtime used for validation.

Full event history is deferred: only loaded on demand for the focused detail panel,
using a second mtime guard on `detail_cache_mtime` and `detail_cache_session`. This
keeps startup fast and caps the per-tick I/O at one JSONL parse regardless of how
many sessions are visible.

**Rejected: full re-read on each tick**

Read the full event log for every session on every poll tick. Correct but O(n ×
avg_events). For 1000 sessions × 20 events each, ~20,000 JSON line parses per tick.
No foundation for lazy detail loading.

**Rejected: eager load all visible sessions for detail**

Calls `read_detail()` for every session in `visible_rows()` on each tick. At 15
sessions × ~10ms per parse ≈ 150ms, this approaches the 200ms budget before
accounting for list refresh. At global scope it breaches the budget.

---

### D3: TUI Layout

**Option A — rejected: expandable 7-row bottom detail panel**

A full-height session list with a detail panel that slides up from the bottom on
Enter. The original design choice, but it was not the implementation target — the
intended design called for an always-visible horizontal split, and the expandable
strip was a step toward that.

**Chosen: 40%/60% horizontal split with width-adaptive modes**

At ≥80 columns: `Constraint::Percentage(40)` session list / `Constraint::Percentage(60)`
detail pane, both always visible. Below 80 columns: list-only. Below 40 columns:
"terminal too narrow" message. The split makes session context and detail visible
simultaneously — no mode-switching needed for investigation. Width adaptation
handles narrow terminals without panics or widget overflow.

**Rejected: tabbed list/detail layout (top-level tabs)**

Tab switching loses the session list entirely while investigating. A failing
session's siblings and coordinator row provide important context — "is this
isolated or systemic?" — that top-level tabs eliminate. The tabbed concept is
retained but scoped to the detail pane only (Summary / History / Remaining tabs).

---

### D4: Session Discovery Scope

**Chosen: Remove repo-id scoping entirely**

`LocalBackend::new()` always sets `base_dir = ~/.koto/sessions/` with no repo-id
segment. Every session occupies `~/.koto/sessions/<session-name>/`. Both engine
commands and the dashboard use the same backend, the same directory, and the same
`list()` implementation — no trait changes required.

A one-time migration helper runs on the first startup where the old per-repo layout
is detected (presence of `~/.koto/sessions/<16-hex-chars>/` subdirectories). The
helper moves each session directory up one level. Sessions with naming collisions
across repos are left in place under their old repo-id path with a warning printed
to stderr. `CloudBackend` retains its repo-id S3 prefix until cloud scope work.

**Rejected: scope parameter on `LocalBackend`** (`scope: Option<RepoId>`)

A global base dir `~/.koto/sessions/` contains repo-id subdirs, not session
subdirs when using per-repo storage — the topology is incompatible between scopes.
`list()` would need to recurse an extra level for the global case, making the
implementation contract silently inconsistent.

**Rejected: dashboard-specific global constructor** (`LocalBackend::global()`)

The engine's sessions still live under repo-id subdirs under the old layout, so
the global constructor sees nothing without also changing where sessions are stored.
Sub-variant with flat storage plus a separate constructor is structurally identical
to the chosen option with unnecessary duplication.

**Rejected: config-driven scope** (`session.scope = "global" | "repo"`)

Makes a structural concern user-configurable, adds a config key that the dashboard
always wants as `"global"`, and doesn't resolve the topology mismatch if storage
stays per-repo.

---

### D5: Concurrent Mutation of Session Intent

**Chosen: Append an `IntentUpdated` event to the JSONL log**

`koto session update <name> --intent "<text>"` appends an `intent_updated` event to
the session's JSONL log using the same `O_APPEND` path that `append_event` already
uses. Readers derive current intent by finding the last `IntentUpdated` payload in
the event log; absent that, intent falls back to the `StateFileHeader.intent` field
(set at `koto init --intent` time). The dashboard's `refresh()` caches the derived
value in `CachedSession.intent`, re-deriving only when the state file's mtime
changes.

The advisory `flock(LOCK_EX | LOCK_NB)` that `SessionBackend::lock_state_file`
provides is only acquired for batch-scoped states. Normal `koto next` runs hold no
lock. Two concurrent `O_APPEND` writers produce a correctly interleaved log without
corruption (POSIX guarantees atomicity for writes well under the kernel limit; each
JSON event line is under 4 KB).

**Rejected: read-modify-write under the existing session lock**

Non-batch `koto next` holds no lock, so the lock doesn't exclude the common
concurrent-write case. An engine append between the read and the write is silently
discarded when the rewritten file wins.

**Rejected: sidecar metadata file**

Concurrent-safe (the engine never touches the sidecar), but requires every reader
to know about a second file type and splits session metadata across two files. The
log-event approach achieves the same safety with less structural complexity.

**Rejected: full state file rewrite with atomic rename**

An engine process that opened the file with `O_APPEND` before the rename continues
writing to the old inode. Events appended after the read but before the rename are
silently discarded — the worst option for correctness.

---

### D6: Module Organization

**Chosen: four sibling files under `src/cli/`**

```
src/cli/
├── mod.rs               # Dashboard variant in Command enum
├── dashboard.rs         # Event loop, --once mode
├── dashboard_data.rs    # Session reading, hierarchy, aggregate counts
├── dashboard_state.rs   # Session tree, visible_rows, expand/collapse, detail cache
└── dashboard_render.rs  # ratatui widgets, layout, tab rendering
```

This follows the `batch.rs`/`batch_view.rs` precedent. The split maps to
testability boundaries: `dashboard_data.rs` and `dashboard_state.rs` have no PTY
dependency; only `dashboard_render.rs` requires ratatui's `TestBackend`. Placing
everything under `src/cli/` signals "this is a CLI command", not a domain concept.

**Rejected: `src/dashboard/` top-level module**

Top-level modules in koto (`engine/`, `template/`, `session/`) own domain logic
consumed by multiple callers. The dashboard is consumed only by the CLI command
dispatcher. Elevating it implies a public API that doesn't exist.

**Rejected: inline in `src/cli/mod.rs`**

`src/cli/mod.rs` is already 4000+ lines. Adding a full TUI event loop would push
it past 5000 and mix unrelated concerns.

## Decision Outcome

The six decisions compose without conflict.

The **single-threaded tick loop** (D1) owns the main thread throughout the
dashboard's lifetime. It drives the **mtime-based HashMap refresh** (D2) on every
Nth tick — no shared state, no locks needed. The refreshed session map feeds the
**application state** (D6: `dashboard_state.rs`), which the **rendering layer**
(D6: `dashboard_render.rs`) reads on every draw. User input — cursor movement,
expand, collapse, tab switching — is handled by the state layer between ticks.

The **40%/60% horizontal split** (D3) makes the detail pane always visible, so the
mtime-guarded detail load (D2) runs on every tick for the focused session rather
than on demand. The tick handler compares focused session mtime against
`detail_cache_mtime` and `focused_id` against `detail_cache_session`; it calls
`read_detail()` only when either changes.

**Flat storage** (D4) makes `list()` scan `~/.koto/sessions/` directly, giving the
dashboard global scope without any change to the `SessionBackend` trait or the
`list()` implementation. The startup migration helper handles existing per-repo
sessions once and prints a summary on the first run after upgrade.

**`IntentUpdated` event appending** (D5) means intent freshness is handled by the
same mtime guard that already governs session list refreshes — no extra I/O beyond
what the list refresh already performs.

The `--once` flag bypasses D1, D2, and D3 entirely: a single stat+read pass,
walking the flattened tree in display order, writing 6 tab-separated columns to
stdout.

## Solution Architecture

### Component Overview

```
koto dashboard
│
├── src/session/local.rs
│   ├── LocalBackend::new()      -- flat base_dir = ~/.koto/sessions/
│   └── migrate_if_needed()      -- one-time per-repo → flat migration
│
├── src/event/mod.rs
│   ├── EventPayload::IntentUpdated { intent }
│   └── derive_intent(events)    -- last IntentUpdated value, or None
│
├── src/cli/session.rs
│   └── handle_update()          -- append IntentUpdated event
│
├── src/cli/dashboard_data.rs    (data layer)
│   ├── scan_sessions()          -- enumerate via SessionBackend::list()
│   ├── stat_and_diff()          -- mtime check, detect adds/removes
│   ├── read_session()           -- header + events + derive_state_from_log
│   └── read_detail()            -- full JSONL parse on demand; no gate guard
│
├── src/cli/dashboard_state.rs   (app state layer)
│   ├── DashboardAppState        -- cursor, view_mode, expand_set, cache fields
│   ├── SessionTree              -- HashMap<id, CachedSession> + roots Vec
│   ├── visible_rows()           -- arbitrary depth, ├─/└─ connectors, severity sort
│   ├── handle_key()             -- j/k/Tab/Enter/Esc/q/r/←/→ dispatch
│   └── tick()                   -- advance timer, trigger poll
│
└── src/cli/dashboard_render.rs  (render layer)
    ├── render_frame()           -- width-adaptive layout dispatch
    ├── render_list()            -- Table with connectors, state, elapsed, task counts
    └── render_detail()          -- Tabs widget: Summary / History / Remaining
```

### Key Data Structures

```rust
// src/cli/dashboard_data.rs

struct CachedSession {
    header: StateFileHeader,            // workflow name, created_at, parent_workflow
    current_state: Option<String>,      // derived from event log
    is_terminal: bool,                  // loaded from compiled template
    intent: Option<String>,             // derived from last IntentUpdated event
    mtime: std::time::SystemTime,       // for cache invalidation
    state_path: PathBuf,                // for re-reads
}

struct SessionTree {
    sessions: HashMap<String, CachedSession>,
    roots: Vec<String>,                 // session IDs with no parent_workflow
}

// src/cli/dashboard_state.rs

struct TaskCounts {
    total: u32,
    running: u32,
    done: u32,
    failed: u32,
    blocked: u32,
    done_blocked: u32,
}

struct VisibleRow {
    depth: usize,
    connector: String,                  // "├─ ", "└─ ", "  ├─ " etc.
    session_id: String,
    display_name: String,
    state: Option<String>,
    elapsed: Duration,
    counts: TaskCounts,
}

enum ViewMode { List, Detail }

struct DashboardAppState {
    tree: SessionTree,
    cursor_idx: usize,                  // index into flattened visible rows
    view_mode: ViewMode,
    focused_id: Option<String>,         // maintained in both List and Detail modes
    active_tab: usize,                  // 0=Summary, 1=History, 2=Remaining
    expanded: HashSet<String>,          // session IDs with children shown
    should_quit: bool,
    tick_count: u32,
    poll_every_n_ticks: u32,
    detail_cache: Option<SessionDetail>,
    detail_cache_mtime: Option<SystemTime>,
    detail_cache_session: Option<String>,
    terminal_width: u16,
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
        // mtime-guarded detail load
        if let Some(ref id) = state.focused_id {
            let session_mtime = state.tree.sessions.get(id).map(|s| s.mtime);
            if Some(id) != state.detail_cache_session.as_ref()
                || session_mtime > state.detail_cache_mtime
            {
                state.detail_cache = dashboard_data::read_detail(
                    &state.tree.sessions[id].state_path, id
                );
                state.detail_cache_mtime = session_mtime;
                state.detail_cache_session = Some(id.clone());
            }
        }
    }
    terminal.draw(|f| dashboard_render::render_frame(f, &state))?;
    if shutdown.load(Ordering::Relaxed) || state.should_quit {
        break;
    }
}
```

### Session Hierarchy Construction

Sessions are grouped into a tree by `parent_workflow` header field. A session with
no `parent_workflow` is a root; a session whose `parent_workflow` matches another
session's `id` is a child of that session. Epoch-branched sessions (IDs containing
`~`) are filtered from the main list by default.

Within each parent, children are sorted by a `session_sort_key` helper that maps
health status to a numeric severity bucket (failed=0, blocked=1, running=2,
unknown=3, done=4) and uses most-recent mtime as a tiebreaker within each bucket.
`visible_rows()` traverses the tree to arbitrary depth, computing the connector
string from the child's position in its parent's child list.

### Layout Modes

| Terminal width | Layout |
|---|---|
| ≥ 80 columns | 40% session list / 60% tabbed detail pane |
| < 80 columns | Session list only |
| < 40 columns | "terminal too narrow" message |

The layout switches on `Event::Resize` without panics or widget overflow.

### Tabbed Detail Pane

The detail pane renders a ratatui `Tabs` widget with three tabs, cycled by the Tab
key:

- **Summary**: current state, directive from `CompiledState`, latest evidence, gate
  result, intent, template_name
- **History**: full chronological event log covering 10 event types
  (`StateAdvanced`, `GateEvaluated`, `EvidenceSubmitted`, `DecisionRecorded`,
  `DirectedTransition`, `Rewound`, `GateOverrideRecorded`, `ContextAdded`,
  `DefaultActionExecuted`, `IntentUpdated`); scrollable; gate condition text from
  the compiled template (no additional disk reads beyond what the Remaining tab
  already loads)
- **Remaining**: unvisited states in topological order from the compiled template

### `--once` Mode

When `--once` is passed, the run function performs a single stat+read pass with no
caching, walks the flattened tree in display order, and writes one tab-separated
line per session to stdout:

```
<session_id>\t<state>\t<elapsed>\t<status>\t<intent>\t<template_name>
```

Where `status` is one of: `running`, `done`, `failed`, `blocked`, `unknown`.
Columns 1–4 match the original format; columns 5–6 are empty strings when the
fields are absent. No ratatui or crossterm initialization occurs in `--once` mode.

### Integration Points

| Integration | Location | Notes |
|---|---|---|
| `SessionBackend::list()` | `session/local.rs` | Scans flat `~/.koto/sessions/` |
| `persistence::read_header()` | `engine/persistence.rs` | First-line parse; cheap for stat hits |
| `persistence::read_events()` | `engine/persistence.rs` | Full event log; used on mtime change |
| `derive_state_from_log()` | `engine/persistence.rs` | Returns `Option<String>` (current state) |
| `derive_machine_state()` | `engine/persistence.rs` | Loads compiled template; detects terminal |
| `derive_last_gate_evaluated()` | `engine/persistence.rs` | Gate result for detail panel |
| `derive_intent()` | `event/mod.rs` | Last `IntentUpdated.intent`, or `None` |
| `Command` enum | `cli/mod.rs` | `Dashboard(DashboardArgs)` variant |

### New Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `ratatui` | `0.29` | TUI widget rendering (Table, Paragraph, Block, Layout, Tabs, Scrollbar) |
| `crossterm` | `0.28` | Cross-platform terminal: raw mode, alternate screen, event poll/read |

Both are pure Rust with no system library requirements. `crossterm` is already an
indirect dependency of `ratatui`, so the effective addition is one new direct
dependency.

## Implementation Approach

### Phase 1: Storage, Schema, and Identity

Lay the foundation with changes that are independently deployable and backward
compatible. No dashboard UI changes in this phase.

- Remove `repo_id` from `LocalBackend::new()` in `src/session/local.rs`
- Add `migrate_if_needed()` startup check: detects 16-hex-char subdirectories,
  moves session directories up one level, warns on collisions
- Add `intent: Option<String>` and `template_name: Option<String>` to
  `StateFileHeader` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- Add `IntentUpdated { intent: String }` to `EventPayload`
- Add `derive_intent(events: &[Event]) -> Option<String>`
- Add `koto session update <name> --intent` subcommand; reject inputs over 1024
  characters with a clear error before appending
- Handle `IntentUpdated` as a recognized no-op in the engine advance loop
- Tests: unit tests for `derive_intent`, migration helper, intent round-trip, and
  the 1024-char limit boundary

### Phase 2: Data Layer Correctness

Fix computation bugs and remove the gate guard. No layout changes.

- Fix elapsed: compute from `StateInitialized` event timestamp using the existing
  `compute_elapsed_since()` rather than hardcoding zero
- Remove gate guard from `read_detail()` — always return `SessionDetail`
- Add `blocked` and `done_blocked` to `TaskCounts`
- Add `CachedSession.intent` field; populate via `derive_intent()` in `refresh()`
  for mtime-changed sessions
- Extend `--once` output to 6 columns (add intent, template_name)
- Tests: unit tests for elapsed computation, `read_detail()` on evidence-only
  sessions, `--once` 6-column output format

### Phase 3: Tree, Ordering, and Detail Cache

Fix tree rendering and wire the mtime-guarded detail cache before touching the layout.

- Extend `visible_rows()` to arbitrary depth
- Add `VisibleRow.connector` field; compute `├─`/`└─` strings from child position
  in parent's child list
- Add `session_sort_key()` helper; replace alphabetical sort with health-severity
  ordering
- Add `detail_cache_mtime` and `detail_cache_session` to `DashboardAppState`
- Maintain `focused_id` in List mode cursor movement (not only when entering Detail
  mode)
- Move expand/collapse key bindings to List mode (`→`/`←` or `l`/`h`)
- Wire mtime-guarded detail load in the tick handler in `dashboard.rs`
- Tests: tree depth tests, connector string tests, ordering tests, cache
  invalidation tests

### Phase 4: Scaffolding (if building fresh)

Add `Dashboard(DashboardArgs)` to the `Command` enum. Add the four module files
with empty pub functions. Add `ratatui` and `crossterm` to `Cargo.toml`. Verify
`cargo build` succeeds before writing logic.

### Phase 5: Rendering and Layout

Replace the layout and build the tabbed detail pane.

- Replace `Constraint::Length(8)` with horizontal 40%/60% split in
  `dashboard_render.rs`
- Add width-gating: ≥80 → split, <80 → list-only, <40 → too-narrow message
- Add `Tabs` widget with Summary, History, Remaining tabs
- Implement each tab's content rendering
- History tab: render 10 event types with gate condition text from compiled
  template (already loaded for Remaining tab; no additional disk reads)
- Tests: render tests for each layout mode, tab switching, History tab gate
  condition display; at least one `TestBackend` test per layout mode

### Phase 6: Tests and CI

Add integration tests under `test/` following the existing Gherkin + fixture
pattern:

1. `--once` output format: invoke binary with a fixture session directory, assert
   6-column tab-separated output matches expected rows
2. `Dashboard` command recognition: assert `koto dashboard --help` exits 0

Add render-layer unit tests using ratatui's `TestBackend`:

```rust
let backend = TestBackend::new(80, 24);
let mut terminal = Terminal::new(backend)?;
terminal.draw(|f| render_frame(f, &mock_state))?;
let buffer = terminal.backend().buffer().clone();
// assert cell content at known positions
```

## Security Considerations

**File system access**: the dashboard reads from `~/.koto/sessions/` — a directory
owned and writable only by the running user (mode `0700`, enforced by
`ensure_koto_root()` in `session/local.rs`). The dashboard opens files with
`File::open` (read-only). No writes to the session directory occur during normal
dashboard operation.

**Path traversal**: session IDs are validated by `validate_session_id()` in
`session/local.rs` before use as path components. The dashboard reads from paths
constructed by `SessionBackend::list()` and `session_dir()`, not from
user-supplied strings. The `--name` argument passes through `validate_session_id()`
before any path construction.

**Intent string length and `O_APPEND` atomicity**: `koto session update --intent`
uses `O_APPEND` to avoid a read-modify-write race with a running engine. POSIX
guarantees `O_APPEND` writes are atomic only up to `PIPE_BUF` bytes (typically
4096 on Linux). Intent strings are expected to be short in practice, but the design
imposes an explicit 1024-character maximum in the `handle_update` CLI handler
before the append. Inputs exceeding this limit are rejected with a clear error
message rather than truncated silently.

**Session namespace collision after migration**: removing repo-id scoping merges
all project sessions into a single flat namespace. Two projects using the same
session name will collide. The migration helper warns on collision and leaves the
conflicting session in place. Users should adopt project-prefixed session names
to avoid collisions; the dashboard's global view is intentional and documented.

**Terminal state restoration**: raw mode and alternate screen must be restored on
all exit paths — normal return, `?`-propagated errors, SIGINT, and SIGTERM. A RAII
guard wrapping `disable_raw_mode()` and `execute!(stdout, LeaveAlternateScreen)`
handles this. Without it, a panic or early error would leave the user's terminal in
raw mode.

**No network access**: the dashboard reads only local files. It does not initiate
any network connections.

**Binary injection**: evidence entries and session names are user-controlled strings
displayed via ratatui's `Paragraph` widget. ratatui renders to a cell buffer before
writing to the terminal; it does not interpret escape sequences in content strings.
Malicious content in a session name or evidence entry cannot inject terminal escape
sequences.

**Dashboard visibility is user-scoped**: state files are owned by the invoking user
(mode 0600). The dashboard surfaces session names, states, gate outcomes, evidence
fields, and intent strings — all already present in the state file. No data leaves
the machine. Intent strings may contain context users consider sensitive; the state
file is not encrypted and is readable by any process running as the same user.

**Signal handling**: `signal-hook` installs handlers for SIGINT and SIGTERM that
set an `AtomicBool`. The dashboard checks this flag at the bottom of each tick and
exits cleanly. No custom signal handlers are registered beyond the existing koto
pattern.

**Dependency surface**: adding `ratatui` and `crossterm` increases the dependency
tree. Both are widely used, pure-Rust crates with no unsafe I/O beyond the terminal
operations they're designed for. Their transitive dependencies should be reviewed
on addition and pinned in `Cargo.lock`.

## Consequences

### Positive

- Users monitoring batch pipelines with 100+ sessions gain a live, hierarchical
  view without scripting. Sessions from all local workspaces appear in a single
  list, sorted by health severity.
- Sessions carry `intent` and `template_name` labels. Operators can identify
  sessions at a glance without running `koto query`.
- Evidence-only sessions — the dominant pattern in shirabe-style workflows — show
  useful detail pane content. The gate guard that caused permanent "No gate
  evaluations recorded" messages is gone.
- Elapsed time, tree connectors, status rollup, and session ordering are all
  correct. The list view is actionable without supplementary CLI queries.
- The three-file split under `src/cli/` keeps the data and state layers fully
  testable without a PTY; only the render layer requires `TestBackend`.
- Reusing `derive_machine_state`, `derive_state_from_log`, and
  `derive_last_gate_evaluated` ensures the dashboard shows the same state as
  `koto status` with no divergence risk.
- The `--once` flag makes the dashboard composable in shell scripts and CI
  pipelines. The 6-column format is backwards compatible: columns 1–4 are
  unchanged.
- The `IntentUpdated` event pattern generalizes to future mutable metadata fields
  (`label`, `tag`, `owner`) — each becomes a new `*Updated` event type with a
  matching `derive_*` helper. No structural change needed.
- The flat session namespace is a natural fit for cloud storage extension. Sessions
  already live at `<base>/<session-name>/` with no repo-id intermediary.
- No daemon, no background process, no IPC: the dashboard is just another CLI
  command with a clean exit.

### Negative

- Two new direct dependencies (`ratatui`, `crossterm`) increase binary size and
  build time. `ratatui` 0.29 + `crossterm` 0.28 add approximately 0.5MB to the
  release binary and ~15s to a clean build on a mid-range workstation.
- Session name collisions across repos are now user-visible. Two repos both using
  `task_issue-1` share a session directory after migration.
- The detail pane performs a `stat()` call on every poll tick for the focused
  session, where before it did zero I/O until Detail mode was entered. A single
  `stat()` is under 1µs, so the overhead is negligible in practice.
- The one-time migration helper must be maintained until all existing installs
  migrate, then removed in a subsequent release.
- Intent is not readable from the header alone after a `koto session update
  --intent` call. The `koto session list` path (header-only reads) will not surface
  updated intent.
- The tick loop runs at 20 Hz regardless of whether the user is actively watching.
  A future improvement could add adaptive idle slowdown.

### Mitigations

- Binary size growth is acceptable given the user value. The `lto = true` and
  `strip = true` release profile settings already minimize size.
- The `--once` flag provides a zero-TUI fallback for users in non-interactive
  environments (CI, scripts, or terminals that don't support alternate screen).
- Name collision warnings from the migration helper tell the user exactly which
  sessions conflict and how to resolve them (rename one before running migration
  again).
- The migration helper is guarded by a cheap directory scan that adds negligible
  startup cost after migration completes.
- The RAII cleanup guard pattern is standard in ratatui applications; the
  implementation follows the ratatui template verbatim to avoid early-return
  pitfalls.
