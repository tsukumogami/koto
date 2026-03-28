---
status: Accepted
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto sessions are machine-local. A workflow started on one machine can't be
  resumed on another without manually copying ~/.koto/sessions/. Adding cloud
  sync requires a config system for backend selection, endpoints, and credentials.
  Neither exists today.
decision: |
  TOML config at ~/.koto/config.toml (user) and .koto/config.toml (project),
  with project > user > default precedence. koto config get/set/unset/list CLI.
  Credentials blocked from project config, env vars override config values.
  CloudBackend wraps LocalBackend and syncs per-key to S3 via rust-s3 (sync,
  no tokio). Monotonic version counter for conflict detection. Cloud feature
  behind a cargo feature flag.
rationale: |
  Config and cloud sync are designed together because the config system's hardest
  consumer is cloud (credentials, env overrides, security). CloudBackend wraps
  LocalBackend so all filesystem logic is reused -- sync is a clean layer on top.
  rust-s3 keeps koto synchronous. Per-key incremental sync minimizes S3 costs
  during burst workloads.
---

# DESIGN: Config system and cloud sync

## Status

Accepted

## Context and problem statement

koto's `LocalBackend` is the only storage backend. It's hardcoded in `build_backend()`
with no way to select an alternative. Sessions live at `~/.koto/sessions/<repo-id>/<name>/`
on one machine. To resume a workflow elsewhere, you'd need to copy that directory
manually.

The PRD (R8, R9, R11) specifies implicit cloud sync via S3-compatible storage, with
a config system for backend selection and credentials. These are Features 2 and 4 in
the session persistence roadmap. They're designed together because the config system's
hardest consumer is cloud sync (credentials, env var overrides, security constraints),
and cloud sync has no value without config to enable it.

Feature 1 (local storage + content ownership) shipped in PR #84. The `SessionBackend`
and `ContextStore` traits are implemented. `CloudBackend` needs to implement both.

## Decision drivers

- Cloud sync must be invisible to agents — zero new commands, zero token cost
- Credentials must never live in project config (committed to git = supply chain risk)
- S3 dependency (rust-s3) must be behind a feature flag — default builds stay light
- Config system is general-purpose, not session-specific
- Conflict detection must handle the "two machines advanced the same workflow" case
- Local backend must remain the zero-config default
- koto must remain a synchronous CLI (no tokio runtime in default builds)

## Considered options

### Decision 1: Config file format and locations

**Context**: koto needs persistent configuration for backend selection, cloud
endpoints, and other settings. Where do config files live and what format?

**Chosen: TOML with dotted key paths at two locations.**

User config at `~/.koto/config.toml`. Project config at `.koto/config.toml`
(committed to git, shared with team). Dotted keys map to TOML tables:
`session.backend = "cloud"` becomes `[session]\nbackend = "cloud"`. Precedence:
project > user > default.

`koto config set` writes to user config by default. `koto config set --project`
writes to project config.

