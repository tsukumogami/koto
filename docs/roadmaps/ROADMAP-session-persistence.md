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
**Needs:** `needs-design` — TOML schema, precedence rules, CLI command design
**Dependencies:** None
**Status:** Not started

Add `koto config get/set` with TOML files at user (`~/.koto/config.toml`) and project
(`.koto/config.toml`) levels. Precedence: project > user > default. `--project` flag
for team-shared config. Env var overrides for credentials. This is useful beyond
sessions — other koto settings can use it. But sessions need it for backend selection.

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
**Needs:** `needs-design` — S3 protocol, implicit sync, version counters, conflict detection
**Dependencies:** Feature 1, Feature 2
**Status:** Not started

Add `CloudBackend` behind a `cloud` feature flag (avoids tokio/aws-sdk in default
builds). Implicit sync built into state-mutating commands and context submissions:
check remote version before operating, upload after. Cloud sync covers both workflow
state and submitted context. Version counter detects conflicts.
`koto session resolve --keep local|remote` for the rare divergence case. S3
credentials from user config or env vars (not project config — supply chain risk).

## Sequencing rationale

Feature 1 (local storage + content ownership) is the foundation and is complete.
It established the `SessionBackend` and `ContextStore` traits, the content CLI,
content-aware gates, and the local filesystem backend.

Feature 2 (config) is the next priority. Features 3 and 4 both need it for backend
selection. It has no dependency on Feature 1 beyond the traits already shipped.

Feature 3 (git backend) depends on the trait (Feature 1) and config (Feature 2).
Lower priority since local is the default, but it enables users who want session
artifacts visible in git.

Feature 4 (cloud sync) is the most complex feature and depends on everything else.
Cloud sync covers both state and context. It should ship last. The S3 dependency
(aws-sdk-s3 + tokio) is behind a feature flag so it doesn't affect users who don't
need cloud.

## Progress

- Feature 1: Done (PR #84 — Phase A session storage + Phase B content ownership)
- Features 2-4: Not started
