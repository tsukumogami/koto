# Decision 5: Version counter and conflict detection

## Question

How does koto detect when two machines have diverged, and how does resolution work?

## Chosen: Monotonic version counter in session metadata

A `version.json` file in the session directory holds a monotonic counter. Every state-mutating operation (`koto init`, `koto next`, `koto context add`) increments the counter after completing locally. On sync:

- **Remote version > local**: remote advanced while this machine was idle. Download remote state before proceeding.
- **Remote version < local**: this machine advanced. Upload local state.
- **Both advanced past the last-sync base**: conflict. Return error with both version numbers.

### Version file format

```json
{
  "version": 7,
  "last_sync_base": 5,
  "machine_id": "a1b2c3"
}
```

`version` is the current local version. `last_sync_base` is the version at the last successful sync. `machine_id` is a random identifier generated on first use (stored in user config), used only for diagnostics.

### Divergence detection algorithm

On every state-mutating command, before performing the local operation:

1. Read local `version.json`: local_version = L, last_sync_base = B
2. Read remote `version.json`: remote_version = R
3. Compare:
   - If R == B: remote hasn't changed since last sync. Proceed locally, increment L, upload.
   - If R > B and L == B: remote advanced, local didn't. Download remote, apply local operation, increment, upload.
   - If R > B and L > B: both advanced. **Conflict error.** "session conflict: local version L, remote version R"
   - If remote doesn't exist: first sync. Upload after local operation.

### Resolution

`koto session resolve --keep local` uploads local state to remote, setting both version and last_sync_base. `koto session resolve --keep remote` downloads remote state, replacing local, and updates last_sync_base. In both cases the resolved version is max(L, R) + 1 to establish a clean baseline.

## Why not the alternatives

### Last-modified timestamp comparison

Clock skew between machines can produce incorrect ordering. Two machines with clocks 30 seconds apart could disagree on which write was "newer," causing silent data loss. This directly violates PRD R9's requirement that diverged versions produce an error. Per-file timestamp comparison also conflicts with the whole-session resolution model -- R9 specifies `--keep local|remote` at the session level, not per-file merging.

### Content hash comparison

Content hashes detect *what* changed but not *when* or *in what order*. The PRD R8 sync protocol requires determining whether the remote is "newer" (download first) or "older" (upload). Hashes alone can't establish this ordering without an additional version counter, at which point you're just adding hashes on top of option 1 for no benefit. Content hashing also requires reading remote hashes before every operation, doubling S3 GET requests in the common (non-conflict) case.

### Vector clock per machine

Vector clocks detect concurrent modification precisely, which is their strength in distributed systems where per-key merge matters. But koto's resolution model is explicitly whole-session: pick local or remote, not per-key reconciliation. A vector clock's extra precision goes unused while adding storage complexity (unbounded vector size as machines join) and implementation complexity (comparison logic, garbage collection of stale machine entries). The monotonic counter captures the only distinction that matters: did both sides advance, or just one?

## Assumptions

- **One machine advances at a time in the common case.** Conflicts are rare. The counter optimizes for the fast path (one-sided advance) and treats divergence as an error to resolve manually.
- **Session-level versioning, not per-file.** The counter tracks the session as a unit. Individual file changes within a session don't get their own version. This matches the sync unit decision (full session sync, not individual files).
- **Machine ID is for diagnostics only.** It's not part of the conflict detection logic. It helps users understand which machine made which version when debugging a conflict.
- **`last_sync_base` is set after successful upload and download.** A failed upload leaves `last_sync_base` unchanged, so the next command retries the upload correctly.
