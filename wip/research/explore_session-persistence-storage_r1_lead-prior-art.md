# Lead: How do other tools handle workflow state outside git?

## Findings

### Terraform
- Pluggable backends (local, S3+DynamoDB, GCS, Consul, Terraform Cloud)
- Configured via `terraform init -backend-config`
- State locking via DynamoDB or equivalent prevents concurrent modification
- `terraform state push/pull` for manual transfers
- Pain point: state drift when someone modifies infra outside Terraform

### Pulumi
- Similar backend model (local, Pulumi Cloud, S3)
- Pulumi Cloud adds collaboration features (history, audit, RBAC)
- Native locking without separate infrastructure (unlike Terraform+DynamoDB)
- Stacks as the unit of state isolation

### GitHub Actions
- Artifacts API for workflow outputs (90-day retention)
- Cache API for build caching (keyed, scoped to branch)
- State tied to workflow run lifecycle — no cross-run state
- Pain point: no native way to share state between workflows

### Bazel / Nx
- Remote cache via HTTP content-addressed storage
- Cache key is a hash of inputs — no manual versioning
- Nx Cloud adds distribution and replay features
- Pain point: cache invalidation when inputs change subtly

### AI coding tools
- Claude Code: CLAUDE.md memory files, conversation compression, no external state
- Aider: git-based session state (committed to repo)
- Cursor: proprietary cloud sync for settings, no workflow state

## Implications

The dominant pattern is Terraform-style: config-driven backend selection, local as
default, cloud as opt-in with auth. Locking matters for concurrent access but koto's
single-CLI-process model makes it less urgent initially. The session-as-directory
model maps well to Terraform's state-file-per-workspace.

Key lesson: Terraform's backend migration (`terraform init -migrate-state`) is a
good model for transitioning from one backend to another.
