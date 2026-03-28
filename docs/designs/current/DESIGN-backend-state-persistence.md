---
status: Current
upstream: docs/designs/current/DESIGN-config-and-cloud-sync.md
problem: |
  State file I/O (append_event, read_events) happens directly in the CLI layer via
  persistence module functions, bypassing the SessionBackend trait entirely. CloudBackend
  never sees state mutations, so cloud sync silently does nothing. 16 direct I/O call
  sites in cli/mod.rs write to file paths that CloudBackend can't intercept.
decision: |
  Add state I/O methods (append_header, append_event, read_events, read_header) to
  the SessionBackend trait. LocalBackend delegates to the persistence module.
  CloudBackend delegates to local then syncs. Big-bang refactor of all 16 CLI call
  sites to route through the backend instead of calling persistence functions directly.
rationale: |
  State I/O is core session lifecycle, not a separate concern. Adding methods to the
  existing trait keeps sync automatic inside the implementation. Big-bang refactor is
  mechanical (swap function name, swap path to session ID) and removing persistence
  imports from the CLI catches missed sites at compile time.
---

# DESIGN: Backend-owned state persistence

## Status

Proposed

## Context and problem statement

koto's `SessionBackend` trait manages session directories (create, cleanup, list) and
`ContextStore` manages content (add, get, exists). But state file I/O — the JSONL
event log that tracks workflow state — happens directly in the CLI layer via
`append_event()` and `read_events()` from `src/engine/persistence.rs`.

This means `CloudBackend` never sees state mutations. When `handle_init` writes the
initial state file, when `handle_next` appends transition events, when `handle_cancel`
writes cancellation events — none go through the backend. Cloud sync was designed to
call `sync_push_state()` from `create()`, but the state file doesn't exist yet at
that point.

16 direct I/O call sites in `src/cli/mod.rs` bypass the backend:
- 8 `append_event()` calls (writes)
- 1 `append_header()` call (write)
- 5 `read_events()` calls (reads)
- 2 `read_header()` calls (reads)

These are spread across 6 handler functions: `handle_init`, `handle_next`,
`handle_rewind`, `handle_cancel`, `handle_decisions_record`, `handle_decisions_list`.

## Decision drivers

- State mutations must be visible to CloudBackend for sync to work
- Reads must go through the backend too (CloudBackend pulls before read)
- Minimize disruption to the persistence module's format logic
- Don't change the JSONL event log format
- LocalBackend must work identically to today
- Removing persistence imports from CLI should catch missed call sites at compile time

## Considered options

### Decision 1: How to extend SessionBackend with state I/O

**Context**: The trait needs state read/write methods. Three approaches.

**Chosen: Add methods to SessionBackend.**

State I/O is core session lifecycle — not a separate concern like content storage.
Adding 4 methods to the existing trait keeps sync automatic inside the implementation.
No new traits, no manual sync calls.

New methods:

```rust
pub trait SessionBackend: Send + Sync {
    // Existing methods...
    fn create(&self, id: &str) -> Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn exists(&self, id: &str) -> bool;
    fn cleanup(&self, id: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<SessionInfo>>;

    // New state I/O methods
    fn append_header(&self, id: &str, header: &StateFileHeader) -> Result<()>;
    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str) -> Result<()>;
    fn read_events(&self, id: &str) -> Result<(StateFileHeader, Vec<Event>)>;
    fn read_header(&self, id: &str) -> Result<StateFileHeader>;
}
```

LocalBackend resolves the path and delegates to the persistence module.
CloudBackend delegates to `self.local`, then syncs after writes and pulls before reads.

**Rejected**: Separate `StateStore` trait — adds a third trait for no benefit since
state I/O is inseparable from session lifecycle. Post-write hook (`sync_state()`) —
error-prone (16 sites must each remember the call) and doesn't solve reads.

### Decision 2: Migration strategy for 16 CLI call sites

**Context**: All 16 call sites are in `src/cli/mod.rs`, in 6 handler functions.

**Chosen: Big-bang refactor.**

The change is mechanical: swap `persistence::append_event(&state_path, payload, ts)`
with `backend.append_event(id, payload, ts)`. Swap `persistence::read_events(&state_path)`
with `backend.read_events(id)`. Remove `session_state_path()` helper since the
backend resolves paths internally.

After migration, remove `persistence` imports from `src/cli/mod.rs`. Any missed call
site becomes a compile error.

The closure in `handle_next`'s advancement loop currently captures a `state_path`
clone. After refactor, it captures a backend reference and session ID string — simpler.

**Rejected**: Helper wrapper module — adds indirection for zero benefit since backend
methods are called directly. Post-write hook — doesn't solve reads (CloudBackend
needs pull-before-read).

## Decision outcome

SessionBackend gains 4 state I/O methods. All 16 CLI call sites are refactored in
one commit to call backend methods instead of persistence functions directly.

LocalBackend is a thin delegation layer: resolves path, calls persistence module.
CloudBackend delegates to `self.local`, then syncs (push after writes, pull before
reads).

The persistence module (`src/engine/persistence.rs`) keeps its format logic unchanged.
It's called by LocalBackend, not by the CLI.

## Solution architecture

### SessionBackend trait extension

