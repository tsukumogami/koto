---
status: Current
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto stores workflow state and context in wip/ committed to git branches. This
  pollutes PRs with temporary artifacts, prevents session transfer between machines,
  and means koto can't validate or control the content agents produce during
  workflows. Even with session directories outside git, agents write directly to
  the filesystem, bypassing koto entirely.
decision: |
  Two-phase approach. Phase A (implemented): SessionBackend trait with LocalBackend
  storing sessions at ~/.koto/sessions/{repo-id}/{name}/. Phase B: ContextStore
  trait for content ownership -- agents submit and retrieve context through koto's
  CLI instead of direct filesystem access. Content stored as files in a ctx/
  subdirectory with a manifest. Hierarchical path keys. Content-aware gate types
  with shell fallback.
rationale: |
  Phase A moves state out of git with zero configuration. Phase B makes koto the
  gatekeeper for workflow context, enabling content validation, immutability
  enforcement, and backend-agnostic access. Files-with-manifest storage is
  debuggable and syncs naturally to S3. Separate ContextStore trait keeps content
  CRUD separate from session lifecycle.
---

# DESIGN: Local session storage

## Status

Current

## Context and problem statement

koto's workflow engine produces temporary artifacts during execution: engine state
files, exploration scopes, research outputs, implementation plans, decision reports,
test plans, and review results. These live in `wip/` committed to git feature
branches. This creates four problems:

1. **Git pollution.** `wip/` artifacts appear in branch diffs and PR file lists,
   mixing temporary workflow state with real code changes.
2. **No session transfer.** Resuming a workflow on a different machine requires
   pushing the branch with its `wip/` artifacts. No independent sync exists.
3. **No storage abstraction.** Skills hardcode `wip/` paths (~150 references across
   shirabe and tsukumogami plugins). Changing storage means updating every skill.
4. **No content control.** Agents read and write workflow context files directly
   through the filesystem. koto provides a directory path but can't validate content
   format, enforce immutability, track writes, or support queries. Gate evaluation
   depends on shell commands checking the filesystem, not on koto's knowledge of
   what content exists.

## Decision drivers

- Zero configuration for local use (no database, no service, no cloud account)
- Agents must not hardcode storage paths
- koto must be able to validate and control content writes
- Multiple agents must be able to submit context concurrently without advancing state
- State and context must be transferable between machines (future cloud backend)
- Existing shell gates must continue to work (non-breaking migration)
- Content must be inspectable for debugging (`cat`, `ls`, not opaque binary formats)

## Considered options

### Decision 1: Trait shape for session lifecycle

**Context**: koto needs an abstraction over session storage that can support local
filesystem, cloud, and git backends.

**Chosen: 5-method trait with `Send + Sync` bounds.**
```rust
pub trait SessionBackend: Send + Sync {
    fn create(&self, id: &str) -> Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn exists(&self, id: &str) -> bool;
    fn cleanup(&self, id: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<SessionInfo>>;
}
```

**Rejected**: Generic type parameter (`run<B: SessionBackend>`) — adds complexity
without benefit since koto controls all call sites. Async trait — no async runtime
in koto today; premature.

### Decision 2: Directory layout

**Context**: Where do session artifacts live on disk?

**Chosen: `~/.koto/sessions/{repo-id}/{name}/`** where `{repo-id}` is a 16-hex-char
SHA-256 hash of the canonicalized working directory path. Isolates sessions by
project without requiring git.

**Rejected**: XDG data directory — splits koto's files across locations.
Inside `.koto/` in the repo — still pollutes git. `/tmp/` — no persistence
across reboots.

### Decision 3: CLI refactoring approach

**Context**: How do existing CLI commands learn about the new storage?

**Chosen: `run()` constructs `LocalBackend` and passes `&dyn SessionBackend` to all
handlers.** `handle_init` calls `backend.create(name)`. Other handlers call
`backend.session_dir(name)` to locate state files. `find_workflows_with_metadata()`
delegates to `backend.list()`.

### Decision 4: Session directory permissions

**Chosen: `~/.koto/` created with mode 0700 on first use.** Subsequent directories
follow the user's umask. Unix-only permission enforcement via `cfg(unix)` guard.

### Decision 5: Repo-ID derivation

