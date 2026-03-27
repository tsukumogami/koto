# Lead: Multi-agent concurrent context submission

## Findings

### Current concurrency model
- Advisory file locking via `flock(LOCK_EX | LOCK_NB)` on state file (non-blocking)
- Append-only JSONL with `sync_data()` ensures atomicity per event
- Evidence merging via `merge_epoch_evidence()` is last-write-wins key merge, not append
- No CAS, no central lock manager, no per-key granules — just POSIX semantics

### Why contention is rare today
- Agents write to SEPARATE files (e.g., `lead-foo.md` vs `lead-bar.md`)
- Orchestrator serializes phase transitions (Phase 2 completes, THEN Phase 3 runs)
- No two agents ever write the same wip/ file concurrently

### If koto owns storage
- Per-key writes from different agents: no contention (same as today)
- Same-key concurrent writes: only happens with accumulation files (findings.md, coordination.json)
- These are always orchestrator-driven (single writer), not multi-agent

### Simplest model
- Per-key advisory flock for writes (same pattern as state file)
- Separate keys per agent (no contention for research outputs)
- Orchestrator serializes accumulation writes (no concurrent same-key writes in practice)

## Implications
- The concurrency "problem" may be smaller than it appears — current patterns already avoid it
- Per-key locking is sufficient for MVP
- Only future multi-orchestrator scenarios (not current) would need CAS or versioning

## Surprises
- Evidence merging is last-write-wins, not append — this would need to change for context accumulation

## Open Questions
- If koto enables concurrent context submission without advancing state, does the orchestrator serialization guarantee still hold?
- Should koto enforce single-writer-per-key or trust agents to coordinate?

## Summary
Koto uses advisory flock + append-only JSONL, and today's separate-files-per-agent model avoids contention entirely. If koto owns storage with per-key writes, concurrency is a non-issue for research outputs. Accumulation files (findings, coordination) are always single-writer (orchestrator), so per-key locking suffices for MVP.
