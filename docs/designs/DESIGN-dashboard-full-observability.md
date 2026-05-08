---
status: Planned
upstream: docs/prds/PRD-dashboard-full-observability.md
problem: |
  The koto dashboard has a working scaffold — event loop, session tree, polling,
  --once output — but five areas are broken or missing. Elapsed time is hardcoded
  to zero. The detail pane gate guard blocks evidence-only sessions. Session
  discovery is scoped to the current working directory's repository hash, making
  it useless for monitoring parallel workflows across multiple workspaces. Tree
  rendering has no connectors and is capped at one level deep. The layout is a
  fixed 8-row vertical strip rather than the intended horizontal split. Three new
  capabilities (tabbed detail pane, session identity fields, global session scope)
  need architectural integration.
decision: |
  Five broken behaviors and three new capabilities are delivered through four
  coordinated changes. Session storage moves to a flat ~/.koto/sessions/<name>/
  namespace by removing the repo-id hash from LocalBackend::new(), making the
  dashboard globally scope-aware without touching the SessionBackend trait. Intent
  becomes a first-class field set at init and updatable mid-workflow via
  koto session update --intent, with updates appended as IntentUpdated events to
  avoid read-modify-write races with a running engine. The always-visible
  horizontal split detail pane stays within the 200ms refresh budget using an
  mtime guard on the focused session. A one-time migration helper handles
  existing per-repo sessions on first startup.
rationale: |
  Every chosen approach reuses an existing pattern: the flat namespace reuses
  list() unchanged, O_APPEND event appending reuses append_event, the mtime
  guard mirrors refresh(), and the new schema fields follow the
  serde(default, skip_serializing_if) pattern already on StateFileHeader. The
  rejected alternatives — scope parameters, read-modify-write header mutation,
  and eager session loading — were ruled out for creating inconsistent directory
  topologies, leaving race windows open under normal engine execution (which holds
  no file lock), and exceeding the 200ms refresh budget at global scope.
---

# DESIGN: koto dashboard — full observability surface

## Status

Planned

## Context and Problem Statement

The dashboard lives in four source files: `src/cli/dashboard.rs` (event loop,
`--once` mode), `src/cli/dashboard_state.rs` (session tree, `visible_rows`,
`TaskCounts`), `src/cli/dashboard_data.rs` (session scanning, event parsing,
detail data), and `src/cli/dashboard_render.rs` (ratatui widgets).

**Five confirmed broken areas in the current code:**

1. **Elapsed is hardcoded.** `visible_rows()` in `dashboard_state.rs` sets
   `elapsed: Duration::from_secs(0)` for every session. The `--once` path uses
   `session.mtime.elapsed()` (file modification time), not event timestamps.
   `compute_elapsed_since()` already parses ISO 8601 event timestamps correctly
   but is only used for gate evaluation timestamps, not for session elapsed time.

2. **Detail pane gate guard.** `read_detail()` in `dashboard_data.rs` returns
   `None` when no `GateEvaluated` event exists in the current epoch. Evidence-only
   sessions — the dominant pattern — permanently show "No gate evaluations recorded."
   The gate-presence guard must be removed and the detail pane made universal.

3. **Session discovery is repo-scoped.** `LocalBackend::new(working_dir)` hashes
   the working directory to derive `~/.koto/sessions/<repo-id>/` as the scan root.
   Sessions in other workspaces are invisible. The backend must scan globally across
   all local sessions regardless of cwd.

4. **Tree rendering is shallow and connector-free.** `visible_rows()` renders only
   depth 0 (roots) and depth 1 (direct children). The render layer uses space
   indentation only — no `├─`/`└─` connectors. `TaskCounts` lacks `blocked` and
   `done_blocked` fields, so status rollup cannot distinguish terminal-blocked
   children from terminal-done children.

5. **Layout is a vertical strip.** `dashboard_render.rs` uses
   `Constraint::Length(8)` for an 8-row detail strip at the bottom. The intended
   design — a horizontal 40%/60% split with both panels visible simultaneously —
   was never built.

