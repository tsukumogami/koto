# Lead: How does the skill-creator skill work?

## Findings

### Structure and location

The skill-creator lives at `/home/dgazineu/.claude/plugins/cache/claude-plugins-official/skill-creator/61c0597779bd/skills/skill-creator/`. Key files:

- `SKILL.md` (~480 lines): main skill definition with workflow phases
- `references/schemas.md` (~430 lines): JSON schemas for skill packaging
- `agents/grader.md`: grading agent for evaluating skill quality
- `agents/analyzer.md`: analysis agent
- `scripts/package_skill.py`: packages output as `.skill` zip files

### Workflow phases

The skill-creator follows a tight iteration loop:

1. **Capture intent**: understand what the user wants the skill to do
2. **Write draft**: produce a SKILL.md and supporting files
3. **Test with subagent pairs**: spawn both a "with-skill" and a "baseline" agent in parallel to compare results
4. **Gather feedback**: use a browser-based viewer for human evaluation
5. **Improve**: iterate based on feedback
6. **Repeat**: loop until quality is sufficient

### Key patterns

- **Parallelism**: spawns both test runs simultaneously and captures timing
- **Structured feedback**: dedicated grader role with discriminating assertions
- **Environment awareness**: adapts to Claude Code vs Claude.ai vs Cowork modes
- **Packaged output**: produces `.skill` zip files and JSON artifacts (evals.json, grading.json, benchmark.json)

## Implications

The skill-creator's iteration loop is its core strength. For a koto template authoring skill, we'd want a similar pattern: capture intent, draft skill+template, validate (maybe via `koto template compile`), iterate. The testing approach (parallel with-skill vs baseline) is powerful but may need adaptation since our output includes templates that can be mechanically validated.

The `.skill` packaging format may not apply. Our skill lives in a marketplace/plugin, not as a standalone `.skill` file. But the workflow pattern transfers directly.

## Surprises

The skill-creator is more of a testing/evaluation harness than a simple generator. It spends most of its complexity on evaluation (grader agents, benchmark artifacts, viewer). This suggests that for our skill, validation of the authored template is at least as important as generation.

## Open Questions

- Should our skill adopt the same eval-driven iteration loop, or is `koto template compile` sufficient validation?
- The skill-creator packages as `.skill` files -- does our marketplace distribution change the output format?
- How much of the skill-creator's testing infrastructure (grader, analyzer) applies to template authoring?

## Summary

The skill-creator follows a tight iteration loop: capture intent, write draft, test with subagent pairs (with-skill and baseline in parallel), gather human feedback via browser viewer, improve, repeat. Its key strengths are parallelism, structured feedback (dedicated grader role), and environment awareness. For koto-template-authoring, adopt the workflow pattern but leverage `koto template compile` as a mechanical validation step alongside or instead of the eval harness.
