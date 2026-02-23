# Friction Log: Multi-PR implement-doc for koto Installation

This document captures friction points, workarounds, and observations while implementing the koto installation design doc using a multi-PR approach instead of the standard single-PR `/implement-doc` workflow.

## Context

- Design doc: `docs/designs/DESIGN-koto-installation.md`
- Tracking branch: `docs/koto-installation` (PR #24)
- 4 issues (#25-#28) across a linear dependency chain with fan-out
- Each issue gets its own branch off `main`, its own PR, merged independently
- Tracking branch holds state file, friction log, and design doc status updates

## Why Multi-PR

The standard `/implement-doc` creates one `impl/` branch with all changes in a single PR. That doesn't work here because:

1. **Issue #26 (tag v0.1.0)** requires the release workflow (#25) to already be on `main` -- GitHub Actions only triggers workflows from the default branch
2. **Issue #28 (tsuku recipe)** goes to a different repository entirely
3. Issues #27 and #28 need real release assets from the v0.1.0 release to test against

This creates a hard constraint: implementation must be merged incrementally, not batched.

## Log

### Phase 0: Setup

**Observation: `workflow-tool state init` works for multi-PR with `--branch` and `--pr` flags**

Reused the existing `docs/koto-installation` branch and PR #24. The state file doesn't encode assumptions about single vs multi-PR -- it tracks issue status, not PR topology. This is the same finding as the tsuku CI consolidation friction log.

**Observation: Tracking branch already has explore artifacts**

The `docs/koto-installation` branch was created by `/explore`, so it has `wip/explore_summary.md` and `wip/research/` artifacts from the design phase. These will need cleanup before the tracking PR is finalized. Not a problem now, but adds to the bookkeeping at the end.

**Friction: QA agent wrote test plan to wrong directory**

The tester agent was spawned from the vision repo (the orchestrator's working directory) and wrote the test plan to `vision/wip/` instead of `koto/wip/`. The agent's cwd was the vision repo, not the koto repo. Had to manually copy the file to the correct location. The `/implement-doc` command should either pass the target repo's wip/ path explicitly in the agent prompt, or ensure the agent's cwd is set to the target repo.

**Observation: TW agent correctly identified minimal doc impact**

1 doc entry: README update with install command. All CI/infrastructure work correctly skipped. This is accurate -- the only user-facing documentation change is adding the `curl | sh` install command to the README.
