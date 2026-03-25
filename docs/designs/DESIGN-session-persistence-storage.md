---
status: Proposed
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto's persistence layer writes engine state to the git working tree (wip/) and
  has no concept of session management. The engine, CLI, and template system all
  assume artifacts live at hardcoded paths relative to the repo root. Adding session
  ownership to koto requires a storage abstraction that works across local filesystem,
  S3-compatible cloud, and git backends, with implicit sync built into existing
  state-mutating commands so skills pay zero token cost for cloud support.
decision: |
  Use a SessionBackend trait with three implementations (local, cloud, git) selected
  via a TOML-based koto config system. Cloud sync wraps existing commands implicitly.
  Engine-provided variables ({{SESSION_DIR}}) replace hardcoded wip/ paths in templates
  at substitution time, separate from user-defined --var variables.
rationale: |
  The trait approach cleanly separates backend logic from CLI commands and avoids
  conditional branching that would grow with each new backend. A simple toml crate
  handles config read/write without the overhead of config-rs. Engine-provided variables
  solve the directive problem (directives aren't shell-executed, so env vars don't work)
  while staying current across backend changes -- unlike init-time persistence.
---

# DESIGN: Session persistence storage

## Status

Proposed

## Context and problem statement

koto's engine persists workflow state to JSONL files (via `workflow-tool state init`
and `state transition`) and skills persist artifacts (research, plans, decisions) to
`wip/` — both in the git working tree. The engine owns its state file lifecycle
(atomic appends, advisory flock, integrity hashes) but has no abstraction over WHERE
files live. Skills hardcode `wip/` paths. There's no session concept tying artifacts
to a workflow.

The PRD (PRD-session-persistence-storage.md) requires koto to own session lifecycle
and location, with three backends (local filesystem, S3-compatible cloud, git
working tree) selected via `koto config`. Cloud sync must be invisible — built into
existing state-mutating commands so agents never call sync operations.

This design needs to solve several technical problems:

1. **Storage abstraction**: a trait or interface that backends implement, operating
   on session directories as a unit. The engine currently writes to hardcoded paths;
   it needs to write to backend-provided paths instead.
2. **Implicit sync**: state-mutating commands (`koto init`, `transition`, `next
   --with-data`) must check remote versions before operating and upload after. This
   wraps existing command logic without changing their signatures or output.
3. **Version tracking**: a monotonic counter per session that detects conflicts when
   two machines diverge. Must be lightweight — a single integer in the session
   metadata.
4. **Config system**: `koto config get/set` with TOML files at user and project
   levels, precedence rules, and env var overrides for credentials.
5. **Template integration**: templates reference `wip/` in gate commands and
   directives. The session directory path must be available for substitution
   (via `{{SESSION_DIR}}` or an environment variable set during command execution).
6. **CLI surface**: `koto session dir|list|cleanup|resolve` commands. These are
   small additions to the existing CLI.

## Decision drivers

- **Zero token cost for sync**: agents call existing koto commands; sync is internal.
  No new commands in the skill→koto interaction path.
- **Agent file tool compatibility**: session directory must be a real filesystem path
  that supports Read/Edit/Write with offset/limit. No proxying file I/O through CLI.
- **Backend simplicity**: the storage trait should be minimal — session directories
  as bundles, not per-file operations. Backends don't need to understand artifact
  content.
- **Existing engine integration**: the JSONL event log, atomic appends, and advisory
  locking must work unchanged within the session directory. The abstraction wraps
  location, not behavior.
- **Config follows established patterns**: `koto config` mirrors git config semantics.
  TOML format, user/project levels, env var overrides for secrets.
- **S3-compatible cloud**: standard protocol, works with AWS, R2, MinIO. Minimal
  HTTP client preferred over full SDK if feasible.
- **Incremental adoption**: skills can migrate from hardcoded `wip/` to session
  dir gradually. Git backend preserves current behavior during transition.

## Considered options

Three decisions shape this design. Each is presented with its chosen option and
the alternatives that were evaluated.

### Decision 1: Storage backend trait shape and engine integration

**Question:** How should the CLI interact with different storage backends, and how
does cloud sync fit into existing command execution?

**Key assumptions:**
- The number of backends will grow slowly (local, cloud, git now; maybe GCS later).
- persistence.rs operates entirely on `&Path` arguments. Backend integration means
  changing which path is used, not rewriting persistence logic.
- Cloud sync must be invisible to agents -- no new commands in the skill-to-koto
  interaction path.