**Three new capabilities needed:**

6. **Tabbed detail pane.** Summary tab (current state, directive from
   `CompiledState`, latest evidence, gate result, intent, template_name). History
   tab (full chronological event log, 10 event types, scrollable, with gate
   condition text from the compiled template). Remaining tab (unvisited states in
   topological order from the compiled template).

7. **Session identity fields.** `intent: Option<String>` and
   `template_name: Option<String>` on `StateFileHeader` using the established
   `#[serde(default, skip_serializing_if = "Option::is_none")]` pattern. Set via
   `koto init --intent "<text>"`. Updatable mid-workflow via
   `koto session update <name> --intent "<text>"`.

8. **`EvidenceSubmitted.summary`.** Optional free-text summary on the
   `EvidenceSubmitted` event payload, following the existing `submitter_cwd`
   pattern. Surfaced in the Summary tab above raw evidence fields.

## Decision Drivers

- **Additive schema safety.** All changes to `StateFileHeader` and
  `EvidenceSubmitted` must follow `#[serde(default, skip_serializing_if =
  "Option::is_none")]`; no schema version bump; existing sessions must
  deserialize cleanly.

- **Concurrent write safety.** `koto session update --intent` writes to a state
  file that the koto engine may be reading and writing concurrently. The mutation
  strategy must not corrupt the state file or the event log.

- **Refresh performance.** A full refresh cycle must complete under 200 ms for
  a session with 500 events. The always-visible horizontal split means detail data
  may need to load on every cursor move rather than only when entering Detail mode.

- **Terminal width adaptation.** Three layout modes: ≥80 columns horizontal split,
  <80 columns list-only, <40 columns "terminal too narrow" message. Layout must
  switch cleanly without panics or widget overflow.

- **Backwards compatibility.** Existing 4-column `--once` consumers continue to
  work. New columns 5–6 are additive. Old state files without new fields
  deserialize cleanly.

- **Compiled template reuse.** The compiled template (already loaded for the
  Remaining tab) provides gate condition text for the History tab. No additional
  disk reads should be required beyond what the Remaining tab already loads.

- **Global scope as F5 foundation.** Session discovery must be
  working-directory-independent so that the same scope model extends cleanly to
  cloud storage (F5) without redesigning discovery.

## Considered Options

### Decision 1: Session discovery scope

`LocalBackend::new(working_dir)` derives a SHA-256 repo-id hash from the
canonicalized working directory and scopes all session I/O to
`~/.koto/sessions/<repo-id>/`. This means `koto dashboard` sees only sessions
from the repo where it was launched — a developer monitoring parallel workflows
across a dozen niwa workspaces sees exactly one workspace's sessions.

The critical constraint is topology. The `SessionBackend` trait exposes `list()`,
`session_dir()`, and `cleanup()`, all of which assume that `base_dir` contains
session-named subdirectories directly. Any solution that retains repo-id
subdirectories under the global base would require `list()` to recurse
differently depending on scope — an inconsistency that silently surfaces as bugs
when the wrong topology is assumed.

#### Chosen: Remove repo-id scoping entirely

Change `LocalBackend::new()` to always set `base_dir = ~/.koto/sessions/` with
no repo-id segment. Every session occupies `~/.koto/sessions/<session-name>/`.
Both engine commands and the dashboard use the same backend, the same directory,
and the same `list()` implementation — no trait changes required.

A one-time migration helper runs on the first startup where the old per-repo
layout is detected (presence of `~/.koto/sessions/<16-hex-chars>/`
subdirectories). The helper moves each session directory up one level. Sessions
with naming collisions across repos are left in place under their old repo-id
path with a warning printed to stderr. `CloudBackend` retains its repo-id S3
prefix until F5.

#### Alternatives Considered

**Scope parameter on `LocalBackend`** (`scope: Option<RepoId>`): `LocalBackend`
accepts an optional flag; `None` means global, `Some(id)` means per-repo.
Rejected because a global base dir `~/.koto/sessions/` contains repo-id subdirs,
not session subdirs — the topology is incompatible between scopes. `list()` would
need to recurse an extra level for the global case, making the implementation
contract silently inconsistent.

