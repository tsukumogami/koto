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
my laptop that I started on my desktop, by running a single command that downloads
the session state from cloud storage.

**As a skill author**, I want to ask koto "where should I write this artifact?"
instead of hardcoding `wip/` paths, so that my skill works regardless of which
storage backend the user has configured.

**As a developer who prefers git-based workflows**, I want to opt into storing session
artifacts in the git working tree (the current behavior), so I can inspect them with
git tools and keep everything in one place.

**As a developer with many workflows**, I want to list which sessions exist and
clean up old ones, so my local disk doesn't fill with stale artifacts.

## Requirements

### Functional

**R1. Session identity.** A session is 1:1 with a koto workflow. When `koto init`
creates a workflow, it also creates a session. The session ID is the workflow name
(the same string passed to `koto init <name>`). The session owns all artifacts
produced during that workflow's execution: engine state, skill artifacts, research
output.

**R2. Session directory resolution.** `koto session dir <name>` returns the
filesystem path to the session's artifact directory. Agents use this path with
their normal file tools (Read/Edit/Write). The directory is created automatically
if it doesn't exist. The path varies by backend but is always a local filesystem
directory that supports standard file operations.

**R3. Session directory structure.** The session directory has a flat layout with
one subdirectory for research artifacts, matching the current wip/ convention:

```
<session-dir>/
  <artifact-name>.md        (scope, findings, plans, summaries)
  <artifact-name>.json      (state files, manifests)
  research/
    <artifact-name>.md      (agent research output)
```

**R4. Local filesystem backend (default).** Sessions are stored in
`~/.koto/sessions/<session-id>/`. No git commits, no branch pollution. This is
the default with zero configuration.

**R5. Cloud storage backend (S3-compatible) with implicit sync.** Sessions can be
synced to any S3-compatible object store (AWS S3, Cloudflare R2, MinIO, etc.) using
standard S3 credentials. Sync is invisible — built into existing koto commands, not
exposed as separate push/pull operations. Skills and agents don't know cloud sync
exists; it adds zero token cost.

On every state-mutating command (`koto init`, `koto transition`, `koto next
--with-data`), koto:
1. Checks the remote version before proceeding. If the remote is newer (another
   machine advanced the workflow), downloads the remote state first.
2. Performs the requested operation locally.
3. Uploads the updated session to the remote.

Between sync points, the local copy is the working copy.

