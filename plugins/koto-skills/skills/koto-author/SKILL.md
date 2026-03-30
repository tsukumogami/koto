---
name: koto-author
description: Guides agents through authoring koto-backed skills with paired SKILL.md and koto template
---

# koto-author

Walks you through creating a koto-backed skill from scratch or converting an existing prose-based skill to use a koto template. Produces a complete skill directory with a SKILL.md entry point and a paired koto template that drives the workflow.

Intended for agent developers who want to build structured, resumable skills on top of koto's state machine.

## Prerequisites

- koto must be installed and on PATH (`koto --version` to verify)
- This skill is installed via the koto-skills plugin

## Input modes

The skill supports two modes, selected at init time:

- **new**: Create a koto-backed skill from scratch
- **convert**: Migrate an existing prose-based skill to use a koto template

## Usage

### Starting a new skill

```bash
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=new
```

### Converting an existing skill

```bash
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=convert
```

### Workflow loop

After initialization, follow the standard koto execution loop:

1. Run `koto next` to read the current state's directive
2. Follow the directive instructions
3. Submit evidence with `koto next --with-data '{"field": "value"}'`
4. Repeat until the workflow reaches the done state

## Reference material

The skill bundles reference material for template authoring:

- **Template format guide**: `${CLAUDE_SKILL_DIR}/references/template-format.md` -- condensed authoring guide covering structure, evidence routing, and advanced features
- **Example templates**: `${CLAUDE_SKILL_DIR}/references/examples/` -- graded examples at increasing complexity
- **hello-koto**: `plugins/koto-skills/skills/hello-koto/hello-koto.md` -- the simplest koto template (good starting point)

Additional guides are available at https://github.com/tsukumogami/koto/tree/main/docs/guides. To list them:

```bash
gh api repos/tsukumogami/koto/contents/docs/guides --jq '.[].name'
```

## Resuming interrupted sessions

If a session is interrupted, koto preserves state. Run `koto status` to see where you left off, then `koto next` to continue.

## Output

The skill produces a new skill directory containing:
- `SKILL.md` -- the skill definition with koto execution loop
- `koto-templates/<skill-name>.md` -- the paired koto template

Both files follow the coupling convention: the SKILL.md references the template via `${CLAUDE_SKILL_DIR}/koto-templates/<skill-name>.md`.

## Complementary skill: skill-creator

If the `/skill-creator:skill-creator` skill is available, load it after the koto-author workflow completes. The skill-creator adds an eval/testing harness that koto-author doesn't cover: it spawns parallel test runs (with-skill vs baseline), grades the output, and iterates on quality. koto-author handles structural correctness (the template compiles, the coupling convention is followed); skill-creator handles behavioral quality (the skill actually works well for its intended use case).

The two skills complement each other: use koto-author to build the skill, then skill-creator to test and refine it.

## This skill's own template

This skill is itself koto-backed. Its template at `${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md` serves as a mid-complexity example (8 states, evidence routing, self-loop, gates). You can inspect it to learn template patterns.