**Dashboard-specific global constructor** (`LocalBackend::global()`): A separate
constructor bypasses repo-id derivation for dashboard use only. Rejected for the
same topology reason — the engine's sessions still live under repo-id subdirs, so
the global constructor sees nothing without also changing where sessions are
stored. Sub-variant 2b (flat storage, global constructor everywhere) is
structurally identical to the chosen option with unnecessary duplication.

**Config-driven scope** (`session.scope = "global" | "repo"`): A `koto.toml` key
allows users to toggle discovery scope. Rejected because it makes a structural
concern user-configurable, adds a config key that the dashboard always wants as
`"global"`, and still doesn't resolve the topology mismatch if storage stays
per-repo.

---

### Decision 2: Concurrent mutation of session intent

`koto session update <name> --intent "<text>"` must overwrite the `intent` field
on a session whose state file (`koto-<name>.state.jsonl`) is an append-only event
log — the first line is a `StateFileHeader`; subsequent lines are event records
written via `O_APPEND`.

Two constraints shape the options. First, the advisory `flock(LOCK_EX | LOCK_NB)`
that `SessionBackend::lock_state_file` provides is only acquired for batch-scoped
states. Normal single-writer `koto next` invocations hold no lock. Any strategy
relying on the existing lock to exclude a concurrent engine run fails for the
common (non-batch) case. Second, `O_APPEND` writes are atomic for payloads well
under the kernel limit (each JSON event line is under 4 KB; the Linux minimum is
4096 bytes). Two concurrent `O_APPEND` writers produce a correctly interleaved log
without corruption.

#### Chosen: Append an `IntentUpdated` event to the JSONL log

When `koto session update <name> --intent "<text>"` is called, the command appends
an `intent_updated` event to the session's JSONL log using the same `O_APPEND`
path that `append_event` already uses. Readers derive the current intent by
finding the last `IntentUpdated` payload in the event log; if none exists, intent
falls back to the `StateFileHeader.intent` field (set at `koto init --intent`
time). The dashboard's `refresh()` path caches the derived value in
`CachedSession.intent`, re-deriving only when the state file's mtime changes.

Implementation is roughly 30 lines across three files: `IntentUpdated { intent:
String }` added to `EventPayload` with `#[serde(skip_deserializing_if)]` so older
readers emit `Unknown` rather than erroring; `handle_update` in
`src/cli/session.rs` calling `backend.append_event`; and `derive_intent(events:
&[Event]) -> Option<String>` in the persistence layer returning the last
`IntentUpdated` value.

#### Alternatives Considered

**Read-modify-write under the existing session lock**: Acquire `flock(LOCK_EX |
LOCK_NB)` before rewriting line 0 of the state file. Rejected because non-batch
`koto next` holds no lock, so the lock doesn't exclude the common concurrent-write
case. An engine append between the read and the write is silently discarded when
the rewritten file wins.

**Sidecar metadata file**: Write `koto-<name>.meta.json` alongside the state file
using atomic rename. Concurrency-safe (the engine never touches the sidecar), but
requires every reader to know about a second file type, adds a permanent
data-model artifact, and splits session metadata across two files. The log-event
approach achieves the same safety with less structural complexity.

**Full state file rewrite with atomic rename**: Read all content, modify line 0,
write to a temp file, `rename(2)`. The rename is atomic, but an engine process
that opened the file with `O_APPEND` before the rename continues writing to the
old inode. Events appended after the read but before the rename are silently
discarded — the worst option for correctness.

---

### Decision 3: Detail pane data loading in the always-visible split layout

The 40%/60% horizontal split makes the detail pane always visible, so it must
show content for the focused session on every poll tick without waiting for user
interaction. `read_detail()` performs a full JSONL parse on every call. Under the
current code, it's only invoked when the user explicitly enters Detail mode.
Calling it unconditionally on every 500ms tick repeats an expensive parse even
when the focused session's file hasn't changed.

