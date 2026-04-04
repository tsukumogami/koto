---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  koto's state machine is per-workflow with no awareness of other workflows.
  When a workflow needs to fan out over a collection of items -- each going
  through its own multi-state lifecycle -- the only option is an external
  orchestrator that manages per-item workflows and tracks the queue outside
  koto. This creates two sources of truth, forces reconciliation logic on
  every consumer, and means koto can't enforce ordering or dependency
  constraints across child workflows. The engine needs parent-child lineage,
  a convergence mechanism for waiting on children, and cross-hierarchy
  queries -- without taking on agent process management.
---

# DESIGN: Hierarchical multi-level workflows

## Status

Proposed

## Context and Problem Statement

koto is a workflow orchestration engine for AI coding agents that enforces
execution order through a state machine. Today each workflow is fully isolated:
its own state file, its own event log, its own evidence and decisions. When a
parent workflow needs to spawn and coordinate child workflows -- for example,
running a multi-phase implementation workflow for each issue in a plan -- the
parent agent must build an external orchestrator that duplicates what koto
already tracks.

The need spans multiple levels of nesting:

- A design workflow produces decisions, hands off to a planning workflow that
  decomposes into issues, then each issue runs through an implementation
  workflow. Today these are completely disconnected.
- A release workflow coordinates across multiple repos, each with its own plan,
  each plan containing multiple issues. Three levels.
- An exploration workflow fans out research agents, converges findings, then
  hands off to a design workflow. The handoff is a file on disk and a manual
  skill invocation.

The gate-transition contract (v0.6.0) established structured gate output that
feeds into transition routing. This foundation enables child workflow status to
be represented as gate data, reusing the existing routing and override
mechanisms rather than inventing new ones.

Issue: #127. Related: #105 (bounded iteration), #87 (workflow-scoped variables).

## Decision Drivers

- **koto is a contract layer, not an execution engine.** koto doesn't launch
  agents. The parent agent spawns children externally (Claude Agent tool,
  subprocesses, etc.) and hands them workflow names. koto tracks relationships
  and exposes state.
- **Minimal advance loop changes.** The advance loop has a clean seven-step
  pipeline. New primitives should plug into existing extension points (gates,
  evidence) rather than adding new steps or stop reasons.
- **Backward compatibility.** Existing workflows with no parent-child
  relationships must continue to work without changes. New fields should be
  optional and default-safe.
- **Both backends must work.** LocalBackend (filesystem) and CloudBackend (S3)
  share the SessionBackend trait. Changes to session storage must be viable for
  both without deep API rework.
- **Public repo, external contributors.** Design decisions need to be
  documented clearly enough that someone without organizational context can
  implement from them.

## Decisions Already Made

These choices were settled during exploration and should be treated as
constraints, not reopened.

1. **Gate-based fan-out over action-based or state-level declaration.**
   A `children-complete` gate type requires zero advance loop changes, reuses
   existing infrastructure (blocking_conditions, gates.* routing, overrides),
   and can be layered with declarative syntax later if needed. Action-based
   requires two new primitives (spawn + wait). State-level declaration is the
   most invasive change and introduces "stateful state" concepts.

2. **Header-only lineage over dual-event or directory nesting.**
   Adding `parent_workflow: Option<String>` to `StateFileHeader` requires
   minimal code changes, is backward-compatible without bumping schema_version,
   and satisfies primary query patterns since `list()` already reads all headers.
   Parent-side `ChildWorkflowSpawned` events deferred until crash-recovery
   requirements are concrete.

3. **Flat storage with metadata filtering over directory-based isolation.**
   Preserves the flat session model both backends depend on. Directory nesting
   would require reworking the entire SessionBackend trait (`create`, `exists`,
   `cleanup`, `list`, `session_dir`). Metadata-based filtering (header fields +
   CLI flags) achieves the same logical relationships.

4. **Naming convention (parent.child) as ergonomic default alongside metadata.**
   Dot-separated names are already valid per `validate_workflow_name()`.
   Convention provides zero-code-change isolation; metadata (`parent` header
   field) provides correctness guarantees. Both complement each other.

5. **Abandon as default parent close policy.**
   The parent agent manages child lifecycle. koto shouldn't force child
   termination when a parent completes. Aligned with Temporal's Parent Close
   Policy model (the only prior art that formalizes this).

6. **External child templates, no implicit state sharing.**
   Validated by all major workflow engine prior art (Temporal, Airflow, Argo,
   Prefect, Conductor, Step Functions). Children use their own template files.
   Parent-to-child data flows through explicit init-time parameters. No mid-
   execution state synchronization.