```rust
pub trait SessionBackend: Send + Sync {
    fn create(&self, id: &str) -> Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn exists(&self, id: &str) -> bool;
    fn cleanup(&self, id: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<SessionInfo>>;

    fn append_header(&self, id: &str, header: &StateFileHeader) -> Result<()>;
    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str) -> Result<()>;
    fn read_events(&self, id: &str) -> Result<(StateFileHeader, Vec<Event>)>;
    fn read_header(&self, id: &str) -> Result<StateFileHeader>;
}
```

### LocalBackend implementation

```rust
impl SessionBackend for LocalBackend {
    fn append_header(&self, id: &str, header: &StateFileHeader) -> Result<()> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::append_header(&path, header)
    }

    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str) -> Result<()> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::append_event(&path, payload, timestamp)
    }

    fn read_events(&self, id: &str) -> Result<(StateFileHeader, Vec<Event>)> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::read_events(&path)
    }

    fn read_header(&self, id: &str) -> Result<StateFileHeader> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::read_header(&path)
    }
}
```

### CloudBackend implementation

```rust
impl SessionBackend for CloudBackend {
    fn append_header(&self, id: &str, header: &StateFileHeader) -> Result<()> {
        self.local.append_header(id, header)?;
        self.sync_push_state(id);  // Now state file exists!
        Ok(())
    }

    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str) -> Result<()> {
        self.local.append_event(id, payload, timestamp)?;
        self.sync_push_state(id);  // Sync after every write
        Ok(())
    }

    fn read_events(&self, id: &str) -> Result<(StateFileHeader, Vec<Event>)> {
        self.sync_pull_state(id);  // Pull before read
        self.local.read_events(id)
    }

    fn read_header(&self, id: &str) -> Result<StateFileHeader> {
        self.sync_pull_state(id);  // Pull before read
        self.local.read_header(id)
    }
}
```

### CLI migration (handle_init example)

Before:
```rust
let state_path = session_state_path(backend, &name);
persistence::append_header(&state_path, &header)?;
persistence::append_event(&state_path, &init_payload, &ts)?;
persistence::append_event(&state_path, &transition_payload, &ts)?;
```

After:
```rust
backend.append_header(&name, &header)?;
backend.append_event(&name, &init_payload, &ts)?;
backend.append_event(&name, &transition_payload, &ts)?;
```

### Backend enum delegation

The `Backend` enum in `src/session/mod.rs` gains 4 new match arms:

```rust
impl SessionBackend for Backend {
    fn append_header(&self, id: &str, header: &StateFileHeader) -> Result<()> {
        match self {
            Backend::Local(b) => b.append_header(id, header),
            Backend::Cloud(b) => b.append_header(id, header),
        }
    }
    // ... same pattern for append_event, read_events, read_header
}
```

### Key interfaces

| Interface | Location | Change |
|-----------|----------|--------|
| `SessionBackend` trait | `src/session/mod.rs` | +4 methods |
| `LocalBackend` | `src/session/local.rs` | +4 method impls (thin delegation) |
| `CloudBackend` | `src/session/cloud.rs` | +4 method impls (delegate + sync) |
| `Backend` enum | `src/session/mod.rs` | +4 match arms |
| `src/cli/mod.rs` | all handlers | 16 call sites migrated |
| `session_state_path()` | `src/cli/mod.rs` | removed (backend resolves paths) |

### What stays the same

- `src/engine/persistence.rs` — unchanged, still has format logic
- JSONL event log format — unchanged
- `ContextStore` trait — unchanged
- `state_file_name()` free function — still used by LocalBackend internally

## Implementation approach

### Phase 1: Extend trait and implement for LocalBackend

Add 4 methods to `SessionBackend`. Implement in `LocalBackend` by delegating to
the persistence module. Update `Backend` enum. All existing tests pass because CLI
still calls persistence directly (trait has default impls or tests are updated).

### Phase 2: Migrate CLI call sites

Big-bang refactor of all 16 call sites in `src/cli/mod.rs`. Remove `persistence`
imports from CLI. Remove `session_state_path()` helper. Compiler catches any missed
sites.

### Phase 3: CloudBackend sync on state I/O

Update `CloudBackend` implementations to sync after writes and pull before reads.
Remove the now-unnecessary `sync_push_state()` call from `create()`.

### Phase 4: Verify cloud sync end-to-end

Run cloud integration tests against R2. Manually verify state files appear in the
bucket after `koto init` and `koto next`.

## Security considerations

No new security surface. State file permissions (0600) are still enforced by the
persistence module. Cloud transport security is unchanged (HTTPS via rust-s3/rustls).
The refactor doesn't change what data is stored or transmitted — only the code path
through which it flows.

## Consequences

### Positive

- Cloud sync actually works for state files
- All state I/O goes through one abstraction — future backends get state sync for free
- CLI handlers are simpler (no path construction, no persistence imports)
- Compiler catches missed call sites after removing persistence imports

### Negative

- SessionBackend trait grows from 5 to 9 methods
- Every state write in CloudBackend triggers an S3 upload (8 uploads during a typical
  koto next call with advancement). Acceptable for a CLI that runs per-command.
- Reads pull from S3 before reading local (adds latency). Mitigated by the existing
  manifest TTL cache pattern.

### Mitigations

- S3 uploads are non-fatal (same pattern as context sync)
- The persistence module is unchanged — format logic is not duplicated
- LocalBackend's implementations are 4-line delegations each
