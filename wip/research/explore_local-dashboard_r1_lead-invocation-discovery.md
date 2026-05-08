# Lead: What is the invocation and session discovery UX?

## Findings

### Session Storage and Discovery Model

**Location:** Sessions are stored under `~/.koto/sessions/<repo-id>/` where `<repo-id>` is derived from the canonicalized working directory. Each session has its own directory: `~/.koto/sessions/<repo-id>/<workflow-name>/`.

Evidence:
- `src/session/local.rs` lines 28-45: `LocalBackend::new()` constructs the base directory as `home/.koto/sessions/<repo_id>`
- `src/session/local.rs` line 68-70: `session_dir()` returns `base_dir.join(id)` for a workflow name
- State files are named `koto-<workflow-name>.state.jsonl` (defined in `src/session/mod.rs` line 147-149)

**Discovery Mechanism:** Sessions are discovered by scanning the repo-scoped session directory. The backend uses `backend.list()` which enumerates all subdirectories, checks for the presence of `koto-<name>.state.jsonl`, and extracts metadata (created_at, template_hash, parent_workflow) from each state file header.

Evidence:
- `src/session/local.rs` lines 85-133: `LocalBackend::list()` reads directory entries, filters for valid state files, and deserializes headers
- `src/discover.rs` lines 83-104: `find_workflows_with_metadata()` delegates to `backend.list()`
- `src/cli/mod.rs` lines 830-879: `Command::Workflows` handler calls `find_workflows_with_metadata()`

### CLI Commands for Session Visibility

The `koto` CLI provides the following commands for session discovery and inspection:

**`koto workflows`** (no arguments required)
- Outputs all active workflows as a JSON array with metadata
- Supports filtering:
  - `--roots`: only root workflows (parent_workflow is None)
  - `--children <NAME>`: only children of a named parent
  - `--orphaned`: workflows whose parent no longer exists
- Evidence: `src/cli/mod.rs` lines 153-166, 830-879

**`koto status <name>`** (read-only, no state changes)
- Shows the current state of a workflow
- Evidence: `src/cli/mod.rs` lines 186-190

**`koto session list`** (subcommand)
- Lists all sessions as JSON with SessionInfo structure containing id, created_at, template_hash, parent_workflow
- Evidence: `src/cli/mod.rs` lines 251-252, 883-885
- Returns the same data as `koto workflows` but through a different API path

**`koto session dir <name>`**
- Prints the absolute session directory path
- Evidence: `src/cli/mod.rs` lines 246-250

The output of these commands is JSON-formatted, not human-readable UI.

### No Existing Dashboard or Watch Mode

**Finding:** There is no existing `koto dashboard`, `koto watch`, or `koto serve` command in the codebase. No server, WebSocket, or long-polling infrastructure exists for live session updates.

Evidence:
- Grepping for `dashboard|server|watch|listen|port` in src/ returns no CLI command handlers
- `src/main.rs` is minimal: only parses CLI args and calls `run(app)`
- The `Command` enum in `src/cli/mod.rs` lines 78-209 lists all available commands; dashboard/watch/serve absent

### Session Identifier and Naming

**Naming Convention:** Session IDs are workflow names, validated with strict pattern rules to prevent path traversal.

Evidence:
- `src/discover.rs` lines 19-71: `validate_workflow_name()` enforces pattern `[a-zA-Z0-9][a-zA-Z0-9._-]*` up to 255 chars
- Hierarchical workflows use dot notation: `parent.task` where `parent` is the parent workflow name and `task` is the child identifier
- `src/session/local.rs` lines 252-297: `relocate()` parses parent from `to` by splitting on the last `.` separator

**No UUID-based session IDs:** Sessions are identified by their workflow names, not UUIDs or random identifiers.

### Session-Feed Data Contract (F2)

The F2 specification (`docs/reference/session-feed.md`) defines a versioned JSONL format for session logs. Each session stores events in `koto-<name>.state.jsonl`:
- **Header line** (line 1): `StateFileHeader` containing schema_version, workflow, template_hash, created_at, session_id (optional), parent_workflow (optional), template_source_dir (optional)
- **Event lines** (subsequent): Each event is a JSON object with seq, timestamp, event_type, and a payload

