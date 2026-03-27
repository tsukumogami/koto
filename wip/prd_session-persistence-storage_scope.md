# /prd Scope: session-persistence-storage (revision)

## Problem Statement
koto owns where workflow context lives (session directories at ~/.koto/sessions/) but not the content itself. Agents read and write files directly in the session directory, bypassing koto entirely. This means koto can't validate content format, enforce immutability, audit access, or support structured queries. The ~50 wip/ artifact patterns across 10+ skills all use direct filesystem access. To enable content validation, controlled multi-agent access, and structured queries, koto needs to own the content through a CLI interface.

## Initial Scope
### In Scope
- Content submission CLI (add/replace by key, via pipe or file reference)
- Content retrieval CLI (get by key, list by pattern)
- Content existence check (exists by key)
- Multi-agent concurrent submission without state advancement
- Content-aware gate types (hybrid: built-in + shell fallback)
- Resume logic via koto queries (replacing filesystem existence checks)
- Skill-to-skill context handoff within shared sessions
- Cloud sync of content (included in sync scope, sequenced after local backend)
- Migration path for existing wip/ patterns

### Out of Scope
- Partial patches / structured field-level updates (future optimization)
- Ad-hoc context injection by users mid-workflow (future)
- State file access by agents (already excluded by design)

## Research Leads
1. What does the revised R2/R11/R13 look like when agents use CLI instead of filesystem?
2. How does content ownership interact with cloud sync (R5)? Content is included in sync.
3. What new requirements emerge for content CLI, content-aware gates, multi-agent submission?

## Coverage Notes
Extensive exploration already completed (wip/explore_content-ownership_findings.md). Research leads are covered by exploration findings. Proceed directly to drafting.
