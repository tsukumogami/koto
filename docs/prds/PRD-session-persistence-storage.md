---
status: Draft
problem: |
  koto stores workflow session state (engine state files, research artifacts, plans,
  decision reports) in a wip/ directory committed to git feature branches. This works
  for solo development but looks unprofessional to other developers, couples state
  location to the git working tree, and prevents transferring sessions between machines
  without pushing/pulling branches. Even after moving state files out of git (via
  session directories), agents still write workflow context directly to the filesystem,
  meaning koto can't validate content, enforce immutability, or audit what was written.
goals: |
  koto owns session state lifecycle, location, and content. Agents submit and retrieve
  workflow context through koto's CLI instead of direct filesystem access. koto can
  validate content format, enforce immutability, and audit writes. Sessions are
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

Four problems with this approach:

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

**koto can't validate or control content.** Even when session directories move state
files out of git, agents still read and write workflow context files directly through
the filesystem. koto provides a directory path and agents write whatever they want
inside it. This means koto can't validate content format, enforce immutability,
track what was written, or support queries on workflow context. Gate evaluation
depends on shell commands checking the filesystem, not on koto's knowledge of what
content exists.

## Goals

1. koto provides a session management API that controls where workflow artifacts are
   stored, so agents don't hardcode storage paths
2. koto owns workflow context content through a CLI interface, so agents submit and
   retrieve context through koto instead of direct filesystem access
3. The default storage backend is local filesystem (outside the git working tree), so
   workflow artifacts don't pollute git history
4. A cloud storage backend enables session transfer between machines without git
5. A git backend preserves the current behavior as an opt-in mode
6. koto can validate content format and enforce immutability on submitted context

## User stories

**As a developer using koto on a team**, I want workflow artifacts to stay out of
git branches, so that my PRs only contain real code changes and my teammates don't
see temporary exploration/implementation state.

**As a developer switching machines**, I want to resume a koto workflow session on
my laptop that I started on my desktop, by running a single command that downloads
the session state from cloud storage.

**As a skill author**, I want to submit workflow context to koto by key and retrieve
it later by key, so that my skill works regardless of which storage backend the user
has configured and I don't need to manage file paths.

**As a skill orchestrator running parallel agents**, I want multiple agents to submit
context to the same session concurrently without advancing workflow state, so that
research agents can work independently and the orchestrator calls `koto next` when
they're all done.

**As a developer who prefers git-based workflows**, I want to opt into storing session
artifacts in the git working tree (the current behavior), so I can inspect them with
git tools and keep everything in one place.

**As a developer with many workflows**, I want to list which sessions exist and
clean up old ones, so my local disk doesn't fill with stale artifacts.

**As a template author**, I want gates to check whether koto has specific context
(not whether a file exists on disk), so that gates work regardless of storage backend.

## Requirements

### Functional

**R1. Session identity.** A session is 1:1 with a koto workflow. When `koto init`
creates a workflow, it also creates a session. The session ID is the workflow name
(the same string passed to `koto init <name>`). The session owns all context
produced during that workflow's execution: engine state, skill artifacts, research
output.

**R2. Content submission.** Agents submit workflow context to koto through the CLI,
keyed by a string identifier. Submission accepts content via stdin pipe or file
reference (`--from-file`). Each submission creates or replaces the content at that
key. koto stores the content in its session storage (opaque to the agent). Example:
`koto context add <name> --key scope.md < /tmp/scope.md` or
`koto context add <name> --key scope.md --from-file /tmp/scope.md`.

**R3. Content retrieval.** Agents retrieve workflow context from koto through the
CLI. `koto context get <name> --key scope.md` writes the content to stdout.
`koto context get <name> --key scope.md --to-file /tmp/scope.md` writes directly
to a file (useful for large content that agents then Read with offset/limit).
`koto context list <name>` lists all keys in the session.

**R4. Content existence check.** `koto context exists <name> --key scope.md`
returns exit code 0 if the key exists, non-zero if it doesn't. This is used by
both gate evaluation and resume logic.

**R5. Multi-agent concurrent submission.** Multiple agents can submit context to
the same session concurrently, as long as they write to different keys. koto
uses per-key locking to prevent corruption. Submission does not advance workflow
state. The orchestrator advances state with `koto next` after agents have
submitted their context.

**R6. Content-aware gate evaluation.** Templates can define gates that check
koto's content store instead of the filesystem. Built-in gate types (`exists`,
`content-match`) handle common checks natively. Shell gates remain available as
a fallback for complex logic, with `koto context exists` callable from shell
gates. Example gate: `koto-context-exists: scope.md` or shell fallback:
`koto context exists $SESSION --key scope.md`.

**R7. Local filesystem backend (default).** Sessions are stored in
`~/.koto/sessions/<session-id>/`. No git commits, no branch pollution. This is
the default with zero configuration. The internal storage format is opaque to
agents.