**Chosen: First 16 hex characters of SHA-256 of canonicalized working directory.**
Reuses existing `sha256_hex()` from `src/cache.rs`. `std::fs::canonicalize()`
resolves symlinks. 16 hex chars = 64 bits of collision space, sufficient for local
use.

### Decision 6: Skill path discovery

**Context**: How do skills find the session directory at runtime?

**Chosen: `koto session dir {name}` CLI command.** Skills call this to discover
the path. Templates use `{{SESSION_DIR}}` for gate commands and directives,
substituted at runtime by `koto next`.

### Decision 7: Runtime variable substitution

**Chosen: `substitute_vars()` in `src/cli/vars.rs` using sequential `str::replace`
with a `HashMap<String, String>`.** Called in `handle_next` to substitute
`{{SESSION_DIR}}` in gate commands (per-invocation in the advance loop) and
directives (before JSON serialization). Template variables that collide with
reserved names produce a runtime error.

### Decision 8: Content storage model

**Context**: How does koto store submitted context internally? Three options:
files in session directory with manifest, JSONL append log per key, SQLite database.

**Chosen: Files in session directory with manifest.** Each key maps to a file in
`ctx/` inside the session directory. A `ctx/manifest.json` tracks metadata (key,
creation time, size, hash). Content submission writes the file first, then updates
the manifest atomically.

**Rejected**: JSONL append log — embeds content in JSON (requires escaping, destroys
debuggability for 20KB markdown files). History/audit not in PRD scope. SQLite —
adds binary dependency, content is opaque to `cat`/`ls`, database-wide locking
conflicts with per-key concurrency, problematic for S3 sync.

### Decision 9: Content CLI interface

**Context**: What command structure and flags for content operations?

**Chosen: Positional key argument with subcommand group.**
```
koto context add <session> <key> [--from-file <path>]   # stdin if no flag
koto context get <session> <key> [--to-file <path>]     # stdout if no flag
koto context exists <session> <key>                     # exit 0/1
koto context list <session> [--prefix <prefix>]         # JSON array
```

Session and key are positional args, matching `koto next <name>` convention.
`--from-file` and `--to-file` are optional; stdin/stdout are defaults.

**Rejected**: `--key` flag — adds verbosity for a mandatory argument.
Session-implicit — breaks multi-agent scenarios where different agents target
different sessions.

### Decision 10: Trait extension for content

**Context**: Where do content operations live in the type system?

**Chosen: Separate `ContextStore` trait.**
```rust
pub trait ContextStore: Send + Sync {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> Result<()>;
    fn get(&self, session: &str, key: &str) -> Result<Vec<u8>>;
    fn exists(&self, session: &str, key: &str) -> bool;
    fn remove(&self, session: &str, key: &str) -> Result<()>;
    fn list(&self, session: &str, prefix: Option<&str>) -> Result<Vec<String>>;
}
```

`LocalBackend` implements both `SessionBackend` and `ContextStore`. Content CRUD
is a different concern from session lifecycle (create/cleanup/list). Future backends
compose storage strategies independently.

**Rejected**: Adding to SessionBackend — conflates lifecycle and content, harder to
test. Composition via field accessor — adds indirection without current benefit.

### Decision 11: Content-aware gate types

**Context**: Templates currently use shell gates (`test -f {{SESSION_DIR}}/file`).
If koto owns content, gates need to check koto's store.

**Chosen: Hybrid — built-in gate types + shell fallback.**

Two new gate types:
- `context-exists`: passes when a key exists. Template: `type: context-exists`, `key: plan.md`
- `context-matches`: passes when content matches a regex. Template: `type: context-matches`, `key: review.md`, `pattern: "## Approved"`

Shell gates (`type: command`) continue to work for non-content checks. The engine's
`evaluate_gates` function dispatches on `gate_type`, adding match arms for the new
types. The `Gate` struct gains optional `key` and `pattern` fields.

**Rejected**: Built-in only — breaks existing shell-gate templates. Shell-only —
circular dependency (koto calls koto), shell overhead in tight loops, no static
validation.

### Decision 12: Content key format

**Context**: What are valid keys and how do they map from current wip/ naming?

**Chosen: Hierarchical path keys.** Keys are path-like strings with `/` as
namespace separator. Validation: `[a-zA-Z0-9._-/]`, no leading/trailing slashes,
no consecutive slashes, no `.`/`..` components.