Evidence:
- `src/engine/persistence.rs`: All functions (`append_header`, `append_event`, `read_events`, `read_header`) work with the state file directly
- `docs/reference/session-feed.md` lines 1-30: Formal specification of header fields
- `src/session/local.rs` lines 199-208: `init_state_file()` bundles header and events into JSONL format

The spec defines 15 event types across three tiers (Tier 1: core, Tier 2: observability, Unknown: system):
- **Tier 1 (Core):** workflow_initialized, transitioned, directed_transition, rewound, evidence_submitted, workflow_cancelled, gate_override_recorded, batch_finalized
- **Tier 2 (Observability):** integration_invoked, context_added, default_action_executed, decision_recorded, gate_evaluated
- Unknown (system): Unknown (for forward compatibility)

**File location specification:** The F2 spec does NOT explicitly define where files live; that's left to the backend implementation. The local backend stores them at `~/.koto/sessions/<repo-id>/<workflow-name>/koto-<workflow-name>.state.jsonl`.

### Backend Abstraction and Configuration

**Backend Configuration:** Koto supports pluggable backends via the `SessionBackend` trait. The backend is selected via configuration:
- Default: `"local"` (files on disk)
- Optional: `"cloud"` (for cloud sync, partially implemented)
- Configured via `session.backend` in project or user config

Evidence:
- `src/cli/mod.rs` lines 496-511: `build_backend()` reads config and constructs the backend
- `src/config/mod.rs` lines 13-20: `SessionConfig` struct with backend field
- `src/session/mod.rs` lines 156-295: `SessionBackend` trait defining the interface

**LocalBackend specifics:**
- Can be overridden at test time via `KOTO_SESSIONS_BASE` environment variable (for tests with controlled storage paths)
- Stores sessions at `~/.koto/sessions/<repo-id>/` with read-only (0600) state files on Unix
- Evidence: `src/cli/mod.rs` lines 514-521; `src/session/local.rs` lines 56-74

### No Workspace Concept

**Finding:** Koto does NOT have a "workspace" or "session directory" concept beyond the convention of grouping sessions by repo-id. Each repository hashes to a single repo-id directory, and all workflows in that repo's invocations share that directory. There is no mechanism to:
- Select or switch between named workspaces
- Organize sessions into logical groups beyond parent-child relationships
- Pin or bookmark specific sessions

Evidence:
- No `Workspace` type in the codebase
- Sessions are scoped globally by repo-id hash, not by user-defined groups
- Parent-child relationships are the only session hierarchy mechanism (`src/session/mod.rs` line 140)

### Summary of Discovery UX

**Current Model:**
1. User runs CLI commands like `koto workflows` or `koto session list` in a project directory
2. Koto reads `~/.koto/sessions/<repo-id>/` and enumerates subdirectories with valid state files
3. Metadata is extracted from each state file header
4. Results are printed as JSON to stdout

**What is NOT present:**
- No dashboard server or web UI
- No session watching or live-update mode
- No session argument auto-detection (user must provide workflow name to commands like `koto next`, `koto status`)
- No session ID auto-increment or UUIDs (names are workflow names only)
- No workspace scoping (all sessions in a repo are treated equally)
- No default session or "current" session concept

## Implications

### For the Dashboard PRD

The dashboard must answer these design questions:

1. **Invocation Model:** How does a user start the dashboard?
   - Option A: `koto dashboard [--port <N>]` (new CLI command)
   - Option B: External tool that watches `~/.koto/sessions/<repo-id>/` directly
   - Option C: Agent embeds a dashboard server in a long-running process

2. **Session Discovery:** Does the dashboard:
   - Auto-discover sessions by listing the directory? (feasible, matches current list() model)
   - Accept a session ID filter on startup? (e.g., `koto dashboard --session <name>`)
   - Require a manifest or explicit registration? (no such mechanism exists today)

3. **Live Updates:** How does the dashboard detect new events?
   - File system polling? (Watch `~/.koto/sessions/<repo-id>/**/koto-*.state.jsonl`)
   - inotify/FSEvents? (Platform-specific, Unix-ready)
   - Callback API from koto engine? (Not yet designed; would require embedding koto as a library)

4. **Workspace Scoping:** Should the dashboard operate on:
   - A single repo (scoped to one `~/.koto/sessions/<repo-id>/`)?
   - All repos the user has run workflows in?
   - User-selected projects (requires new config mechanism)?

