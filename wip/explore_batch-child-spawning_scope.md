# Explore Scope: batch-child-spawning

## Visibility

Public

## Core Question

How should koto let a parent workflow declare a DAG of children -- possibly
growing at runtime -- and own their spawning, ordering, and completion
detection, so consumers stop writing spawn loops in SKILL.md prose? The
mechanism must handle dynamic task addition (a running child spawns siblings
or grandchildren), inter-task dependencies, and failure routing in a way that
composes with v0.7.0 hierarchical workflows.

## Context

Issue #129 blocks shirabe's adoption of koto for hierarchical templates
(tsukumogami/shirabe#67). Today, a parent uses `koto init --parent` + the
`children-complete` gate, but the consumer has to run the spawn loop in prose:
query children, compute next-ready from a dependency graph, spawn, repeat.
That's brittle beyond ~5 issues and forces every consumer to re-implement
scheduling.

The user has a concrete shape in mind: a skill-layer script converts a plan
document into a structured task list (name, template, variables, waits_on),
submits it to the parent via `--with-data`, and from that point koto owns
spawning, ordering, and completion. The skill shouldn't re-engage until the
parent reaches a terminal state.

Constraints from the scoping conversation:
- Inter-task dependency ordering is mandatory in v1 (GH-issue dependencies)
- Task lists grow at runtime: a running child must be able to spawn siblings
  or grandchildren
- Failure routing tradeoffs are open; exploration must recommend a default
- Shirabe PR #67 is out of scope; it revisits after this ships
- v0.7.0 primitives (parent_workflow header, children-complete gate,
  hierarchical context reads) stay as the foundation

## In Scope

- Evidence shape for declaring a batch and appending to it mid-flight
- Inter-task dependency ordering inside koto (DAG scheduler)
- Dynamic task addition from a running child
- Failure-routing semantics with a recommended default + tradeoffs
- Resume and idempotency for partially-materialized batches
- Interaction with existing `children-complete` gate and `parent_workflow` field
- Implementation surface inside koto (action, gate, engine loop, CLI)
- How grandchildren relate to the originating batch (same vs nested)

## Out of Scope

- Cross-batch dependency edges (DAG edges crossing batch boundaries)
- Distributed execution across machines (local filesystem only)
- Replacing one-off `koto init --parent` — it remains the primitive
- Shirabe's work-on-plan.md template design (PR #67)
- A new DSL for task lists — reuse frontmatter and evidence mechanisms
- Cascade semantics beyond the advisory-only stance from #127

## Research Leads

1. **What's the concrete shape of the declarative task list and its materialization trigger?** (lead-evidence-shape)
   Define the schema for a batch entry (name, template, vars, waits_on) and
   where the template declares materialization: frontmatter field, state-level
   action, new gate type, or a dedicated action verb. Specify the CLI surface
   (`koto next <parent> --with-data tasks=@file.json`) and what the compiler
   validates at template load. Show 2-3 concrete template+evidence pairs
   covering single-issue, DAG-of-issues, and dynamically-grown batches.

2. **How does a running child add siblings or grandchildren to the batch mid-flight?** (lead-dynamic-additions)
   Investigate both readings of "sibling or grandchild": (A) append to the
   original batch with the same batch identity, (B) start a nested batch
   underneath the running child. For each: what CLI surface, what persistence,
   how the parent's children-complete gate interacts, how resume handles "add
   was requested but spawn didn't happen yet". Recommend one or justify
   supporting both.

3. **How should koto route failures in a DAG batch, and what should the default be?** (lead-failure-routing)
   Compare pause-on-failure, fail-fast, skip-dependents, continue-independent.
   For each: what the parent sees, how dependents are handled, what resume
   looks like, how it maps to the user's GH-issue use case. Recommend a default
   and state whether policy should be per-batch, per-task, or global. Cite
   prior art where relevant.

4. **How do other workflow engines handle declarative DAG spawning with dynamic additions and failures?** (lead-prior-art)
   Survey Temporal child workflows, Airflow dynamic task mapping, Argo
   Workflows DAG templates, GitHub Actions reusable workflows, Prefect flows.
   Focus on: data models for tasks, dependency declaration, mid-flight DAG
   mutation, failure routing, idempotency on restart. Extract patterns that
   fit koto's state-machine + file-based-persistence model; discard ones that
   assume a running daemon or centralized scheduler.

5. **Where does this plug into koto's code, and how does resume work?** (lead-koto-integration)
   Read src/engine/, src/cli/mod.rs, src/gate.rs, src/template/, and the
   existing children-complete implementation. Propose where materialization,
   scheduling, and resume logic live. Identify the idempotency key for
   "spawn task X" (child name? batch-id + index?). Specify what state must
   be persisted at the parent vs derivable from child state files on disk.
   Flag any v0.7.0 assumptions this would invalidate.
