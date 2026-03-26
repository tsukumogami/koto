---
status: Proposed
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto writes workflow state to hardcoded paths in the git working tree (wip/). There's
  no abstraction over where session artifacts live, no session lifecycle management, and
  no way to change the storage location without updating every skill. This design covers
  the foundational feature: a SessionBackend trait, a local filesystem backend, session
  CLI commands, and the CLI refactoring needed to use backend-provided paths instead of
  hardcoded wip/.
decision: |
  A SessionBackend trait with six methods (create, session_dir, state_file_path, cleanup, list, exists)
  and a LocalBackend that stores sessions at ~/.koto/sessions/<repo-id>/<name>/. The
  repo-id is the first 16 hex characters of a SHA-256 hash of the canonicalized working
  directory path, scoping sessions per-project. Sessions live entirely outside the repo
  (zero repo footprint, invisible to non-users). Agent file tools access ~/.koto/ without
  sandbox restrictions. CLI commands get the session path from the backend instead of
  constructing wip/ paths. Skill migration to `koto session dir` ships alongside this
  feature as a coordinated release.
rationale: |
  The trait boundary provides a clean extension point for cloud and git backends without
  over-engineering the initial implementation. LocalBackend is zero-config and requires
  no new dependencies. Session directories as bundles (not per-file operations) keeps the
  trait surface minimal. The CLI refactoring changes one path-construction function,
  not the command logic itself.
---

# DESIGN: Local session storage

## Status

Proposed

## Context and problem statement

koto's CLI constructs state file paths via `workflow_state_path()` in `src/discover.rs`,
which returns `<working-dir>/koto-<name>.state.jsonl`. Skills write artifacts to `wip/`
in the same working directory. Both are hardcoded to the git working tree.

The session persistence roadmap (ROADMAP-session-persistence.md) sequences five features.
This design covers feature 1: the storage abstraction and local backend. It doesn't
cover config system (feature 2), engine-provided variables (feature 3), git backend
(feature 4), or cloud sync (feature 5). Those will get their own designs.

The goal is narrow: after this ships, `koto init` creates a session directory at
`~/.koto/sessions/<repo-id>/<name>/`, state files live there instead of the working tree, and
skills that use `koto session dir` get the right path. The trait is designed so future
backends slot in without changing command logic.

## Decision drivers

- **Narrow scope**: only local filesystem, no cloud, no config system, no git backend
- **Future-proof trait**: the trait shape must accommodate cloud sync (sync_down/sync_up)
  when feature 5 adds it, but this design doesn't implement those methods
- **Minimal CLI disruption**: one path-construction change, not a rewrite of command logic
- **Zero new dependencies**: LocalBackend uses std::fs only
- **Session = workflow**: 1:1 mapping, session ID = workflow name

## Considered options

### Decision 1: trait shape

The backend trait needs to support three backends eventually (local, cloud, git) but
this design only implements local. The question is how much of the future interface to
define now.

Key assumptions:
- Cloud sync will need sync_down/sync_up methods (from the broader design research)
- Git backend will need to know the repo root
- All backends produce a local filesystem path that agents can use with file tools

#### Chosen: minimal trait with extension points

Define the trait with the methods needed today plus placeholder methods for sync that
return no-ops. This avoids designing the sync protocol now while reserving the trait
surface.

```rust
pub trait SessionBackend: Send + Sync {
    /// Create a new session directory. Returns the path.
    fn create(&self, id: &str) -> Result<PathBuf>;

    /// Return the session directory path (no I/O, just path computation).
    fn session_dir(&self, id: &str) -> PathBuf;

    /// Return the state file path within the session directory.
    fn state_file_path(&self, id: &str) -> PathBuf;

    /// Check if a session exists.
    fn exists(&self, id: &str) -> bool;

    /// Remove all session artifacts.
    fn cleanup(&self, id: &str) -> Result<()>;

    /// List all sessions.
    fn list(&self) -> Result<Vec<SessionInfo>>;
}
```