The session list already uses mtime-based diffing in `refresh()`: sessions whose
files haven't been modified since the last scan are skipped entirely. Extending
the same pattern to the detail layer keeps the data loading model consistent.

#### Chosen: Tick-based load with mtime guard on the focused session

On each poll tick, after `refresh()` completes, the tick loop compares the focused
session's current file mtime against `detail_cache_mtime`. If the mtime is newer,
or if `focused_id` changed since the last load (tracked in
`detail_cache_session`), `read_detail()` is called and the result stored in
`detail_cache`. Otherwise the cache is reused without I/O.

Two new fields on `DashboardAppState` carry this state: `detail_cache_mtime:
Option<SystemTime>` and `detail_cache_session: Option<String>`. A prerequisite
change maintains `focused_id` in List mode (currently it's only set when entering
Detail mode), so the guard has a valid session ID to track.

#### Alternatives Considered

**Load focused session on every tick (no mtime guard)**: Correct but performs a
full JSONL parse on every tick even when nothing has changed — roughly 2.4 seconds
of unnecessary work per minute for each idle session being viewed. Rejected in
favor of the one-extra-field mtime guard.

**Load on cursor move plus tick refresh**: Triggers `read_detail()` on every
`j`/`k` keypress in addition to the polling tick. Provides the freshest data at
cursor arrival but causes redundant reads during rapid navigation. The 500ms poll
interval is fast enough that keystroke-level triggering adds no perceivable UX
improvement.

**Eager load all visible sessions**: Calls `read_detail()` for every session in
`visible_rows()` on each tick. At 15 sessions × ~10ms per parse ≈ 150ms, this
approaches the 200ms budget before accounting for list refresh. At global scope
with more sessions it breaches the budget. Rejected as not viable.

---

### Decision 4: Migration helper invocation model

Existing sessions stored under `~/.koto/sessions/<repo-id>/` need to move to
`~/.koto/sessions/<session-name>/` when the user first runs koto under the new
storage layout. Two approaches exist for triggering the migration.

#### Chosen: Automatic detection on startup

Any koto subcommand that initializes `LocalBackend` checks for the presence of
16-hex-char subdirectories under `~/.koto/sessions/`. If found, the migration runs
in place and a summary is printed to stderr. Users see migration output once, on
the first run after upgrade, without any required action.

#### Alternatives Considered

**Explicit migration command** (`koto session migrate`): Users must run the
command manually before the new layout takes effect. Safer (user controls timing)
but requires every existing user to discover and run the command — or silently see
no sessions until they do. The automatic detection approach is equivalent in safety
(it detects the old layout precisely before touching anything) and eliminates the
user-action requirement.

## Decision Outcome

**Chosen: D1 flat storage + D2 JSONL append + D3 mtime guard + D4 auto-migrate**

### Summary

Five broken behaviors are fixed and three new capabilities are added through
changes that deliberately reuse existing patterns rather than introducing new
infrastructure.

Session storage moves to a flat namespace at `~/.koto/sessions/<session-name>/`.
This single change to `LocalBackend::new()` — removing the `repo_id` join — makes
the dashboard globally scope-aware without any change to `list()`, `session_dir()`,
or `cleanup()`. A startup migration helper handles existing per-repo sessions,
running once and printing a summary when the old layout is detected.

Intent becomes a first-class field across the workflow lifecycle. `StateFileHeader`
gains `intent: Option<String>` and `template_name: Option<String>` using the
established `#[serde(default, skip_serializing_if = "Option::is_none")]` pattern.
Both are set at `koto init` time. Intent can also be updated mid-workflow via
`koto session update <name> --intent "<text>"`, which appends an `IntentUpdated`
event to the JSONL log — the only concurrent-write-safe approach given that normal
`koto next` runs hold no lock. The dashboard derives the current intent by scanning
for the last `IntentUpdated` event on each mtime-changed refresh, with the header
field as the fallback for sessions that predate the event type.