**Chosen: SessionBackend trait with sync-on-command wrapper (Option A).** Each
backend implements a trait with five methods: `create`, `session_dir`, `sync_down`,
`sync_up`, and `cleanup`, plus a `list` query. The CLI constructs the backend once
from configuration and passes it to command handlers. State-mutating commands follow
a three-phase protocol: sync down, do work, sync up. A `with_sync` helper extracts
this pattern so individual commands don't repeat it.

The trait boundary is a natural test seam. Cloud sync logic can be tested against a
mock backend without touching S3. Adding a fourth backend requires one new struct,
zero changes to command handlers. The version counter check lives inside `sync_down`,
so individual commands can't forget it.

**Rejected: Layered cache model (Option B).** A simpler approach where each
state-mutating command checks `if cloud_configured { sync() }`. Fewer abstractions
up front, but the conditional spreads into every command handler, and the git backend
becomes a special case rather than a polymorphic implementation. This converges toward
a trait anyway as complexity grows. Starting with the trait avoids the refactor.

**Rejected: Event-log-centric model (Option C).** Syncs only the JSONL event log,
not the full session directory. Minimal S3 surface area, but breaks the core
requirement that agents need filesystem paths to all session artifacts -- research
outputs, plans, decision reports. An agent switching machines would have engine state
but none of the artifacts it references. The PRD explicitly says backends operate on
session directories as bundles.

### Decision 2: Configuration system implementation

**Question:** How should backend selection and cloud credentials be configured?

**Key assumptions:**
- koto's config needs are narrow: two TOML files, a handful of env vars, dot-key
  access. No need for a config framework.
- The `toml` crate is already the de facto standard for TOML in Rust (used by Cargo).
- Cloud credentials must come from environment variables in CI environments. Config
  file credentials are a convenience for local development.

**Chosen: Simple TOML with the `toml` crate (Option A).** Add `toml = "0.8"` as a
dependency. Define merge logic (~30 lines of recursive table overlay) and dot-notation
key navigation (~20 lines). Two config files: user (`~/.koto/config.toml`) and project
(`.koto/config.toml`). Merge precedence: project over user over defaults. A fixed
mapping of config keys to env vars handles credentials -- `AWS_ACCESS_KEY_ID` overrides
`session.cloud.access_key`, and so on.

This follows koto's existing pattern: `serde_json` for JSON, `serde_yml` for YAML,
each with hand-written logic on top. One new dependency, full read+write support,
explicit env var handling.

**Rejected: config-rs crate (Option B).** Built-in source layering sounds appealing,
but config-rs is read-only by design. `koto config set` would still need the `toml`
crate for the write path, resulting in two config mechanisms. The crate's env var
mapping uses prefix conventions (`KOTO_SESSION_BACKEND`) that don't match the
requirement to honor standard AWS env vars. Net savings minimal, added complexity
from dual systems.

**Rejected: Hand-rolled parser (Option C).** Zero new dependencies, but TOML parsing
isn't trivial even for the subset we need. Several hundred lines of parser code with
ongoing edge-case maintenance. The `toml` crate has already solved escape sequences,
multiline strings, and inline tables. One well-known dependency is a smaller
maintenance cost than a custom parser.

### Decision 3: Template path substitution for session directories

**Question:** How should templates reference the session directory path when it varies
by backend?

**Key assumptions:**
- Templates reference `wip/` in both gate commands (shell-executed) and directives
  (returned as text to agents). Any solution must work for both.
- The `--var` substitution system using `{{KEY}}` syntax is already implemented. The
  `Variables::substitute()` function handles pattern replacement.
- SESSION_DIR must reflect the current backend, even if the backend changes after
  `koto init`.

**Chosen: Engine-provided variables, separate from user --var (Option C).** The engine
injects `SESSION_DIR` (and potentially `WORKFLOW_NAME` later) at substitution time,
not at init time. Same `{{KEY}}` syntax as user variables. The substitution function
merges user and engine variables, with engine variables taking precedence. Templates
don't need to declare `SESSION_DIR` in their variables block -- it's always available.

The decisive factor is directives. Directives aren't shell-executed; they're returned
as text to agents. An env var like `$KOTO_SESSION_DIR` in a directive is a literal
string the agent can't expand. Engine-provided variables go through `{{KEY}}`
substitution before the directive is returned, so the agent sees the resolved path.

Engine variables are computed at substitution time (during `koto next`), so they're
always current. If a user switches from the local backend to git, the next
substitution picks up the new path without re-initializing the workflow.

Name collisions are handled by rejecting `--var` keys that match engine variable names
at `koto init` time, with a clear error message.