Key mapping from current wip/ conventions:

| Current wip/ path | koto key |
|---|---|
| `wip/explore_{topic}_scope.md` | `scope.md` |
| `wip/explore_{topic}_findings.md` | `findings.md` |
| `wip/research/explore_{topic}_r1_lead-foo.md` | `research/r1/lead-foo.md` |
| `wip/design_{topic}_coordination.json` | `coordination.json` |
| `wip/issue_{N}_plan.md` | `plan.md` |
| `wip/implement-{topic}-state.json` | `state.json` |

The command prefix and topic drop out because the session already scopes by workflow
name. What remains is the artifact's structural role.

**Rejected**: Flat strings — retains redundant naming noise, no prefix-based listing.
Structured metadata — over-engineers querying for patterns prefix matching handles.

## Decision outcome

The design splits into two phases that build on each other.

**Phase A (implemented)**: `SessionBackend` trait with `LocalBackend` storing
sessions at `~/.koto/sessions/{repo-id}/{name}/`. CLI refactored to thread
`&dyn SessionBackend` through all handlers. `{{SESSION_DIR}}` variable
substitution. Session subcommands (`dir`, `list`, `cleanup`). Auto-cleanup on
terminal state. 235 tests.

**Phase B (this design)**: `ContextStore` trait for content ownership. Agents
submit context through `koto context add` and retrieve through `koto context get`.
Content stored as files in `ctx/` with a manifest. Hierarchical path keys. Built-in
gate types (`context-exists`, `context-matches`) with shell fallback.

Together, koto owns both the location and content of workflow artifacts. The
`SessionBackend` trait manages session lifecycle. The `ContextStore` trait manages
what's inside sessions. `LocalBackend` implements both.

## Solution architecture

### Overview

Content ownership adds three components to the existing session model:

1. **ContextStore trait** (`src/session/context.rs`) — defines content CRUD operations
2. **Content CLI** (`src/cli/context.rs`) — `koto context add/get/exists/list` commands
3. **Built-in gate types** — `context-exists` and `context-matches` in the gate evaluator

### Components

```
src/session/
  mod.rs          -- SessionBackend trait, SessionInfo, state_file_name() [existing]
  local.rs        -- LocalBackend: SessionBackend + ContextStore [extended]
  context.rs      -- ContextStore trait definition [new]
  validate.rs     -- validate_session_id(), validate_context_key() [extended]

src/cli/
  mod.rs          -- run(), command dispatch [extended with Context variant]
  context.rs      -- handle_context_add/get/exists/list [new]
  session.rs      -- koto session subcommands [existing]
  vars.rs         -- substitute_vars() [existing]

src/gate.rs       -- evaluate_gates() [extended with context-exists, context-matches]
src/template/types.rs -- Gate struct [extended with key, pattern fields]
```

### ContextStore trait

```rust
// src/session/context.rs
pub trait ContextStore: Send + Sync {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> Result<()>;
    fn get(&self, session: &str, key: &str) -> Result<Vec<u8>>;
    fn ctx_exists(&self, session: &str, key: &str) -> bool;
    fn remove(&self, session: &str, key: &str) -> Result<()>;
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> Result<Vec<String>>;
}
```

### LocalBackend content storage

`LocalBackend` implements `ContextStore` by storing content as files:

```
~/.koto/sessions/<repo-id>/<name>/
  koto-<name>.state.jsonl          # engine state (existing)
  ctx/                             # content store root
    manifest.json                  # key metadata index
    scope.md                       # flat key
    findings.md                    # flat key
    research/                      # hierarchical key prefix
      r1/
        lead-cli-ux.md
        lead-concurrency.md
    coordination.json
```

The manifest tracks metadata per key:

```json
{
  "keys": {
    "scope.md": {"created_at": "2026-03-27T10:00:00Z", "size": 1234, "hash": "a1b2c3..."},
    "research/r1/lead-cli-ux.md": {"created_at": "2026-03-27T10:05:00Z", "size": 5678, "hash": "d4e5f6..."}
  }
}
```

Write order: content file first, manifest second. On crash, orphaned content without
a manifest entry is harmless and detectable. Manifest writes are atomic (write to
temp, rename). Per-key advisory flock on the content file prevents concurrent writes
to the same key.