The five broken areas are repaired as part of the same work. Elapsed time is
computed from the `StateInitialized` event timestamp rather than hardcoded to zero.
The gate guard is removed from `read_detail()`, making the detail pane universal
for evidence-only, gate-based, and hybrid sessions. `TaskCounts` gains `blocked`
and `done_blocked` fields for correct status rollup. `visible_rows()` is extended
to arbitrary depth with `├─`/`└─` connectors, and health-severity ordering
(failed → blocked → running → unknown → done, recency as tiebreaker within each
bucket) replaces the current alphabetical sort.

The layout replaces the `Constraint::Length(8)` vertical strip with a
width-adaptive horizontal split: ≥80 columns shows the 40%/60% list-detail split,
<80 columns shows list-only, <40 columns shows a "terminal too narrow" message. The
detail pane loads data on each tick using the mtime guard on the focused session —
one `stat()` syscall per idle tick, one JSONL parse only when the file changes —
keeping the full refresh cycle well under 200ms. The tabbed detail pane has three
tabs: Summary (current state, directive, latest evidence, gate result, intent),
History (full chronological event log, 10 event types, scrollable, with gate
condition text from the compiled template), and Remaining (unvisited states from
the compiled template in topological order).

The `--once` mode gains two new columns — intent and template_name at positions 5
and 6 — while the existing four-column format is preserved for backwards
compatibility.

### Rationale

The three core decisions compose naturally. Removing repo-id from storage enables
global discovery without a topology change to the trait interface. Intent as a
JSONL event means the dashboard's existing mtime guard handles freshness at no
extra I/O cost — intent is re-derived only when the session file changes. The
mtime-guarded detail load keeps the always-visible split within budget because at
most one JSONL parse fires per tick regardless of how many sessions are visible.

The implementation consistently reuses existing patterns: mtime guards mirror
`refresh()`, `O_APPEND` event appending mirrors `append_event`, and the
`#[serde(default, skip_serializing_if)]` pattern mirrors every other optional
field on `StateFileHeader`. This limits the blast radius of each change and makes
each piece independently testable.

## Solution Architecture

### Overview

The dashboard is four source files in `src/cli/`. This design modifies all four,
adds one new subcommand to `src/cli/session.rs`, and changes storage layout in
`src/session/local.rs`. The compiled template (already loaded for the Remaining
tab) is shared with the History tab for gate condition text, avoiding extra disk
reads.

### Components

**`src/session/local.rs` — flat storage**

`LocalBackend::new()` drops the `repo_id` computation and sets `base_dir =
~/.koto/sessions/` unconditionally. The constructor no longer needs a
`working_dir` argument for local backend path purposes; `build_local_backend()`
in `src/cli/mod.rs` is updated accordingly.

A `migrate_if_needed(base: &Path)` function called at backend construction
detects subdirectories whose names are 16-char hex strings (the old repo-id
format). For each such directory, its session subdirectories are moved up one
level to `base_dir`. Name collisions print to stderr and are left in place.

**`src/event/mod.rs` — `IntentUpdated` event**

`EventPayload` gains `IntentUpdated { intent: String }`. The variant uses
`#[serde(rename = "intent_updated")]` and the engine's advance loop handles it as
a recognized no-op to prevent spurious `Unknown` log warnings. A
`derive_intent(events: &[Event]) -> Option<String>` free function (also in this
module or `src/persistence.rs`) returns the last `IntentUpdated.intent` value, or
`None` if no such event exists.

**`src/cli/session.rs` — `koto session update --intent`**

A new `handle_update(backend, name, intent)` function appends an `IntentUpdated`
event using `backend.append_event`. The CLI enum gets a new `Update { name:
String, intent: String }` variant under the `Session` subcommand. The help text:
`koto session update <name> --intent "<text>"`.

**`src/cli/dashboard_state.rs` — state, tree, and ordering**

