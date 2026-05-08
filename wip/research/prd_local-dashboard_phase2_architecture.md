# Phase 2 Research: Architecture Perspective

## Lead 5: Command Interface Design for V2 Compatibility

### Findings

**Current CLI structure (src/cli/mod.rs lines 67-209):**
The `koto` CLI uses `clap::Subcommand` with top-level variants including `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`, `Session`, `Context`, `Status`, `Decisions`, `Overrides`, and `Config`. Subcommands are nested: `Session` contains `SessionCommand` enum with `Dir`, `List`, `Cleanup`, `Resolve`; `Template` contains `TemplateSubcommand` with `Compile`, `Validate`, `ValidateFeed`, `Export`.

**Pattern for adding subcommands:**
New top-level commands are added as variants to the `Command` enum. Nested subcommands (e.g., `koto session list`) use a parent command variant that holds a `subcommand` field of type (Subcommand enum), matched in the main `run()` dispatcher at line 777.

**Command routing in `run()` (lines 777-1075):**
The pattern is:
```rust
Command::Workflows { roots, children, orphaned } => {
    let backend = build_backend()?;
    let metadata = find_workflows_with_metadata(&backend)?;
    // ... filter and output
}
Command::Session { subcommand } => {
    let backend = build_backend()?;
    match subcommand { ... }
}
```

**Three viable namespace options for dashboard:**

1. **Top-level `koto dashboard` (Option A — Recommended for MVP)**
   - Direct parallel to `koto workflows`, `koto status`, `koto next`
   - Command variant: `Dashboard { name: Option<String>, #[arg(long)] interval: Option<u64>, ... }`
   - Matches user mental model: "show me this workflow's dashboard"
   - Preserves daemon room: daemon could be `koto daemon start/stop` (separate top-level commands)

2. **Under `koto session` (Option B — Subcommand nesting)**
   - `koto session dashboard <name>` or `koto session list --interactive`
   - Semantically correct but longer invocation
   - Splits session management—`cleanup` would be `koto session cleanup`, but observation becomes `koto session dashboard`
   - V2 daemon would be unclear: `koto session daemon start` doesn't read right

3. **`koto observe` or `koto watch` (Option C — Verbose but future-proof)**
   - `koto observe <name>` or `koto watch <name>` as top-level
   - Explicitly signals "monitoring" intent (distinct from `status` one-shot)
   - Leaves `daemon` namespace entirely separate for V2
   - More characters to type; conflicts with no other tools observed

**Rationale for Option A:**
The MVP should use **top-level `koto dashboard [<name>]`** because:
- Consistent with existing commands (`koto status <name>`, `koto workflows`)
- Clear that it's a session-focused tool, not a general observer
- V2 daemon mode can coexist as `koto daemon start/stop` (separate namespace, zero conflict)
- Users can alias or script; MVP usability > future namespace perfection

**Flags for MVP `koto dashboard`:**
```rust
Dashboard {
    /// Workflow name (optional; omitted = show all)
    name: Option<String>,
    
    /// Poll interval in milliseconds (default 500)
    #[arg(long, default_value = "500")]
    interval: u64,
    
    /// One-shot output (exit after first display, no polling)
    #[arg(long)]
    once: bool,
    
    /// Output format: "text" (default) or "json"
    #[arg(long, default_value = "text")]
    format: String,
}
```

These flags support both interactive use (`koto dashboard my-workflow`) and non-interactive dispatch (`koto dashboard --once --format json --name parent | jq .state`).

### Implications for Requirements

1. **Command interface stability:** Top-level `koto dashboard` avoids the namespace collision risk that nesting under `Session` would create. When V2 daemon lands, it will be `koto daemon start` (or `koto daemon attach`), not an override of dashboard.

2. **MVP does not gate V2 daemon:** The MVP command interface fully survives the daemon era. A V2 user running `koto dashboard --daemon` is impossible (daemon is separate), but that's acceptable—the normal flow is `koto daemon start` once per repo, then `koto dashboard` repeatedly.

3. **Flag vocabulary:** Accepting `--interval` and `--once` in MVP makes non-interactive scripting possible from day one, which is crucial for downstream tools (CI, agent monitors, test harnesses). These flags are cheap to implement and powerful.

### Open Questions

1. Should `koto dashboard --daemon` be explicitly rejected (early error) or silently treated as a non-daemon flag?
   - Recommend: explicit reject with helpful message ("use `koto daemon start` instead").

2. What happens if the user runs `koto dashboard nonexistent-name`?
   - Recommend: exit 1 with JSON error, same as `koto status nonexistent-name`.

3. Should the interval be configurable globally via `koto config set dashboard.interval 250`?
   - Defer to V2; MVP default of 500ms is reasonable for discovery phase.

---

## Coverage Gap A: Epoch-Branched Sessions

### Findings