### Content CLI

```
koto context add <session> <key> [--from-file <path>]
koto context get <session> <key> [--to-file <path>]
koto context exists <session> <key>
koto context list <session> [--prefix <prefix>]
```

- `add`: validates key (Decision 12 rules), reads content from stdin or `--from-file`,
  calls `context_store.add()`, prints nothing on success
- `get`: calls `context_store.get()`, writes to stdout or `--to-file`
- `exists`: calls `context_store.ctx_exists()`, exits 0 or 1
- `list`: calls `context_store.list_keys()`, prints JSON array to stdout

`add` does not advance workflow state. Agents submit context independently;
the orchestrator calls `koto next` when ready.

### Content-aware gates

Template syntax:

```yaml
states:
  awaiting-research:
    gates:
      research-complete:
        type: context-exists
        key: research/r1/lead-cli-ux.md
      review-approved:
        type: context-matches
        key: review.md
        pattern: "## Approved"
      ci-passing:
        type: command
        command: "gh run view --json conclusion -q '.conclusion' | grep success"
```

Gate struct extension:

```rust
pub struct Gate {
    pub gate_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default)]
    pub timeout: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
}
```

`evaluate_gates` dispatches on `gate_type`:
- `"command"`: existing shell evaluation
- `"context-exists"`: calls `context_store.ctx_exists(session, &gate.key)`
- `"context-matches"`: calls `context_store.get(session, &gate.key)`, applies regex

### Key interfaces

| Interface | Location | Used by |
|-----------|----------|---------|
| `SessionBackend` trait | `src/session/mod.rs` | CLI session commands |
| `ContextStore` trait | `src/session/context.rs` | CLI context commands, gate evaluator |
| `LocalBackend` | `src/session/local.rs` | implements both traits |
| `validate_context_key()` | `src/session/validate.rs` | `koto context add` |
| `koto context` subcommands | `src/cli/context.rs` | agents, skills |
| `context-exists` gate | `src/gate.rs` | template gate evaluation |
| `context-matches` gate | `src/gate.rs` | template gate evaluation |

### Data flow

```
Agent submits context:
  koto context add my-wf research/r1/lead-cli-ux.md --from-file /tmp/findings.md
    -> validate_context_key("research/r1/lead-cli-ux.md")
    -> create_dir_all(ctx/research/r1/)
    -> write content to ctx/research/r1/lead-cli-ux.md
    -> update ctx/manifest.json (atomic rename)

Agent retrieves context:
  koto context get my-wf scope.md --to-file /tmp/scope.md
    -> read ctx/scope.md
    -> write to /tmp/scope.md

Gate evaluation during koto next:
  gate type: context-exists, key: research/r1/lead-cli-ux.md
    -> check manifest for key presence
    -> return GateResult::Passed or GateResult::Failed
```

## Implementation approach

### Phase A: Session module and CLI refactoring (implemented)

Session directories at `~/.koto/sessions/{repo-id}/{name}/`. `SessionBackend`
trait and `LocalBackend`. CLI threading. `{{SESSION_DIR}}` variable substitution.
Session subcommands. Auto-cleanup. Shipped in PR #84.

### Phase B1: ContextStore trait and LocalBackend implementation

Create `src/session/context.rs` with the `ContextStore` trait. Extend `LocalBackend`
to implement `ContextStore` with files-in-ctx/ storage and manifest. Add
`validate_context_key()` to `src/session/validate.rs`. Unit tests for all trait
methods, key validation, manifest atomicity, and concurrent access.

Deliverables:
- `src/session/context.rs` — trait definition
- `src/session/local.rs` — ContextStore implementation
- `src/session/validate.rs` — key validation
- Unit tests

### Phase B2: Content CLI commands

Add `koto context add/get/exists/list` subcommands. Thread `&dyn ContextStore`
through `run()`. Implement stdin/stdout piping and `--from-file`/`--to-file` flags.
Integration tests.

Deliverables:
- `src/cli/context.rs` — command handlers
- `src/cli/mod.rs` — Context variant in Command enum
- Integration tests

### Phase B3: Content-aware gate types

Extend the `Gate` struct with `key` and `pattern` fields. Add `context-exists` and
`context-matches` branches to `evaluate_gates`. Thread `&dyn ContextStore` into the
gate evaluation closure. Update template validation to recognize the new gate types.
Tests.

