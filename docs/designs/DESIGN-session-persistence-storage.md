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
  placeholder -- to be filled during design phases
rationale: |
  placeholder -- to be filled during design phases
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