**Session naming and tilde reservation (src/discover.rs lines 41–49 and src/session/validate.rs):**
Workflow names reject tilde (`~`) explicitly:
```rust
if name.contains('~') {
    return Err(format!(
        "workflow name '{}' contains '~', which is reserved for internal epoch branching",
        ...
    ));
}
```

Tilde is reserved for **epoch-branched child sessions** created when a batch parent is rewound.

**How epoch branches are created (src/cli/mod.rs lines 1430–1501, `rewind_relocate_children`):**

When `koto rewind` is called on a batch parent:
1. Count all `Rewound` events in the log → `epoch = 3` (for example)
2. For each child session matching `<parent>.<task>`:
   - Relocate it to `<parent>~3.<task>` via `backend.relocate()`
3. Return the relocated count and branch prefix

Example: If parent `research` has children `research.r1`, `research.r2`, and we rewind at epoch 3:
- `research.r1` → `research~3.r1`
- `research.r2` → `research~3.r2`
- New epoch children will be spawned at `research.r4.x`, `research.r4.y`, etc.

**Are epoch-branched sessions listed in `koto workflows`?**
Yes. The `find_workflows_with_metadata()` function (src/discover.rs line 90) calls `backend.list()`, which scans the session directory via `fs::read_dir()` and returns ALL sessions, including those with tilde names. The function applies no filtering.

Test evidence (src/discover.rs tests): No test specifically excludes epoch branches from metadata output.

**What does `koto status` show about epoch-branched children?**
The status output includes a `children` array from `query_children()` (src/cli/mod.rs line 1412), which lists all sessions whose `parent_workflow` matches the parent name. No filtering is applied; epoch-branched children are included.

**User experience implication:**
Running `koto workflows` after a rewind will show:
```
[
  { "name": "research", ... },
  { "name": "research~3.r1", ... },
  { "name": "research~3.r2", ... },
  { "name": "research.r4.x", ... }
]
```

This is noisy and confusing; users see both the "old" epoch-1 children (relocated with `~3` prefix) and the "new" epoch-4 children alongside the parent.

### Implications for Requirements

**The dashboard MUST handle epoch-branched sessions:**

1. **Hide them in the main list view (recommended):** When `koto dashboard` is called without arguments, filter out sessions whose name contains `~`. Show only "current-epoch" children:
   ```rust
   let visible = metadata
       .into_iter()
       .filter(|wf| !wf.name.contains('~'))
       .collect();
   ```

2. **Show them in focused view (parent-only dashboard):** When `koto dashboard research` is called, include a collapsible "archived_epochs" section listing superseded children by epoch:
   ```json
   {
     "name": "research",
     "state": "active",
     "children": [...],
     "archived_epochs": {
       "3": ["research~3.r1", "research~3.r2"],
       "5": ["research~5.r4", "research~5.r5"]
     }
   }
   ```

3. **Filter children queries:** The `query_children(backend, parent_name)` helper used by `koto status` and rewind returns ALL children. The dashboard should wrap this and filter:
   ```rust
   let current_epoch_children = query_children(backend, parent_name)
       .into_iter()
       .filter(|child| !child.name.contains('~'))
       .collect();
   ```

4. **Update `koto workflows --children parent`:** This should probably also filter out epoch branches by default, with an optional flag to show archived:
   ```
   koto workflows --children parent --include-epochs
   ```

**Why this matters:**
- **Accumulation:** A batch parent can generate 10+ epoch branches over a week of rewinding. Without filtering, the session list becomes unreadable.
- **Mental model:** Users think of the dashboard as a "live status board." Archived epochs are historical; they clutter the present view.
- **Search/filter UX:** If a user is looking for a specific child, seeing both `research.r1` and `research~3.r1` is confusing—which is the "active" one?

### Open Questions

