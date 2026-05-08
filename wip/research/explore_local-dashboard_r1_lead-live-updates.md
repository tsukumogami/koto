# Lead: How should live updates work?

## Findings

### How koto writes its session feed

**Write mechanism**: koto uses append-only JSONL files via `src/engine/persistence.rs`:
- `append_header()` writes a single header line to the state file at session init (line 14-38)
- `append_event()` appends one event per call, auto-assigning monotonically-increasing seq numbers (line 45-87)
- Each write calls `file.sync_data()` before returning (lines 34, 84) to ensure durability before the next event is written
- The design is explicit: "Calls `sync_data()` after every write" per the function docs (line 44)

**Implication**: Each event is individually persisted to disk with explicit fsync. This is not batched — writes are synchronous and ordered, making real-time file-monitoring feasible.

### Session file storage and path convention

**Storage location**: `~/.koto/sessions/<repo-id>/<session-id>/koto-<session-id>.state.jsonl`
- Repo-ID is a 16-character hex hash of the canonicalized working directory (src/session/local.rs:600-605)
- Session directory is created via `LocalBackend::new()` during session initialization
- State file name follows the convention: `koto-{id}.state.jsonl` (src/session/mod.rs:148)

**Example path**: `~/.koto/sessions/a3f5b2c1deadbeef/issue-42/koto-issue-42.state.jsonl`

**Implication**: The dashboard knows the session location and can construct the file path given a working directory and session ID. No lookup service is needed for path resolution.

### File-watch and notification mechanisms in the codebase

**Current state**: Zero file-watch or notification mechanisms found in koto's codebase.
- `grep -r "inotify|kqueue|watch|notify|tail"` returned no relevant hits in `src/`
- No dependencies on `notify`, `inotify-rs`, `notify-debounce`, or similar file-watching crates in `Cargo.toml` (tested against current manifest)
- The codebase has no file-watch integration points; this would be entirely new

**Implication**: The dashboard will need to implement its own file-watch strategy. This is a greenfield requirement.

### Write frequency during a typical workflow run

Event frequency varies by workflow phase:

1. **Initialization** (once per session):
   - `workflow_initialized` (seq 1) — sent at `koto init` time
   - Initial `transitioned` event (seq 2) to the first state
   - **Latency**: happens once, at workflow start

2. **Active state phase** (per state visit):
   - `gate_evaluated` events (Tier 2) emitted for each gate check, potentially multiple times per gate during polling loops
   - `default_action_executed` (Tier 2) emitted if the state has an automatic action
   - `evidence_submitted` (Tier 1) emitted when an agent submits evidence
   - `decision_recorded` (Tier 2) emitted when an agent records a structured decision
   - Example: a state with 3 gates that fail after 5 evaluation attempts would produce 15 `gate_evaluated` events before stopping

3. **Transitions** (per state change):
   - `transitioned`, `directed_transition`, or `rewound` events (Tier 1) — exactly one per state change
   - **Frequency**: depends on template structure and gate pass/fail rates

4. **Batch operations** (if present):
   - `scheduler_ran` (Tier 3) emitted only on non-trivial scheduler ticks (at least one child spawned, reclassified, errored, or skipped)
   - `child_completed` events (Tier 2) when a child reaches terminal state
   - `batch_finalized` (Tier 1) when batch children all complete

**Event frequency analysis** from session-feed spec (docs/reference/session-feed.md):
- No explicit "events per minute" guarantee in the contract
- Tier 2 and Tier 3 events (gate evals, scheduler ticks) can accumulate during polling or waiting phases
- No throttling or batching is documented; each event is independently flushed

**Implication**: Write frequency is **not constant**. Short bursts (5-15 events within seconds during gate evaluation or evidence submission) are typical, with quiet periods between state transitions. A dashboard refresh rate of 100-500ms would capture most activity without overwhelming network or rendering.

### Write guarantees and ordering from the session-feed spec

From `docs/reference/session-feed.md` (lines 268-360):

1. **Ordering guarantee** (line 328-329):
   > "Events are strictly monotonic on `seq`. A gap in `seq` indicates file corruption."
   - Readers MUST treat a non-final seq gap as a `StateFileCorrupted` error

2. **Durability guarantee** (line 331-332):
   > "Each event is flushed with `sync_data()` before the next event is written."
   - Confirmed by code inspection: every append in persistence.rs calls sync_data()

3. **Atomicity guarantee** (line 334-336):
   > "Each event write is atomic at the line level. A line is either complete and valid JSON, or truncated (partial write)."
   - Implication: no corruption mid-file; only the final line can be incomplete

4. **Partial-write recovery** (line 338-340):
   > "A truncated final line (incomplete JSON) is recoverable. Readers SHOULD discard the truncated line and treat all preceding complete events as valid."
   - koto's own `read_events` does this with stderr warning (persistence.rs:206-213)

5. **Single-writer guarantee** (line 342-343):
   > "Only the koto process that owns a session writes to its log. No external process should append to a session log while koto holds it."
   - Enforced by session locking (src/session/local.rs:320-356)

**Implication**: The dashboard can safely tail the JSONL file and parse complete lines as they appear. The monotonic seq ordering and single-writer guarantee mean:
- No race conditions between dashboard reads and koto writes
- Reading up to the last complete line is always safe
- Missing the final line (if incomplete) is harmless and recoverable by checking seq continuity

## Implications

### For live-update strategy choice

The investigation validates **three viable live-update approaches** with different tradeoffs:

1. **Inotify/kqueue file-watch (tight coupling)**
   - Pros: sub-millisecond latency, efficient, OS-native
   - Cons: platform-specific, adds new dependency
   - Best for: low-latency dashboards (e.g., real-time gate eval feedback)
   - **Feasible**: Yes. File durability guarantees mean watching is safe.

2. **Polling interval (loose coupling)**
   - Pros: no new dependencies, cross-platform, simple
   - Cons: update latency is polling interval (e.g., 500ms = 500ms max lag)
   - Best for: dashboards where 100-500ms lag is acceptable
   - **Feasible**: Yes. Read-last-seq pattern is lightweight; no full replay needed after init.

3. **Direct koto state-write hook (tight coupling)**
   - Pros: zero-latency, zero overhead for non-dashboard workflows
   - Cons: adds callback interface to engine, requires invasive changes to persistence layer
   - Best for: embedded dashboards (koto as a library)
   - **Feasible**: Possible but requires design work on callback contract.

### For PRD specification

The PRD must make explicit choices about:

1. **Update latency expectation**: What is the acceptable lag between event write and display (sub-second? 100ms? 1 second)?
   - This determines whether inotify is necessary or polling is acceptable
   - No current SLA is documented

2. **Refresh rate**: How often should the display poll/check for updates?
   - Given burst patterns (5-15 events in seconds), a 200-500ms refresh seems reasonable
   - Tier 3 events (scheduler_ran) are suppressed; only non-trivial ticks appear, so update volume is bounded

3. **Tail-from behavior**: Should the dashboard display the entire event log replayed from disk, or only events since the last sync?
   - Replaying the full log is safe (idempotent); starting from last-sync is more efficient
   - The seq field ensures no events are silently missed

4. **Partial-write handling**: Should the dashboard detect and warn on truncated final lines, or silently skip?
   - Current koto behavior: stderr warning, graceful recovery
   - Dashboard could adopt the same pattern

### Why this matters for exploration

Without explicit live-update choices, the PRD cannot specify:
- The dashboard invocation model (embedded in koto? standalone web? terminal UI?)
- The rendering framework (must support chosen update mechanism)
- The session discovery and reconnection behavior
- Network/latency expectations for remote dashboards (F5/F6 depend on these choices)

These decisions are prerequisites for the rendering-approach lead investigation.

## Surprises

1. **No existing file-watch infrastructure**: Koto has zero file-watch or notification code, even though the session-feed contract explicitly supports tail-based readers. This is a pure greenfield requirement for the dashboard.

2. **Explicit sync_data() on every write**: The codebase is disciplined about fsync discipline — every append calls sync_data(). This is stronger than many event-log systems and makes real-time tailing safe.

3. **Batch finalization without explicit completion events**: The session-feed spec notes (line 792-793):
   > "There is no `workflow_completed` event. Consumers determine completion by inspecting the most recent `transitioned` event and checking whether its `to` field matches the template's defined terminal states."
   - This is a known gap. The dashboard must maintain its own terminal-state lookup or calculate it from the template.

4. **Single-writer enforcement via flock, not coordination**: Session locking uses advisory flock (src/session/local.rs:320-356), not a coordination service. This works for local machine scenarios but becomes complex for S3-backed dashboards (F5).

## Open Questions

1. **Update latency SLA**: What is the acceptable time from event write to display?
   - Sub-second? 100ms? Depends on UX (live gate evals feel snappier with low latency)
   - **Needs decision**: Drives the choice between inotify and polling

2. **Refresh pattern**: Should the dashboard:
   - Continuously tail the file from wherever it left off?
   - Poll and batch-update at fixed intervals?
   - Hybrid (inotify for visibility, batch refresh for rendering)?
   - **Needs decision**: Constrains the rendering framework choice

3. **Terminal state detection**: How does the dashboard know when a workflow has finished?
   - Option A: Read the template, compare against the final transitioned event's `to` field
   - Option B: Poll until no new events for a timeout period
   - Option C: Expect a new event type (requires F2 contract extension)
   - **Current state**: A is the documented pattern; B is what typical tail-based readers do

4. **Partial-line recovery**: When the file ends with a truncated JSON line:
   - Should the dashboard detect and warn the user?
   - Should it automatically recover (skip the line)?
   - Should it wait for the line to complete before parsing?
   - **Current pattern**: koto silently recovers with stderr warning

5. **Session discovery**: How does the dashboard find sessions?
   - Auto-discover from `~/.koto/sessions/<repo-id>/`?
   - Require explicit session ID from the user?
   - Watch for `koto-*.state.jsonl` files appearing in real-time?
   - **Affects**: Invocation model and startup UX

6. **Hierarchy reconstruction**: How does the dashboard infer the session tree?
   - Option A: Parse `parent_workflow` from the header of each session, reconstruct tree on load
   - Option B: Watch the parent's `child_completed` events to infer structure
   - Option C: Read a separate manifest file (doesn't exist yet)
   - **Current state**: Headers have `parent_workflow` field; tree reconstruction is possible from headers alone

## Summary

Koto writes events using append-only JSONL with explicit fsync after each append, ensuring safe real-time tailing. Session files are stored at stable, predictable paths (`~/.koto/sessions/<repo-id>/<session-id>/koto-<session-id>.state.jsonl`) with monotonic seq ordering and single-writer guarantees. No file-watch infrastructure exists in koto today, so the dashboard must implement its own via inotify/kqueue or polling—the choice depends on latency requirements (100ms vs. 500ms lag) that the PRD must specify. Terminal state detection, session discovery, and hierarchy reconstruction all require explicit PRD decisions; the session-feed contract provides the data foundation but leaves timing expectations and update latency unstated.

