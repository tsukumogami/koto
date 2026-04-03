# Exploration Findings: koto-user-skill

## Core Question

We have a `koto-author` skill for agents who write koto templates, but no equivalent
skill for agents who run koto-backed workflows. The two personas have different
knowledge needs and may warrant separate plugins. We also need a structural mechanism
to keep both skills current as koto's capabilities evolve.

## Round 1

### Key Insights

1. **koto-user is a packaging problem, not a content gap.** (plugin-structure, adversarial-demand)
   `AGENTS.md` (550 lines at the plugin root) already covers the entire koto-user knowledge
   domain. The skill would add installability, skill-standard framing, and the eval harness
   connection — not new content. No external demand signals exist (no user-filed issues, no
   failure events), but the drift pattern across PRs #120–#125 is a concrete structural problem
   that validates the work.

2. **`koto status` is referenced in koto-author SKILL.md but doesn't exist in the CLI.**
   (koto-user-persona) This is an active bug in the current skill, independent of the new work.

3. **koto-author has 4 high-severity gaps from the gate-transition roadmap.** (koto-author-gaps)
   - `gates.*` routing syntax absent from template-format.md Layer 3
   - Structured gate output schemas absent (exit_code, exists, matches fields per gate type)
   - `override_default` gate field undocumented
   - `koto overrides record/list` CLI entirely absent
   An agent following koto-author today will write legacy-gate templates that compile only in
   permissive mode and fail `koto template compile` in strict mode. The koto-author template
   (`koto-author.md`) uses the legacy gate pattern, undermining its own credibility.

4. **The LLM eval harness is ready and unused.** (freshness-mechanisms)
   `eval.sh` + `eval-plugins.yml` with `ANTHROPIC_API_KEY` in repo secrets are in place.
   No eval cases exist. CI silently skips the step. Writing 5–8 eval cases costs zero
   infrastructure setup.

5. **Plugin placement is not a real decision.** (plugin-structure)
   Same plugin (`koto-skills`), definitively. One directory + one line in `plugin.json`.

### Tensions

**AGENTS.md placement revealed a deeper issue.** The user clarified that the current
`plugins/koto-skills/AGENTS.md` is only loaded by Claude Code when working inside the
plugin directory — not when working at the koto repo root. They want a root-level
`koto/AGENTS.md` for repo-wide context loading. Additionally, skills should use custom
`references/` files linked from SKILL.md, not AGENTS.md, for skill-specific content.
This reframes the existing `plugins/koto-skills/AGENTS.md` as misplaced — it's serving
as a skill reference file using the wrong mechanism.

**Adversarial validation vs. concrete need.** No external demand signals, but the persona
agent found a live bug (koto status) and the gaps agent found 4 high-severity holes. The
question isn't "is there demand for a new skill?" but "is current guidance actively wrong?"
The answer is yes.

### Gaps

- Whether `koto query` exists (mentioned in CLAUDE.local.md; not confirmed in CLI source)
- `context-matches` gate semantics (no functional test fixture examined)
- Variable substitution (`{{VAR}}`) documentation status

### Decisions

- Parallel workstreams: koto-author update and koto-user creation are independent; neither
  blocks the other.
- Root `koto/AGENTS.md` needed for repo-wide context loading, separate from skill files.
- Skills use custom `references/` files referenced from SKILL.md — not AGENTS.md for
  skill-specific content.
- Same plugin: koto-user goes in `plugins/koto-skills/skills/koto-user/`, not a separate plugin.
- `plugins/koto-skills/AGENTS.md` needs to be reconsidered — it's serving as a skill
  reference file using the wrong mechanism (should be in references/, not AGENTS.md).

### User Focus

User chose parallel workstreams. Clarified that AGENTS.md should be at the koto repo root
(picked up by all Claude Code sessions) rather than buried in the plugin directory. Skills
should reference custom files from their own `references/` directories, not rely on
AGENTS.md for skill-specific content.

## Decision: Crystallize

## Accumulated Understanding

Three distinct workstreams are now in scope:

**Workstream A: koto-author update**
Fix 4 high-severity content gaps (gates.* routing, structured output schemas, override_default,
koto overrides CLI) and 2 medium-severity gaps (blocking_conditions details, --allow-legacy-gates).
Fix the dead `koto status` reference. Update the koto-author.md template to use gates.* routing.
Update complex-workflow.md example. This is corrective work on an existing skill.

**Workstream B: koto-user creation**
Create `plugins/koto-skills/skills/koto-user/` with:
- SKILL.md: when-to-use, action-dispatch table, session lifecycle
- `references/`: custom reference files for runtime contract details
  (action values, expects schema, blocking_conditions, override flow, rewind)
- koto-templates/: possibly a koto-backed skill if the loop warrants it
- eval cases targeting: gate-blocked handling, evidence submission, override two-step flow
The existing `plugins/koto-skills/AGENTS.md` content should move into koto-user's
references/ directory rather than remain as a misplaced AGENTS.md.

**Workstream C: root AGENTS.md**
Create `koto/AGENTS.md` (repo root) with essential context for any Claude Code session:
koto CLI overview, key concepts, where to look. This is the "global" context layer,
lighter than the full skill content. Probably replaces or supersedes the plugin-buried
AGENTS.md currently at `plugins/koto-skills/AGENTS.md`.

**Freshness mechanism:**
- CLAUDE.local.md trigger list: already in place (Workstream A/B are the first exercise of it)
- Eval cases: write 5-8 in `plugins/koto-skills/evals/` using existing harness
- Structured coverage tests: small set of CI grep checks for high-value markers
- `eval-plugins.yml` path filter: consider expanding to `src/**` to catch engine-only PRs
