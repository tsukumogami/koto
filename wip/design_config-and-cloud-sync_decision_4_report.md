# Decision 4: Sync protocol and timing

## Decision

**Option 4: Sync on state-mutating + context add, per-key incremental** -- sync on every mutating command including `koto context add`, uploading only the changed key + manifest rather than the full session.

## Evaluation

### Option 4: Sync on state-mutating + context add, per-key incremental (chosen)

The PRD is explicit about sync timing. R8 lists the exact trigger commands: "On every state-mutating command (`koto init`, `koto next`, `koto context add`)." This rules out Options 2 and 3 outright. The question then becomes: full session upload (Option 1) or per-key incremental (Option 4)?

Per-key incremental wins on three grounds.

**Cost control under burst workloads.** During Phase 2 (discover), agents submit 5-10 context keys in quick succession. With full-session sync, each `koto context add` uploads the entire session -- 5-20 files at ~200KB total, repeated 5-10 times. That's 25-200 S3 PUT operations for files that haven't changed. Per-key incremental uploads 1 content file + 1 manifest update per `koto context add`, so the same burst costs 10-20 PUTs instead of 25-200. The manifest is tiny (a few KB of JSON), so the bandwidth difference is significant as sessions grow.

**Latency at the CLI boundary.** Each `koto context add` blocks until sync completes (or fails gracefully per R17). Uploading 1-2 small files is faster than uploading 5-20 files. Since agents call `koto context add` synchronously and wait for exit, shorter sync time means less wall-clock delay per command invocation.

**Download side is similar.** The "check remote version" step (R8 step 1) reads the remote manifest to compare versions. If the remote is newer, download only keys that differ. For full-session sync this would mean downloading everything; per-key incremental downloads only what changed. Since the common case is single-machine usage (remote is not newer), the download path rarely activates, but when it does, incremental is still cheaper.

The complexity argument against Option 4 is real but manageable. The implementation needs: (1) a manifest with per-key hashes (already exists -- `KeyMeta` has a `hash` field in `context.rs`), (2) a session-level version counter for conflict detection (Decision 5 covers this), and (3) upload logic that sends only the changed key file + updated manifest. The `ContextStore` trait already operates per-key (`add`, `get`, `ctx_exists`), so the cloud backend's `add` method naturally maps to "upload this key + manifest."

The S3 interaction pattern per `koto context add`:
1. GET manifest from remote (1 request)
2. Compare version counters
3. If remote is newer: GET only keys where remote hash differs from local (0-N requests, typically 0)
4. Perform local `add` operation
5. PUT the content file + PUT updated manifest (2 requests)

That's 3 requests in the common case (no remote changes). For `koto next`, the same pattern applies but the state file is also uploaded (4 requests).

**Strengths:**
- Directly satisfies PRD R8's sync trigger list (init, next, context add)
- Minimal S3 operations per command (2-4 PUTs in common case)
- Low latency per sync since only changed data transfers
- Existing `KeyMeta.hash` field enables incremental diffing without new schema
- `ContextStore` trait's per-key interface maps cleanly to per-key upload
- Scales to larger sessions without proportional cost increase

**Weaknesses:**
- More S3 requests than batching context syncs at state transitions (Option 2)
- Manifest must be read from remote on every mutating command (1 GET per command)
- Per-key locking (R5) must extend to remote operations to prevent manifest corruption from concurrent `context add` calls

### Option 1: Sync on every state-mutating command, full session (rejected)

Satisfies R8's trigger list but wastes bandwidth and S3 requests. During a 10-key burst, uploading the full session 10 times means re-uploading unchanged state files and earlier context keys repeatedly. For a session with 15 files, that's ~150 PUT operations instead of ~20. The simplicity advantage ("just tar and upload everything") doesn't justify the cost multiplier, especially since the per-key model aligns with the existing `ContextStore` trait interface.

### Option 2: Sync on state transitions only, incremental (rejected)

Directly violates PRD R8, which lists `koto context add` as a sync trigger. Context submitted between state transitions wouldn't be available remotely until the next `koto next`. If an agent submits research findings on machine A and the user wants to continue on machine B before advancing state, those findings are invisible. The PRD's design intent is clear: context is available remotely as soon as it's submitted.

### Option 3: Lazy sync with explicit flush (rejected)

Directly violates PRD R8's "invisible -- built into existing koto commands" requirement. An explicit `koto session sync` command is the opposite of invisible. Skills and agents would need to know about sync, adding token cost and complexity to every skill that produces context. The PRD ruled this out explicitly.

## Implementation notes

The `CloudBackend` should implement both `SessionBackend` and `ContextStore`, wrapping a `LocalBackend` for the local cache. Each trait method that mutates state follows the sync protocol: read remote manifest, merge if needed, perform local operation, upload changes.

For the manifest GET on every command: consider caching the remote manifest with a short TTL (e.g., 5 seconds) so that rapid-fire `koto context add` calls don't each make a separate GET. This is safe because R5 says sessions are single-writer per key, so the only scenario where the remote changes between calls is multi-machine usage, where a 5-second staleness window is acceptable.

R17 (resilience) means every S3 operation is wrapped in error handling that logs warnings and falls back to local-only. The version counter tracks "last successfully synced version" so the next mutating command retries the upload. Failed downloads on the check step mean the command proceeds with local state only.

Per-key locking for concurrent `context add` (R5) should use the same file-lock mechanism as the local backend, extended to cover the remote upload. Since agents write to different keys, the manifest update is the contention point -- serialize manifest writes with a session-level lock held for the duration of the remote PUT.