5. **Session Hierarchy Display:** How should parent-child relationships be rendered?
   - Tree view of parent workflows with children as sub-nodes?
   - Flat list with parent metadata on each child?
   - Collapsible sections per parent?

### For Implementation

The session storage model is stable and well-tested:
- State files are immutable-append JSONL (suitable for tailing/streaming)
- Headers are self-contained (can be read independently for fast discovery)
- Session naming is deterministic (no UUIDs to resolve)
- Parent-child relationships are stored in the header

The dashboard can leverage:
- `backend.list()` for initial discovery (O(N) directory scan)
- Direct file I/O for subsequent reads (no API required; state files are readable)
- `src/engine/persistence` helpers (public, reusable) for parsing headers and events
- Session-feed spec (`docs/reference/session-feed.md`) as the contract

## Surprises

1. **No session ID or UUID:** Sessions are purely identified by workflow name. There is no randomized session ID unless explicitly set in the header (field is optional and unused in practice). This simplifies discovery but means session names are the sole identifier.

2. **Repo-scoped, not directory-scoped:** Sessions are grouped by a hash of the repository (canonicalized working directory), not the project directory where `koto init` was run. Two separate clones of the same repository (different paths, same content) will have different repo-ids and separate session directories. This is intentional but may surprise users expecting sessions to be path-relative.

3. **No default backend configuration required:** The local backend is the hardcoded default (in `src/config/mod.rs` line 32). Users who never configure `session.backend` still get local file storage. Cloud sync is opt-in.

4. **F2 spec is silent on file location:** The session-feed data contract defines the JSONL format but doesn't constrain where files live. The spec assumes an abstract "backend" and lets implementations choose storage. This is good for flexibility but means the dashboard spec must explicitly require a storage location (or document the assumption).

5. **Session list is not incremental:** There is no "list changes since timestamp" API. The dashboard must re-scan the entire directory to detect new sessions. For single-digit workflows this is negligible; at scale (thousands of sessions) this could become a bottleneck.

## Open Questions

1. **Should the dashboard be a separate binary or a koto CLI subcommand?**
   - Implication: Affects how the tool is invoked and which dependencies it carries
   - Needs human input on deployment model (agent tool vs. standalone service)

2. **What is the target deployment environment?**
   - Agent's local machine (read `~/.koto/sessions/` directly)?
   - Headless server (would need SSH or S3 access to session storage)?
   - Managed cloud service (would need API to push events)?
   - Implication: Determines whether the dashboard reads local files or pulls from an API

3. **Does the dashboard need to support cloud backend sessions?**
   - Cloud backend sessions are synced to S3; would the dashboard need S3 credentials?
   - Or is the initial scope local-backend-only?
   - Implication: Affects session discovery mechanism and authentication model

4. **Should the dashboard show all repos or be scoped to one?**
   - If all repos: dashboard must scan multiple `~/.koto/sessions/<repo-id>/` directories
   - If one repo: how does the user specify which repo? Environment variable? Startup argument? Config?
   - Implication: Affects session discovery and filtering logic

5. **What is the update cadence?**
   - Poll every N milliseconds? (If so, what latency is acceptable?)
   - Watch for file changes in real-time? (Requires fsnotify or platform APIs)
   - Push from koto engine? (Would require a callback or event sink)
   - Implication: Affects responsiveness and resource usage of the dashboard

6. **How should the dashboard handle session metadata extraction at scale?**
   - Reading 1000 state file headers sequentially could be slow
   - Should headers be cached (and invalidated how)?
   - Should discovery be paginated or lazy-loaded?
   - Implication: Performance and UX at scale

## Summary

The koto engine discovers sessions by repo-scoped directory enumeration: listing `~/.koto/sessions/<repo-id>/` for subdirectories with valid `koto-<name>.state.jsonl` files. The CLI provides `koto workflows` and `koto session list` as the primary discovery interfaces, both returning JSON metadata (id, created_at, template_hash, parent_workflow). **There is no existing dashboard, watch mode, or server infrastructure.** The session-feed spec (F2) defines the JSONL log format but not file location; the local backend stores logs as immutable-append files at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`. The main open questions are whether the dashboard is a CLI subcommand or standalone binary, whether it targets local or cloud storage, and what update mechanism (polling vs. fsnotify vs. push) best serves the deployment environment.