Deliverables:
- `src/gate.rs` — new gate type evaluation
- `src/template/types.rs` — Gate struct extension
- Template validation updates
- Gate evaluation tests

### Phase B4: Documentation and template updates

Update hello-koto template to use content-aware gates instead of filesystem checks.
Update skill authoring guide and manual test documentation. Update CLI usage docs
with `koto context` commands.

Deliverables:
- Updated templates and documentation
- Updated CLI usage guide

## Security considerations

**Session ID and key validation.** Session IDs use allowlist
`^[a-zA-Z][a-zA-Z0-9._-]*$`. Content keys use `[a-zA-Z0-9._-/]` with per-component
rejection of `.` and `..` (each path segment between slashes is validated
independently), no leading/trailing slashes, no consecutive slashes, and a maximum
total length of 255 characters. Both prevent path traversal. Validation runs at
write boundaries (`create()` for sessions, `context add` for keys).

**Home directory trust.** `~/.koto/sessions/` is writable by the current user. No
elevated permissions. `~/.koto/` created with mode 0700 on first use.

**Cross-project session access.** Adding `~/.koto` to the agent sandbox allowlist
grants access to all projects' sessions. Acceptable because agents already have
filesystem access to `~/` via file tools.

**Content injection.** Agents submit arbitrary content. koto stores it as-is (no
sanitization). Skills that read content must treat it as untrusted input if they
execute it. Built-in gates (`context-matches` with regex) are evaluated by koto,
not by shell, reducing injection surface.

**Regex engine safety.** The `context-matches` gate type evaluates user-provided
regex patterns against stored content. koto must use a linear-time regex engine
(Rust's `regex` crate provides this by default — no backtracking) to prevent
ReDoS attacks where a crafted pattern causes exponential evaluation time.

**Manifest concurrency.** Per-key advisory flock prevents concurrent writes to the
same content file, but the shared `manifest.json` needs its own serialization.
Implementation uses a separate manifest flock: lock manifest, read-modify-write,
unlock. Since manifest updates are small (JSON key insertion), lock contention
is minimal even with concurrent multi-agent writes to different keys.

**Manifest integrity.** The manifest is a convenience index, not a security boundary.
If an attacker can modify files in `~/.koto/`, they already have full user access.
The manifest hash is for corruption detection, not tamper resistance.

**No secrets in context.** Context artifacts (research, plans, reviews) aren't
secrets but may contain project-specific information. Local backend doesn't transmit
off-machine. Cloud sync adds exposure — addressed in the cloud backend design.

## Consequences

### Positive

- State files and workflow context move out of git entirely. PRs show only real changes.
- koto owns content writes, enabling future validation, format enforcement, and audit.
- Agents interact through a stable CLI (`koto context add/get/exists/list`). Skills
  work regardless of backend.
- Content-aware gates eliminate shell dependency for common checks. Templates declare
  intent (`context-exists: plan.md`) instead of implementation (`test -f ...`).
- Hierarchical keys with prefix listing support organized, queryable context stores.
- Multiple agents submit context concurrently without advancing state.
- The `ContextStore` trait provides a clean extension point for cloud and git backends.

### Negative

- Agents can't use file tools (Read/Edit/Write) directly on context. They pipe
  through `koto context get --to-file` then Read the temp file. One extra step per
  read.
- Replace-only semantics mean accumulation patterns (findings.md across rounds)
  require read-modify-replace through the CLI. Works because orchestrators serialize,
  but is less natural than direct file append.
- Two traits (`SessionBackend` + `ContextStore`) for backend implementors instead of
  one. Acceptable because koto controls all implementations.
- Manifest adds complexity. Write ordering (content first, manifest second) and atomic
  rename are needed for crash safety.

### Mitigations

- `--to-file` flag on `koto context get` minimizes the pipe-to-temp-file overhead.
  Agents Read the temp file with offset/limit for targeted access.
- Accumulation via read-modify-replace is safe under orchestrator serialization. If
  concurrent accumulation becomes needed, append operations can be added to
  `ContextStore` without breaking the replace-only API.
- Manifest atomic rename uses the standard write-to-temp-rename pattern, battle-tested
  in koto's state file persistence.
