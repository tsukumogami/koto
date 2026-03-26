---
status: Planned
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto writes workflow state to hardcoded paths in the git working tree (wip/). There's
  no abstraction over where session artifacts live, no session lifecycle management, and
  no way to change the storage location without updating every skill. This design covers
  the foundational feature: a SessionBackend trait, a local filesystem backend, session
  CLI commands, runtime variable substitution for `{{SESSION_DIR}}` in templates, and
  the CLI refactoring needed to use backend-provided paths instead of hardcoded wip/.
decision: |
  A SessionBackend trait with five methods (create, session_dir, cleanup, list, exists)
  and a LocalBackend that stores sessions at ~/.koto/sessions/{repo-id}/{name}/. The
  repo-id is the first 16 hex characters of a SHA-256 hash of the canonicalized working
  directory path, scoping sessions per-project. Sessions live entirely outside the repo
  (zero repo footprint, invisible to non-users). Agent file tools access ~/.koto/ without
  sandbox restrictions. CLI commands get the session path from the backend instead of
  constructing wip/ paths. Skills use `koto session dir` as their sole path discovery
  mechanism. Templates use `{{SESSION_DIR}}` in gate commands and directives, substituted
  at runtime by `handle_next`.
rationale: |
  The trait boundary provides a clean extension point for cloud and git backends without
  over-engineering the initial implementation. LocalBackend is zero-config and requires
  no new dependencies. Session directories as bundles (not per-file operations) keeps the
  trait surface minimal. The CLI refactoring changes one path-construction function,
  not the command logic itself.
---

# DESIGN: Local session storage

## Status

Accepted

## Context and problem statement

koto's CLI constructs state file paths via `workflow_state_path()` in `src/discover.rs`,
which returns `<working-dir>/koto-<name>.state.jsonl`. Skills write artifacts to `wip/`
in the same working directory. Both are hardcoded to the git working tree.

The session persistence roadmap (ROADMAP-session-persistence.md) sequences the work.
This design covers the storage abstraction, local backend, and runtime variable
substitution for `{{SESSION_DIR}}`. Without variable substitution, templates can't
reference the session directory in gate commands or directives, making the storage
move useless to template authors. Config system, git backend, and cloud sync get
their own designs.

After this ships, `koto init` creates a session directory at
`~/.koto/sessions/<repo-id>/<name>/`, state files live there instead of the working
tree, templates use `{{SESSION_DIR}}` to reference the session path, and skills use
`koto session dir` for path discovery. The trait is designed so future backends slot
in without changing command logic.

## Decision drivers

- **Narrow scope**: only local filesystem, no cloud, no config system, no git backend
- **Future-proof trait**: the trait shape must accommodate cloud sync (sync_down/sync_up)
  when the cloud sync feature adds it, but this design doesn't implement those methods
- **Minimal CLI disruption**: one path-construction change, not a rewrite of command logic
- **Zero new dependencies**: LocalBackend uses std::fs only
- **Session = workflow**: 1:1 mapping, session ID = workflow name

## Considered options

### Decision 1: trait shape

The backend trait needs to support three backends eventually (local, cloud, git) but
this design only implements local. The question is how much of the future interface to
define now.

Key assumptions:
- Cloud sync will need sync_down/sync_up methods
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

    /// Check if a session exists.
    fn exists(&self, id: &str) -> bool;

    /// Remove all session artifacts.
    fn cleanup(&self, id: &str) -> Result<()>;

    /// List all sessions.
    fn list(&self) -> Result<Vec<SessionInfo>>;
}
```

Cloud sync methods (sync_down, sync_up) are NOT in this trait yet. The cloud sync
feature will add them when that backend ships. Adding methods to a trait is a breaking change
in Rust, but since koto controls all implementations (no external consumers), this is
fine.

#### Alternatives considered

- **Full trait including sync methods**: defines sync_down/sync_up now with no-op
  defaults. More future-proof but designs the sync interface before we need it. The
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

#### Chosen: session directory at ~/.koto/ with state file inside

```
~/.koto/
  sessions/
    my-workflow/
      koto-my-workflow.state.jsonl  (engine state, same format as today)
      <skill artifacts>            (subdirectories and files created by skills)