**R6. Conflict detection.** If both local and remote have diverged (local version
is N, remote version is M, neither is an ancestor of the other), koto returns an
error from the command that detected the conflict: "session conflict: local version
N, remote version M." The agent or user resolves via `koto session resolve --keep
local|remote`. This should be rare — it only happens if two machines advance the
same workflow without syncing.

**R7. Git working tree backend (opt-in).** Sessions are stored in the git working
tree at a configurable path (default: `wip/`). Selected via configuration.
Intended for users who want artifacts committed to branches.

**R8. Backend configuration via `koto config`.** Configuration uses a `koto config`
subcommand following the git/tsuku pattern:

```
koto config get session.backend
koto config set session.backend cloud
koto config set session.cloud.endpoint https://s3.us-east-1.amazonaws.com
koto config set session.cloud.bucket my-koto-sessions
```

Configuration precedence (highest to lowest):

1. Project config: `.koto/config.toml` in the repo root
2. User config: `~/.koto/config.toml`
3. Default: `local`

`koto config set` writes to user config by default. `koto config set --project`
writes to project config (committed to git, shared with team).

Cloud backend config keys: `session.cloud.endpoint`, `session.cloud.bucket`,
`session.cloud.region` (optional). Credentials via `session.cloud.access_key` /
`session.cloud.secret_key` in config, or the standard `AWS_ACCESS_KEY_ID` /
`AWS_SECRET_ACCESS_KEY` environment variables (env vars take precedence over
config file credentials to avoid committing secrets).

**R9. Session lifecycle commands.**

| Command | Behavior |
|---------|----------|
| `koto session dir <name>` | Print the session directory path |
| `koto session list` | List all local sessions with workflow name and last-modified time |
| `koto session cleanup <name>` | Remove all artifacts for a session (local and cloud) |
| `koto session resolve --keep local\|remote` | Resolve a version conflict |

No push/pull commands. Sync is implicit in existing koto commands (R5). The resolve
command is the only sync-related operation a user would ever run, and only in the
rare conflict case.

**R10. Automatic cleanup on workflow completion.** When a workflow reaches a terminal
state, koto removes the local session directory. Cloud artifacts are also removed
unless the user has configured retention.

**R11. Agent file tool compatibility.** Session artifact paths returned by
`koto session dir` are standard filesystem paths. Agents can use Read (with
offset/limit), Edit (targeted string replacement), and Write tools directly on
these paths. koto does not proxy file I/O.

**R12. Template wip/ references.** Templates that reference `wip/` in gate commands
or directives must work with the session directory instead. `koto session dir` output
(or an equivalent mechanism like an environment variable set by koto) replaces
hardcoded `wip/` references. This is a koto-engine concern, not a skill concern.

### Non-functional

**R13. Token efficiency.** Agents never transmit file content through koto CLI
commands to read or write session artifacts. `koto session dir` returns a path;
all file I/O uses agent tools directly on that path. This preserves the offset/limit
and targeted-edit optimizations that agent tools provide.

**R14. No external dependencies for local backend.** The local filesystem backend
works with zero configuration and no external services. A fresh koto install works
immediately.

**R15. Cloud sync resilience.** Cloud sync failures log a warning but don't block
local workflow execution. The local copy is always the source of truth. koto retries
the upload on the next state-mutating command automatically.

## Acceptance criteria

- [ ] `koto init <name>` creates a session directory alongside the workflow state
- [ ] `koto session dir <name>` returns the correct path for the configured backend
- [ ] Default backend stores artifacts in `~/.koto/sessions/<name>/`
- [ ] Session directory has flat layout with `research/` subdirectory
- [ ] Cloud backend syncs session artifacts to an S3-compatible store on state-mutating commands
- [ ] On a new machine, `koto next` automatically downloads the remote session before proceeding
- [ ] Diverged local and remote versions produce a conflict error (not silent overwrite)
- [ ] Git backend stores artifacts in the git working tree at the configured path
- [ ] `koto config get/set` reads and writes configuration values
- [ ] Backend selection follows precedence: project config > user config > default
- [ ] `koto session list` shows all local sessions with names and timestamps
- [ ] `koto session cleanup <name>` removes local and cloud artifacts
- [ ] Workflow completion triggers automatic session cleanup
- [ ] Agents can Read/Edit/Write files in the session directory using standard paths
- [ ] Cloud sync failure logs a warning and doesn't block the workflow
- [ ] `koto session resolve --keep local|remote` resolves version conflicts
- [ ] Cloud sync adds zero agent-visible commands (invisible to skills)
- [ ] Templates using `{{SESSION_DIR}}` or equivalent resolve to the session path

## Out of scope

- **Specific S3 provider recommendation.** The PRD specifies S3-compatible; which
  provider to use (AWS S3, Cloudflare R2, MinIO) is the user's choice via config.
- **Multi-user concurrent access.** Sessions are single-writer. Concurrent editing
  by multiple agents or users on the same session is not supported.
- **Real-time sync.** Sync happens at discrete boundaries, not continuously.
- **Encryption at rest.** Session artifacts aren't encrypted locally. Cloud transport
  uses HTTPS.
- **Skill migration.** Updating existing skills to use the new session API is
  separate work. This PRD covers the koto capability; skills adopt it incrementally.
- **S3 SDK vs minimal client.** Whether to use a full S3 SDK (like `aws-sdk-s3`) or
  implement minimal HTTP calls is a design decision.

## Known limitations

- **Local sessions aren't shared.** Without cloud sync configured, sessions are
  machine-local. Sharing requires explicit opt-in via cloud backend configuration.
- **Cloud sync latency.** Syncing a session directory (up to ~20 files, ~200KB total)
  adds time at state transitions. Acceptable for the save-at-transition model but
  would be prohibitive for per-write sync.
- **Git backend loses CI cleanup enforcement.** The current CI check that wip/ is
  empty before merge doesn't apply when artifacts are stored outside git. Users on
  the local backend rely on koto's automatic session cleanup instead.

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

**Implicit sync, no push/pull.** Cloud sync is built into existing koto commands
rather than exposed as separate operations. This means skills and agents never call
sync commands — zero token cost for cloud support. Sync happens at state transition
boundaries as a unit (bundle-level), not per-file. Conflict detection uses a
monotonic version counter. This model is closer to Terraform Cloud (state is always
remote, local is a cache) than to git (explicit push/pull).

**Session = workflow.** A session is 1:1 with a workflow, identified by the workflow
name. This is the simplest model and matches the existing convention where `wip/`
artifacts are scoped to a single workflow. If multi-session workflows are ever needed,
the session ID can be extended without breaking the API.