**R8. Cloud storage backend (S3-compatible) with implicit sync.** Sessions can be
synced to any S3-compatible object store (AWS S3, Cloudflare R2, MinIO, etc.) using
standard S3 credentials. Sync is invisible -- built into existing koto commands, not
exposed as separate push/pull operations. Skills and agents don't know cloud sync
exists; it adds zero token cost.

On every state-mutating command (`koto init`, `koto next`,
`koto context add`), koto:
1. Checks the remote version before proceeding. If the remote is newer (another
   machine advanced the workflow), downloads the remote state first.
2. Performs the requested operation locally.
3. Uploads the updated session to the remote.

Cloud sync covers both workflow state and submitted context. Between sync points,
the local copy is the working copy.

**R9. Conflict detection.** If both local and remote have diverged (local version
is N, remote version is M, neither is an ancestor of the other), koto returns an
error from the command that detected the conflict: "session conflict: local version
N, remote version M." The agent or user resolves via `koto session resolve --keep
local|remote`. This should be rare -- it only happens if two machines advance the
same workflow without syncing.

**R10. Git working tree backend (opt-in).** Sessions are stored in the git working
tree at a configurable path (default: `wip/`). Selected via configuration.
Intended for users who want artifacts committed to branches. When using the git
backend, context operations (`add`, `get`, `exists`, `list`) read and write files
directly in the configured directory, so the agent experience is the same.

**R11. Backend configuration via `koto config`.** Configuration uses a `koto config`
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

**R12. Session lifecycle commands.**

| Command | Behavior |
|---------|----------|
| `koto session dir <name>` | Print the session directory path (for backward compatibility during migration) |
| `koto session list` | List all local sessions with workflow name and last-modified time |
| `koto session cleanup <name>` | Remove all artifacts for a session (local and cloud) |
| `koto session resolve --keep local\|remote` | Resolve a version conflict |

No push/pull commands. Sync is implicit in existing koto commands (R8). The resolve
command is the only sync-related operation a user would ever run, and only in the
rare conflict case.

**R13. Automatic cleanup on workflow completion.** When a workflow reaches a terminal
state, koto removes the session's content and local storage. Cloud artifacts are also
removed unless the user has configured retention. A `--no-cleanup` flag preserves
the session for debugging.

**R14. Template variable substitution.** Templates can use `{{SESSION_DIR}}` in gate
commands and directives. koto substitutes the session path at runtime. For
content-aware gates (R6), templates can reference context keys directly without
filesystem paths.

### Non-functional

**R15. Token efficiency.** Content submission and retrieval support both stdin/stdout
piping and direct file flags (`--from-file`, `--to-file`). Large context (research
files can be 20KB+) flows through pipes or file references without consuming agent
conversation tokens. For retrieval, agents can use `--to-file` to write content
directly to a temp file, then Read it with offset/limit for targeted access. This
preserves the surgical-read optimizations agent tools provide.

**R16. No external dependencies for local backend.** The local filesystem backend
works with zero configuration and no external services. A fresh koto install works
immediately.

**R17. Cloud sync resilience.** Cloud sync failures log a warning but don't block
local workflow execution. The local copy is always the source of truth. koto retries
the upload on the next state-mutating command automatically.

## Acceptance criteria

- [ ] `koto init <name>` creates a session alongside the workflow state
- [ ] `koto context add <name> --key X` accepts content via stdin or `--from-file` and stores it
- [ ] `koto context get <name> --key X` outputs stored content to stdout or to `--to-file`
- [ ] `koto context exists <name> --key X` returns exit code 0 if key exists, non-zero otherwise
- [ ] `koto context list <name>` lists all context keys in the session
- [ ] Multiple agents can `koto context add` to different keys concurrently without corruption
- [ ] `koto context add` does not advance workflow state
- [ ] Content-aware gates (`koto-context-exists: X`) evaluate against koto's content store
- [ ] Shell gates can call `koto context exists` as a fallback
- [ ] Default backend stores context in `~/.koto/sessions/<name>/` (opaque to agents)
- [ ] Cloud backend syncs both state and context to S3-compatible store on state-mutating commands
- [ ] `koto context add` triggers cloud sync when cloud backend is configured
- [ ] On a new machine, `koto next` automatically downloads remote session (state + context)
- [ ] Diverged local and remote versions produce a conflict error (not silent overwrite)
- [ ] Git backend stores context as files in the git working tree at the configured path
- [ ] `koto config get/set` reads and writes configuration values
- [ ] Backend selection follows precedence: project config > user config > default
- [ ] `koto session list` shows all local sessions with names and timestamps
- [ ] `koto session cleanup <name>` removes local and cloud artifacts
- [ ] Workflow completion triggers automatic session cleanup (content + state)
- [ ] `--no-cleanup` flag preserves session on workflow completion
- [ ] Cloud sync failure logs a warning and doesn't block the workflow
- [ ] `koto session resolve --keep local|remote` resolves version conflicts
- [ ] Templates using `{{SESSION_DIR}}` resolve to the session path
- [ ] Content-aware gates work with all backends (local, cloud, git)

## Out of scope

- **Partial patches or structured updates.** Context submission is replace-only.
  Agents that need to accumulate content read the current value, modify it, and
  submit the replacement. Structured append or field-level update operations are
  future work.
- **State file access by agents.** Agents don't read the JSONL state file directly.
  Anything agents need about workflow state comes through koto's CLI (`koto next`,
  `koto status`, `koto query`).
- **Ad-hoc context injection by users mid-workflow.** Users can't add context outside
  of a koto command. Future work may allow this for interactive debugging.
- **Multi-user concurrent access.** Sessions are single-writer (one agent or
  orchestrator at a time per key). Concurrent access from multiple users on the same
  session is not supported.
- **Real-time sync.** Sync happens at discrete boundaries, not continuously.
- **Encryption at rest.** Session artifacts aren't encrypted locally. Cloud transport
  uses HTTPS.
- **Skill migration.** Updating existing skills to use the new context API is
  separate work. This PRD covers the koto capability; skills adopt it incrementally.
- **S3 SDK vs minimal client.** Whether to use a full S3 SDK (like `aws-sdk-s3`) or
  implement minimal HTTP calls is a design decision.

## Known limitations

- **Token cost for large context.** Reading context through `koto context get`
  and piping to a file adds a step compared to direct file reads. For large
  artifacts, agents pipe to a temp file and then use Read with offset/limit. This
  preserves the targeted-access optimizations agent tools provide, at the cost of
  one extra pipe operation.
- **No partial reads.** `koto context get` returns the full content. Agents that
  need only a section must read the full content and parse client-side. Structured
  queries on context sections are future work.
- **Local sessions aren't shared.** Without cloud sync configured, sessions are
  machine-local. Sharing requires explicit opt-in via cloud backend configuration.
- **Cloud sync latency.** Syncing a session (state + context, up to ~20 files,
  ~200KB total) adds time at state transitions and context submissions. Acceptable
  for the save-at-boundary model but would be prohibitive for per-keystroke sync.
- **Git backend loses CI cleanup enforcement.** The current CI check that wip/ is
  empty before merge doesn't apply when artifacts are stored outside git. Users on
  the local backend rely on koto's automatic session cleanup instead.
- **Replace-only semantics for accumulation.** Skills that accumulate context
  across rounds (e.g., findings.md, decisions.md) must read-modify-replace through
  koto's CLI. This works because orchestrators serialize phase transitions, but it
  means koto doesn't track content history or support native append operations.

## Decisions and trade-offs

**koto owns location AND content.** Investigated two models: (1) koto provides a
directory path and agents write files directly (location-only ownership), and
(2) agents submit content through koto's CLI (location + content ownership). Chose
option 2 because it enables content validation, immutability enforcement, audit
trails, and backend-agnostic access. The trade-off is one extra pipe operation for
large artifacts, which is acceptable given the control it provides.

**Replace-only for MVP.** Investigated append and field-level update operations for
workflow context that accumulates across rounds (~40% of current wip/ artifacts).
Chose replace-only because orchestrators serialize phase transitions, making
agent-driven read-modify-replace safe. Append and structured updates are future
optimizations, not blocked by replace-only semantics.

**Content-aware gates with shell fallback.** Investigated three gate models:
(1) all gates become koto-internal operations, (2) shell gates call koto CLI, and
(3) hybrid with built-in gate types plus shell fallback. Chose hybrid because it
offers a non-breaking upgrade path -- existing shell gates keep working while new
templates use built-in types.

**Shared session spanning skill pipeline.** Investigated session-per-skill vs.
shared session for skill-to-skill handoffs (explore -> design -> plan). Chose
shared session because skills already share the same wip/ directory today, and
a shared session is the simplest model. Session chaining or inheritance is future
work for clean lifecycle separation.

**Implicit sync, no push/pull.** Cloud sync is built into existing koto commands
rather than exposed as separate operations. This means skills and agents never call
sync commands -- zero token cost for cloud support. Sync happens at state transition
and context submission boundaries. Context is included in cloud sync scope.

**Session = workflow.** A session is 1:1 with a workflow, identified by the workflow
name. This is the simplest model and matches the existing convention where `wip/`
artifacts are scoped to a single workflow. If multi-session workflows are ever needed,
the session ID can be extended without breaking the API.

**No backward compatibility constraint.** There are no external users of koto today.
The wip/ model can be replaced cleanly rather than requiring indefinite coexistence.
The git backend preserves the option for users who want the old behavior.
