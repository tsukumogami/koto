---
status: Active
theme: |
  Move koto's session state out of the git working tree and make koto the owner
  of both storage location and workflow context content. Features build
  incrementally: local storage with content ownership first (removes wip/ from
  git, agents interact through koto's CLI), then config system, git compatibility
  mode, and cloud sync. Each feature is independently useful.
scope: |
  Covers the full session persistence capability from PRD-session-persistence-storage.md:
  local storage with content ownership and runtime variable substitution, config
  system, git backend, and S3 cloud sync. Excludes specific cloud provider
  recommendations.
---

# ROADMAP: Session persistence

## Status

Active

## Theme

koto stores workflow session state (engine state, skill artifacts, research output)
in `wip/` committed to git branches. This is a solo-developer convention that doesn't
scale to teams or multi-machine workflows. Even with session directories outside git,
agents write workflow context directly to the filesystem, meaning koto can't validate
content, enforce immutability, or audit writes.

This roadmap sequences the work to replace `wip/` with koto-managed session storage
where koto owns both location and content, building from the simplest useful change
(local filesystem with content CLI) through config and cloud sync.

Each feature is independently shippable. You get value from Feature 1 alone (state
files and workflow context move out of git, agents interact through koto's CLI).
Later features add config, git compatibility, and cloud transferability.

## Features

### Feature 1: Local session storage with content ownership
**Dependencies:** None
**Status:** Done (PR #84)

Two phases of the same feature:

**Phase A (done):** `SessionBackend` trait and `LocalBackend` implementation. `koto init`
creates a session directory at `~/.koto/sessions/<repo-id>/<name>/`. CLI commands use
backend-provided paths. `koto session dir|list|cleanup`, automatic cleanup on workflow
completion, `{{SESSION_DIR}}` runtime variable substitution in gate commands and
directives. 235 tests.

**Phase B (done):** Make koto the owner of workflow context content. Agents submit
and retrieve context through koto's CLI (`koto context add`, `koto context get`,
`koto context exists`, `koto context list`) instead of reading/writing files directly
in the session directory. Content submission is decoupled from state advancement, so
multiple agents can submit context concurrently without calling `koto next`.
Content-aware gate types replace filesystem-based gates. Replace-only semantics for
MVP. Skills migrate from wip/ filesystem access to koto context CLI.

Design at `docs/designs/current/DESIGN-local-session-storage.md` (status: Current).

### Feature 2: Config system
**Dependencies:** None
**Status:** Done (PR #98)

`koto config get/set/unset/list` with TOML files at user (`~/.koto/config.toml`) and
project (`.koto/config.toml`) levels. Precedence: project > user > default. `--project`
flag for team-shared config. Credential allowlist prevents secrets in project config.
Env vars override config values.

Design at `docs/designs/current/DESIGN-config-and-cloud-sync.md` (status: Current).

### Feature 3: Git backend
**Needs:** `needs-design` — how git backend maps to context CLI operations
**Dependencies:** Feature 1, Feature 2
**Status:** Not started

Add `GitBackend` that stores session artifacts in the git working tree at a
configurable path (default: `wip/`). Selected via `koto config set session.backend
git`. This is the backward-compatibility mode for users who want the current behavior.
Context operations (`add`, `get`, `exists`, `list`) map to file reads/writes in the
configured directory.

### Feature 4: Cloud sync (S3-compatible)
**Dependencies:** Feature 1, Feature 2
**Status:** Done (PR #98, designed and built with Feature 2)

`CloudBackend` wraps `LocalBackend` and syncs per-key to S3 via `rust-s3` (sync,
no tokio). Behind a `cloud` cargo feature flag. Implicit sync on every mutating
command. Monotonic version counter with three-way conflict detection.
`koto session resolve --keep local|remote` for rare divergence. S3 credentials from
user config or env vars. Supports any S3-compatible provider (AWS, Cloudflare R2,
MinIO).

Design at `docs/designs/current/DESIGN-config-and-cloud-sync.md` (status: Current).
Setup guide at `docs/guides/cloud-sync-setup.md`.

## Sequencing rationale

Feature 1 (local storage + content ownership) was the foundation. Features 2 and 4
(config + cloud sync) were designed and built together because the config system's
hardest consumer was cloud credentials. Feature 3 (git backend) is the only remaining
item.

Feature 3 (git backend) depends on the trait (Feature 1) and config (Feature 2), both
complete. It's a thin `GitBackend` implementation that maps context operations to file
reads/writes in the working tree.

## Progress

- Feature 1: Done (PR #84 — session storage + content ownership)
- Feature 2: Done (PR #98 — config system, designed with Feature 4)
- Feature 3: Not started (git backend)
- Feature 4: Done (PR #98 — cloud sync, designed with Feature 2)
