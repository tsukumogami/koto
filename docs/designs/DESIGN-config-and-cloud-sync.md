---
status: Proposed
upstream: docs/prds/PRD-session-persistence-storage.md
problem: |
  koto sessions are machine-local. A workflow started on one machine can't be
  resumed on another without manually copying ~/.koto/sessions/. Adding cloud
  sync requires a config system for backend selection, endpoints, and credentials.
  Neither exists today.
decision: |
  (to be determined through design process)
rationale: |
  (to be determined through design process)
---

# DESIGN: Config system and cloud sync

## Status

Proposed

## Context and problem statement

koto's `LocalBackend` is the only storage backend. It's hardcoded in `build_backend()`
with no way to select an alternative. Sessions live at `~/.koto/sessions/<repo-id>/<name>/`
on one machine. To resume a workflow elsewhere, you'd need to copy that directory
manually.

The PRD (R8, R9, R11) specifies implicit cloud sync via S3-compatible storage, with
a config system for backend selection and credentials. These are Features 2 and 4 in
the session persistence roadmap. They're designed together because the config system's
hardest consumer is cloud sync (credentials, env var overrides, security constraints),
and cloud sync has no value without config to enable it.

Feature 1 (local storage + content ownership) shipped in PR #84. The `SessionBackend`
and `ContextStore` traits are implemented. `CloudBackend` needs to implement both.

## Decision drivers

- Cloud sync must be invisible to agents — zero new commands, zero token cost
- Credentials must never live in project config (committed to git = supply chain risk)
- S3 dependency (aws-sdk-s3 + tokio) must be behind a feature flag — default builds stay light
- Config system is general-purpose, not session-specific — other koto settings may use it
- Conflict detection must handle the "two machines advanced the same workflow" case
- Local backend must remain the zero-config default — cloud is opt-in
