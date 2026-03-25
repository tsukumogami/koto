# Lead: What are the viable mediums for agent-koto state exchange?

## Findings

Five mediums evaluated:

**Filesystem paths (current model, relocated)**: Agents use Read/Edit/Write tools
with offset/limit and targeted replacement. Koto manages the directory location.
Token cost: ~20% of full content for partial reads. Best for large artifacts
(research files, plans). No protocol overhead.

**CLI stdout (koto session read/write)**: Content flows through command output.
Simple, no new infrastructure. But 100% content traversal on every read — loses
offset/limit optimization. High token cost for anything over a few KB.

**UNIX domain sockets / HTTP daemon**: ~50% lower latency than TCP loopback.
Requires koto to run as a persistent process. Adds protocol complexity (JSON-RPC,
HTTP). Agent tools would need to call the daemon instead of file tools — not
compatible with current Claude Code tool ecosystem without extensions.

**MCP server**: Standardized resource exposure via JSON-RPC 2.0. Would let agents
access session state as MCP resources. Protocol overhead similar to sockets. Aligns
with Claude Code's plugin model but adds a dependency on MCP infrastructure.

**SQLite embedded**: 35% faster than filesystem for some workloads. Atomic
transactions. But couples schema to implementation, agents can't use file tools
(Read/Edit/Write) on SQLite — would need CLI or MCP wrapper. Good for structured
data (state, manifests), poor for prose (research files).

## Implications

Filesystem paths are the strongest option for preserving agent efficiency. The key
change is WHERE the files live, not HOW agents access them. Structured data (state
files, manifests) could use a different medium (CLI, SQLite) without the same token
concerns since they're small.

A hybrid is worth considering: files for large unstructured artifacts, CLI or
structured API for small coordination data.

## Open Questions

- Could MCP resources provide the offset/limit semantics that file tools have?
- Is there a way to extend agent tools to read from non-filesystem sources efficiently?
