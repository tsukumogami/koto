# Decision: Storage backend trait shape and engine integration

## Decision

**Option A: SessionBackend trait with sync-on-command wrapper.**

Option A wins because it cleanly separates backend-specific logic from CLI command logic, makes the sync protocol testable in isolation, and handles all three backends (local, cloud, git) through a single dispatch mechanism without conditional branching scattered through command handlers.

## Evaluation

### Option A: SessionBackend trait

Strengths:
- Each backend is a self-contained implementation. Adding a fourth backend (e.g., GCS) requires no changes to CLI commands.
- The trait boundary is a natural test seam. Cloud sync logic can be tested with a mock backend.
- Sync-on-command wrapping is explicit: each state-mutating command calls `sync_down`, does work, calls `sync_up`. The protocol is visible in one place.
- The trait can enforce the version counter check inside `sync_down`, so individual commands can't forget it.

Weaknesses:
- Slightly more code than Option B (trait definition, three implementations vs. one implementation + conditional).
- The trait must be designed carefully to avoid leaking backend-specific concerns.

### Option B: Layered cache model

Strengths:
- Simpler initial implementation. No trait dispatch, just `if cloud_configured { sync() }`.
- Fewer abstractions for a reader to understand.

Weaknesses:
- The "if cloud configured" conditional spreads into every state-mutating command, or requires a wrapper function that looks a lot like a trait anyway.
- The git backend becomes a special case (different base path) rather than a polymorphic implementation. That special-casing grows as backends get more distinct.
- Testing cloud sync requires either real S3 or mocking at the HTTP level. There's no clean interface to substitute.

**Verdict:** Option B converges toward Option A as complexity grows. Starting with the trait avoids a refactor later.

### Option C: Event-log-centric model

Strengths:
- Minimal S3 surface area (one file per session).
- Simpler conflict detection (compare single-file version counters).

Weaknesses:
- Breaks R11 (agents need filesystem paths to session artifacts, not just the event log). Skill artifacts like research outputs, plans, and decision reports wouldn't transfer between machines.
- Defeats the primary use case of cross-machine session resumption. An agent switching machines would have the engine state but none of the artifacts it references.
- The PRD explicitly says the backend operates on session directories as bundles, not per-file.

**Verdict:** Eliminated. Fails core requirements.

## Detailed design for Option A

### Trait definition

```rust
/// A session storage backend.
///
/// All methods operate on session directories as atomic bundles.
/// The local filesystem is always the working copy -- backends
/// that sync to remote storage download to a local directory first.
pub trait SessionBackend: Send + Sync {
    /// Create a new session directory. Returns the local filesystem
    /// path where the session's artifacts will live.
    ///
    /// For local: creates ~/.koto/sessions/<id>/
    /// For cloud: creates ~/.koto/sessions/<id>/ (same as local)
    /// For git: creates <repo>/<wip_path>/<id>/
    fn create(&self, id: &str) -> anyhow::Result<std::path::PathBuf>;

    /// Return the local filesystem path for an existing session.
    /// Does not create the directory or perform any I/O.
    fn session_dir(&self, id: &str) -> std::path::PathBuf;

    /// Download remote state to the local session directory.
    /// Returns the remote version number, or None if no remote
    /// state exists.
    ///
    /// Local backend: no-op, returns None.
    /// Cloud backend: downloads from S3 if remote is newer.
    /// Git backend: no-op, returns None.
    fn sync_down(&self, id: &str) -> anyhow::Result<Option<u64>>;

    /// Upload local session directory to remote storage.
    /// Increments and returns the new version number.
    ///
    /// Local backend: no-op, returns 0.
    /// Cloud backend: uploads to S3, bumps version.
    /// Git backend: no-op, returns 0.
    fn sync_up(&self, id: &str) -> anyhow::Result<u64>;

    /// Remove all session artifacts (local and remote).
    fn cleanup(&self, id: &str) -> anyhow::Result<()>;

    /// List all known sessions with metadata.
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
}

pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub last_modified: String,
    pub version: u64,
}
```

### Why these five methods and not more

