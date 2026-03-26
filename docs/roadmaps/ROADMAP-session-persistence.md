---
status: Active
theme: |
  Move koto's session state out of the git working tree and into a koto-managed
  storage layer with pluggable backends. The features build incrementally: local
  storage first (removes wip/ from git), then config system, then template
  integration, then git compatibility mode, and finally cloud sync. Each feature
  is independently useful.
scope: |
  Covers the full session persistence capability from PRD-session-persistence-storage.md:
  local storage, config system, engine-provided variables, git backend, and S3 cloud
  sync. Excludes skill migration (updating shirabe/tsukumogami to use session API)
  and specific cloud provider recommendations.
---

# ROADMAP: Session persistence

## Status

Active

## Theme

koto stores workflow session state (engine state, skill artifacts, research output)
in `wip/` committed to git branches. This is a solo-developer convention that doesn't
scale to teams or multi-machine workflows. This roadmap sequences the work to replace
`wip/` with koto-managed session storage, building from the simplest useful change
(local filesystem) through to cloud sync with S3.

Each feature is independently shippable. You get value from feature 1 alone (wip/
moves out of git). Later features add config, template integration, git compatibility,
and cloud transferability.

## Features

### Feature 1: Local session storage
**Needs:** `needs-design` — storage abstraction trait shape and CLI integration points
**Dependencies:** None
**Status:** Not started

Add `SessionBackend` trait and `LocalBackend` implementation. `koto init` creates a
session directory at `~/.koto/sessions/<name>/`. `koto session dir <name>` returns the
path. Refactor CLI commands to use backend-provided paths instead of hardcoded `wip/`.
Engine state files (JSONL) move into the session directory. `koto session list` and
`koto session cleanup` manage sessions. Automatic cleanup on workflow completion.

This is the minimum change that removes wip/ from git for new workflows.

### Feature 2: Config system
**Needs:** `needs-design` — TOML schema, precedence rules, CLI command design
**Dependencies:** None
**Status:** Not started

Add `koto config get/set` with TOML files at user (`~/.koto/config.toml`) and project
(`.koto/config.toml`) levels. Precedence: project > user > default. `--project` flag
for team-shared config. Env var overrides for credentials. This is useful beyond
sessions — other koto settings can use it. But sessions need it for backend selection.

### Feature 3: Engine-provided variables
**Needs:** `needs-design` — variable injection model, interaction with existing --var
**Dependencies:** Feature 1
**Status:** Not started

Add engine-provided variables (starting with `{{SESSION_DIR}}`) that are computed at
runtime rather than declared in templates or passed via `--var`. Templates use
`{{SESSION_DIR}}` in gate commands and directives instead of hardcoded `wip/`. Extends
the existing `Variables::substitute()` infrastructure. Rejects `--var` keys that
shadow engine variable names.

### Feature 4: Git backend
**Needs:** `needs-design` — how git backend maps to existing wip/ conventions
**Dependencies:** Feature 1, Feature 2
**Status:** Not started

Add `GitBackend` that stores session artifacts in the git working tree at a
configurable path (default: `wip/`). Selected via `koto config set session.backend
git`. This is the backward-compatibility mode for users who want the current behavior.
With features 1-2 in place, the git backend is a thin implementation of the
`SessionBackend` trait that points `session_dir()` at the working tree.

### Feature 5: Cloud sync (S3-compatible)
**Needs:** `needs-design` — S3 protocol, implicit sync, version counters, conflict detection
**Dependencies:** Feature 1, Feature 2
**Status:** Not started

Add `CloudBackend` behind a `cloud` feature flag (avoids tokio/aws-sdk in default
builds). Implicit sync built into state-mutating commands: check remote version
before operating, upload after. Version counter in `session.meta.json` detects
conflicts. `koto session resolve --keep local|remote` for the rare divergence case.
S3 credentials from user config or env vars (not project config — supply chain risk).

## Sequencing rationale

Feature 1 (local storage) is the foundation — every other feature depends on the
`SessionBackend` trait it introduces. It's also the highest immediate value: removing
wip/ from git is the primary user complaint.

Feature 2 (config) has no technical dependency on feature 1, but features 4 and 5
need it for backend selection. It can be built in parallel with feature 1.

Feature 3 (engine variables) depends on feature 1 because `{{SESSION_DIR}}` needs a
session directory to point at. It could ship with feature 1 or immediately after.

Feature 4 (git backend) depends on both the trait (feature 1) and config (feature 2)
for backend selection. It's lower priority since the default is local, but it unblocks
users who want the old behavior.

Feature 5 (cloud sync) is the most complex feature and depends on everything else.
It should ship last. The S3 dependency (aws-sdk-s3 + tokio) is behind a feature flag
so it doesn't affect users who don't need cloud.

**Parallel opportunities:** Features 1 and 2 can be built in parallel. Feature 3
can start as soon as feature 1 lands.

## Progress

All features not started. PRD is accepted at docs/prds/PRD-session-persistence-storage.md.
Design doc exists at docs/designs/DESIGN-session-persistence-storage.md but covers
the full scope — needs to be rescoped to feature 1 only.
