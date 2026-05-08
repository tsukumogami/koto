# Decision 2: Session State Model

## Chosen: Option C
Maintain a `HashMap<String, CachedSession>` in memory; on each poll cycle, stat every session file, apply a diff to detect adds/removes, and re-read only files whose mtime has advanced since the last check.

## Rationale

The critical constraint is what "current state" actually requires. The header line contains only session name, template hash, `created_at`, and `parent_workflow` — not the current workflow state. Current state must be derived from the event log via `derive_state_from_log`, which scans events from the last `transitioned`/`directed_transition`/`rewound` event. This means the dashboard cannot avoid reading events; it can only avoid re-reading them when nothing has changed.

Option C addresses this directly. `stat()` on a JSONL file costs roughly 1–5µs on a warm filesystem cache. For 100 sessions, statting all files takes well under 1ms. For 1000 sessions, it stays under 10ms. Only files whose `mtime` has advanced since the previous tick are re-read; on most poll cycles in a stable batch run, that is a small fraction of the total. The in-memory `HashMap` keyed by session path carries the last-known derived state, so unchanged sessions contribute zero I/O. Adds and removes are detected by comparing the current scan of session directories against the keys in the map.

The dashboard never needs full event history for the list view — only the current state name and a few aggregate counts. Full event history is deferred to the focused detail view, which can call `read_events` on demand for a single session. This means the cache entry is small: header fields plus the derived current state, plus the mtime used to validate the cache entry. Memory overhead for 1000 sessions is a few kilobytes.

## Rejected Options

### Option A: Full re-read on each tick
Simple to implement and correct for small session counts, but untenable at scale. Reading the full event log for every session on every tick is O(n * avg_events). A batch run with 1000 sessions and 20 events each requires ~20,000 JSON line parses per tick. At 500ms poll intervals that is manageable in isolation, but the I/O competes with the engine writing new events — and for long-running sessions with hundreds of events the per-file cost grows. It also provides no foundation for the focused detail view's lazy loading pattern; everything gets read regardless of what the user is looking at.

### Option B: mtime-based cache (as described)
Option B as stated is functionally identical to Option C. The distinction is that B describes the cache invalidation strategy while C names the data structure explicitly. They converge on the same implementation. The report adopts Option C's framing because the explicit `HashMap` makes the add/remove diff logic visible in the design, which matters for correctness when sessions are cleaned up between poll cycles.

## Data Structure Sketch

```rust
/// Minimal per-session data retained between poll cycles.
struct CachedSession {
    /// Parsed header (workflow name, created_at, parent_workflow, template_hash).
    header: StateFileHeader,
    /// Current state derived from the event log at last read.
    /// `None` if no state-changing event has been written yet (just initialized).
    current_state: Option<String>,
    /// mtime of the state file at the time this entry was populated.
    /// Used to detect staleness without re-reading the file.
    mtime: std::time::SystemTime,
    /// Absolute path to the state file, for re-reads.
    state_path: std::path::PathBuf,
}

struct DashboardState {
    /// All known sessions for the current repo, keyed by session id.
    sessions: std::collections::HashMap<String, CachedSession>,
    /// Ordered list of root session ids (no parent_workflow) for rendering.
    /// Rebuilt whenever `sessions` changes.
    roots: Vec<String>,
}

impl DashboardState {
    /// Refresh on each poll tick.
    ///
    /// 1. Scan the session base directory to enumerate current session ids.
    /// 2. Remove entries for session ids that no longer exist on disk.
    /// 3. For each remaining/new session, stat the state file.
    ///    - If the path is new (not in `sessions`): read header + events, insert.
    ///    - If mtime > cached mtime: re-read events only, update current_state + mtime.
    ///    - If mtime unchanged: skip (cache hit).
    /// 4. Rebuild `roots` if any insertions or removals occurred.
    fn refresh(&mut self, backend: &LocalBackend) -> anyhow::Result<()> {
        // ... stat loop, diff, selective read_events calls ...
        todo!()
    }

    /// Aggregate counts for display (running, blocked, complete, etc.).
    /// Derived from `sessions` values; O(n) scan of in-memory data, no I/O.
    fn aggregate_counts(&self) -> AggregateCounts {
        todo!()
    }
}

struct AggregateCounts {
    total: usize,
    running: usize,   // has a current_state, not terminal
    complete: usize,  // terminal state (requires template coupling — deferred)
    unknown: usize,   // current_state is None (just initialized)
}
```

## Assumptions

- Session state files are fsynced on every write (confirmed from `persistence.rs`). `mtime` change is a reliable signal that new events exist.
- The dashboard reads sessions for a single repo (scoped by `repo_id` hash). Session count at the relevant scale is 100–1000; not tens of thousands.
- Terminal state detection (comparing current state against the template's terminal states) is deferred from this model. The `complete` count in `AggregateCounts` can be implemented later by loading the compiled template on cache miss.
- Full event history (for gate display in the focused detail panel) is loaded on demand per session, not cached in `DashboardState`. The detail view calls `persistence::read_events` directly.
- `derive_state_from_log` from `src/engine/persistence.rs` is reused as-is; the cache stores its output, not a re-implemented derivation.
- The poll cycle runs on a single thread (consistent with Decision 1's likely single-threaded tick loop outcome). No concurrent access to `DashboardState` is assumed.