`TaskCounts` gains two fields: `blocked: u32` and `done_blocked: u32`. The
`visible_rows()` function is generalized to traverse the session tree to arbitrary
depth, computing the connector string (`├─ `, `└─ `, and their indented
equivalents) from the child's position in its parent's child list. Sessions are
sorted by a `session_sort_key(session: &CachedSession) -> (u8, Reverse<SystemTime>)`
helper that maps health status to a numeric severity bucket (failed=0, blocked=1,
running=2, unknown=3, done=4) and uses most-recent mtime as the tiebreaker within
each bucket. `DashboardAppState` gains `detail_cache_mtime: Option<SystemTime>`
and `detail_cache_session: Option<String>`. The `focused_id` field is maintained
in the List mode cursor movement handler, not only when entering Detail mode.

**`src/cli/dashboard_data.rs` — data layer fixes and JSONL-derived fields**

`read_detail()` drops the early-return gate guard. It now always returns a
`SessionDetail` regardless of whether the session has gate evaluations. Elapsed
time is computed from the `StateInitialized` event timestamp using the existing
`compute_elapsed_since()` function rather than hardcoded to zero.

`refresh()` is extended to call `derive_intent(events)` for any session whose
mtime changed. The derived value is written to `CachedSession.intent`. History tab
data is built from a full event scan per session (10 event types: `StateAdvanced`,
`GateEvaluated`, `EvidenceSubmitted`, `DecisionRecorded`, `DirectedTransition`,
`Rewound`, `GateOverrideRecorded`, `ContextAdded`, `DefaultActionExecuted`,
`IntentUpdated`). Remaining tab data comes from the compiled template's state list
minus visited states.

**`src/cli/dashboard_render.rs` — layout and tabbed pane**

The `Constraint::Length(8)` vertical strip is replaced with a horizontal split:
`Constraint::Percentage(40)` for the session list and `Constraint::Percentage(60)`
for the detail pane when terminal width ≥ 80 columns. Below 80 columns the detail
pane is hidden; below 40 columns a "terminal too narrow" message replaces the
entire layout. The detail pane renders a ratatui `Tabs` widget with three tabs:
Summary, History, Remaining. The active tab is tracked in `DashboardAppState`. The
tree connector string is rendered as-is from `VisibleRow.connector` (a new field
populated by `visible_rows()`).

**`src/cli/dashboard.rs` — event loop and `--once`**

The tick handler calls `read_detail()` conditionally: after `refresh()` completes,
compare focused session mtime against `detail_cache_mtime` and `focused_id`
against `detail_cache_session`. Reload only when either changes. Expand/collapse
key bindings (`→`/`←` or `l`/`h`) are active in List mode (currently bound to
Detail mode). The `--once` output path adds two tab-separated fields after the
existing four: `intent` (empty string if absent) and `template_name` (empty string
if absent).

### Key Interfaces

```rust
// src/session/local.rs
impl LocalBackend {
    // working_dir no longer needed for path; retained only for API compat
    pub fn new() -> Self { ... }
}

fn migrate_if_needed(base: &Path) { ... }

// src/event/mod.rs
pub enum EventPayload {
    // existing variants ...
    IntentUpdated { intent: String },
}

pub fn derive_intent(events: &[Event]) -> Option<String> { ... }

// src/cli/dashboard_state.rs
pub struct TaskCounts {
    pub total: u32,
    pub running: u32,
    pub done: u32,
    pub failed: u32,
    pub blocked: u32,      // new
    pub done_blocked: u32, // new
}

pub struct DashboardAppState {
    // existing fields ...
    pub detail_cache_mtime: Option<SystemTime>,   // new
    pub detail_cache_session: Option<String>,      // new
}

pub struct VisibleRow {
    // existing fields ...
    pub connector: String, // new: "├─ ", "└─ ", "  ├─ " etc.
}

fn session_sort_key(session: &CachedSession) -> (u8, Reverse<SystemTime>) { ... }

// src/cli/dashboard_data.rs
pub struct CachedSession {
    // existing fields ...
    pub intent: Option<String>, // new: derived from JSONL
}

// read_detail no longer returns None for evidence-only sessions
pub fn read_detail(path: &Path, session_id: &str) -> Option<SessionDetail> { ... }
```

