# /prd Scope: koto-user-skill

## Problem Statement

The koto-skills plugin has one skill (`koto-author`) but no equivalent for agents
running koto-backed workflows. When koto capabilities change — as happened across
PRs #120–#125 (gate-transition roadmap) — skills are not updated alongside the code,
causing agent guidance to drift from actual behavior. Additionally, the existing
`plugins/koto-skills/AGENTS.md` is buried inside the plugin directory and is only
loaded by Claude Code when working inside that directory, not repo-wide.

## Initial Scope

### In Scope

- **koto-user skill**: a new installable skill in `plugins/koto-skills/skills/koto-user/`
  covering the complete agent runtime loop (koto init, koto next action dispatch, evidence
  submission, gate blocking and overrides, rewind, session lifecycle)
- **Root-level AGENTS.md**: a new `AGENTS.md` at the koto repo root loaded by any Claude
  Code session working in koto — high-level orientation, not full skill content
- **koto-author gaps**: update the existing koto-author skill to cover the gate-transition
  roadmap additions (gates.* routing, structured gate output schemas, override_default,
  koto overrides CLI, blocking_conditions details, --allow-legacy-gates)
- **Freshness mechanism**: eval cases in `plugins/koto-skills/evals/` using the existing
  `eval.sh` + `eval-plugins.yml` harness to catch semantic drift
- **AGENTS.md disposition**: remove or repurpose `plugins/koto-skills/AGENTS.md` — its
  content belongs in koto-user's `references/` directory, not as a plugin-root AGENTS.md

### Out of Scope

- Changes to koto CLI behavior or template format
- New gate types (jira, http, json-command)
- Visualization UI
- Changes to other plugins or skills outside koto-skills

## Research Leads

Exploration was completed in one round. The following are open questions for PRD
phase to resolve:

1. **koto-user skill boundary**: what content should live in SKILL.md vs. references/
   files? What level of detail should the skill provide vs. delegating to inline
   `koto --help` output?

2. **Root AGENTS.md scope**: what exactly should the repo-root `AGENTS.md` contain?
   Should it be koto orientation only (key concepts, where to find docs) or include
   the full runtime loop summary? What's the right boundary between root AGENTS.md
   and the koto-user skill?

3. **koto-author update scope**: of the 4 high-severity gaps and 2 medium-severity
   gaps enumerated in research, which belong in a new "structured gate output"
   reference file vs. updates to the existing `template-format.md`? Should
   `complex-workflow.md` be updated in place or should a new example be added?

4. **Eval case design**: what are the 5-8 behaviors most likely to regress? What
   format should eval cases take (prompt + patterns)? Should evals trigger on
   source-only PRs (`src/**`) in addition to plugin PRs?

5. **koto status command**: research found that `koto status` is referenced in
   koto-author SKILL.md but does not exist in `src/cli/mod.rs`. Verify this and
   confirm the correct alternative (`koto query`? `koto workflows`?). This is a
   live bug fix, not a new requirement.

## Coverage Notes

- The exploration did not confirm whether `koto query` exists in the CLI (mentioned in
  CLAUDE.local.md but not verified in source). The PRD process should confirm the full
  CLI surface before writing requirements for koto-user.
- `context-matches` gate semantics were not verified (no functional test fixture examined).
  PRD should document this gate type's behavior correctly.
- Variable substitution behavior (`{{VAR}}` in directives) was identified as undocumented
  — PRD should decide whether koto-user or koto-author should cover this.

## Decisions from Exploration

- **Plugin placement**: koto-user lives in `plugins/koto-skills/skills/koto-user/` (same
  plugin as koto-author). One directory + one `plugin.json` line. Separate plugin ruled out.
- **AGENTS.md at plugin root is misplaced**: `plugins/koto-skills/AGENTS.md` should not
  serve as skill reference content via the AGENTS.md auto-loading mechanism. Content should
  move to skill `references/` directories explicitly linked from SKILL.md.
- **Root-level AGENTS.md needed**: `koto/AGENTS.md` at the repo root for repo-wide context
  loading by any Claude Code session. Lighter content than the full skill.
- **Skills use custom references/ files**: not AGENTS.md for skill-specific content.
- **Parallel workstreams**: koto-author update and koto-user creation are independent — neither
  blocks the other.
- **Eval harness already in place**: `eval.sh` + `eval-plugins.yml` with `ANTHROPIC_API_KEY`
  in repo secrets. No infrastructure to build — just write cases in `evals/`.
- **File-change CI heuristics ruled out**: high false-positive rate without reliable signal.
- **`koto status` is a live bug**: referenced in koto-author SKILL.md, absent from CLI source.
  Fix required regardless of new work.