```

The session directory starts with just the state file. Skills create their own
subdirectories (e.g., `research/`) as needed — the backend doesn't know about skill
artifact conventions. No separate metadata file — the `StateFileHeader` in the JSONL
state file already contains the workflow name, creation timestamp, and schema version.

The state file keeps its current name (`koto-<name>.state.jsonl`) — this preserves
compatibility with existing tooling that parses the state file format.

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
- Skills call `koto session dir` to discover the session path (see Decision 6)

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

Command handlers call `state_file_name(name)` (a free function) to construct the
state file name, then join it with `backend.session_dir(name)` to get the full path.
The naming convention (`koto-<name>.state.jsonl`) is a free function because it
doesn't vary across backends — it's not a backend-specific behavior.

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

#### Chosen: `~/.koto/sessions/<repo-id>/`

Sessions live entirely outside the repo at `~/.koto/sessions/<repo-id>/<name>/`.
The `repo-id` is the first 16 hex characters of a SHA-256 hash of the canonicalized
working directory path (see Decision 5 for details).

```
~/.koto/
  sessions/
    a1b2c3d4e5f6g7h8/     ← first 16 hex chars of SHA-256(/home/user/repos/my-project)
      my-workflow/
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

Sessions live at `~/.koto/sessions/<repo-id>/<name>/`, where repo-id must uniquely
identify a project directory. The hash algorithm, path canonicalization, truncation
length, and collision handling need to be specified.

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
- **SHA-256, 8 hex characters**: shorter but 32 bits gives
  ~50% collision probability at ~65,000 projects. Unnecessarily risky when 16 characters
  costs nothing extra.
- **SHA-256, full 64 hex characters**: maximum collision resistance but unwieldy in
  terminal output. No practical benefit over 16 characters at human-scale project counts.
- **Blake3 or FNV hash**: faster algorithms, but speed is irrelevant for hashing a
  single path string. Would add new dependencies when SHA-256 is already available.
- **Hybrid slug+hash** (e.g., `my-project-a1b2c3d4`): extracting a meaningful prefix
  requires heuristics and adds complexity. The hash alone is sufficient with
  `koto session dir`.

### Decision 6: how skills discover the session path

Skills need to write artifacts (research outputs, plans, decision reports) into the
session directory. The question is how they discover where that directory is.

Key assumptions:
- Skills don't know (and shouldn't know) which backend is active
- Skills run as agent workflows that invoke koto CLI commands
- The session directory path must be available before the skill writes its first artifact

#### Chosen: `koto session dir` as the sole discovery mechanism

Skills call `koto session dir <name>` to get the session path. This is the only
supported way to resolve a session directory — skills never construct paths themselves.
Shell-based skills capture the output in a variable:

```bash
SESSION_DIR=$(koto session dir "$name")
```

File-tool-based skills call `koto session dir` once and use the returned path for all
subsequent Read/Edit/Write operations.

This keeps the backend fully opaque to skills. Whether sessions live at `~/.koto/`,
in the git working tree, or in cloud storage, the discovery mechanism is the same.

#### Alternatives considered

- **KOTO_SESSION_DIR env var**: skills read the env var to find the session path.
  Creates a parallel discovery mechanism that competes with `koto session dir` and
  adds a contract that's hard to deprecate. The CLI command is strictly more capable
  (it can validate the session exists, create it if needed, etc.).
- **Template-provided variable**: templates include `{{SESSION_DIR}}` that the engine
  substitutes at runtime. This complements `koto session dir` — see Decision 7.
- **Convention-based paths**: skills assume a fixed path relative to the working
  directory (e.g., `wip/`). Couples skills to a specific backend and breaks when
  the storage location changes.

### Decision 7: how `{{SESSION_DIR}}` is substituted in templates

Templates reference the session directory in gate commands (`test -f {{SESSION_DIR}}/plan.md`)
and directives ("Write to `{{SESSION_DIR}}/plan.md`"). Variable substitution doesn't
exist in the engine yet — `{{VAR}}` tokens are currently returned raw to the agent.
The question is where to add substitution and how much infrastructure to build.

Key assumptions:
- `SESSION_DIR` is the only built-in variable needed for this feature
- No escaping mechanism is needed yet for literal `{{SESSION_DIR}}` in templates
- Simple sequential `str::replace` is sufficient (no recursion or ordering concerns)

#### Chosen: runtime substitution at the output boundary in `handle_next`

Substitute `{{SESSION_DIR}}` at two points in the `handle_next` flow:

1. **Gate commands**: replace `{{SESSION_DIR}}` in each `Gate.command` string before
   passing to `evaluate_gates()`. The gate closure runs inside the `advance_until_stop`
   loop and is invoked per-state, so substitution must happen inside the closure on
   every invocation, not once before the loop.
2. **Directives**: replace `{{SESSION_DIR}}` in the `directive` string of `NextResponse`
   before serialization to stdout.

Both use a single utility function:

```rust
// src/cli/vars.rs
pub fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key);  // produces {{KEY}}
        result = result.replace(&token, value);
    }
    result
}
```

