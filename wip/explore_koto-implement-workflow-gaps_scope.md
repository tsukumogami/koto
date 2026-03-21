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

- `/work-on`, `/just-do-it`, and other commands (unless they share core requirements)
- Cross-agent delegation (tracked as koto issue #41)
- Shirabe skill extraction
- Adding new koto subcommands (not the direction)

## Research Leads

1. **What external actions and checks does `/implement` need at each phase?**
   Read the implement skill, phase files, and state-management schema to catalog every
   non-agent action: CI checks, PR creation, issue status queries, approval gates.

2. **How does koto's integration system work today, and what can it already drive?**
   Read the template format and integration field docs to understand invocation model,
   response handling, and config structure.

3. **Which of /implement's external actions map cleanly to koto integrations, and which don't?**
   Cross-reference leads 1 and 2 — look for shape mismatches, especially around
   polling/query patterns vs fire-and-forget.

4. **What does `DESIGN-workflow-tool-oss.md` say about the extraction path and anticipated integrations?**
   This strategic design in the vision project likely enumerates what koto needs to
   support workflow-tool replacement.

5. **Can /implement's multi-issue state model be expressed as koto template state + event log?**
   Focus on the wip/ state file schema, the controller loop, and dependency graph
   sequencing — these are structurally most different from a single-workflow koto session.
