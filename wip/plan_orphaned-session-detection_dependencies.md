# Plan Dependencies: DESIGN-orphaned-session-detection

## Summary
- Total issues: 5
- Issues with no dependencies: 1
- Maximum dependency depth: 3 (Issue 1 -> Issue 3 -> Issue 4)

## Dependency Graph

```
Issue 1 (no deps)
├── Issue 2 (blocked by 1)
├── Issue 3 (blocked by 1)
│   └── Issue 4 (blocked by 1, 3)
└── Issue 5 (blocked by 1)
```

## Issue Dependencies

| Issue | Title | Blocked By | Blocks |
|-------|-------|------------|--------|
| 1 | feat(engine): add shared template-source-status check module | None | 2, 3, 4, 5 |
| 2 | refactor(engine): route stale-template-source-dir warnings through the shared module | 1 | None |
| 3 | feat(session): thread template-source-status through SessionInfo and both list() backends | 1 | 4 |
| 4 | feat(cli): surface stale template_source_dir on koto status and koto session list | 1, 3 | None |
| 5 | fix(cli): diagnose stale template_source_dir in koto init's already-exists error | 1 | None |

## Parallelization Opportunities

- **Immediate start**: Issue 1 (no dependencies) -- must land first since every other issue depends on it.
- **After Issue 1**: Issues 2, 3, and 5 can be worked in parallel -- they touch disjoint files
  (`path_resolution.rs`/`batch.rs`; `session/mod.rs`+`local.rs`+`cloud.rs`;
  `cli/mod.rs`'s collision paths respectively) and none depends on another.
- **After Issue 3**: Issue 4 (also needs Issue 1, already satisfied).

Since this is single-pr mode, all 5 issues land in one PR; the parallelization
note above informs implementation order within that PR (an implementer or
`/execute` can tackle 2, 3, and 5 in any order once 1 is done), not separate
PR scheduling.

## Critical Path

Issue 1 -> Issue 3 -> Issue 4

Length: 3 issues

## Validation

- [x] No circular dependencies
- [x] All blockers exist in issue list
- [x] At least one issue has no dependencies (Issue 1)
- [x] Critical path length is reasonable (3 of 5 issues -- Issues 2 and 5 sit off the critical path entirely)