The vars map is built once per `handle_next` call with one entry:
`SESSION_DIR` -> `backend.session_dir(name)`. If a template declares `SESSION_DIR`
in its `variables:` block, `handle_next` returns a runtime error — built-in variable
names are reserved and cannot be shadowed. When `--var` lands (issue #67), user
variables merge into the same map with the same override protection, and the same
`substitute_vars` function handles everything.

Runtime resolution is correct because `SESSION_DIR` depends on the backend, which
could change after init (backend switch, repo relocation). The state file stores raw
`{{SESSION_DIR}}` tokens, and each `koto next` call resolves them fresh.

#### Alternatives considered

- **Compile-time substitution in `koto init`**: bakes the resolved path into the state
  file at initialization. Breaks if the session directory changes after init (backend
  switch, `~/.koto` relocation). Runtime substitution is strictly more correct.
- **Shell environment variable for gates only**: sets `SESSION_DIR` as an env var when
  spawning `sh -c`. Only solves gates, not directives — forces two different
  substitution mechanisms. Also makes gate behavior depend on implicit env state rather
  than explicit command strings.
- **Build the full `--var` infrastructure now**: implements `Variables::substitute()` as
  a general-purpose system with CLI flag parsing, validation, and override protection.
  Violates the narrow-scope constraint. `--var` needs its own design (issue #67). The
  chosen `HashMap + substitute_vars` approach is the natural foundation that `--var`
  will extend, so no work is wasted.

## Decision outcome

The seven decisions compose cleanly. The trait provides the abstraction boundary (D1).
LocalBackend stores sessions at `~/.koto/sessions/<repo-id>/<name>/` (D2) outside the
repo entirely (D4). The repo-id is the first 16 hex characters of a SHA-256 hash of
the canonicalized working directory path (D5). The CLI constructs the backend once and
threads it through command handlers (D3). The state file moves into the session
directory but keeps its name and format. Skills discover the session path through
`koto session dir` (D6). Templates use `{{SESSION_DIR}}` in gate commands and
directives, substituted at runtime by `handle_next` (D7).

After this ships, `koto init my-workflow` creates
`~/.koto/sessions/<repo-id>/my-workflow/` with `koto-my-workflow.state.jsonl`. `koto next my-workflow` substitutes `{{SESSION_DIR}}`
in gate commands and directives, then reads state from the session directory.
`koto session dir my-workflow` prints the path for skill-level discovery.

## Solution architecture

### Components

**`src/session/mod.rs` — trait and types**

```rust
pub trait SessionBackend: Send + Sync {
    fn create(&self, id: &str) -> Result<PathBuf>;
    fn session_dir(&self, id: &str) -> PathBuf;
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

/// State file naming convention. Free function, not on the trait —
/// the naming convention doesn't vary across backends.
pub fn state_file_name(id: &str) -> String {
    format!("koto-{}.state.jsonl", id)
}

impl SessionBackend for LocalBackend {
    fn create(&self, id: &str) -> Result<PathBuf> {
        validate_session_id(id)?;
        let dir = self.base_dir.join(id);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.base_dir.join(id)
    }

    fn exists(&self, id: &str) -> bool {
        self.base_dir.join(id).join(state_file_name(id)).exists()
    }

    fn cleanup(&self, id: &str) -> Result<()> {
        let dir = self.base_dir.join(id);
        if dir.exists() { fs::remove_dir_all(&dir)?; }
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionInfo>> {
        // scan base_dir for subdirectories containing a state file
        // extract metadata from StateFileHeader
    }
}
```

**`src/session/validate.rs` — session ID validation**

Allowlist: `^[a-zA-Z][a-zA-Z0-9._-]*$`. IDs must start with a letter, which rejects
`.` and `..` (path traversal) without a separate check. Called by `create()` at the
session creation boundary. Other methods (`session_dir`, `exists`, `cleanup`) are
pure path computations or read-only checks that don't need validation — IDs that
reach them were already validated at creation time.

**`src/cli/mod.rs` — refactored command dispatch**

`run()` constructs `LocalBackend` and passes it to handlers. `handle_init` calls
`backend.create(name)` then writes the initial state file into the returned directory.
Other handlers call `backend.session_dir(name)` to locate the state file.

**`src/cli/vars.rs` — variable substitution**

```rust
pub fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key);
        result = result.replace(&token, value);
    }
    result
}
```

Called by `handle_next` to substitute `{{SESSION_DIR}}` in gate commands before
shell execution and in directives before JSON serialization. The vars map is built
once per call: `{"SESSION_DIR": backend.session_dir(name)}`. Future `--var` support
adds user entries to the same map.

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
| `validate_session_id()` | `src/session/validate.rs` | `create()` |
| `state_file_name()` | `src/session/mod.rs` | CLI command handlers |
| `substitute_vars()` | `src/cli/vars.rs` | `handle_next` (gates + directives) |
| `koto session` subcommands | `src/cli/session.rs` | agents, users |

### Data flow

```
koto init my-wf
  → discover repo root (git rev-parse --show-toplevel)
  → LocalBackend::create("my-wf")
    → validate_session_id("my-wf")
    → mkdir ~/.koto/sessions/<repo-id>/my-wf/
  → write koto-my-wf.state.jsonl into session dir
  → print JSON result (same as today)

koto next my-wf
  → build vars map: {SESSION_DIR: backend.session_dir("my-wf")}
  → read koto-my-wf.state.jsonl from session dir
  → substitute {{SESSION_DIR}} in gate commands before shell execution
  → advance workflow (existing logic, unchanged)
  → substitute {{SESSION_DIR}} in directive before JSON serialization
  → print directive JSON with resolved paths

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

The `list()` implementation scans for state files (`koto-*.state.jsonl`) and extracts
metadata from the `StateFileHeader`. Subdirectories without a state file are skipped.

Deliverables:
- `src/session/` module (3 files)
- `Cargo.toml` — add `dirs` dependency
- Unit tests

### Phase 2: CLI refactoring and variable substitution

Thread `&dyn SessionBackend` through `run()` → command handlers. Replace
`workflow_state_path()` calls with `backend.session_dir()`. Update `handle_init` to
call `backend.create()`. Update `find_workflows_with_metadata()` to delegate to
`backend.list()` instead of scanning the working directory, so that `koto workflows`
reflects the new storage location.

Add `src/cli/vars.rs` with `substitute_vars()`. Wire it into `handle_next`: build
the vars map from `backend.session_dir(name)`, substitute `{{SESSION_DIR}}` in gate
commands before shell execution and in directives before JSON serialization. This is
~45 lines of new code.

Verify all existing tests pass with state files in the new location. Add tests for
variable substitution in gates and directives.

Deliverables:
- `src/cli/mod.rs` — refactored command dispatch
- `src/cli/vars.rs` — variable substitution utility
- `src/discover.rs` — `workflow_state_path()` used internally by LocalBackend only,
  `find_workflows_with_metadata()` updated to scan session directory
- Updated integration tests
- Variable substitution tests

### Phase 3: session subcommands and auto-cleanup

Add `koto session dir|list|cleanup` subcommands. Add automatic cleanup when a workflow
reaches a terminal state. End-to-end tests.

Update existing templates and documentation that hardcode `wip/` paths to use
`{{SESSION_DIR}}`. This includes `hello-koto.md` (gate command and directive),
the skill authoring guide, and test documentation.

Deliverables:
- `src/cli/session.rs` — session subcommands
- Auto-cleanup logic in the advance path
- Updated templates and documentation (`hello-koto.md`, `custom-skill-authoring.md`)
- End-to-end tests

## Security considerations

**Session ID validation.** Session IDs are used in filesystem paths. The allowlist
`^[a-zA-Z][a-zA-Z0-9._-]*$` requires IDs to start with a letter, which rejects `.`
and `..` (path traversal) without a separate check. Validation runs at the creation
boundary (`create()`). Other methods receive IDs that were already validated at
creation time.

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
off the machine. The cloud sync feature will add exposure considerations.

## Consequences

### Positive

- State files move out of the repo entirely. PRs never show session artifacts.
- Zero repo footprint — koto is invisible to developers who don't use it.
- The SessionBackend trait provides a clean extension point for cloud and git backends.
- `koto session dir` gives skills a stable API for artifact location.
- `{{SESSION_DIR}}` gives template authors a way to reference the session path in gate
  commands and directives without hardcoding paths.
- The `substitute_vars` utility provides a clean extension point for future `--var`
  support (issue #67) with no second substitution path needed.
- Auto-cleanup on workflow completion prevents stale session accumulation.
- Works without git — sessions are scoped by working directory path, not git repo.
- Agent file tools (Read/Edit/Write) access `~/.koto/` without sandbox restrictions.

### Negative

- Sessions live outside the repo. Agent Bash commands (as opposed to file tools) may
  need sandbox configuration to access `~/.koto/`. For Claude Code:
  `sandbox.filesystem.allowRead: ["~/.koto"]` in settings.
- The `dirs` crate is needed for home directory detection (adds one dependency).
- If a user moves or renames their project directory, the repo-id hash changes and
  existing sessions become orphaned at the old hash. `koto session list` can detect
  this by checking whether the source directory still exists.
- Empty session directories (directory exists but state file is missing) are invisible
  to `exists()` and `list()` but still occupy disk.

### Mitigations

- Sandbox configuration for Bash commands is a one-time user setup, documented in
  koto's installation guide.
- `dirs` is a small, widely-used crate with no transitive dependencies.
- Orphaned sessions from directory renames can be cleaned up with `koto session cleanup`.
- `list()` skips directories without a state file, preventing empty entries from
  appearing. A future `koto session gc` could detect and remove these orphans.