1. Should `koto session cleanup parent~3.r1` be prevented (since it's an archived child)?
   - Recommend: allow it (users may want to free storage), but warn in output.

2. Should the dashboard show a summary like "3 archived epochs with 12 children total"?
   - Recommend: yes in the parent-focused view, to signal there's historical depth.

3. Should epoch branches be auto-cleaned after N days, or left for audit trails?
   - Defer to V2; MVP should leave them as-is (non-destructive).

---

## Coverage Gap B: Performance Envelope

### Findings

**`backend.list()` complexity (src/session/local.rs lines 85–133):**
```rust
fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
    let entries = fs::read_dir(&self.base_dir)?;  // Single syscall
    for entry in entries {
        // Per-entry checks:
        let dir_name = entry.file_name().to_str()?;
        let state_path = entry.path().join(state_file_name(&dir_name));
        if !state_path.exists() {
            continue;  // is_file check
        }
        // Read header from state file
        persistence::read_header(&state_path)?;
    }
}
```

Per-session cost: One `stat` (to check existence) + one `open()` + reading the first JSON line (to extract created_at, template_hash, parent_workflow).

**Scalability estimate:**
- 100 sessions: ~100 stat calls + 100 open calls + 100 single-line reads = ~200ms on typical SSD (assuming 1-2ms per I/O)
- 500 sessions: ~1s (linear complexity)
- No caching or indexing in MVP

**`derive_state_from_log` complexity (src/engine/persistence.rs line 235):**
Reverse iteration over events:
```rust
pub fn derive_state_from_log(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(...)
}
```
Cost: O(1) for most sessions (first reverse scan finds a state-changing event); worst-case O(n) if the session is very old and the latest event is not a state change (rare).

**Template loading cost:**
Loading a compiled template from `~/.koto/cache/compiled/<hash>.json`:
- Typical size: 50–500 KB (depends on state count)
- Cost: open + read + JSON parse = ~5–20ms per template

**Polling at 500ms with 100 sessions:**
```
Poll cycle: read_dir() + 100×(stat + open + parse header) = ~200ms
Interval: 500ms
Overhead: 40% CPU per core if polling on every 500ms interval
```

This is acceptable for a local CLI; not a bottleneck.

**Startup time (no cache):**
```
koto dashboard (list all):
  - backend.list() for all sessions: ~200ms for 100 sessions
  - Total: ~200ms
  
koto dashboard <name> (single session):
  - backend.read_events(): one open + full read = ~10–50ms depending on log size
  - derive_machine_state(): template load + JSON parse = ~10ms
  - Total: ~20–60ms
```

MVP startup time goal: <1s for repo-wide dashboard (100 sessions), <100ms for single-session view.

### Open Questions

1. Should the dashboard cache session metadata in a `.koto/dashboard-cache` file?
   - Recommend: defer to V2; MVP throughput is acceptable.

2. For the polling loop, should we batch stat calls or use inotify (Linux)?
   - Recommend: defer; sequential polling is simpler and sufficient for MVP.

3. If a session's log grows to 100 MB (long-running workflows), does `read_events()` become a bottleneck?
   - Recommend: `derive_machine_state()` only reads the header + scans for the latest state-change event; MVP should avoid full log replay for dashboard.

---

## Coverage Gap C: Update Latency

### Findings

**Poll interval semantics (src/cli/mod.rs lines 700–775, `execute_with_polling`):**
The engine's polling mechanism uses a hardcoded pattern:
```rust
loop {
    let output = run_shell_command(...);
    if all_gates_passed { break; }
    if Instant::now() >= deadline { break; }
    std::thread::sleep(Duration::from_secs(polling.interval_secs));
}
```

This is template-driven (from the workflow YAML), not user-configurable at runtime.

**For dashboard, 500ms is proposed as a default:**
- At 500ms, a gate failure is visible within 1–2 poll cycles (0.5–1s latency to first update)
- Gate retry feedback loop: user observes a failed gate, koto retries or escalates, user sees the result within 1s
- This matches typical CI/CD dashboard expectations (GitHub Actions, Jenkins both default to 1–5s polling)

**Configurable interval rationale:**
- Default 500ms keeps the dashboard "snappy" without hammering the filesystem
- Users might want 1000ms for low-power devices or high-latency sessions
- Users might want 100ms for critical gate observation (rare, but defensible)

**Hardcoded vs. configurable:**
- MVP: hardcoded 500ms (simplest)
- Post-MVP: add `--interval` flag (trivial to add; clap handles this automatically)
- Post-post-MVP: `koto config set dashboard.interval 250` (requires config plumbing)

### Implications for Requirements

1. **MVP must specify a default poll interval in the PRD:** Recommend 500ms as a balance between responsiveness and I/O load.

2. **Users should be able to override via flag:** `--interval 1000` or `--interval 250` to scale latency to their use case.

3. **Latency SLO:** The dashboard should aim to display new events within 2× the poll interval (i.e., <1.5s at 500ms default). This is achievable with MVP's sequential polling.

4. **Live gate evaluation feedback:** For a user watching a gate retry, 500ms latency is acceptable. They see the retry outcome within 1 second of submission, which is faster than they can react anyway.

### Open Questions

1. Should the dashboard batch session updates or fetch each session independently?
   - Recommend: fetch each session independently (simpler); parallel fetching deferred to V2.

2. Should `--once` (one-shot) mode skip the interval entirely?
   - Recommend: yes; `--once` is synchronous and returns immediately.

3. Should the interval be enforced as a minimum (prevent <100ms user foot-guns)?
   - Recommend: no minimum check in MVP; users own their system resources.

---

## Summary

The MVP `koto dashboard` command should be top-level (like `koto status` and `koto workflows`) to avoid namespace collision with future V2 daemon mode, which will use separate `koto daemon start/stop` commands. Epoch-branched sessions created by batch rewinds must be filtered from the main session list (shown only in an "archived_epochs" section when viewing a specific parent), to prevent dashboard noise as branches accumulate. Performance is acceptable for 100–500 sessions with 500ms polling; a single `backend.list()` scan costs ~200ms for 100 sessions, well within the interactive budget. The poll interval should default to 500ms (matching typical CI dashboards) and be user-overridable via `--interval`, with a `--once` flag for non-interactive use cases like scripting and testing.

