# Exploration Findings: content-ownership

## Core Question

Should koto own the cumulative context of workflow execution (the files currently stored under wip/) instead of letting agents read and write them directly through the filesystem? If so, what CLI interface and storage model enables agents to submit, retrieve, and query context through koto?

## Round 1

### Key Insights

- **Three CLI primitives cover the MVP**: `add` (replace/create), `get` (read by key), `exists` (check presence). These handle 60% of artifacts natively and the remaining 40% via agent-driven read-modify-replace. (Lead 1, Lead 3)
- **Context submission must be separate from state advancement**: today `--with-data` on `koto next` is the only way to give koto content, and it's coupled to state transitions. Agents need to submit context independently — multiple agents building research before the orchestrator calls `next`. (Lead 1)
- **Concurrency is a non-issue for MVP**: agents write to distinct keys (no contention), orchestrators serialize phase transitions (no concurrent same-key writes). Per-key advisory flock suffices. (Lead 2)
- **The engine doesn't constrain the gate model**: closure-based gate evaluator supports any approach. Hybrid gates (built-in types + shell fallback) offer a non-breaking upgrade path. `koto ctx exists` serves double duty for gates AND resume logic. (Lead 4)
- **Handoff design shapes the session model**: simplest MVP is a shared session spanning the full skill pipeline. Each skill reads/writes context in the same session. (Lead 5)
- **Resume migration is straightforward**: replace `test -f wip/<artifact>` with `koto ctx exists --key <key>`. Content-aware checks (marker parsing) need `koto ctx get` with client-side parsing. (Lead 6)
- **Storage**: JSONL append-only logs per key (consistent with state file format, natural for Rust BufWriter + sync_data pattern). (Lead 1)

### Tensions

- **Replace-only vs. richer operations**: 60% of artifacts are create-once (replace works). 40% use accumulation or updates. Agent-driven read-modify-replace covers the gap since orchestrators serialize, but is it "good enough forever" or "MVP that needs expanding"?
- **Skill-owned resume vs. koto-owned resume**: keeping resume in skills preserves autonomy; moving it to koto eliminates fragile cascades but creates tight coupling.

### Gaps

- Full agent research outputs were reconstructed from summaries (agents ran in read-only mode)
- No prototype or proof-of-concept to validate CLI ergonomics

### Decisions

- MVP scope: replace-only operations (add/get/exists)
- Context submission decoupled from state advancement
- Shared session model for MVP
- "Context" terminology, not "evidence"

### User Focus

Ready to crystallize. The MVP shape is clear and the research covered the full surface.

## Accumulated Understanding

koto should own workflow context through a CLI interface with three core primitives: `add` (submit/replace content by key), `get` (retrieve content by key), and `exists` (check presence). Context submission is decoupled from state advancement — agents can submit context without calling `koto next`. Storage uses JSONL append-only logs per key, consistent with the existing state file format. A shared session spans the full skill pipeline for MVP.

The 40% of artifacts that use accumulation or update patterns (findings.md, coordination.json, bakeoff files) are handled by agent-driven read-modify-replace, since orchestrators serialize phase transitions. True append and field-update operations are future optimizations.

Gate evaluation migrates to a hybrid model: built-in gate types (exists, content-match) for common checks, shell fallback for complex logic. Resume logic replaces filesystem checks with `koto ctx exists` and `koto ctx get` queries, keeping resume cascades in skills rather than coupling them to koto's state machine.

The migration surface spans 10+ skills across shirabe and tsukumogami plugins with ~50 distinct wip/ artifact patterns, but the patterns are regular and the CLI primitives cover them uniformly.

## Decision: Crystallize