Cloud sync methods (sync_down, sync_up) are NOT in this trait yet. Feature 5 will
add them when the cloud backend ships. Adding methods to a trait is a breaking change
in Rust, but since koto controls all implementations (no external consumers), this is
fine.

#### Alternatives considered

- **Full trait including sync methods**: defines sync_down/sync_up now with no-op
  defaults. More future-proof but designs the sync interface before we need it. The
  broader design doc (DESIGN-session-persistence-storage.md) sketched this, but the
  sync protocol details aren't settled and shouldn't constrain the initial trait.
- **No trait, just LocalBackend struct**: simpler but would require refactoring when
  the second backend arrives. The trait costs almost nothing (one extra file, trait
  object dispatch) and makes the extension point explicit.

### Decision 2: where session directories live and what's in them

Sessions need a home directory and an internal layout.

Key assumptions:
- Session ID = workflow name (from PRD R1)
- Session IDs are validated: `^[a-zA-Z][a-zA-Z0-9._-]*$` (must start with letter,
  rejecting `.`/`..` path traversal)
- The JSONL state file currently lives at `<working-dir>/koto-<name>.state.jsonl`

#### Chosen: ~/.koto/sessions/<repo-id>/<id>/ with state file inside

```
~/.koto/
  sessions/
    my-workflow/
      session.meta.json       (created by koto, tracks session metadata)
      koto-my-workflow.state.jsonl  (engine state, same format as today)
      <skill artifacts>       (subdirectories and files created by skills)
```

The session directory starts with only `session.meta.json` and the state file. Skills
create their own subdirectories (e.g., `research/`) as needed — the backend doesn't
know about skill artifact conventions.

`session.meta.json` contains:
```json
{
  "schema_version": 1,
  "id": "my-workflow",
  "created_at": "2026-03-24T10:00:00Z"
}
```

The state file keeps its current name (`koto-<name>.state.jsonl`) — this preserves
compatibility with `workflow-tool` and makes the migration path clearer.

#### Alternatives considered

- **XDG data directory** (`$XDG_DATA_HOME/koto/sessions/`): more standards-compliant
  on Linux but adds complexity for macOS/Windows. `~/.koto/` is simpler and follows
  the pattern of tools like `~/.docker/`, `~/.cargo/`, `~/.npm/`.
- **State file renamed**: could drop the `koto-` prefix since the session directory
  already scopes by name. But keeping the existing name means less code to change and
  existing format-detection logic works unchanged.

### Decision 3: how the CLI switches from hardcoded paths to backend paths