- `create` + `session_dir` cover R2 and R4. `session_dir` is a pure path computation (no I/O) so agents can call it cheaply and repeatedly.
- `sync_down` + `sync_up` cover R5. They're separate rather than a single `sync` because the protocol requires checking remote state *before* local mutation and uploading *after*.
- `cleanup` covers R9/R10. It removes both local and remote artifacts in one call.
- `list` covers R9. It returns only local sessions (cloud backend doesn't list remote-only sessions; you'd need to sync them down first).

Methods deliberately excluded:
- No `resolve` method on the trait. Conflict resolution (`koto session resolve`) is a CLI-level operation that picks a winner and calls `sync_up` to overwrite. The backend doesn't need special logic for it.
- No `exists` method. `session_dir` returns a path; the caller checks `path.exists()`.

### Integration into CLI commands

The `run()` function in `src/cli/mod.rs` constructs the backend once from configuration, then passes it to command handlers. State-mutating commands follow a three-phase protocol:

```
Phase 1: sync_down(id)  -- pull remote state if newer
Phase 2: <command logic> -- read state, mutate, write events
Phase 3: sync_up(id)    -- push updated state to remote
```

Concrete integration points:

**`koto init`** (creates workflow + session):
```
1. backend.create(name)        -- creates session directory
2. write header + init events  -- existing persistence.rs logic, unchanged
3. backend.sync_up(name)       -- upload initial state
```

**`koto next --with-data`** (submits evidence, may transition):
```
1. backend.sync_down(name)     -- ensure local state is current
2. read_events, evaluate gate, append_event  -- existing logic
3. backend.sync_up(name)       -- upload if state changed
```

**`koto next`** (read-only, no data):
```
1. backend.sync_down(name)     -- ensure local state is current
2. read_events, derive state, emit directive  -- existing logic
3. (no sync_up -- nothing changed)
```

**`koto rewind`**:
```
1. backend.sync_down(name)
2. existing rewind logic
3. backend.sync_up(name)
```

**`koto cancel`**:
```
1. backend.sync_down(name)
2. existing cancel logic
3. backend.sync_up(name)
```

The sync wrapper can be extracted into a helper:

```rust
fn with_sync<F, T>(backend: &dyn SessionBackend, id: &str, mutating: bool, f: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    backend.sync_down(id)?;
    let result = f()?;
    if mutating {
        backend.sync_up(id)?;
    }
    Ok(result)
}
```

This keeps sync logic out of individual command handlers.

### S3 sync protocol

The cloud backend wraps the local backend (session directories always live at `~/.koto/sessions/<id>/`). Cloud adds remote mirroring.

**Upload (sync_up):**
1. Read `session.meta.json` from the session directory. Extract `version`.
2. Increment version: `new_version = version + 1`.
3. Write updated `session.meta.json` with `new_version`.
4. List all files in the session directory.
5. For each file, upload to `s3://<bucket>/sessions/<id>/<filename>` using a PUT request.
6. Upload `session.meta.json` last (acts as the commit marker).

**Download (sync_down):**
1. Fetch `s3://<bucket>/sessions/<id>/session.meta.json`.
2. If not found (404), return `None` -- no remote state exists.
3. Parse remote version from the fetched metadata.
4. Read local `session.meta.json`. Compare versions.
5. If remote version > local version: download all files from the S3 prefix, overwriting local. Return `Some(remote_version)`.
6. If remote version == local version: no-op. Return `Some(local_version)`.
7. If remote version < local version: no-op (local is ahead, will be pushed on sync_up). Return `Some(local_version)`.
8. If remote version != local version and local version != remote version - 1 and local has been modified since last sync: conflict. Return error per R6.

**S3 client:** Use `aws-sdk-s3` or a minimal HTTP client with AWS Signature V4. Given the small number of operations (PUT, GET, LIST, DELETE on a single prefix), a minimal client avoids pulling in the full AWS SDK dependency tree. However, the full SDK handles endpoint discovery, retries, and credential chain resolution. Recommend starting with `aws-sdk-s3` behind a feature flag (`cloud` feature) so the dependency is opt-in.

**Resilience (R15):** `sync_up` failures are caught and logged as warnings. A `sync_pending` flag is written to `session.meta.json`. On the next `sync_down`, if `sync_pending` is true, koto retries the upload before checking remote state.

