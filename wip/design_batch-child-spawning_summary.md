# Design Summary: batch-child-spawning

## Input Context (Phase 0)

**Source:** /shirabe:explore handoff
**Problem:** Koto v0.7.0 provides hierarchical workflow primitives
(parent_workflow header, koto init --parent, children-complete gate) but
requires consumers to run the spawn loop themselves in SKILL.md prose.
For workflows with a known DAG of children (GH-issue dependencies,
multi-step plans), this is brittle and blocks shirabe's adoption of koto
for hierarchical templates (tsukumogami/shirabe#67). This design
specifies a declarative alternative where the parent submits a task
list as evidence and koto owns materialization, dependency-ordered
scheduling, completion detection, and failure routing.

**Constraints:**
- Must compose with v0.7.0 primitives unchanged (no regressions)
- Stateless CLI model (no daemon, no persistent cursor)
- Append-only state file semantics preserved (no header mutation)
- Primary model is flat batch with sibling-level `waits_on` dependencies
  (Reading A); nested batches (Reading B) remain as v0.7.0
- Dynamic additions required (running children can append tasks)
- Default failure policy is skip-dependents, per-batch configurable
- Child naming is deterministic `<parent>.<task>`
- Scheduler runs at CLI layer in `handle_next`, not inside the advance
  loop
- Storage: nothing new; derive from evidence events + on-disk children
- Backward compatibility for pre-batch templates and state files

**Open questions for design phase:**
- Atomic child-spawn window in `handle_init` (header+event atomicity)
- Forward-compat diagnosability when batch template runs on pre-batch
  koto binary (format_version bump vs `deny_unknown_fields`)
- Child-template path resolution across parent/child working directories
- Retry mechanics (`retry_failed` evidence action surface)
- Observability via `koto status` and `koto workflows --children`
- Exact name for the template hook (`batch` vs `materialize_children`
  vs `batch_spawn`)

## Current Status

**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-04-11

## Exploration Artifacts

- `wip/explore_batch-child-spawning_scope.md`
- `wip/explore_batch-child-spawning_findings.md`
- `wip/explore_batch-child-spawning_decisions.md`
- `wip/explore_batch-child-spawning_crystallize.md`
- `wip/research/explore_batch-child-spawning_r1_lead-evidence-shape.md`
- `wip/research/explore_batch-child-spawning_r1_lead-dynamic-additions.md`
- `wip/research/explore_batch-child-spawning_r1_lead-failure-routing.md`
- `wip/research/explore_batch-child-spawning_r1_lead-prior-art.md`
- `wip/research/explore_batch-child-spawning_r1_lead-koto-integration.md`
