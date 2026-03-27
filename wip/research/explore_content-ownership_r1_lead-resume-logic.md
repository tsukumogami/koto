# Lead: Resume logic migration

## Findings

### Current resume pattern
Every skill uses cascading file-existence checks:
```
explore: crystallize.md → findings w/ marker → findings → research files → scope.md
design: doc status → security research → Solution Architecture → coordination.json
plan: GitHub issues → review.md → dependencies.md → manifest.json → decomposition.md
prd: doc status → phase2 research → scope.md
work-on: context.md → cleanup commit → summary → implementation → plan → introspection → baseline
```

### Two types of resume checks
1. **File existence**: `if wip/<artifact> exists → resume at phase N` (most common)
2. **Content parsing**: `if wip/explore_<topic>_findings.md has "## Decision: Crystallize"` → specific phase

### Content-aware checks are significant
- Explore: "## Decision: Crystallize" marker in findings.md
- Plan: YAML frontmatter fields (execution_mode, input_type)
- Design: coordination.json decision status fields
- These go beyond simple exists/not-exists

### Migration options

**Option A: Keep resume in skills, query koto**
- Skills call `koto ctx exists <session> --key <key>` instead of `test -f`
- Skills call `koto ctx get <session> --key <key>` and parse content for markers
- Minimal change to skill logic; koto is just the storage layer
- Resume cascades stay in skill SKILL.md files

**Option B: Move resume into koto**
- koto knows the workflow template and can determine current phase
- `koto status` already reports current state — extend to report available context
- Skills don't need resume logic; `koto next` always returns the right directive
- Requires koto to understand skill-specific phase semantics (tight coupling)

**Option C: Metadata-based resume**
- Each context key carries metadata: `{phase: 3, round: 2, skill: "explore"}`
- Resume = query koto for highest-phase context key
- `koto ctx list --session X --sort-by phase --limit 1` gives resume point
- Requires consistent metadata discipline across all skills

### Simplest migration path
Option A: replace filesystem checks with koto CLI queries. Skills keep their resume logic. `koto ctx exists` and `koto ctx get` are the only new primitives needed. No koto-side phase awareness required.

## Implications
- `koto ctx exists` is the most important primitive — needed for both resume AND gate evaluation
- Content-aware resume means `koto ctx get` with client-side parsing is also needed
- Option A preserves skill autonomy; Option B centralizes control in koto

## Surprises
- work-on's resume logic checks for git COMMITS, not just wip/ files — this pattern doesn't map to koto context at all
- Some resume cascades are ~10 levels deep (plan skill), making them fragile

## Open Questions
- Should koto eventually own resume logic (Option B), or is skill-side resume permanently correct?
- How does git-commit-based resume (work-on) coexist with koto-context-based resume?
- Should `koto ctx get` support `--grep` or `--contains` for content-aware checks?

## Summary
Resume logic migration is straightforward for MVP: replace `test -f wip/<artifact>` with `koto ctx exists --key <key>`. But some skills parse file content for markers and metadata, requiring `koto ctx get` with client-side parsing. The simplest path keeps resume logic in skills and uses koto as the storage layer, avoiding tight coupling between koto and skill-specific phase semantics.