### Data Flow

**Session list refresh (500ms tick):**
1. `refresh()` scans `~/.koto/sessions/` (flat, global)
2. For each session with changed mtime: reads header, parses JSONL for task counts
   and `derive_intent()`
3. Updates `CachedSession` in the tree (including `CachedSession.intent`)
4. `visible_rows()` produces `VisibleRow` list with connectors, sorted by health severity

**Detail pane load (same tick, after refresh):**
1. Determine `focused_id` from cursor position (List and Detail modes)
2. If `focused_id != detail_cache_session` or `mtime > detail_cache_mtime`: call
   `read_detail()`, update `detail_cache`, `detail_cache_mtime`,
   `detail_cache_session`
3. Otherwise: reuse `detail_cache` (zero I/O)

**Intent derivation:**
1. `refresh()` calls `derive_intent()` on full event scan for changed sessions
2. `derive_intent()` returns last `IntentUpdated.intent` value, or `None`
3. `None` is displayed as the `StateFileHeader.intent` fallback in the Summary tab

**`--once` output (6 columns):**
```
<session_id>\t<status>\t<state>\t<elapsed>\t<intent>\t<template_name>
```
Columns 1–4 unchanged. Columns 5–6 are empty strings when the fields are absent.

## Implementation Approach

### Phase 1: Storage, schema, and identity

Lay the foundation with changes that are independently deployable and backward
compatible. No dashboard UI changes in this phase.

- Remove `repo_id` from `LocalBackend::new()` in `src/session/local.rs`
- Add `migrate_if_needed()` startup check
- Add `intent: Option<String>` and `template_name: Option<String>` to
  `StateFileHeader` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- Add `IntentUpdated { intent: String }` to `EventPayload`
- Add `derive_intent(events: &[Event]) -> Option<String>`
- Add `koto session update <name> --intent` subcommand to `src/cli/session.rs`;
  reject inputs over 1024 characters with a clear error before the append
- Handle `IntentUpdated` as a recognized no-op in the engine advance loop
- Tests: unit tests for `derive_intent`, migration helper, intent round-trip,
  and the 1024-char limit boundary

### Phase 2: Data layer correctness

Fix the computation bugs and remove the gate guard. No layout changes.

- Fix elapsed: compute from `StateInitialized` event timestamp in `dashboard_data.rs`
  using the existing `compute_elapsed_since()`
- Remove gate guard from `read_detail()` — always return `SessionDetail`
- Add `blocked` and `done_blocked` to `TaskCounts`
- Add `CachedSession.intent` field; populate it via `derive_intent()` in `refresh()`
  for mtime-changed sessions
- Extend `--once` output to 6 columns (add intent, template_name)
- Tests: unit tests for elapsed computation, `read_detail()` on evidence-only
  sessions, `--once` 6-column output format

### Phase 3: Tree, ordering, and detail cache

Fix tree rendering and wiring before touching the layout.

- Extend `visible_rows()` to arbitrary depth
- Add `VisibleRow.connector` field; compute `├─ `/`└─ ` strings based on child
  position in parent's child list
- Add `session_sort_key()` helper; replace `all_ids.sort()` and `roots.sort()` in
  `visible_rows()` with health-severity ordering
- Add `detail_cache_mtime` and `detail_cache_session` to `DashboardAppState`
- Maintain `focused_id` in List mode cursor movement
- Move expand/collapse key bindings to List mode
- Wire mtime-guarded detail load in the tick handler in `dashboard.rs`
- Tests: tree depth tests, connector string tests, ordering tests,
  cache invalidation tests

### Phase 4: Rendering and layout

Replace the layout and build the tabbed detail pane.

- Replace `Constraint::Length(8)` with horizontal 40%/60% split in
  `dashboard_render.rs`
