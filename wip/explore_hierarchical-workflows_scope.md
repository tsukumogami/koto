# Explore Scope: hierarchical-workflows

## Visibility

Public

## Core Question

How should koto model hierarchical workflows where a parent workflow spawns children via `koto init --parent`, children run independently through their own templates, and the parent can query child state/evidence to inform its own transitions? The parent agent manages child agent lifecycle; koto owns the data contract and state relationships.

## Context

Issue #127 describes the need. Today koto's state machine is per-workflow with no awareness of other workflows. When a workflow needs to iterate over a collection (e.g., issues in an implementation plan), consumers build external orchestrators that duplicate state tracking. The gate-transition contract (v0.6.0) is stable and provides structured gate output that child outcomes could build on.

Related issues: #105 (bounded iteration -- children are a superset of repeated state visits), #87 (workflow-scoped variables -- parent-to-child context passing may build on variable promotion).

The user clarified: koto doesn't launch agents. The parent agent uses external mechanisms (Claude Agent tool, subprocesses) to spawn child agents and hands them child workflow names. Fan-out looks gate-like from the parent's perspective. Cross-hierarchy queries primarily flow parent-to-child (parent reads child evidence/decisions), though child-to-parent and sibling queries are plausible future extensions.

## In Scope

- Parent-child lineage model (`koto init --parent`)
- Fan-out/convergence semantics (how parent states wait for children)
- Cross-hierarchy queries (parent reading child state/evidence/decisions)
- State file organization and event log changes for hierarchy
- Advance loop behavior with pending children
- Isolation boundaries (preventing cross-hierarchy pollution)

## Out of Scope

- Agent process management (caller's responsibility)
- New gate type implementations beyond child-status checking
- UI/visualization of hierarchies
- Sibling-to-sibling queries (future extension)
- Child-to-parent queries (acknowledged as plausible, no driving use case yet)

## Research Leads

1. **How do other workflow engines model parent-child workflow relationships?**
   Temporal, Airflow, Argo, Prefect all have sub-workflow concepts. Understanding the design space (inline vs. external children, result propagation, failure semantics) prevents reinventing badly.

2. **What's the right primitive for fan-out -- a new gate type, a new action type, or something else?**
   The user suggested fan-out looks like a gate. But it could also be a new `spawn` action, or a state-level `children` declaration. Each has different implications for the advance loop and the template schema.

3. **How should `koto init --parent` change the state file and event log?**
   The lineage link needs to be durable and discoverable. Options: header field pointing to parent, a new `ChildWorkflowSpawned` event on the parent's log, directory nesting, or some combination. This affects everything downstream.

4. **What query interface lets a parent read child workflow data without coupling?**
   The parent needs child outcomes, maybe child evidence. The interface design determines how much koto exposes vs. what the agent stitches together.

5. **How does the advance loop behave when a state's children are pending?**
   Today the loop stops on gates, evidence, integrations. Children add a fourth stop reason. How does this interact with existing gate evaluation -- are pending children a blocking condition, or a separate concept?

6. **What isolation model prevents cross-hierarchy pollution in `koto workflows` and queries?**
   Today all state files are peers. With hierarchies, `koto workflows` needs to distinguish roots from children, and queries within a hierarchy shouldn't surface unrelated workflows.
