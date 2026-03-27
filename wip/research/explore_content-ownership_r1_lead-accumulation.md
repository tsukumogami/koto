# Lead: Accumulation and update patterns in content-owned model

## Findings

### Three lifecycle patterns

**Pattern A: Create-once (60% of wip/ files)**
- Research outputs, baselines, review reports
- Written once, read by later phases, never updated
- Simple replace semantics (`koto ctx add --key X`) suffice

**Pattern B: Accumulation (15% of wip/ files)**
- findings.md appended each converge round (but actually read-all, synthesize, rewrite)
- decisions.md true append (new section per round)
- Agent-driven: orchestrator reads old content, merges with new, rewrites entire file

**Pattern C: In-place updates (25% of wip/ files)**
- coordination.json: field-level status updates (pending→complete)
- bakeoff files: agent appends "## Revised Position" section via SendMessage
- analysis.md: review_rounds counter increment

### Key insight: findings.md is NOT true append
- Phase 3 reads ALL prior research + prior findings, synthesizes, rewrites entire file
- The "Accumulated Understanding" section is rewritten each round (not appended)
- This is agent-driven read-modify-replace, not a storage append operation

### Minimal operations for MVP
1. **Replace** (`koto ctx add --key X < file`): covers 60% of artifacts
2. **Read-modify-replace** (agent reads via `koto ctx get`, modifies, writes via `koto ctx add`): covers accumulation pattern since only one orchestrator writes at a time
3. **Append** (`koto ctx append --key X < content`): optional optimization for decisions.md true-append pattern

Field-level updates (coordination.json) can use read-modify-replace for MVP since the orchestrator serializes writes.

### SendMessage creates collaborative mutations
- Phase 4: orchestrator sends peer context to validator agent via SendMessage
- Validator reads its bakeoff file, appends "## Revised Position", writes back
- This is agent-driven mutation, not orchestrator-driven
- Under content-ownership: agent needs both read and write access to its own key

## Implications
- Replace-only is sufficient for MVP if accumulation uses agent-driven read-modify-replace
- Field-level updates and append operations are optimizations, not requirements
- The orchestrator serialization guarantee makes concurrency a non-issue for accumulation

## Surprises
- Some resume checks parse content within files (e.g., "## Decision: Crystallize" marker in findings.md), not just file existence
- Bakeoff files are mutated by delegate agents (not the orchestrator), requiring agent-level write access

## Open Questions
- Should koto track content metadata (round number, phase, agent ID) alongside the content?
- Is versioning needed for audit trails, or is the JSONL event log sufficient?

## Summary
Replace-only semantics cover 60% of wip/ artifacts. The remaining 40% use accumulation or in-place update patterns, but since only one orchestrator writes at a time, agent-driven read-modify-replace suffices for MVP. True append and field-level update operations are optimizations for later. The main surprise is that some resume checks parse content within files, not just existence.
