# Explore Scope: koto-implement-workflow-gaps

## Core Question

koto has an intentionally minimal CLI surface (`koto next`, `koto init`, `koto rewind`,
`koto cancel`). The `/implement` workflow currently runs on the workflow-tool in
`private/tools`. We want to identify what integrations koto needs to support `/implement`
running on top of its existing surface — not by adding subcommands, but by wiring in
external capabilities (like GitHub CI status, PR creation) through koto's integration
system.

## Context

koto's surface is intentionally small. The integration field in templates (`integration:
<name>`) routes states to named external handlers. koto invokes those handlers and
records their output as `integration_invoked` events. The `/implement` workflow is a
multi-issue, multi-session orchestration that coordinates a dependency graph of GitHub
issues through coding, PR creation, CI checking, and review. The goal is to express
that workflow as a koto template backed by integrations rather than a bespoke
workflow-tool implementation.

## In Scope

- The `/implement` command's phase structure and what it needs at each state
- koto's existing integration mechanism and what it can already drive
- Shape mismatches between koto's integration model and /implement's patterns
- The `DESIGN-workflow-tool-oss.md` strategic design's extraction path
- The wip/ state file schema for /implement and whether it fits koto's event log model

## Out of Scope

- `/implement` multi-issue orchestration (deferred — requires orchestrator layer above koto)
- Cross-agent delegation (tracked as koto issue #41)
- Adding new koto subcommands (not the direction)

## Round 1 Decisions

- `/implement` requires a multi-workflow orchestrator layer above koto — not the first target
- Integration runner config system is deferred; phase 1 may not need it
- Focus shifted to merged work-on/just-do-it as the first koto-backed shirabe skill
- shirabe already has /work-on; /just-do-it could merge into it cleanly

## Round 2 Research Leads

1. **What is the actual overlap between work-on and just-do-it, and what's the merge cost?**
   Read both skill structures in the tools repo. Map where they share phases vs. diverge.
   Identify whether the difference is structural or just a template parameter (e.g. optional issue).

2. **What would a koto template for the merged work-on/just-do-it skill look like?**
   Map the phases to koto states, identify evidence gates at each transition, figure out
   where branching happens (e.g. "does this need a design doc?"), and assess whether the
   result is a clean linear state machine or requires complex branching.

3. **Does the merged skill need any koto integrations, or is the agent the integration?**
   Specifically: does CI checking need to be a koto gate/integration, or is "wait for CI
   and report back" something the agent directive can handle? What about PR creation — does
   koto need to know about it, or is it just an action the agent takes within a state?
