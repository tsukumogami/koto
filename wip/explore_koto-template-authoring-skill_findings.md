# Exploration Findings: koto-template-authoring-skill

## Core Question

How should we build a skill, bundled in koto, that helps Claude Code agents author new skills whose execution is driven by koto templates?

## Round 1

### Key insights

- The skill-creator's core value is its eval loop (parallel test runs, grader agents, browser viewer), but koto's `template compile` command provides a cheaper, deterministic validation step that covers structural correctness (lead: skill-creator-patterns)
- Templates use a readable source format (Markdown + YAML frontmatter) that compiles deterministically to JSON. Authors never touch JSON. 13+ compile-time validations catch structural errors (lead: template-format)
- The coupling convention is established: `koto-templates/` subdirectory within the skill, referenced via `${CLAUDE_SKILL_DIR}`, SHA-256 hash-locked on `koto init` (lead: skill-template-coupling)
- Distribution is solved: koto already has a marketplace. Add a new plugin (lead: skill-distribution)
- The state machine mental model layers cleanly: linear (states + transitions) -> branching (evidence routing) -> advanced (gates, variables). Evidence routing mutual exclusivity is the key non-obvious authoring constraint (lead: state-machine-concepts)
- No external demand signals in repo artifacts. One design doc defers `koto generate` as "may be useful later." The user (maintainer) is the primary demand signal (lead: adversarial-demand)

### Tensions

- Eval-driven vs compile-driven validation: the skill-creator relies on agent evaluation, koto has mechanical compilation. Not in conflict -- compile is the baseline, eval could be a stretch goal. But v1 should not invest in eval infrastructure.

### Gaps

- No concrete template source file was examined in detail. Agents described the format abstractly. The skill will need real examples as reference material.
- The skill-creator's SKILL.md (~480 lines) wasn't read in full. Its workflow phases could inform our skill's structure more precisely.

### Decisions

- Validation via `koto template compile`, not eval agents
- Distribution via koto's existing marketplace as a new plugin
- Layered teaching approach (linear -> evidence routing -> advanced)
- Proceed despite absent external demand (maintainer intent is sufficient)

### User Focus

Auto-mode: no user narrowing input. Decisions made via research-first protocol.

## Accumulated Understanding

We're building a meta-skill: a skill for authoring skills-with-koto-templates. The output is always a paired SKILL.md + bundled template under `koto-templates/`.

The skill-creator skill provides the workflow pattern (capture intent -> draft -> validate -> iterate), but our validation step is `koto template compile` rather than agent-based evaluation. This simplifies the skill significantly.

The template format is well-defined: Markdown with YAML frontmatter, states with directives and transitions, optional evidence routing, gates, and variables. The compiler validates 13+ rules including transition target existence and evidence routing mutual exclusivity.

Distribution uses koto's existing marketplace as a new plugin. The file-system coupling convention (`koto-templates/`, `${CLAUDE_SKILL_DIR}`, SHA-256 hash-lock) is established by existing skills like `work-on`.

The skill should teach template authoring in layers: start with linear workflows, introduce evidence-based branching, then cover advanced features. The evidence routing mutual exclusivity constraint needs explicit teaching -- it's the most common non-obvious authoring mistake.

## Decision: Crystallize
