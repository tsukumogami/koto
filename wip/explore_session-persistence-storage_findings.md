# Exploration Findings: session-persistence-storage

## Core Question

How should koto manage workflow session state with pluggable storage backends,
replacing the current wip/ model?

## Round 1

### Key Insights

- Files remain the best medium for agent-koto state exchange. Agent tools
  (Read/Edit/Write) are optimized for files with offset/limit and targeted edits.
  CLI stdout loses these optimizations. The question is where files live, not
  whether to use files. (exchange-mediums lead)
- "Koto owns it" means four separable things: lifecycle management, location
  abstraction, cleanup responsibility, and coordination tracking. Koto already
  does lifecycle for engine state. The new work is location abstraction and
  cleanup. (ownership-model lead)
- The wip/ surface is ~15 file patterns across 3 categories (issue workflow,
  implementation state, research artifacts), all 1-20KB markdown or JSON. Nothing
  requires special handling. (wip-surface-area lead)
- Terraform's config-driven backend model is the closest match: local default,
  cloud opt-in, init-based backend selection. (prior-art lead)
- Bundle-level sync at state transition boundaries (not per-file) keeps cloud
  integration simple. (backends-sync lead)
- 150+ files reference wip/ across shirabe/tsukumogami plugins. A compatibility
  layer (koto session dir returns backend-appropriate path) enables gradual
  migration without big-bang. (migration-path lead)

### Tensions

- Manifest ownership: should koto track session contents via manifest (enables
  coordination but adds API surface) or just provide a directory (simpler but
  no coordination)? Current model has no manifest — resume scans by filename.

### Gaps

- Template directives and gate commands that reference wip/ paths would break
  if the session directory changes. Addressable during design.
- CI wip/ cleanup check needs updating for non-git backends. Addressable during
  design.

### User Focus

Ready to decide on artifact type.

## Accumulated Understanding

Koto should add a session management layer that provides a directory path for
each workflow session. Agents write artifacts to this directory using their
normal file tools (preserving token efficiency). The directory location depends
on the configured backend: `~/.koto/sessions/<id>/` for local, `wip/` for git
(backward compatible), cloud-synced local directory for remote.

The API surface is small: `koto session dir` (or equivalent) returns the path.
`koto session cleanup` removes all artifacts. Sync to cloud happens at session
boundaries (init, transition, complete), not on every write. Backend selection
is config-driven (koto.toml or CLI flag), following Terraform's model.

Migration from wip/ is gradual: skills replace hardcoded `wip/` with the
koto-provided path. In git mode, the path IS `wip/`, so unmigrated skills
still work. The CI check adapts to validate cleanup based on the active backend.

## Decision: Crystallize
