# Decision: How to extend SessionBackend with state I/O methods

## Choice: Option 1 -- Add methods to SessionBackend

## Rationale

### State I/O belongs in SessionBackend because it is core session lifecycle

The four persistence operations (`append_header`, `append_event`, `read_events`, `read_header`) define what a session *is*. A session without state I/O is just a directory. Putting these methods on the same trait that manages session creation, existence checks, and cleanup keeps the abstraction coherent. Callers get one object that handles the full session lifecycle.

### The separate ContextStore trait exists because it is a genuinely different concern

ContextStore is a key-value content store -- agents put/get arbitrary blobs by name. It has its own manifest, locking, and sync semantics. That justified a separate trait. State I/O is not analogous: it is append-only JSONL with sequence validation, and it is the primary data the session manages. Modeling it as a second separate trait (Option 2) would fragment the session abstraction without any concrete benefit.

### Format logic stays in the persistence module

The trait methods on LocalBackend delegate to `persistence::append_header(path, ...)`, `persistence::read_events(path, ...)`, etc. The path is computed from `self.session_dir(id)` + `state_file_name(id)`. No JSONL parsing or sequence validation code moves into the trait implementation. CloudBackend delegates to `self.local.<method>(...)` then calls `self.sync_push_state(id)`, exactly like it does for ContextStore operations today.

### Option 3 is fragile at scale

There are 16 call sites in cli/mod.rs. Each would need a manual `backend.sync_state(id)` call after every persistence operation. Missing one means silent data loss in cloud mode. Option 1 makes sync automatic inside the trait implementation.

### Option 2 adds mechanical cost with no payoff

A third trait means a third `impl ... for Backend` dispatch block (5 match arms per method, 4 methods = 20 lines of boilerplate). koto controls all implementations, so there is no extensibility argument for keeping the traits separate. The Backend enum already dispatches SessionBackend and ContextStore; adding state I/O to SessionBackend costs zero additional dispatch infrastructure.

## Methods to add

```rust
pub trait SessionBackend: Send + Sync {
    // ... existing methods ...

    /// Write the header line to a new state file.
    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()>;

    /// Append an event to the state file, returning the assigned seq number.
    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str) -> anyhow::Result<u64>;

    /// Read all events from the state file.
    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)>;

    /// Read the header line from a state file.
    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader>;
}
```

## Migration plan

1. Add the four methods to the `SessionBackend` trait.
2. Implement on `LocalBackend`: compute path, delegate to `persistence::*` functions.
3. Implement on `CloudBackend`: delegate to `self.local.*`, then call version check + `sync_push_state` for writes (same pattern as ContextStore).
4. Add dispatch arms to the `Backend` enum.
5. Migrate cli/mod.rs call sites from `persistence::append_event(&state_path, ...)` to `backend.append_event(id, ...)`. Remove the manual `state_path` computation at each site.

## Risks

- **Trait size**: SessionBackend grows from 5 to 9 methods. Acceptable given that all methods are session-lifecycle operations.
- **Signature change**: The trait methods take `id: &str` instead of `path: &Path`. The path is an implementation detail that callers should not need to compute.
