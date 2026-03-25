# Lead: How should storage backends plug in and what's the sync model?

## Findings

### Backend abstraction

A `SessionBackend` trait operating on session bundles (not individual files) is the
cleanest approach. The trait surface:

- `init(session_id) -> Result<SessionDir>`
- `load(session_id) -> Result<SessionDir>` (download from remote if needed)
- `sync(session_id) -> Result<()>` (upload local changes to remote)
- `cleanup(session_id) -> Result<()>`
- `list() -> Result<Vec<SessionId>>`

Bundle semantics: the backend manages a directory of files as a unit. It doesn't
need to understand individual files within the session.

### Three concrete backends

**LocalFileBackend** (default): `~/.koto/sessions/<workflow-id>/`. No config needed.
Files written directly by agents via file tools. Cleanup deletes the directory.

**CloudBackend**: HTTP REST API with 4 endpoints (GET/POST/DELETE/LIST). Sync at
discrete boundaries (init, checkpoint, complete) — not continuous. Upload/download
session as a tarball or zip. Auth via API key in config.

**GitBackend** (legacy opt-in): writes to `wip/` in the git working tree. Preserves
current behavior for users who want it. Selected via koto config.

### Sync model for cloud

Discrete sync at state transitions (init, transition, evidence submit, complete) —
not on every file write. This matches the state machine's natural pace: you don't
need to sync mid-research-agent, only when the workflow advances.

Conflict detection: compare session version (monotonic counter or timestamp) on
sync. If remote is newer, report to user — don't auto-resolve. Workflows are
inherently single-writer; conflicts indicate a mistake, not a normal condition.

### Config-driven selection

```toml
# ~/.koto/config.toml
[session]
backend = "local"  # local | cloud | git

[session.cloud]
endpoint = "https://api.koto.dev/sessions"
api_key_env = "KOTO_API_KEY"

[session.git]
path = "wip"  # relative to repo root
```

CLI override: `koto init --session-backend git` for one-off use.

## Implications

The trait-based approach with bundle semantics keeps the backend interface simple.
Three backends cover all use cases: local for solo dev, cloud for transferability,
git for backward compatibility.

Discrete sync avoids the complexity of real-time synchronization while providing
the transferability the user asked for.
