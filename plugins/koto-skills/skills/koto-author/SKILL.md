---
name: koto-author
description: Guides agents through authoring koto-backed skills with paired SKILL.md and koto template
---

# koto-author

Walks you through creating a koto-backed skill from scratch or converting an existing prose-based skill to use a koto template. Produces a complete skill directory with a SKILL.md entry point and a paired koto template that drives the workflow.

Intended for agent developers who want to build structured, resumable skills on top of koto's state machine.

## When to use koto-author

Use this skill when you want **structured, resumable workflows** in your skills. koto is a good fit when:

- Your skill has multiple phases that must run in order
- Phases have conditional branching (different paths based on agent decisions)
- You want resumability if a session is interrupted
- You want to separate workflow mechanics (ordering, branching, gating) from domain logic

If your skill is a single linear task with no decision points, koto adds unnecessary overhead. A plain SKILL.md is simpler.

## Prerequisites

- koto must be installed and on PATH (`koto --version` to verify)
- This skill is installed via the koto-skills plugin

## Usage

Choose your mode:

- **new**: You have the intent but no existing skill -- creating from scratch
- **convert**: You have a prose-based SKILL.md and want to move its workflow to koto

```bash
# New skill
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=new

# Convert existing skill
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=convert
```

After init, follow the koto execution loop:

1. Run `koto next` to read the current state's directive
2. Do the work the directive asks for
3. Submit evidence with `koto next --with-data '{"field": "value"}'`
4. Repeat until the workflow reaches the done state

Run `koto status` at any point to see where you are.

## What to expect

The workflow has 8 states:

1. **entry** -- confirm your mode (new or convert)
2. **context_gathering** -- describe your skill's workflow (new) or analyze the existing SKILL.md (convert)
3. **phase_identification** -- map workflow phases to state machine states
4. **state_design** -- define states, transitions, evidence routing, and gates. You'll read the format guide and pick an example template here.
5. **template_drafting** -- write the koto template file
6. **compile_validation** -- run `koto template compile` to validate. If it fails, you get 3 attempts to fix errors before escalating.
7. **skill_authoring** -- write the paired SKILL.md (new) or refactor the existing one (convert)
8. **integration_check** -- verify the coupling convention and generate the mermaid preview

## Reference material

The skill bundles reference material, loaded during specific states:

- **Template format guide** (`${CLAUDE_SKILL_DIR}/references/template-format.md`) -- read during state_design and template_drafting. Covers structure (Layer 1), evidence routing (Layer 2), and advanced features (Layer 3). Read only the layers you need.
- **Example templates** (`${CLAUDE_SKILL_DIR}/references/examples/`) -- read during state_design. Pick the one matching your complexity:
  - Branching workflows? `evidence-routing-workflow.md`
  - Gates, retries, split topology? `complex-workflow.md`
  - Simple linear flow? Inspect `hello-koto` instead
- **hello-koto** -- the simplest koto template. Good for understanding basic syntax.

Additional guides are available at https://github.com/tsukumogami/koto/tree/main/docs/guides. To list them:

```bash
gh api repos/tsukumogami/koto/contents/docs/guides --jq '.[].name'
```

## Resuming interrupted sessions

koto preserves state across interruptions. Run `koto status` to see where you left off, then `koto next` to continue.

## Output

The skill produces a new skill directory containing:
- `SKILL.md` -- the skill definition with koto execution loop
- `koto-templates/<skill-name>.md` -- the paired koto template
- `koto-templates/<skill-name>.mermaid.md` -- state diagram preview

Both files follow the coupling convention: the SKILL.md references the template via `${CLAUDE_SKILL_DIR}/koto-templates/<skill-name>.md`.

## Troubleshooting

**"koto: command not found"** -- koto isn't on PATH. Install it or add its directory to PATH.

**"template not found"** -- `${CLAUDE_SKILL_DIR}` may not be set. Verify with `echo $CLAUDE_SKILL_DIR` and check the template exists at `$CLAUDE_SKILL_DIR/koto-templates/koto-author.md`.

**Template won't compile after 3 attempts** -- the directive tells you to escalate. Common causes: state name typos, overlapping evidence routing conditions, missing directive body sections. Run `koto template compile <path>` manually to see the full error.

**"session already exists"** -- a previous run didn't finish. Run `koto status` to check, then either `koto next` to resume or start a new session.

## Complementary skill: skill-creator

If the `/skill-creator:skill-creator` skill is available, load it after the koto-author workflow completes. The skill-creator adds an eval/testing harness that koto-author doesn't cover: it spawns parallel test runs (with-skill vs baseline), grades the output, and iterates on quality. koto-author handles structural correctness (the template compiles, the coupling convention is followed); skill-creator handles behavioral quality (the skill actually works well for its intended use case).

The two skills complement each other: use koto-author to build the skill, then skill-creator to test and refine it.

## This skill's own template

This skill is itself koto-backed. Its template at `${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md` serves as a mid-complexity example (8 states, evidence routing, self-loop, gates). You can inspect it to learn template patterns.
