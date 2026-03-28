# Decision 6: CloudBackend trait implementation

## Question

How does CloudBackend implement SessionBackend + ContextStore, what S3 crate to use, and how does the cargo feature flag work?

## Chosen: Option 1 -- Local cache + sync wrapper around LocalBackend, with rust-s3 crate

### Architecture

CloudBackend wraps a LocalBackend. All operations execute locally first through the inner LocalBackend, then sync to/from S3 as a separate concern. The local cache IS the LocalBackend's storage.

```rust
#[cfg(feature = "cloud")]
pub struct CloudBackend {
    local: LocalBackend,
    s3: S3Client,     // wraps rust-s3 bucket handle
    prefix: String,   // S3 key prefix (repo-id scoped)
}
```

CloudBackend implements both `SessionBackend` and `ContextStore` by delegating every call to `self.local` and then performing the corresponding S3 operation. For reads, it pulls from S3 first (if the local cache is stale or missing), then delegates to local. For writes, it writes locally first, then pushes to S3.

### Why wrap LocalBackend instead of reimplementing

LocalBackend already handles all the filesystem mechanics: directory creation, flock-based manifest locking, atomic temp-file writes, path traversal validation, and `ensure_koto_root` permissions. Reimplementing this in CloudBackend would duplicate 200+ lines of tested code. The wrapper approach means CloudBackend inherits all of this behavior and only adds the sync layer.

The trait methods are all synchronous (`fn add(&self, ...) -> anyhow::Result<()>`), so CloudBackend blocks on S3 calls within the same method. This is acceptable because koto operations are CLI-invoked (one operation per command), not high-throughput server workloads.

### S3 crate: rust-s3

`rust-s3` (crate name `rust-s3`) is the right fit for three reasons:

1. **Sync-friendly**: Provides blocking APIs without requiring a tokio runtime. Koto is currently fully synchronous -- adding tokio as a required dependency for cloud support would increase compile times and binary size significantly for a feature that makes a handful of HTTP calls per CLI invocation.

2. **Non-AWS provider support**: Explicitly supports custom endpoints (Cloudflare R2, MinIO, etc.) via the `Bucket::new` region/endpoint configuration. The PRD requires non-AWS provider support.

3. **Lighter dependency tree**: Pulls in fewer transitive dependencies than `aws-sdk-s3`, which requires the full AWS SDK chain (aws-config, aws-credential-types, aws-smithy-runtime, etc.).

### Feature flag structure

```toml
[features]
default = []
cloud = ["dep:rust-s3"]

[dependencies]
rust-s3 = { version = "0.35", optional = true }
```

The `cloud` feature gates:
- The `CloudBackend` struct and its `SessionBackend`/`ContextStore` impls
- The `S3Client` wrapper module
- Any config keys related to cloud sync (`sync.bucket`, `sync.endpoint`, etc.)
- CLI flags or subcommands that trigger sync operations

Default `cargo install koto` excludes cloud entirely. Users opt in with `cargo install koto --features cloud`.

### Sync protocol sketch

**Push (after write operations):**
1. Local operation completes via `self.local`
2. Upload changed file(s) to S3 at `{prefix}/{session}/{relative_path}`
3. If S3 upload fails, log a warning but don't fail the operation (R17: local operations work when cloud is unreachable)

**Pull (before read operations):**
1. Check S3 for the object's ETag or last-modified
2. If remote is newer (or local is missing), download to local cache
3. Delegate read to `self.local`
4. If S3 is unreachable, fall back to local cache (R17)

**Session list:**
1. Merge local sessions with S3-listed sessions
2. Download any sessions that exist remotely but not locally

### Offline resilience (PRD R17)

CloudBackend treats S3 failures as non-fatal for all operations. The local cache always has the last-known state. When connectivity returns, the next operation syncs. This is a deliberate tradeoff: eventual consistency between machines, but guaranteed local availability.

## Rejected options

### Option 2: Direct S3 operations

Every read/write hitting S3 directly means every CLI command incurs network latency. `koto next` (the most frequent operation, called by agents between every state transition) would add 100-500ms per call. More critically, it fails completely when offline, violating R17. There's no local fallback because there's no local state. The only advantage -- no cache consistency issues -- doesn't justify the latency and fragility costs.

### Option 3: Hybrid with async background sync

Introduces a background thread for S3 sync, which means the process might exit before sync completes. In koto's usage pattern (short-lived CLI invocations), the background thread would almost never finish. You'd need a daemon or sync-on-next-invocation mechanism, which is just Option 1 with extra complexity. The eventual consistency guarantee is identical to Option 1, but Option 1 achieves it with simpler, deterministic sync points.

### S3 crate: aws-sdk-s3

Requires tokio runtime. Even with `tokio::runtime::Builder::new_current_thread()` to run blocking, it pulls in tokio, hyper, tower, and the full AWS SDK stack. This adds 30+ transitive dependencies and increases compile time substantially. Overkill for a feature that makes a handful of S3 calls per CLI command.

### S3 crate: reqwest + manual signing

Reimplementing S3 request signing (SigV4) is error-prone and would need ongoing maintenance as providers evolve. The savings over rust-s3 are marginal -- rust-s3 already uses reqwest internally. This trades a well-tested library for a hand-rolled implementation with no clear benefit.

## Assumptions

- koto will remain a synchronous CLI tool for the foreseeable future. If koto ever adopts an async runtime for other reasons, the S3 crate choice could be revisited.
- rust-s3's blocking mode works without tokio. If a future rust-s3 version drops sync support, we'd pin the version or switch crates.
- Conflict resolution between machines (two machines writing different state for the same session) is out of scope for this decision. The sync protocol uses last-writer-wins semantics.
- Decision 1's config format (TOML) will store cloud credentials/endpoints under a `[sync]` table, with credential values sourced from environment variables per Decision 3.
