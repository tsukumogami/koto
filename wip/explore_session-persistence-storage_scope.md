# Explore Scope: session-persistence-storage

## Core Question

How should koto manage workflow session state (engine state + skill artifacts) with
pluggable storage backends, replacing the current model where agents write directly
to `wip/` in the git working tree? The solution must preserve agents' ability to use
optimized file Read/Edit/Write tools rather than forcing all I/O through CLI commands.

## Context

koto currently relies on `wip/` in the git working tree for all session persistence.
Engine state files (`.state.jsonl`) are managed by `workflow-tool`, but skill artifacts
(exploration scope, research files, decision reports, implementation plans, test plans)
are written directly by agents using file tools. CI enforces `wip/` is clean before
merge; squash-merge keeps artifacts out of main branch history.

This works for solo development but has problems for broader adoption:
- Committing temporary workflow state to git looks unprofessional to other developers
- Session state is tied to a git branch — transferability requires push/pull
- Agents directly manage file locations, coupling skills to the `wip/` convention

Three shifts are needed:
1. **Storage backend**: move from git-committed files to a koto-managed location
2. **Ownership**: koto becomes the API for all workflow state, not just engine state
3. **Token efficiency**: koto manages paths, agents keep direct file I/O. A `koto
   session path <key>` that returns a filesystem path is better than `koto session
   read/write` that forces content through stdout. Agents use the returned path
   with their optimized Read/Edit/Write tools (offset/limit, targeted replacement).

## In Scope

- How koto should own session state (API, lifecycle, location)
- What medium agents use to exchange state with koto (files, CLI, socket, other)
- Token efficiency and agent tool compatibility for the chosen medium
- Storage backends: local default, cloud option, git as opt-in
- Migration path from current `wip/` model

## Out of Scope

- Koto engine's internal state machine format (`.state.jsonl` structure stays)
- Specific cloud provider selection or implementation
- Pricing, auth UX for cloud storage

## Research Leads

1. **What's the full `wip/` surface area — file patterns, writers, lifecycles?**
   Map every distinct file type, which skill/agent writes it, when it's created,
   when it's cleaned up. This inventory defines what the session API must handle.

2. **What are the viable mediums for agent-koto state exchange, and what are the
   token/performance trade-offs of each?**
   Files are the current medium and work well with agent tooling (Read/Edit/Write
   with offset/limit, targeted replacement). But are there alternatives? CLI
   commands with stdout, UNIX sockets, shared memory, a koto daemon, extending
   agent tools, or something else? What are the trade-offs in token usage, latency,
   and implementation complexity for each? Don't assume files are the answer —
   investigate.

3. **How should koto own session state while agents interact with it efficiently?**
   Today agents write directly to `wip/`. Koto needs ownership, but the interface
   must not degrade agent efficiency. What API shapes are possible? Path resolution
   (koto points to files, agents use tools)? Key-value store? Structured API? What
   does "koto owns it" actually mean operationally — lifecycle management, location
   abstraction, cleanup, or all of these?

4. **How do other tools handle workflow state outside git?**
   Terraform state backends, Pulumi backends, GitHub Actions artifacts/cache,
   Nx Cloud, Bazel remote cache. What patterns exist for "workflow state that
   isn't in your repo"? How do they handle the agent/tool interaction model?
   What worked, what didn't?

5. **How should storage backends plug in and what's the sync model?**
   Local filesystem, cloud storage, git working tree. How does the backend
   selection work? For cloud: full upload/download at boundaries vs incremental?
   Conflict resolution for multi-machine? What's the minimum viable remote backend?

6. **What's the migration path from the current `wip/` model?**
   Skills today hardcode `wip/` paths. Can migration be gradual or does it need
   a big-bang switch? What about backward compatibility for users on older koto
   versions? How does the git-as-opt-in mode coexist with the new default?