### Version counter: location and update mechanism

**Location:** `session.meta.json` inside the session directory.

```json
{
  "id": "issue-42-exploration",
  "created_at": "2026-03-24T10:00:00Z",
  "version": 5,
  "sync_pending": false,
  "backend": "cloud"
}
```

Why in the session directory rather than a separate database:
- Self-contained: the session directory is the complete unit of state. No external index to corrupt or lose.
- Portable: moving a session directory (backup, manual copy) carries its version counter.
- Simple: no SQLite, no global lock file, no registry.

**Update mechanism:**
- `create()` writes `session.meta.json` with `version: 0`.
- Every `sync_up()` increments the version before uploading.
- `sync_down()` overwrites the local `session.meta.json` with the remote copy when the remote is newer.
- The version counter is monotonic. It never decreases. Rewinding a workflow doesn't rewind the version -- the version tracks sync state, not workflow state.

### Conflict detection flow

Conflicts arise when two machines both advance the same session without syncing.

**Detection (inside sync_down):**
1. Read local `session.meta.json`: `local_version = 5`.
2. Fetch remote `session.meta.json`: `remote_version = 7`.
3. Check: was the last successful sync at version 5? (stored as `last_synced_version` in local metadata.)
4. If `local_version == last_synced_version`: no local changes since last sync. Remote is simply ahead. Download remote state.
5. If `local_version > last_synced_version`: local has unsynchronized changes. Remote also advanced. **Conflict.**

**Conflict error:**
```json
{
  "error": "session conflict: local version 6, remote version 7",
  "hint": "run 'koto session resolve --keep local' or 'koto session resolve --keep remote'"
}
```

**Resolution (`koto session resolve`):**
- `--keep local`: increment local version to `max(local, remote) + 1`, force `sync_up`. Remote is overwritten.
- `--keep remote`: discard local session directory, `sync_down` to get remote state. Local is overwritten.

Both paths clear the conflict state and resume normal operation.

### Metadata fields in session.meta.json (complete)

```json
{
  "id": "issue-42-exploration",
  "created_at": "2026-03-24T10:00:00Z",
  "version": 5,
  "last_synced_version": 4,
  "sync_pending": false,
  "backend": "cloud"
}
```

- `version`: bumped on every successful sync_up.
- `last_synced_version`: the version at which local and remote were last known to agree. Used for conflict detection.
- `sync_pending`: true if the last sync_up failed. Triggers retry on next command.
- `backend`: which backend created this session. Prevents accidental backend mismatch.

### Backend implementations summary

| Method | Local | Cloud | Git |
|--------|-------|-------|-----|
| `create` | mkdir `~/.koto/sessions/<id>/` | same as local + initial sync_up | mkdir `<repo>/<wip_path>/<id>/` |
| `session_dir` | `~/.koto/sessions/<id>` | same as local | `<repo>/<wip_path>/<id>` |
| `sync_down` | no-op, returns None | fetch remote meta, download if newer | no-op, returns None |
| `sync_up` | no-op, returns 0 | upload all files, bump version | no-op, returns 0 |
| `cleanup` | rm -rf local dir | rm -rf local dir + delete S3 prefix | rm -rf local dir |
| `list` | scan `~/.koto/sessions/` | scan `~/.koto/sessions/` (local only) | scan `<repo>/<wip_path>/` |

### Dependency impact

New dependencies (behind `cloud` feature flag):
- `aws-sdk-s3` + `aws-config` for S3 operations
- `tokio` (required by AWS SDK, but only activated with the cloud feature)
- `toml` for config file parsing (needed regardless of backend)

The local and git backends add zero new dependencies beyond what's already in Cargo.toml.

### Migration path

1. Add `SessionBackend` trait and `LocalBackend` implementation.
2. Refactor CLI commands to accept a `&dyn SessionBackend` and use `with_sync` wrapper.
3. Add `GitBackend` implementation.
4. Add `CloudBackend` behind the `cloud` feature flag.
5. Add `koto config` subcommand for backend selection.
6. Add `koto session` subcommands.

Steps 1-2 can ship without changing any external behavior -- the local backend with no-op sync is equivalent to the current behavior.
