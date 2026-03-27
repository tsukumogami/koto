# Lead: Skill-to-skill handoff flows through koto

## Findings

### Current handoff patterns
- **explore→prd**: creates `wip/prd_<topic>_scope.md` (wip artifact)
- **explore→design**: creates `wip/design_<topic>_summary.md` (wip) + `docs/designs/DESIGN-<topic>.md` (deliverable)
- **explore→plan**: creates `wip/plan_<topic>_summary.md` (wip)
- **design→plan**: `docs/designs/DESIGN-<topic>.md` (deliverable, not wip)
- **plan→implement**: `docs/plans/PLAN-<topic>.md` (deliverable) + state file

### Two types of handoff
1. **Through wip/ (koto should own)**: scope files, summaries, decision briefs
2. **Through deliverables (agents own)**: DESIGN docs, PLAN docs, PRD docs

### koto has no handoff mechanism today
- Skills create wip/ files as implicit shared state
- Receiving skill independently checks for wip/ artifacts during Phase 0/1
- No explicit handoff protocol, no session chaining, no context transfer

### Key question: same session or different sessions?
- Current: all wip/ files share the same filesystem namespace (same branch)
- Under content-ownership: each skill invocation could be a separate koto session
- Handoff = transferring context keys from one session to another
- Or: handoff = one long-running session that spans multiple skills

### Options
1. **Shared session**: one koto session spans explore→design→plan. All context accumulates in one place. Simple but couples skill lifecycles.
2. **Session chaining**: each skill gets its own session. Handoff via `koto ctx export <src-session> --keys <pattern> | koto ctx import <dst-session>`. Clean separation but adds ceremony.
3. **Session inheritance**: new session can "mount" a parent session's context as read-only. Explore's context visible to design without copying.

## Implications
- The handoff flow is a design decision that shapes the entire session model
- Shared session is simplest for MVP (matches current single-branch model)
- Session chaining/inheritance is needed long-term for clean lifecycle separation

## Surprises
- koto currently has zero awareness of skill-to-skill flows — all coordination is in the skill prompts
- The explore→design handoff creates BOTH a wip file and a deliverable, which need different ownership

## Open Questions
- Should koto sessions map 1:1 to skills, or 1:1 to branches/PRs?
- If shared session, how does cleanup work when explore completes but design hasn't started?
- Should context keys carry metadata about which skill created them?

## Summary
Koto has no handoff mechanism — skills use implicit wip/ file sharing. For content-ownership, the simplest MVP is a shared session spanning the full skill pipeline (explore→design→plan). Long-term, session chaining or inheritance enables clean lifecycle separation. The handoff question fundamentally shapes the session model.