The CLI currently constructs state file paths in one place (`workflow_state_path` in
discover.rs) and wip/ paths are hardcoded in skills (outside koto's codebase). The
question is how koto's own commands switch to using the backend.

Key assumptions:
- `workflow_state_path()` is the single point of path construction for engine state
- `handle_next`, `handle_init`, and other command handlers receive the state path
- Skills will eventually call `koto session dir` but that's feature 3 / skill migration

#### Chosen: backend constructed in run(), passed to command handlers

The `run()` function in `src/cli/mod.rs` (the CLI entry point) constructs the backend
once from hardcoded `LocalBackend::new()` (no config system yet — that's feature 2).
Command handlers receive `&dyn SessionBackend` and call `backend.session_dir(name)` to
get paths instead of calling `workflow_state_path()`.

```rust
pub fn run(app: App) -> Result<()> {
    let working_dir = std::env::current_dir()?;
    let backend = LocalBackend::new(&working_dir)?;
    match app.command {
        Command::Init { name, .. } => handle_init(&backend, &name, ...),
        Command::Next { name, .. } => handle_next(&backend, &name, ...),
        // ...
    }
}
```

Command handlers call `backend.state_file_path(name)` to locate the JSONL state file,
replacing direct calls to `workflow_state_path()`. The state file naming convention
(`koto-<name>.state.jsonl`) is owned by the backend, not by callers.

#### Alternatives considered

- **Global static backend**: set once at startup, accessed via a global. Avoids
  threading `&dyn SessionBackend` through every handler. But globals are harder to
  test and don't compose with future per-command backend overrides.
- **Keep workflow_state_path, add session_dir alongside**: minimal change but creates
  two path systems (one for state files, one for session artifacts) that diverge.
  Better to unify under the backend from the start.

### Decision 4: session directory location

koto must be invisible to developers who don't use it — no committed files, no
gitignore entries, no trace in the repo. Sessions also need to work in non-git
directories.

Key finding from investigation: agent file tools (Read/Edit/Write) in Claude Code
are NOT sandboxed to the repo. The sandbox only restricts Bash commands. File tools
can access any path the OS user has permissions for. This matches the Claude Code
precedent — `~/.claude/` stores per-project state outside repos, scoped by project
path hash.

#### Chosen: ~/.koto/sessions/<repo-id>/

Sessions live entirely outside the repo at `~/.koto/sessions/<repo-id>/<name>/`.
The `repo-id` is the first 16 hex characters of a SHA-256 hash of the canonicalized
working directory path (see Decision 5 for details).

```
~/.koto/
  sessions/
    a1b2c3d4e5f6g7h8/     ← first 16 hex chars of SHA-256(/home/user/repos/my-project)
      my-workflow/
        session.meta.json
        koto-my-workflow.state.jsonl
    e5f6a7b8c9d0e1f2/     ← first 16 hex chars of SHA-256(/home/user/repos/other-project)
      other-workflow/
        ...
```

Properties:
- **Zero repo footprint** — nothing committed, no gitignore, invisible to non-users
- **Works without git** — doesn't depend on .git/ or git config
- **Agent compatible** — file tools aren't sandboxed; for Bash sandbox, users add
  `~/.koto` to `sandbox.filesystem.allowRead`/`allowWrite` in agent settings
- **Per-project scoping** — repo-id prevents cross-project name collisions
- **Same pattern as Claude Code** — `~/.claude/projects/<path-hash>/` is the
  established precedent

#### Alternatives considered

- **In-repo with nested gitignore** (`~/.koto/sessions/<repo-id>/.gitignore` containing `*`):
  follows the DVC/venv/JetBrains pattern. But commits a gitignore file — koto
  becomes visible to non-users. Rejected because the core constraint is zero repo
  footprint.
- **.git/koto/sessions/**: invisible to git by definition. But fails for non-git
  repos and ties session storage to git infrastructure. koto isn't a git extension.
- **Symlink from repo to ~/.koto/ + .git/info/exclude**: bridges sandbox to external
  storage. But fragile (`git clean -fdx` breaks it), Windows symlinks need developer
  mode, and the exclude edit is a git config change.
- **In-repo gitignored directory**: requires root .gitignore modification or
  auto-management. Any committed artifact makes koto visible to non-users.

### Decision 5: repo-id derivation

The design places sessions at `~/.koto/sessions/<repo-id>/<name>/` but didn't specify
how repo-id is computed. The hash algorithm, path canonicalization, truncation length,
and collision handling all need to be pinned down.

Key assumptions:
- sha2 and hex crates are already in Cargo.toml with a `sha256_hex()` utility in
  `src/cache.rs`
- Users don't routinely browse `~/.koto/sessions/` — they use `koto session dir`
- Hash collisions at 64 bits are negligible for per-user project counts (birthday
  paradox gives ~0.1% at 100k projects)

#### Chosen: SHA-256 of canonicalized path, truncated to 16 hex characters

Derive repo-id as follows:

1. **Canonicalize** the working directory path using `std::fs::canonicalize()`. This
   resolves symlinks and removes trailing slashes, `.`, and `..` components.
2. **Hash** the canonicalized path's UTF-8 bytes with SHA-256 using the existing
   `sha256_hex()` function from `src/cache.rs`.
3. **Truncate** to the first 16 hex characters (64 bits).
4. **No collision handling.** At 64 bits, collisions are astronomically unlikely for
   per-user project counts.

```rust
use crate::cache::sha256_hex;

fn repo_id(working_dir: &Path) -> std::io::Result<String> {
    let canonical = std::fs::canonicalize(working_dir)?;
    let hash = sha256_hex(canonical.to_string_lossy().as_bytes());
    Ok(hash[..16].to_string())
}
```

This reuses the existing `sha256_hex` function with zero new dependencies.
Canonicalization via `std::fs::canonicalize()` handles every ambiguity at once:
symlinks, trailing slashes, relative paths, and `.`/`..` components. The 16-character
length provides strong collision resistance while keeping directory names compact.

On systems where `canonicalize()` resolves symlinks, two symlinks to the same repo
resolve to the same canonical path and the same repo-id. This is the desired behavior.
`canonicalize()` requires the path to exist at call time — if called with a non-existent
directory, it returns an error. This is correct: you can't create a session for a
directory that doesn't exist.

#### Alternatives considered

- **Path slug (Claude Code style)**: replace `/` with `-`, producing human-readable
  names like `-home-user-repos-my-project`. This is what Claude Code uses for
  `~/.claude/projects/`. Slugs grow proportionally with path depth, can hit filesystem
  name length limits (255 chars) for deeply nested workspaces, and expose directory
  structure. The human-readability benefit is minimal since users interact through
  `koto session dir`.
- **SHA-256, 8 hex characters**: matches the original design doc example. 32 bits gives
  ~50% collision probability at ~65,000 projects. Unnecessarily risky when 16 characters
  costs nothing extra.
- **SHA-256, full 64 hex characters**: maximum collision resistance but unwieldy in
  terminal output. No practical benefit over 16 characters at human-scale project counts.
- **Blake3 or FNV hash**: faster algorithms, but speed is irrelevant for hashing a
  single path string. Would add new dependencies when SHA-256 is already available.
- **Hybrid slug+hash** (e.g., `my-project-a1b2c3d4`): extracting a meaningful prefix
  requires heuristics and adds complexity. The hash alone is sufficient with
  `koto session dir`.

### Decision 6: migration strategy for existing wip/ workflows

Feature 1 moves session state to `~/.koto/sessions/`. Skills (shirabe, tsukumogami)
hardcode ~150 `wip/` path references. Feature 4 (git backend) restores `wip/` as
opt-in but depends on Feature 2 (config). The question is what fills the gap.

Key assumptions:
- Skills are maintained by the same team that ships Feature 1. No third-party skill
  ecosystem exists that would break without notice.
- `koto session dir` ships in the same release as the storage move (Feature 1 Phase 3
  doesn't slip to a separate release).
- The ~150 hardcoded `wip/` path references follow predictable patterns and are
  mechanically replaceable.

#### Chosen: `koto session dir` as the sole contract, with coordinated skill migration

Skills call `koto session dir <name>` to get the session path. The "migration" is a
skill-side change: replace hardcoded `wip/` path construction with calls to
`koto session dir`. This ships alongside Feature 1, not after it.

The concrete migration:
1. Feature 1 ships with all three implementation phases (including `koto session dir`).
2. Before or simultaneously, shirabe and koto-skills are updated to call
   `koto session dir` instead of hardcoding `wip/`. Skills that run shell commands use
   `SESSION_DIR=$(koto session dir "$name")` and construct paths from that variable.
3. The two changes (koto Feature 1 + skill migration) are released together. Neither
   ships without the other.

No compatibility layer, env var override, or detect-and-warn mechanism in koto itself.
The migration problem is a coordination problem, not a technical one.

#### Alternatives considered

- **KOTO_SESSION_DIR env var override**: skills read the env var if set, fall back to
  `wip/`. Creates a parallel discovery mechanism that competes with `koto session dir`,
  adds a contract that's hard to deprecate, and solves a coordination problem with
  unnecessary technical machinery.
- **Detect-and-warn**: koto detects `wip/` artifacts and warns the user. Reactive
  (warns after breakage), requires koto to understand skill artifact patterns (coupling),
  and adds transition-only code.
- **Ship Features 1+4 together**: blocks the highest-value change behind lower-priority
  work. Feature 4 depends on Feature 2, so this bundles three features into one release,
  contradicting the roadmap's incremental sequencing.
- **Skill migration as hard prerequisite**: functionally identical to the chosen approach
  but with less accurate framing. "Coordinated release" better describes the relationship.

## Decision outcome

The six decisions compose cleanly. The trait provides the abstraction boundary (D1).
LocalBackend stores sessions at `~/.koto/sessions/<repo-id>/<name>/` (D2) outside the
repo entirely (D4). The repo-id is the first 16 hex characters of a SHA-256 hash of
the canonicalized working directory path (D5). The CLI constructs the backend once and
threads it through command handlers (D3). The state file moves into the session
directory but keeps its name and format. Skill migration to `koto session dir` ships
as a coordinated release alongside Feature 1 (D6).

After this ships, `koto init my-workflow` creates
`~/.koto/sessions/<repo-id>/my-workflow/` with `session.meta.json` and
`koto-my-workflow.state.jsonl`. `koto next my-workflow` reads from there.
`koto session dir my-workflow` prints the path. Skills that call `koto session dir`
get the right location for their artifacts.

## Solution architecture

### Components

**`src/session/mod.rs` — trait and types**

```rust
pub trait SessionBackend: Send + Sync {
    fn create(&self, id: &str) -> Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn state_file_path(&self, id: &str) -> PathBuf;
    fn exists(&self, id: &str) -> bool;
    fn cleanup(&self, id: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<SessionInfo>>;
}

pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
}
```

**`src/session/local.rs` — LocalBackend**

```rust
pub struct LocalBackend {
    base_dir: PathBuf,  // ~/.koto/sessions/<repo-id>/
}

impl LocalBackend {
    pub fn new(working_dir: &Path) -> Result<Self> {
        let home = dirs::home_dir().ok_or("no home directory")?;
        let repo_id = repo_id(working_dir)?;
        Ok(Self { base_dir: home.join(".koto").join("sessions").join(repo_id) })
    }

    /// Test-only constructor that uses an arbitrary base directory.
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

fn repo_id(working_dir: &Path) -> std::io::Result<String> {
    let canonical = std::fs::canonicalize(working_dir)?;
    let hash = sha256_hex(canonical.to_string_lossy().as_bytes());
    Ok(hash[..16].to_string())
}

impl SessionBackend for LocalBackend {
    fn create(&self, id: &str) -> Result<PathBuf> {
        validate_session_id(id)?;
        let dir = self.base_dir.join(id);
        fs::create_dir_all(&dir)?;
        write_session_meta(&dir, id)?;
        Ok(dir)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.base_dir.join(id)
    }

    fn state_file_path(&self, id: &str) -> PathBuf {
        self.base_dir.join(id).join(format!("koto-{}.state.jsonl", id))
    }

    fn exists(&self, id: &str) -> bool {
        validate_session_id(id).is_ok()
            && self.base_dir.join(id).join("session.meta.json").exists()
    }

    fn cleanup(&self, id: &str) -> Result<()> {
        validate_session_id(id)?;
        let dir = self.base_dir.join(id);
        if dir.exists() { fs::remove_dir_all(&dir)?; }
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionInfo>> {
        // scan base_dir, read session.meta.json from each subdirectory
        // skip entries without session.meta.json (orphan detection)
    }
}
```

**`src/session/validate.rs` — session ID validation**

Allowlist: `^[a-zA-Z][a-zA-Z0-9._-]*$`. IDs must start with an alphanumeric character,
which rejects `.` and `..` (path traversal) without a separate check. Called by all
public `SessionBackend` methods that accept an ID, not just `create()`.

**`src/cli/mod.rs` — refactored command dispatch**

`run()` constructs `LocalBackend` and passes it to handlers. `handle_init` calls
`backend.create(name)` then writes the initial state file into the returned directory.
Other handlers call `backend.session_dir(name)` to locate the state file.

**`src/cli/session.rs` — session subcommands**

```
koto session dir <name>     → print backend.session_dir(name)
koto session list           → print backend.list() as JSON
koto session cleanup <name> → call backend.cleanup(name)
```

### Key interfaces

| Interface | Location | Used by |
|-----------|----------|---------|
| `SessionBackend` trait | `src/session/mod.rs` | all CLI commands |
| `LocalBackend` | `src/session/local.rs` | `run()` in cli/mod.rs |
| `validate_session_id()` | `src/session/validate.rs` | all `SessionBackend` methods |
| `koto session` subcommands | `src/cli/session.rs` | agents, users |

### Data flow

```
koto init my-wf
  → discover repo root (git rev-parse --show-toplevel)
  → LocalBackend::create("my-wf")
    → validate_session_id("my-wf")
    → mkdir ~/.koto/sessions/<repo-id>/my-wf/
    → write session.meta.json
  → write koto-my-wf.state.jsonl into session dir
  → print JSON result (same as today)

koto next my-wf
  → LocalBackend::session_dir("my-wf")
    → ~/.koto/sessions/<repo-id>/my-wf/
  → read koto-my-wf.state.jsonl from session dir
  → advance workflow (existing logic, unchanged)
  → print directive JSON (same as today)

koto session dir my-wf
  → print ~/.koto/sessions/<repo-id>/my-wf/
```

## Implementation approach

### Phase 1: session module and LocalBackend

Create `src/session/mod.rs`, `src/session/local.rs`, `src/session/validate.rs`.
Implement the trait and LocalBackend. Add `dirs` crate for home directory detection.
Implement `repo_id()` using `sha256_hex()` from `src/cache.rs` with
`std::fs::canonicalize()` and 16-character truncation (Decision 5). Unit tests for
create, session_dir, exists, cleanup, list, ID validation, and repo-id derivation.

The `list()` implementation should skip subdirectories that lack `session.meta.json`
(orphan detection for partial/corrupt session directories).

Deliverables:
- `src/session/` module (3 files)
- `Cargo.toml` — add `dirs` dependency
- Unit tests

### Phase 2: CLI refactoring

Thread `&dyn SessionBackend` through `run()` → command handlers. Replace
`workflow_state_path()` calls with `backend.session_dir()`. Update `handle_init` to
call `backend.create()`. Update `find_workflows_with_metadata()` to delegate to
`backend.list()` instead of scanning the working directory, so that `koto workflows`
reflects the new storage location. Verify all existing tests pass with state files
in the new location.

Deliverables:
- `src/cli/mod.rs` — refactored command dispatch
- `src/discover.rs` — `workflow_state_path()` used internally by LocalBackend only,
  `find_workflows_with_metadata()` updated to scan session directory
- Updated integration tests

### Phase 3: session subcommands and auto-cleanup

Add `koto session dir|list|cleanup` subcommands. Add automatic cleanup when a workflow
reaches a terminal state. End-to-end tests.

This phase must ship in the same release as Phases 1-2. `koto session dir` is the
contract that skills depend on (Decision 6), so it can't slip to a follow-up release.

Deliverables:
- `src/cli/session.rs` — session subcommands
- Auto-cleanup logic in the advance path
- End-to-end tests

### Coordinated skill migration

Not part of koto's implementation, but part of Feature 1's release scope (Decision 6).
Shirabe and koto-skills must be updated to call `koto session dir` instead of
hardcoding `wip/` paths. This is a mechanical replacement of ~150 path references.
The skill updates and koto Feature 1 ship together as a coordinated release.

## Security considerations

**Session ID validation.** Session IDs are used in filesystem paths. The allowlist
`^[a-zA-Z][a-zA-Z0-9._-]*$` requires IDs to start with a letter, which rejects `.`
and `..` (path traversal) without a separate check. Validation runs in all public
`SessionBackend` methods that accept an ID, not just `create()`, preventing bypass
through direct calls to `session_dir()` or `cleanup()`.

**Home directory trust.** `~/.koto/sessions/<repo-id>/` is writable by the current user.
No elevated permissions needed. The `~/.koto/` directory should be created with mode
0700 on first use to prevent other users from reading session artifacts. Subsequent
permissions follow the user's umask.

**Cross-project session access.** Adding `~/.koto` to the agent sandbox allowlist
grants access to all projects' session directories, not just the current one. This is
inherent to the home-directory storage model. An agent running in project A could read
or write session artifacts from project B. This is acceptable because agents already
have filesystem access to `~/` via file tools (which aren't sandboxed), and the sandbox
allowlist only affects Bash commands.

**Path canonicalization.** `std::fs::canonicalize()` converts paths to UTF-8 via
`to_string_lossy()`, which replaces invalid bytes with U+FFFD. Two paths differing
only in invalid UTF-8 sequences could hash identically. This is an edge case on
modern systems where paths are almost always valid UTF-8, but the implementation
should log a warning if lossy conversion occurs.

**No secrets in session artifacts.** Session directories contain workflow state and
skill artifacts (research, plans, decisions). These aren't secrets, but they may
contain project-specific information. The local backend doesn't transmit anything
off the machine. Cloud sync (feature 5) will add exposure considerations.

## Consequences

### Positive

- State files move out of the repo entirely. PRs never show session artifacts.
- Zero repo footprint — koto is invisible to developers who don't use it.
- The SessionBackend trait provides a clean extension point for cloud and git backends.
- `koto session dir` gives skills a stable API for artifact location.
- Auto-cleanup replaces manual `rm -rf wip/` and CI enforcement.
- Works without git — sessions are scoped by working directory path, not git repo.
- Agent file tools (Read/Edit/Write) access `~/.koto/` without sandbox restrictions.

### Negative

- Skills that hardcode `wip/` paths must be updated before Feature 1 ships. The
  coordinated release (Decision 6) means Feature 1's effective scope includes the
  skill migration (~150 path references across shirabe and koto-skills).
- There's no backward-compatible transition period. Old skill versions and new koto
  versions are incompatible. Acceptable because skills and koto are co-versioned.
- Sessions live outside the repo. Agent Bash commands (as opposed to file tools) may
  need sandbox configuration to access `~/.koto/`. For Claude Code:
  `sandbox.filesystem.allowRead: ["~/.koto"]` in settings.
- The `dirs` crate is needed for home directory detection (adds one dependency).
- If a user moves or renames their project directory, the repo-id hash changes and
  existing sessions become orphaned at the old hash. `koto session list` can detect
  this by checking whether the source directory still exists.
- Partial/corrupt session directories (directory exists but `session.meta.json` is
  missing) are invisible to `exists()` and `list()` but still occupy disk.

### Mitigations

- The git backend (feature 4) restores wip/ as an opt-in preference, not a
  compatibility bridge. Users who want `wip/` in their repo can configure it later.
- Sandbox configuration for Bash commands is a one-time user setup, documented in
  koto's installation guide.
- `dirs` is a small, widely-used crate with no transitive dependencies.
- Orphaned sessions from directory renames can be cleaned up with `koto session cleanup`.
- `list()` skips directories without `session.meta.json`, preventing corrupt entries
  from appearing. A future `koto session gc` could detect and remove these orphans.
