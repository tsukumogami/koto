---
status: Draft
problem: |
  koto stores workflow session state (engine state files, research artifacts, plans,
  decision reports) in a wip/ directory committed to git feature branches. This works
  for solo development but looks unprofessional to other developers, couples state
  location to the git working tree, and prevents transferring sessions between machines
  without pushing/pulling branches. Agents write directly to wip/ paths, creating a
  tight coupling between skill code and storage location that makes it hard to change
  where state lives.
goals: |
  koto owns session state lifecycle and location. Agents interact with session
  artifacts efficiently without knowing where they're stored. Sessions are
  transferable between machines via cloud storage. The default storage is local
  filesystem (outside git), with cloud and git as configurable alternatives.
---

# PRD: Session persistence storage

## Status

Draft

## Problem statement

koto's workflow engine and the skills that run on it (shirabe, tsukumogami) produce
temporary artifacts during workflow execution: engine state files, exploration scope
documents, research outputs, implementation plans, decision reports, test plans, and
review results. Today these all live in a `wip/` directory in the git working tree,
committed to feature branches and cleaned up before merge.

Three problems with this approach:

**It looks unprofessional.** Other developers see `wip/` artifacts in branch diffs
and PR file lists. Temporary workflow state mixed with real code changes is confusing
and gives the impression of a messy development process.

**Sessions aren't transferable.** To resume a workflow on a different machine, you
have to push the branch with its `wip/` artifacts and pull it elsewhere. There's no
way to sync session state independently of git.

**Agents own the storage location.** Skills hardcode `wip/` paths (~150 file
references across shirabe and tsukumogami plugins). koto doesn't control where
artifacts live. Changing the storage location means updating every skill that writes
to `wip/`.

## Goals

1. koto provides a session management API that controls where workflow artifacts are
   stored, so agents don't hardcode storage paths
2. The default storage backend is local filesystem (outside the git working tree), so
   workflow artifacts don't pollute git history
3. A cloud storage backend enables session transfer between machines without git
4. A git backend preserves the current behavior as an opt-in mode
5. Agents retain efficient file access (Read/Edit/Write tools with offset/limit and
   targeted replacement) for session artifacts

## User stories

**As a developer using koto on a team**, I want workflow artifacts to stay out of
git branches, so that my PRs only contain real code changes and my teammates don't
see temporary exploration/implementation state.

**As a developer switching machines**, I want to resume a koto workflow session on
my laptop that I started on my desktop, without having to push/pull a git branch
with temporary artifacts.

**As a skill author**, I want to ask koto "where should I write this artifact?"
instead of hardcoding `wip/` paths, so that my skill works regardless of which
storage backend the user has configured.

**As a developer who prefers git-based workflows**, I want to opt into storing session
artifacts in the git working tree (the current behavior), so I can inspect them with
git tools and keep everything in one place.

## Requirements

### Functional

**R1. Session directory resolution.** koto provides a command or interface that
returns a filesystem path for a given workflow session's artifact storage. Agents
use this path with their normal file tools (Read/Edit/Write).

**R2. Local filesystem backend (default).** Sessions are stored in a koto-managed
directory outside the git working tree (e.g., `~/.koto/sessions/<session-id>/`).
No git commits, no branch pollution. This is the default with zero configuration.

**R3. Cloud storage backend (S3-compatible).** Sessions can be synced to any
S3-compatible object store (AWS S3, Cloudflare R2, MinIO, etc.) using standard S3
credentials (access key, secret key, endpoint URL). Sync happens at session
boundaries (start, checkpoint, complete), not on every file write. This enables
session transfer between machines.

**R4. Git working tree backend (opt-in).** Sessions are stored in the git working
tree (preserving the current `wip/` behavior). Selected via configuration. Intended
for users who want artifacts committed to branches.

**R5. Backend configuration.** The storage backend is selected via a configuration
file (e.g., `~/.koto/config.toml` or project-level `.koto/config.toml`) or CLI flag.

**R6. Session lifecycle management.** koto tracks which sessions exist, their
associated workflows, and their artifacts. koto can list, inspect, and clean up
sessions.