**Rejected: Built-in template variable at init time (Option A).** SESSION_DIR would
be stored in the `WorkflowInitialized` event alongside user variables. Simpler (one
variable pool, one substitution pass), but the init-time value goes stale if the
backend changes. Every template would also need to declare SESSION_DIR in its variables
block, adding boilerplate for something the engine should just provide.

**Rejected: Environment variable during command execution (Option B).** Setting
`KOTO_SESSION_DIR` in the environment before spawning gate commands works for gates
but fails for directives. Also mixes two substitution systems (`{{VAR}}` and `$VAR`),
making templates inconsistent and confusing.

## Decision outcome

The three decisions compose into a layered system:

1. **Config** determines the active backend. `koto config` reads TOML files (project
   over user over defaults), applies env var overrides for credentials, and produces
   a backend selection with its parameters.

2. **Backend** owns session directory location and remote sync. The `SessionBackend`
   trait provides a uniform interface. `LocalBackend` stores sessions at
   `~/.koto/sessions/<id>/`. `CloudBackend` wraps `LocalBackend` and adds S3
   mirroring. `GitBackend` stores sessions at `<repo>/<wip_path>/<id>/`. The CLI
   constructs one backend instance from config and threads it through command handlers.

3. **Engine variables** bridge the gap between storage and templates. At substitution
   time, the engine calls `backend.session_dir(workflow_name)` to get the current
   path and injects it as `SESSION_DIR`. Gate commands and directives both resolve
   `{{SESSION_DIR}}` through the same substitution pass that handles user `--var`
   values.

**Data flow for a state-mutating command (e.g., `koto next --with-data`):**

```
koto config   -->  backend selection
                      |
                  sync_down(id)        pull remote if newer
                      |
                  session_dir(id)      resolve local path
                      |
                  Variables::substitute()  inject SESSION_DIR + user vars
                      |
                  <existing command logic>  read events, evaluate gate, append
                      |
                  sync_up(id)          push updated state to remote
```

Agents see none of this. They call `koto next`, get a directive with resolved paths,
and use their file tools on those paths. Cloud sync is invisible.

## Solution architecture

### Overview

The session persistence system adds four components to koto: a storage abstraction
layer, a configuration module, session management CLI commands, and engine-provided
template variables. These integrate into the existing CLI dispatch and engine execution
paths without changing the JSONL persistence format, the advisory locking model, or
the gate evaluation logic.

### Components

**SessionBackend trait.** The central abstraction. Five mutating methods plus one query:

```rust
pub trait SessionBackend: Send + Sync {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn sync_down(&self, id: &str) -> anyhow::Result<Option<u64>>;
    fn sync_up(&self, id: &str) -> anyhow::Result<u64>;
    fn cleanup(&self, id: &str) -> anyhow::Result<()>;
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
}
```

`session_dir` is a pure path computation (no I/O). `sync_down` and `sync_up` are
separate because the protocol requires checking remote state before mutation and
uploading after. No `resolve` method -- conflict resolution is CLI-level logic that
picks a winner and calls `sync_up`. No `exists` method -- callers check
`session_dir(id).exists()`.

**LocalBackend.** Sessions at `~/.koto/sessions/<id>/`. All sync methods are no-ops
returning `None`/`0`. Zero new dependencies.

**CloudBackend (feature-gated).** Wraps `LocalBackend` -- session directories always
live locally. Cloud adds remote mirroring via `aws-sdk-s3` behind a `cloud` Cargo
feature flag. This keeps `tokio` and the AWS SDK out of default builds.

Upload protocol: read version from `session.meta.json`, increment, upload all files
to `s3://<bucket>/sessions/<id>/`, upload metadata last (commit marker). Download
protocol: fetch remote metadata, compare versions, download all files if remote is
newer. Conflict detection uses `last_synced_version` in local metadata.

Failed uploads set a `sync_pending` flag in `session.meta.json`. The next `sync_down`
retries the upload before checking remote state.

**GitBackend.** Sessions at `<repo>/<wip_path>/<id>/` where `wip_path` defaults to
`wip/`. All sync methods are no-ops. This backend preserves current behavior for users
who want artifacts in the git working tree.

**Config module (src/config.rs).** Loads and merges TOML config from two sources:

| Level | Path | Written by |
|-------|------|-----------|
| User | `~/.koto/config.toml` | `koto config set <key> <value>` |
| Project | `.koto/config.toml` | `koto config set --project <key> <value>` |

Merge precedence: project > user > hardcoded defaults. A fixed env var mapping
overrides credential keys:

| Config key | Env var |
|---|---|
| `session.cloud.access_key` | `AWS_ACCESS_KEY_ID` |
| `session.cloud.secret_key` | `AWS_SECRET_ACCESS_KEY` |
| `session.cloud.region` | `AWS_REGION` / `AWS_DEFAULT_REGION` |
| `session.cloud.endpoint` | `AWS_ENDPOINT_URL` |

**Session CLI commands.** Added as a `Session` variant in the `Command` enum, following
the existing `Template`/`TemplateSubcommand` pattern:

| Command | Behavior |
|---------|----------|
| `koto session dir <name>` | Print session directory path |
| `koto session list` | List local sessions with name, timestamp, version |
| `koto session cleanup <name>` | Remove local + remote artifacts |
| `koto session resolve --keep local\|remote` | Force-resolve version conflict |

**Engine-provided variables.** The substitution function (in the `--var` system) merges
two variable maps: user-provided and engine-provided. Engine variables are computed
fresh at substitution time. Initial set: `SESSION_DIR`. The engine calls
`backend.session_dir(workflow_name)` and injects the result. Unresolved `{{KEY}}`
patterns produce a warning, not an error, to support incremental template migration.

### Key interfaces

**Session metadata (`session.meta.json`):**

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

The version counter is monotonic and tracks sync state, not workflow state. Rewinding
a workflow doesn't rewind the version. `last_synced_version` enables conflict
detection: if both local and remote have advanced past the last sync point, there's
a conflict.

**Config file format (TOML):**

```toml
[session]
backend = "cloud"

[session.cloud]
endpoint = "https://s3.us-east-1.amazonaws.com"
bucket = "my-koto-sessions"
region = "us-east-1"

[session.git]
path = "wip/"
```

**Sync wrapper:**

```rust
fn with_sync<F, T>(
    backend: &dyn SessionBackend,
    id: &str,
    mutating: bool,
    f: F,
) -> anyhow::Result<T>
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

### Data flow

```
                    +-----------------+
                    |  koto config    |
                    | (TOML + env)    |
                    +--------+--------+
                             |
                    backend selection
                             |
                    +--------v--------+
                    | SessionBackend  |
                    | (trait object)  |
                    +--------+--------+
                             |
              +--------------+--------------+
              |              |              |
     +--------v---+  +------v------+  +----v-------+
     | LocalBackend|  |CloudBackend |  | GitBackend |
     | ~/.koto/    |  |  local +    |  | <repo>/    |
     | sessions/   |  |  S3 mirror  |  | wip/<id>/  |
     +--------+----+  +------+------+  +----+-------+
              |              |              |
              +--------------+--------------+
                             |
                     local filesystem path
                             |
              +--------------+--------------+
              |                             |
     +--------v--------+          +--------v--------+
     | Engine variables |          | Agent file tools|
     | {{SESSION_DIR}}  |          | Read/Edit/Write |
     | in gate commands |          | on session dir  |
     | and directives   |          |                 |
     +-----------------+          +-----------------+
