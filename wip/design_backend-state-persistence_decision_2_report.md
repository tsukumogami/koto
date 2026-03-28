# Decision 2: Migration strategy for CLI call sites

## Question
How should the 16 CLI call sites in `src/cli/mod.rs` be migrated from direct persistence I/O to backend-routed I/O?

## Decision: Option 1 -- Big-bang refactor

Replace all 16 call sites in one pass. Each `append_event(&state_path, ...)` becomes `backend.append_event(id, ...)`, each `read_events(&state_path)` becomes `backend.read_events(id)`, and `append_header` follows the same pattern.

## Rationale

**Option 3 (post-write hook) is eliminated immediately.** The problem statement requires reads to go through the backend too -- `CloudBackend` may need to pull before read. A post-write notification hook does nothing for reads. It also creates a "remember to call this" discipline problem across 16 sites that will silently fail when forgotten. This option doesn't solve the stated problem.

**Option 2 (helper wrapper) adds a layer that doesn't earn its keep.** The `state_io` module would wrap every persistence function to route through the backend -- but that's exactly what adding methods to `SessionBackend` already does (from Decision 1). A wrapper that calls `backend.append_event(id, ...)` is just an indirection over calling `backend.append_event(id, ...)` directly. The "smaller diff per call site" argument doesn't hold: each call site changes from `persistence::append_event(&state_path, payload, ts)` to `backend.append_event(id, payload, ts)` regardless of whether that call goes through a wrapper. The wrapper adds a module, adds tests for that module, and adds a question of "do I call the wrapper or the backend?" that the big-bang approach avoids entirely.

**Option 1 (big-bang) is the cleanest.** The 16 call sites are concentrated in 6 handler functions, all in a single file. The change at each site is mechanical: swap the function, swap the first argument from `&state_path` to session ID. This is a textbook search-and-replace refactor -- the kind that benefits from being done atomically so no call site is left in an inconsistent intermediate state.

### The closure in handle_next

The `append_closure` at line 1540 captures `state_path_clone` and passes it to the engine's advance loop. After migration, the closure captures a reference to the backend and the session ID instead:

```rust
let session_id = name.clone();
let mut append_closure = |payload: &EventPayload| -> Result<(), String> {
    backend.append_event(&session_id, payload, &now_iso8601())
        .map(|_| ())
        .map_err(|e| e.to_string())
};
```

The backend is already passed as `&dyn SessionBackend` to `handle_next`, so the closure can borrow it. The `state_path_clone` was needed because `state_path` was borrowed elsewhere; with backend routing, the session ID is a cheap `String` clone. This is actually simpler than the current pattern.

### Risk assessment

The 16 call sites are in 6 functions, all in one file, all following the same pattern. The refactor is mechanical. The main risk is a missed call site -- but a `grep` for `persistence::append_event\|persistence::read_events\|persistence::append_header` after the change catches any stragglers. Removing the persistence imports from `mod.rs` would cause a compile error if any direct call remains.

## Call site inventory

| Handler | reads | writes | Notes |
|---------|-------|--------|-------|
| handle_init | 0 | 3 | append_header + 2x append_event |
| handle_rewind | 1 | 1 | read_events + append_event |
| handle_next | 2 | 4 | 2x read_events, 3x append_event + 1 in closure |
| handle_decisions_record | 2 | 1 | 2x read_events + append_event |
| handle_decisions_list | 1 | 0 | read_events |
| handle_cancel | 1 | 1 | read_events + append_event |
| **Total** | **7** | **10** | **17 total** (note: actual count is 17, not 16) |

## Verification plan

1. Remove `persistence::{append_event, append_header, read_events}` from the CLI import block.
2. Compiler errors flag any missed call site.
3. Existing functional tests (which exercise init, next, rewind, cancel, decisions) validate end-to-end behavior unchanged for `LocalBackend`.
4. Cloud backend tests verify sync_push_state is called after writes.