**R7. Session cleanup.** koto can atomically remove all artifacts for a completed
session. No more manual `rm -rf wip/` or reliance on CI to enforce cleanup.

**R8. Agent file tool compatibility.** Agents must be able to use Read (with
offset/limit), Edit (targeted string replacement), and Write tools on session
artifact paths. The storage medium must support standard filesystem operations.

### Non-functional

**R9. Token efficiency.** The session management API must not degrade agent token
efficiency. Agents should not need to transmit full file content through CLI stdout
to read or write artifacts.

**R10. No external dependencies for local backend.** The local filesystem backend
must work with zero configuration and no external services. A fresh koto install
should work immediately.

**R11. Cloud sync resilience.** Cloud sync failures must not block local workflow
execution. The local copy is the source of truth; cloud is a sync target.

## Acceptance criteria

- [ ] `koto session` (or equivalent) provides a path to the session artifact directory
- [ ] Default backend stores artifacts in a local directory outside the git working tree
- [ ] Cloud backend syncs session artifacts to an S3-compatible object store
- [ ] Git backend stores artifacts in the git working tree (backward compatible with wip/)
- [ ] Backend is configurable via config file or CLI flag
- [ ] `koto session cleanup` removes all artifacts for a session
- [ ] Agents can use Read/Edit/Write file tools on paths returned by koto
- [ ] Cloud sync failure doesn't block local workflow execution
- [ ] Session state is resumable after interruption (from local storage)

## Out of scope

- **Specific S3 provider recommendation.** The PRD specifies S3-compatible; which
  provider to use (AWS S3, Cloudflare R2, MinIO) is the user's choice via config.
- **Multi-user concurrent access.** Sessions are single-writer. Concurrent editing
  by multiple agents or users on the same session is not supported.
- **Real-time sync.** Sync happens at discrete boundaries, not continuously.
- **Encryption at rest.** Session artifacts aren't encrypted locally. Cloud transport
  uses HTTPS.
- **Skill migration.** Updating existing skills to use the new API is separate work.
  This PRD covers the koto capability, not the skill updates.

## Open questions

- Should the session directory be flat (all artifacts in one directory) or structured
  (subdirectories for research, state, etc.)? The current wip/ model uses a flat
  directory with a `research/` subdirectory.
- What S3 operations are needed? Likely just PutObject, GetObject, DeleteObject,
  and ListObjectsV2. Should the design use a full S3 SDK or a minimal HTTP client?
- Should koto expose an environment variable (e.g., `KOTO_SESSION_DIR`) that agents
  can read, or require a CLI call (`koto session dir`)?

## Known limitations

- **Local sessions aren't shared.** Without cloud sync configured, sessions are
  machine-local. This is the intended default — sharing requires explicit opt-in.
- **Cloud sync latency.** Syncing a session directory (potentially 20+ files, up to
  ~200KB total) adds time at session boundaries. Acceptable for the save-at-checkpoint
  model but would be prohibitive for per-write sync.
- **Git backend loses cleanup enforcement.** The current CI check that wip/ is empty
  before merge doesn't apply when artifacts are stored outside git. Users on the
  local backend rely on koto's session cleanup instead.

## Decisions and trade-offs

**Files as the agent-state medium.** Investigated CLI stdout, UNIX sockets, MCP
servers, SQLite, and shared memory as alternatives. Files won because agent tools
(Read/Edit/Write) are optimized for filesystem access with offset/limit and targeted
edits. Routing all I/O through CLI commands would lose these optimizations and
increase token usage for large artifacts (research files can be 20KB+).

**Koto owns location, not content.** koto provides the directory path; agents write
whatever they want inside it. koto doesn't parse or validate artifact content — it
manages the container (create, track, clean up, sync). This keeps the API surface
small and avoids coupling koto to skill-specific file formats.

**No backward compatibility constraint.** There are no external users of koto today.
The wip/ model can be replaced cleanly rather than requiring indefinite coexistence.
The git backend preserves the option for users who want the old behavior.

**Bundle-level cloud sync.** Sync the session directory as a unit at state transition
boundaries (init, transition, complete), not per-file. This keeps the cloud integration
simple (upload/download a directory) and matches the state machine's natural pace.