```

## Implementation approach

Six phases, ordered so each builds on the previous. Phases 1-2 can ship without
changing external behavior.

### Phase 1: SessionBackend trait + LocalBackend + CLI refactor

Add the `SessionBackend` trait and `LocalBackend` implementation. Refactor CLI
command handlers to accept `&dyn SessionBackend` and use the `with_sync` wrapper.
`LocalBackend` with no-op sync is equivalent to current behavior -- this phase is
a pure internal refactor.

Files: `src/session.rs` (trait + LocalBackend), updates to `src/cli/mod.rs`.

### Phase 2: Config system

Add `src/config.rs` with TOML loading, merge logic, dot-notation key access, and
env var overrides. Add `koto config get/set` CLI subcommand. Add `toml = "0.8"`
dependency.

After this phase, `koto config set session.backend local` works but doesn't change
behavior (local is the default).

### Phase 3: Session CLI commands

Add `koto session dir|list|cleanup` subcommands. These are thin wrappers around
`SessionBackend` methods. `koto session resolve` is added but only useful once the
cloud backend exists.

### Phase 4: Engine-provided variables

Wire `SESSION_DIR` into the variable substitution system. At substitution time, the
engine calls `backend.session_dir(workflow_name)` and merges the result into the
variable map. Add reserved-name validation to `koto init` so `--var SESSION_DIR=...`
produces a clear error.

This phase extends the existing `Variables::substitute()` -- it doesn't build
substitution from scratch.

### Phase 5: GitBackend

Implement `GitBackend` with configurable `wip_path`. Session directories live at
`<repo>/<wip_path>/<id>/`. All sync methods are no-ops. This gives users who prefer
git-based workflows an explicit opt-in path.

### Phase 6: CloudBackend (behind feature flag)

Add `aws-sdk-s3`, `aws-config`, and `tokio` behind a `cloud` Cargo feature flag.
Implement `CloudBackend` wrapping `LocalBackend` with S3 mirroring. Wire up the
version counter, conflict detection, `sync_pending` retry, and the `session resolve`
command.

This is the largest phase but is fully opt-in -- users who don't enable the `cloud`
feature get no new dependencies.

## Security considerations

**S3 credential handling.** Cloud credentials can come from two sources: the config
file (`session.cloud.access_key`) or environment variables (`AWS_ACCESS_KEY_ID`). Env
vars always take precedence over config file values. This is intentional -- CI
environments should never store credentials in files, and env vars override prevents
accidentally committed secrets from being used when a safer source is available.

The user config file (`~/.koto/config.toml`) should have `0600` permissions if it
contains credentials. koto doesn't enforce this but could warn on world-readable
config files containing credential keys.

Project config (`.koto/config.toml`) is committed to git. It should never contain
credentials -- only endpoint, bucket, and region. The design separates these
deliberately: connection parameters in project config (shared with team), credentials
in user config or env vars (per-machine).

**Session data exposure.** When using the cloud backend, all session artifacts
(research outputs, plans, decision reports, engine state) are uploaded to S3. Users
should understand that cloud sync means their workflow artifacts leave the local
machine. The S3 bucket's access policy determines who can read them. koto uses HTTPS
for transport but doesn't add its own encryption layer.

**No encryption at rest.** Session artifacts aren't encrypted on the local filesystem
or in S3 (beyond whatever server-side encryption the S3 provider offers). The PRD
explicitly lists this as out of scope. Users with sensitive artifacts should configure
S3 server-side encryption at the bucket level.

**Reserved variable names.** Engine-provided variables (`SESSION_DIR`) can't be
overridden by user `--var` arguments. This prevents a template from being tricked into
writing artifacts to an unexpected path. Validation happens at `koto init` time with
a clear error message.

## Consequences

### Positive

- **Agents stop hardcoding paths.** Skills use `{{SESSION_DIR}}` instead of `wip/`,
  decoupling artifact location from skill code. Backend changes require zero skill
  updates.
- **Clean git history.** The default local backend keeps workflow artifacts out of
  branches entirely. PRs contain only code changes.
- **Cross-machine sessions.** The cloud backend enables resuming workflows on a
  different machine with no manual file copying. Sync is invisible to agents.
- **Incremental migration.** The git backend preserves current behavior. Skills can
  migrate from hardcoded `wip/` to `{{SESSION_DIR}}` gradually, and both approaches
  work during the transition.
- **Testable backend logic.** The trait boundary lets tests substitute a mock backend,
  verifying sync protocol without real S3 calls.

### Negative

- **New dependency for cloud.** The `cloud` feature flag pulls in `aws-sdk-s3`,
  `aws-config`, and `tokio`. These are heavy dependencies, though they're opt-in and
  don't affect users who stay on the local or git backend.
- **Sync latency at transitions.** Cloud sync adds network round-trips to every
  state-mutating command. For small session directories (~20 files, ~200KB) this
  should be a few seconds, but it's nonzero.
- **Lost CI cleanup enforcement.** The current CI check that `wip/` is empty before
  merge doesn't apply when artifacts live outside git. Users on the local backend
  rely on koto's automatic cleanup at workflow completion instead.
- **New concept: engine variables.** Template authors need to learn that some
  `{{KEY}}` values come from the engine rather than `--var`. The list starts small
  (just `SESSION_DIR`) but it's another thing to document.

### Mitigations

- **Dependency weight:** The `cloud` feature flag is off by default. Binary size and
  compile time are unaffected for users who don't need cloud sync.
- **Sync latency:** Session directories are small by design. If latency becomes an
  issue, the cloud backend can be optimized to upload only changed files (tracked via
  local file hashes) rather than the full directory.
- **CI enforcement gap:** A future enhancement could add a `koto session check` that
  CI runs to verify no stale sessions exist, replacing the `wip/` emptiness check with
  a backend-aware equivalent.
- **Engine variable discoverability:** `koto next` could include available engine
  variables in its JSON output when a template uses `{{KEY}}` patterns, helping
  template authors discover what's available.