**Rejected**: Flat env-file (no nesting, contradicts PRD R11 TOML spec). JSON
(no comments, harder to hand-edit). Separate files per concern (unnecessary
complexity for koto's small config surface).

### Decision 2: Config CLI commands

**Context**: How do users and agents interact with configuration?

**Chosen: get/set/unset/list subcommands.**

```
koto config get <key>                    # print raw value, exit 1 if unset
koto config set <key> <value>            # write to user config
koto config set --project <key> <value>  # write to project config
koto config unset <key>                  # remove from user config
koto config unset --project <key>        # remove from project config
koto config list                         # dump resolved config as TOML
koto config list --json                  # machine-readable resolved config
```

Positional args for key/value. `list` resolves all layers (defaults + user +
project + env overrides) and shows the final result.

**Rejected**: Omitting `list` (loses debuggability for zero savings). Interactive
wizard (incompatible with agent callers).

### Decision 3: Credential handling (critical)

**Context**: S3 credentials are secrets. They must not leak into git-committed
project config. They must work in CI/CD (env vars) and on developer machines
(persistent storage).

**Chosen: Env var override with config-file blocklist.**

Credential resolution order: env vars > user config > error.

Credentials (`session.cloud.access_key`, `session.cloud.secret_key`) can be stored
in user config (`~/.koto/config.toml`) but are blocked from project config. `koto
config set --project session.cloud.access_key` returns an error. Env vars
`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` override config values.

The blocklist is a static array of key prefixes checked at write time. Any key
matching `session.cloud.access_key` or `session.cloud.secret_key` is rejected
for project config.

**Rejected**: Env vars only (worse developer ergonomics, no persistent credential
storage). AWS SDK credential chain (heavy dependency, designed for AWS not
S3-compatible stores, requires async runtime). Encrypted credential store (master
password breaks CI/CD, keyring adds platform dependencies).

### Decision 4: Sync protocol and timing

**Context**: When does koto sync with the cloud? What's the sync unit?

**Chosen: Per-key incremental sync on every mutating command.**

Sync triggers: `koto init`, `koto next`, `koto context add`, `koto session cleanup`.

Per `koto context add` (typical case):
1. GET remote manifest (1 request)
2. Compare version counters (Decision 5)
3. If remote newer: GET only keys where hash differs (0-N requests)
4. Perform local operation
5. PUT changed content file + updated manifest (2 requests)

Common case is 3 S3 requests. The manifest's `KeyMeta.hash` field enables
incremental diffing — only changed keys are transferred. A short TTL cache
(~5 seconds) on the remote manifest reduces GETs during rapid sequential calls
(e.g., 5-10 context adds in a discover phase).

**Rejected**: Full-session sync (wastes bandwidth during burst submissions — 150
PUTs vs 20 for a 10-key burst). Sync on state transitions only (violates PRD R8
which includes `koto context add`). Lazy sync with explicit flush (violates
invisible sync requirement).

### Decision 5: Version counter and conflict detection

**Context**: How does koto detect when two machines have diverged?

**Chosen: Monotonic version counter with `last_sync_base`.**

A `version.json` file in each session:

```json
{
  "version": 7,
  "last_sync_base": 5,
  "machine_id": "a1b2c3"
}
```

Divergence detection uses three-way comparison:
- Remote == last_sync_base: safe to proceed locally, increment, upload
- Remote > last_sync_base AND local == last_sync_base: remote is newer, download
  first, apply local op, increment, upload
- Remote > last_sync_base AND local > last_sync_base: **conflict** — both machines
  advanced independently

Resolution: `koto session resolve --keep local|remote` sets version to
`max(local, remote) + 1` to establish a clean baseline.

**Rejected**: Timestamp comparison (clock skew causes silent data loss). Content
hashes (can't establish ordering, would need a counter anyway). Vector clocks
(precision unused since resolution is pick-a-side, not per-key merge).

### Decision 6: CloudBackend implementation

**Context**: How does CloudBackend implement the existing traits? What S3 crate?

**Chosen: Local cache wrapping `LocalBackend` + `rust-s3` behind feature flag.**

```rust
#[cfg(feature = "cloud")]
pub struct CloudBackend {
    local: LocalBackend,
    s3: S3Client,
    prefix: String,  // S3 key prefix (repo-id scoped)
}
```

CloudBackend delegates all filesystem operations to `LocalBackend`, then syncs
changes to S3 as a separate layer. This reuses all of LocalBackend's tested logic
(flock locking, atomic manifest writes, path validation, 0700 permissions).

`rust-s3` provides sync S3 operations without requiring tokio. It supports custom
endpoints for non-AWS providers (Cloudflare R2, MinIO). The dependency is behind
a cargo feature flag:

```toml
[features]
default = []
cloud = ["dep:rust-s3"]

[dependencies]
rust-s3 = { version = "0.35", optional = true }
```

S3 failures are non-fatal: operations succeed locally and log a warning to stderr.
The next command retries the upload.

**Rejected**: Direct S3 operations (every op hits network, fails offline, 100-500ms
latency per call). Async background sync (CLI process exits before sync completes).
aws-sdk-s3 (requires tokio, 30+ transitive deps). Manual S3 signing (reimplements
SigV4, error-prone).

## Decision outcome

The config system and cloud sync compose as two layers over the existing session
model.

**Config layer**: TOML files at user and project levels, resolved with precedence.
`koto config get/set/unset/list` CLI. Credentials blocked from project config.
`build_backend()` in `src/cli/mod.rs` reads the resolved `session.backend` value
to construct `LocalBackend` or `CloudBackend`.

**Cloud layer**: `CloudBackend` wraps `LocalBackend`. Every operation runs locally
first (fast, works offline), then syncs per-key to S3. Monotonic version counter
detects conflicts. `rust-s3` keeps koto synchronous. The entire cloud feature is
behind a cargo feature flag — default builds have zero cloud dependencies.

## Solution architecture

### Components

```
src/config/
  mod.rs          -- Config struct, load/merge logic, defaults
  resolve.rs      -- Precedence resolution (project > user > default > env)
  validate.rs     -- Credential blocklist enforcement

src/cli/
  config.rs       -- koto config get/set/unset/list handlers

src/session/
  cloud.rs        -- CloudBackend (wraps LocalBackend + rust-s3) [feature = "cloud"]
  sync.rs         -- S3 sync logic: push/pull/conflict detection [feature = "cloud"]
  version.rs      -- version.json read/write/compare [feature = "cloud"]
```

### Config module

```rust
// src/config/mod.rs
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct KotoConfig {
    #[serde(default)]
    pub session: SessionConfig,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SessionConfig {
    #[serde(default = "default_backend")]
    pub backend: String,  // "local" or "cloud"
    #[serde(default)]
    pub cloud: CloudConfig,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CloudConfig {
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,  // blocked from project config
    pub secret_key: Option<String>,  // blocked from project config
}

fn default_backend() -> String { "local".to_string() }
```

Config resolution:
1. Load defaults (all fields have defaults)
2. Overlay user config (`~/.koto/config.toml`) if it exists
3. Overlay project config (`.koto/config.toml`) if it exists
4. Override credentials from env vars (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)

### CloudBackend

```rust
// src/session/cloud.rs
#[cfg(feature = "cloud")]
pub struct CloudBackend {
    local: LocalBackend,
    bucket: s3::Bucket,
    prefix: String,
}

#[cfg(feature = "cloud")]
impl CloudBackend {
    pub fn new(working_dir: &Path, config: &CloudConfig) -> Result<Self> {
        let local = LocalBackend::new(working_dir)?;
        let bucket = create_bucket(config)?;
        let prefix = repo_id(working_dir)?;
        Ok(Self { local, bucket, prefix })
    }
}
```

`SessionBackend` and `ContextStore` implementations delegate to `self.local` for
filesystem operations, then call sync methods:

```rust
#[cfg(feature = "cloud")]
impl ContextStore for CloudBackend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> Result<()> {
        self.local.add(session, key, content)?;
        self.sync_push(session, key);  // non-fatal
        Ok(())
    }

    fn get(&self, session: &str, key: &str) -> Result<Vec<u8>> {
        self.sync_pull_if_newer(session, key);  // non-fatal
        self.local.get(session, key)
    }
    // ...
}
```

### Sync logic

```rust
// src/session/sync.rs
impl CloudBackend {
    fn sync_push(&self, session: &str, key: &str) {
        if let Err(e) = self.try_push(session, key) {
            eprintln!("warning: cloud sync failed: {}", e);
        }
    }

    fn try_push(&self, session: &str, key: &str) -> Result<()> {
        // 1. Read local version.json
        // 2. GET remote version.json
        // 3. Check for conflicts (Decision 5 logic)
        // 4. PUT content file to S3
        // 5. PUT updated manifest to S3
        // 6. PUT updated version.json to S3
        // 7. Update local last_sync_base
        Ok(())
    }

    fn sync_pull_if_newer(&self, session: &str, key: &str) {
        if let Err(e) = self.try_pull(session, key) {
            eprintln!("warning: cloud sync failed: {}", e);
        }
    }
}
```

### Version file

```rust
// src/session/version.rs
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionVersion {
    pub version: u64,
    pub last_sync_base: u64,
    pub machine_id: String,
}
```

### Backend selection in run()

```rust
// Updated build_backend() in src/cli/mod.rs
fn build_backend() -> Result<Box<dyn SessionBackend>> {
    let config = load_config()?;
    let working_dir = std::env::current_dir()?;

    match config.session.backend.as_str() {
        "local" => Ok(Box::new(LocalBackend::new(&working_dir)?)),
        #[cfg(feature = "cloud")]
        "cloud" => Ok(Box::new(CloudBackend::new(&working_dir, &config.session.cloud)?)),
        #[cfg(not(feature = "cloud"))]
        "cloud" => anyhow::bail!("cloud backend requires the 'cloud' feature: cargo install koto --features cloud"),
        other => anyhow::bail!("unknown backend: {}", other),
    }
}
```

### Key interfaces

| Interface | Location | Used by |
|-----------|----------|---------|
| `KotoConfig` struct | `src/config/mod.rs` | `build_backend()`, config CLI |
| `load_config()` | `src/config/resolve.rs` | `run()` in cli/mod.rs |
| `koto config` subcommands | `src/cli/config.rs` | users, agents |
| `CloudBackend` | `src/session/cloud.rs` | `build_backend()` when backend=cloud |
| `SessionVersion` | `src/session/version.rs` | sync logic |
| Credential blocklist | `src/config/validate.rs` | `koto config set --project` |

### Data flow

```
koto config set session.backend cloud
  -> validate key is not credential + project
  -> write to ~/.koto/config.toml: [session] backend = "cloud"

koto context add my-wf research/lead-1.md < /tmp/findings.md
  -> load_config() resolves backend = "cloud"
  -> build_backend() constructs CloudBackend(LocalBackend, S3Bucket)
  -> CloudBackend.add("my-wf", "research/lead-1.md", content)
    -> self.local.add(...)  (write to local ctx/)
    -> self.sync_push("my-wf", "research/lead-1.md")
      -> GET remote version.json (check for conflicts)
      -> PUT ctx/research/lead-1.md to S3
      -> PUT ctx/manifest.json to S3
      -> PUT version.json to S3 (version incremented)
  -> print nothing (success)

koto session resolve --keep local
  -> load local version.json (version=7, last_sync_base=5)
  -> set version = max(7, remote_version) + 1
  -> set last_sync_base = version
  -> force-upload entire session to S3
```

## Implementation approach

### Phase 1: Config module and CLI

Create `src/config/` module. Implement `KotoConfig` struct, TOML loading from two
locations, precedence merge, credential blocklist. Add `koto config get/set/unset/list`
CLI commands. Create `.koto/` directory on first `set --project`. Unit tests for
config loading, merge, blocklist.

Deliverables:
- `src/config/mod.rs`, `resolve.rs`, `validate.rs`
- `src/cli/config.rs`
- Tests

### Phase 2: Backend selection

Wire `load_config()` into `build_backend()`. When `session.backend = "local"`,
construct `LocalBackend` (same as today). When `session.backend = "cloud"` and
the cloud feature is disabled, return a helpful error. Integration tests verifying
backend selection.

Deliverables:
- Updated `src/cli/mod.rs`
- Integration tests

### Phase 3: CloudBackend with sync

Implement `CloudBackend` wrapping `LocalBackend`. Add `rust-s3` behind feature
flag. Implement push/pull sync with per-key incremental transfers. Implement
`version.json` and conflict detection. `koto session resolve --keep local|remote`.
Tests with mocked S3 (or localstack if available).

Deliverables:
- `src/session/cloud.rs`, `sync.rs`, `version.rs`
- Updated `Cargo.toml` with feature flag
- Tests

### Phase 4: Documentation

Update CLI usage docs with `koto config` commands. Document cloud setup guide
(endpoint, bucket, credentials). Update README with cloud sync capability.

Deliverables:
- Updated docs

## Security considerations

**Credential storage.** Project config uses an allowlist of safe keys (`endpoint`,
`bucket`, `region`). Any key NOT on the allowlist is rejected for `--project` writes.
This is safer than a blocklist because new credential-like keys are blocked by
default. User config (`~/.koto/config.toml`) has no restrictions — it inherits the
0700 permissions on `~/.koto/`.

**Credential redaction.** `koto config list` redacts credential values, showing
`session.cloud.access_key = <set>` instead of the raw value. This applies to both
TOML and JSON output. Raw credential values are never printed to stdout.

**Env var override.** `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` env vars
override config file credentials. This is the standard CI/CD pattern. Env vars
are visible in `/proc` on Linux but are the accepted trade-off for non-interactive
credential injection.

**S3 transport.** All S3 communication uses HTTPS by default. Custom endpoints
must use HTTPS unless `session.cloud.allow_insecure = true` is explicitly set.
Plaintext HTTP is rejected by default to prevent accidental credential exposure
on the wire. The `allow_insecure` key is documented as development-only (MinIO
without TLS).

**Config file permissions.** The config module independently ensures `~/.koto/`
has 0700 permissions when writing `config.toml`, not relying on LocalBackend
having run first. A user running `koto config set` before any session creation
still gets proper permissions. Project config at `.koto/config.toml` uses the
repo's umask — the allowlist ensures no secrets reach this file regardless of
permissions.

**Version counter tampering.** The version counter prevents silent data loss from
concurrent edits, not from malicious actors. An attacker who can modify
`version.json` in S3 already has bucket access (e.g., setting version to
`u64::MAX` as a DoS vector). The counter is a consistency mechanism, not a
security boundary.

**S3 bucket access.** koto uses a single bucket with repo-id prefixed keys. All
sessions from all projects share one bucket. An agent with the credentials can
access any project's sessions. This mirrors the local model where `~/.koto/`
contains all projects' sessions. In CI/CD environments with shared credentials,
any job can access any project's workflow state. For isolation, use per-project
IAM policies scoped to the repo-id prefix.

## Consequences

### Positive

- Workflows resume on any machine with `koto config set session.backend cloud` and
  credentials. No manual file copying.
- Sync is invisible to agents — zero new commands, zero token cost. Agents don't know
  whether they're running locally or cloud-synced.
- Config system is general-purpose. Future koto settings (template defaults, output
  format preferences) can use the same infrastructure.
- CloudBackend reuses all of LocalBackend's tested filesystem logic. Sync is a clean
  layer on top, not a parallel implementation.
- Per-key incremental sync minimizes S3 costs during burst workloads.
- Feature flag keeps default builds light — zero cloud dependencies for users who
  don't need it.
- Conflict detection prevents silent data loss from concurrent edits on different
  machines.

### Negative

- `rust-s3` adds a dependency (behind feature flag). Users who `cargo install koto
  --features cloud` get a larger binary.
- S3 requests add latency to every mutating command (typically 50-200ms for 2-3
  requests). Acceptable for a CLI that runs per-command, not per-keystroke.
- Config resolution happens on every command invocation. Must be fast (read two TOML
  files, check env vars). No caching across commands since each invocation is
  independent.
- Monotonic version counter is session-level, not per-key. A conflict on any key
  requires resolving the entire session. Acceptable because conflicts are rare and
  resolution is pick-a-side.

### Mitigations

- Manifest TTL cache (~5s) reduces redundant S3 GETs during rapid sequential
  commands.
- S3 failures are non-fatal (PRD R17). Operations succeed locally; the next command
  retries. Users are never blocked by cloud outages.
- `koto config list` shows the fully resolved config, making it easy to debug
  precedence issues.
- The `cloud` feature flag means `cargo install koto` (no features) produces the same
  lean binary as today.