- Add width-gating: ≥80 columns → split, <80 → list-only, <40 → too-narrow message
- Add `Tabs` widget with Summary, History, Remaining tabs
- Implement each tab's content rendering using the `SessionDetail` fields populated
  in Phase 2 and Phase 3
- History tab: render 10 event types with gate condition text from compiled template
  (already loaded for Remaining tab)
- Tests: render tests for each layout mode, tab switching, History tab gate condition
  display

## Security Considerations

**Session namespace collision after migration**

Removing the repo-id scoping (D1) merges all project sessions into a single flat
namespace. Two projects using the same session name will collide; the migration
helper warns on collision and leaves the conflicting session in place, but the
incoming session is not moved. Users should adopt project-prefixed session names
(e.g., `myproject-feature-branch`) to avoid collisions. The dashboard's global
view is intentional — document it so users are not surprised by sessions from
other projects appearing in the list.

**Intent string length and `O_APPEND` atomicity**

`koto session update --intent` uses `O_APPEND` to avoid a read-modify-write race
with a running engine. POSIX guarantees `O_APPEND` writes are atomic only up to
`PIPE_BUF` bytes (typically 4096 on Linux). Intent strings are expected to be
short in practice, but the design imposes an explicit 1024-character maximum in
the `handle_update` CLI handler before the append. Inputs exceeding this limit are
rejected with a clear error message rather than truncated silently.

**Dashboard visibility is user-scoped**

State files are owned by the invoking user (mode 0600, `.koto` root mode 0700).
The dashboard surfaces session names, states, gate outcomes, evidence fields, and
intent strings — all already present in the state file. No data leaves the
machine. Intent strings may contain context users consider sensitive; the state
file is not encrypted and is readable by any process running as the same user.

## Consequences

### Positive

- The dashboard shows all sessions on the local machine without requiring the user
  to launch it from a specific directory. The primary monitoring use case — watching
  parallel workflows across multiple niwa workspaces — is now fully supported.
- Sessions have human-readable `intent` and `template_name` labels. Operators can
  identify sessions at a glance without running `koto query`.
- Evidence-only sessions (the dominant pattern in shirabe-style workflows) show
  useful detail pane content. The gate guard that caused permanent "No gate
  evaluations recorded" messages is gone.
- Elapsed time, tree connectors, status rollup, and session ordering are all
  correct. The list view is actionable without supplementary CLI queries.
- The `IntentUpdated` event pattern generalizes to future mutable metadata fields
  (`label`, `tag`, `owner`) — each becomes a new `*Updated` event type with a
  matching `derive_*` helper. No structural change needed.
- The flat session namespace is a natural fit for the F5 cloud extension. No
  topology change is required when adding an S3 backend — sessions already live at
  `<base>/<session-name>/` with no repo-id intermediary.

### Negative

- Session name collisions across repos are now user-visible. Two repos both using
  `task_issue-1` share a session directory after migration.
- Intent is not readable from the header alone after a `koto session update
  --intent` call. The `koto session list` path (header-only reads) will not surface
  updated intent.
- The one-time migration helper must be maintained until all existing installs
  migrate, then removed in a subsequent release.
- The detail pane now performs a `stat()` call on every poll tick for the focused
  session, where before it did zero I/O until Detail mode was entered. This is a
  minor but real increase.

### Mitigations

- **Name collisions**: The migration helper detects collisions and leaves conflicting
  sessions in place under their old repo-id path with a stderr warning. The warning
  tells the user exactly which sessions conflict and how to resolve them (rename one
  before running migration again).
- **JSONL-only updated intent**: Updated intent is cached in `CachedSession.intent`
  and shown in the list view and detail pane. The `session list` path not showing
  updated intent is acceptable — listing use cases don't need mid-workflow updates.
- **Migration helper longevity**: The helper is guarded by a cheap directory scan
  that adds negligible startup cost after migration completes. A removal milestone
  can be set once telemetry shows adoption has reached the expected threshold.
- **Per-tick `stat()`**: A single `stat()` syscall is under 1 µs on Linux. At a
  500ms poll interval this adds less than 0.0002% overhead per tick cycle.
