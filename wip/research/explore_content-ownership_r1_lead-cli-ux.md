# Lead: CLI UX for context submission and retrieval

## Findings

### Existing koto patterns
- `--with-data` on `koto next` accepts inline JSON only, 1 MB size limit, schema-validated against state-specific accepts blocks
- State file uses append-only JSONL with `sync_data()` after each write
- Advisory flock for concurrent access prevention

### Three input patterns needed
1. **Stdin piping** (`< file` or `|`): for streaming, large content, agent tool integration
2. **File reference** (`--from path`): for agent file tools that produce temp files
3. **Inline data** (`--data '...'`): for small payloads, JSON metadata

### Namespacing
- Hierarchical flat keys: `research/phase-1.md`, `design/decision-log.md`
- Can evolve to orthogonal metadata labels (kubectl-style) in future
- Topic-scoping already established in current wip/ naming conventions

### Context vs evidence separation
- Context submission (`koto context add`) should be separate from state advancement (`koto next --with-data`)
- Different concurrency models: non-blocking append-only writes for context, advisory flock for state advancement
- Enables multiple agents writing different keys simultaneously

### Storage recommendation
- JSONL append-only logs per key for MVP (consistent with state file format)
- Natural for Rust `BufWriter` + `sync_data` pattern already in persistence layer
- Migrate to indexed lookup if query performance becomes a bottleneck

## Implications
- Separating context from evidence inverts the current model where agents manage wip/ directly
- Establishes koto as authoritative store for workflow cumulative context
- Three input patterns cover all existing agent interaction models

## Surprises
- `--with-data` is schema-validated against state-specific accepts blocks, making it unsuitable for free-form context

## Open Questions
- JSONL vs SQLite for storage backend
- Should retrieval support pattern matching (glob) or only exact keys?
- How does `koto context get` handle large files — stream to stdout or write to temp file?

## Summary
Koto's existing `--with-data` model is too narrow for context ownership. A successful CLI needs three input patterns (stdin, --from, --data), separate context submission from state advancement, and JSONL append-only storage for MVP. The biggest open question is whether storage should use JSONL (simple, consistent) or SQLite (queryable, versioned).
